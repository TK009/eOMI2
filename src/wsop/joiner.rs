// WSOP joiner-side platform driver (ESP-specific).
//
// Drives the OnboardSm state machine on real hardware by performing active
// WiFi scans, connecting to the onboarding AP, sending OMI HTTP requests for
// JoinRequest/JoinResponse exchange, managing keypairs and verification display.
//
// This module is gated behind both `esp` and `secure_onboarding` features.

use crate::board;
use crate::error::{Error, Result};
use crate::wsop::crypto::{self, Keypair, VerifyCode};
use crate::wsop::display::{DisplayMode, OnboardDisplay};
use crate::wsop::onboard_sm::{
    FailReason, OnboardAction, OnboardConfig, OnboardEvent, OnboardSm, OnboardState,
};
use crate::wsop::protocol::{
    self, JoinRequest, JoinResponse, WifiCredentials, STATUS_APPROVED, STATUS_DENIED,
};
use crate::ws2812::Ws2812;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use log::{info, warn};

/// Hidden SSID broadcast by the gateway for onboarding.
const ONBOARD_SSID: &str = "_eomi_onboard";

/// Gateway's OMI path for writing join requests.
const JOIN_REQUEST_PATH: &str = "/Objects/OnboardingGateway/JoinRequest";

/// Gateway's OMI path for reading join responses.
const JOIN_RESPONSE_PATH: &str = "/Objects/OnboardingGateway/JoinResponse";

/// Gateway HTTP base URL (AP default IP).
const GATEWAY_BASE_URL: &str = "http://192.168.4.1";

/// Maximum number of overall retry attempts (fresh keypair each time) (FR-115).
const MAX_RETRY_ATTEMPTS: u32 = 6;

/// Result of the onboarding process.
pub enum OnboardResult {
    /// Onboarding succeeded — credentials are stored in NVS.
    /// Contains the number of credentials stored.
    Success {
        num_creds: usize,
        /// WS2812 driver returned from display (if color mode), for reuse.
        ws2812: Option<Ws2812>,
    },
    /// Onboarding failed — fall back to captive portal.
    Fallback {
        /// WS2812 driver returned from display (if color mode), for reuse.
        ws2812: Option<Ws2812>,
    },
}

/// Run the full WSOP joiner onboarding flow.
///
/// This is called from main.rs before WifiSm is constructed, when no WiFi
/// credentials exist and `secure_onboarding` is enabled.
///
/// The flow:
/// 1. Active scan for hidden SSID `_eomi_onboard`
/// 2. Connect to onboarding AP
/// 3. Generate X25519 keypair, display verification code
/// 4. Write JoinRequest via OMI HTTP
/// 5. Poll JoinResponse at intervals
/// 6. On approval: decrypt credentials, store in NVS, zero key
/// 7. On failure: return Fallback for captive portal
///
/// Retries up to MAX_RETRY_ATTEMPTS with fresh keypair each attempt.
pub fn run_onboarding(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    wifi_nvs: &mut esp_idf_svc::nvs::EspNvs<esp_idf_svc::nvs::NvsDefault>,
    wifi_cfg: &mut crate::wifi_cfg::WifiConfig,
    hostname: &str,
    ws2812_driver: Option<Ws2812>,
) -> Result<OnboardResult> {
    info!("WSOP: starting joiner onboarding flow");

    // Set up verification display
    let mut display = match board::onboard_display_mode() {
        "color" => {
            if let Some(ws) = ws2812_driver {
                OnboardDisplay::color(ws)
            } else {
                OnboardDisplay::none()
            }
        }
        "digit" => {
            if let Some(pin) = board::led_pin() {
                OnboardDisplay::digit(pin)
            } else {
                warn!("WSOP: digit mode configured but no LED pin found in board config");
                OnboardDisplay::none()
            }
        }
        _ => OnboardDisplay::none(),
    };

    let config = OnboardConfig::default();
    let mut retry_count: u32 = 0;

    loop {
        if retry_count >= MAX_RETRY_ATTEMPTS {
            info!("WSOP: exhausted {} retry attempts, falling back to portal", MAX_RETRY_ATTEMPTS);
            let ws2812 = display.stop();
            return Ok(OnboardResult::Fallback { ws2812 });
        }

        if retry_count > 0 {
            info!("WSOP: retry attempt {}/{}", retry_count + 1, MAX_RETRY_ATTEMPTS);
        }

        // Fresh keypair per attempt (FR-115, verification color changes)
        let mut keypair = Keypair::generate();
        let pubkey = keypair.public_bytes();
        let verify_code = VerifyCode::from_pubkey(&pubkey);

        // Display verification code
        if let Err(e) = display.show(verify_code.byte) {
            warn!("WSOP: display error: {}", e);
        }

        // Generate random nonce
        let mut nonce = [0u8; 8];
        unsafe {
            esp_idf_svc::sys::esp_fill_random(nonce.as_mut_ptr() as *mut _, nonce.len());
        }

        // Get MAC address
        let mac = get_sta_mac(wifi);

        let mut sm = OnboardSm::new(config.clone());
        let mut action = sm.initial_action();
        // Store the last approved response ciphertext for the Decrypt action
        let mut last_ciphertext: Vec<u8> = Vec::new();
        // Monotonic tick counter for digit-mode blink pattern (FR-131)
        let mut blink_tick: u32 = 0;

        loop {
            match action {
                OnboardAction::StartScan => {
                    info!("WSOP: scanning for hidden SSID '{}'", ONBOARD_SSID);
                    match active_scan_for_ssid(wifi, ONBOARD_SSID) {
                        Ok(true) => {
                            info!("WSOP: onboarding AP found");
                            action = sm.handle_event(OnboardEvent::ApFound);
                        }
                        Ok(false) => {
                            info!("WSOP: onboarding AP not found in this scan pass");
                            action = sm.handle_event(OnboardEvent::ApNotFound);
                        }
                        Err(e) => {
                            warn!("WSOP: scan error: {}", e);
                            action = sm.handle_event(OnboardEvent::ApNotFound);
                        }
                    }
                }

                OnboardAction::Connect => {
                    info!("WSOP: connecting to onboarding AP");
                    match connect_to_onboard_ap(wifi, ONBOARD_SSID) {
                        Ok(()) => {
                            info!("WSOP: connected to onboarding AP");
                            action = sm.handle_event(OnboardEvent::Connected);
                        }
                        Err(e) => {
                            warn!("WSOP: connection failed: {}", e);
                            action = sm.handle_event(OnboardEvent::ConnectFailed);
                        }
                    }
                }

                OnboardAction::SendRequest => {
                    info!("WSOP: sending JoinRequest to gateway");
                    let timestamp = crate::http::now_secs() as u32;

                    let request = JoinRequest {
                        name: hostname.chars().take(32).collect(),
                        mac,
                        pubkey,
                        nonce,
                        timestamp,
                    };

                    match send_join_request(&request, display.mode()) {
                        Ok(()) => {
                            info!("WSOP: JoinRequest sent successfully");
                            action = sm.handle_event(OnboardEvent::RequestSent);
                        }
                        Err(e) => {
                            warn!("WSOP: failed to send JoinRequest: {}", e);
                            // Treat send failure as connection failure
                            action = sm.handle_event(OnboardEvent::ConnectFailed);
                        }
                    }
                }

                OnboardAction::WaitPoll { ms } => {
                    info!("WSOP: waiting {}ms before polling", ms);
                    display.sleep_with_blink(ms, verify_code.byte, &mut blink_tick);

                    match poll_join_response(&nonce) {
                        Ok(Some(response)) => {
                            // Validate nonce (FR-114)
                            if response.nonce_echo != nonce {
                                warn!("WSOP: nonce mismatch in response");
                                action = sm.handle_event(OnboardEvent::NonceMismatch);
                            } else if response.status == STATUS_APPROVED {
                                info!("WSOP: join request APPROVED");
                                last_ciphertext = response.ciphertext;
                                action = sm.handle_event(OnboardEvent::ResponseApproved);
                            } else if response.status == STATUS_DENIED {
                                info!("WSOP: join request DENIED");
                                action = sm.handle_event(OnboardEvent::ResponseDenied);
                            } else {
                                warn!("WSOP: unknown response status: {}", response.status);
                                action = sm.handle_event(OnboardEvent::NoResponse);
                            }
                        }
                        Ok(None) => {
                            action = sm.handle_event(OnboardEvent::NoResponse);
                        }
                        Err(e) => {
                            warn!("WSOP: poll error: {}", e);
                            action = sm.handle_event(OnboardEvent::NoResponse);
                        }
                    }
                }

                OnboardAction::Decrypt => {
                    info!("WSOP: decrypting credentials");
                    match crypto::seal_open(&last_ciphertext, &keypair.secret) {
                        Some(plaintext) => {
                            match WifiCredentials::deserialize(&plaintext) {
                                Ok(creds) => {
                                    info!("WSOP: credentials decrypted — SSID: {}", creds.ssid);

                                    // Zero private key in RAM (FR-142)
                                    zero_keypair(&mut keypair);

                                    // Store credentials in NVS (FR-141)
                                    wifi_cfg.ssids.push((creds.ssid.clone(), creds.credential.clone()));
                                    if !crate::wifi_cfg::save_wifi_config(wifi_nvs, wifi_cfg) {
                                        warn!("WSOP: failed to persist credentials to NVS");
                                    }

                                    // Stop display, release NeoPixel (FR-133)
                                    let ws2812 = display.stop();
                                    info!("WSOP: onboarding succeeded");

                                    return Ok(OnboardResult::Success {
                                        num_creds: wifi_cfg.ssids.len(),
                                        ws2812,
                                    });
                                }
                                Err(e) => {
                                    warn!("WSOP: credential deserialization failed: {:?}", e);
                                    action = sm.handle_event(OnboardEvent::DecryptFailed);
                                }
                            }
                        }
                        None => {
                            warn!("WSOP: decryption failed (wrong key or tampered)");
                            action = sm.handle_event(OnboardEvent::DecryptFailed);
                        }
                    }
                }

                OnboardAction::Idle => {
                    // Terminal state reached
                    break;
                }
            }
        }

        // Zero keypair regardless of outcome (FR-101)
        zero_keypair(&mut keypair);

        match sm.state() {
            OnboardState::Succeeded => {
                // Should not reach here — success is handled in Decrypt above
                unreachable!("Succeeded state should be handled in Decrypt action");
            }
            OnboardState::Failed { reason } => {
                match reason {
                    FailReason::Timeout | FailReason::ConnectionFailed => {
                        // No gateway found or can't connect — fall back immediately
                        info!("WSOP: failed ({:?}), falling back to portal", reason);
                        let ws2812 = display.stop();
                        return Ok(OnboardResult::Fallback { ws2812 });
                    }
                    FailReason::Denied | FailReason::DecryptionError | FailReason::NonceMismatch => {
                        // Retriable failures — try again with new keypair
                        retry_count += 1;
                        info!("WSOP: failed ({:?}), will retry with fresh keypair", reason);
                        // Disconnect from onboarding AP before retry
                        let _ = wifi.disconnect();
                        continue;
                    }
                }
            }
            _ => {
                // Shouldn't reach here with a non-terminal state
                warn!("WSOP: unexpected non-terminal state after SM loop");
                let ws2812 = display.stop();
                return Ok(OnboardResult::Fallback { ws2812 });
            }
        }
    }
}

/// Perform an ESP-IDF active scan targeting the hidden onboarding SSID (FR-110).
///
/// Uses `show_hidden=true` and sets the target SSID to force probe requests
/// on each channel (required for hidden SSID discovery). The standard
/// `wifi.scan()` wrapper filters out hidden SSIDs, so we use the raw ESP-IDF
/// scan API directly.
fn active_scan_for_ssid(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    target_ssid: &str,
) -> Result<bool> {
    use esp_idf_svc::sys::*;

    // Ensure WiFi is started in STA mode for scanning
    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;
    wifi.start()?;

    // Build a scan config targeting the specific hidden SSID
    let mut ssid_buf = [0u8; 32];
    let ssid_bytes = target_ssid.as_bytes();
    let copy_len = ssid_bytes.len().min(31);
    ssid_buf[..copy_len].copy_from_slice(&ssid_bytes[..copy_len]);

    let scan_config = wifi_scan_config_t {
        ssid: ssid_buf.as_ptr() as *mut u8,
        bssid: core::ptr::null_mut(),
        channel: 0, // scan all channels
        show_hidden: true,
        scan_type: wifi_scan_type_t_WIFI_SCAN_TYPE_ACTIVE,
        scan_time: wifi_scan_time_t {
            active: wifi_active_scan_time_t {
                min: 120,
                max: 300,
            },
            passive: 0,
        },
        home_chan_dwell_time: 0,
    };

    // Start scan (blocking = true waits for completion)
    let err = unsafe { esp_wifi_scan_start(&scan_config, true) };
    if err != ESP_OK {
        return Err(Error::Owned(format!("esp_wifi_scan_start failed: {}", err)));
    }

    // Get number of results
    let mut ap_count: u16 = 0;
    unsafe { esp_wifi_scan_get_ap_num(&mut ap_count) };

    if ap_count == 0 {
        return Ok(false);
    }

    // Retrieve scan results (cap at 20 to limit stack usage)
    let max_results = ap_count.min(20) as usize;
    let mut records = vec![wifi_ap_record_t::default(); max_results];
    let mut found_count = max_results as u16;
    unsafe {
        esp_wifi_scan_get_ap_records(&mut found_count, records.as_mut_ptr());
    }

    // Check if target SSID is in the results
    for record in &records[..found_count as usize] {
        let ssid_len = record.ssid.iter().position(|&b| b == 0).unwrap_or(record.ssid.len());
        if let Ok(ssid_str) = core::str::from_utf8(&record.ssid[..ssid_len]) {
            if ssid_str == target_ssid {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Connect to the onboarding AP as a STA client.
fn connect_to_onboard_ap(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    ssid: &str,
) -> Result<()> {
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| Error::Msg("SSID too long"))?,
        // Onboarding AP is open (no password)
        ..Default::default()
    }))?;

    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;

    Ok(())
}

/// Send a JoinRequest to the gateway via OMI HTTP write (FR-111, FR-112).
fn send_join_request(request: &JoinRequest, display_mode: DisplayMode) -> Result<()> {
    let request_bytes = request.serialize()
        .map_err(|e| Error::Owned(format!("JoinRequest serialization failed: {:?}", e)))?;

    let encoded = protocol::base64_encode(&request_bytes);

    // Prefix the base64 payload with the display mode so the gateway
    // can include it in the PendingRequests JSON for the owner.
    // Format: "color:<base64>" or "digit:<base64>"
    let display_mode_str = display_mode.as_str();
    let prefixed_value = format!("{}:{}", display_mode_str, encoded);
    let body = format!(
        r#"{{"{}": {{"value": "{}"}}}}"#,
        JOIN_REQUEST_PATH, prefixed_value,
    );

    let url = format!("{}/write", GATEWAY_BASE_URL);
    http_post(&url, &body)?;

    Ok(())
}

/// Poll the gateway's JoinResponse InfoItem for a response matching our nonce (FR-113).
///
/// Returns `Ok(Some(response))` if a valid response is found, `Ok(None)` if no
/// response yet, or `Err` on HTTP errors.
fn poll_join_response(expected_nonce: &[u8; 8]) -> Result<Option<JoinResponse>> {
    let url = format!("{}/read{}", GATEWAY_BASE_URL, JOIN_RESPONSE_PATH);

    let body = match http_get(&url) {
        Ok(b) => b,
        Err(e) => {
            warn!("WSOP: HTTP GET failed: {}", e);
            return Ok(None);
        }
    };

    // Parse the OMI response to extract the base64-encoded JoinResponse value
    // The response format is JSON with the value field containing base64 data
    let value = match extract_omi_value(&body) {
        Some(v) => v,
        None => return Ok(None), // No value yet
    };

    if value.is_empty() {
        return Ok(None);
    }

    let response_bytes = protocol::base64_decode(&value)
        .map_err(|e| Error::Owned(format!("base64 decode failed: {:?}", e)))?;

    let response = JoinResponse::deserialize(&response_bytes)
        .map_err(|e| Error::Owned(format!("JoinResponse deserialize failed: {:?}", e)))?;

    // Filter by nonce (FR-114) — only return responses matching our nonce
    if response.nonce_echo == *expected_nonce {
        Ok(Some(response))
    } else {
        Ok(None) // Response for a different joiner
    }
}

/// Get the STA interface MAC address.
fn get_sta_mac(wifi: &BlockingWifi<EspWifi<'static>>) -> [u8; 6] {
    let mut mac = [0u8; 6];
    unsafe {
        esp_idf_svc::sys::esp_wifi_get_mac(
            esp_idf_svc::sys::wifi_interface_t_WIFI_IF_STA,
            mac.as_mut_ptr(),
        );
    }
    mac
}

/// Zero out a keypair's secret key bytes in RAM (FR-101, FR-142).
fn zero_keypair(keypair: &mut Keypair) {
    // Overwrite the secret key memory with zeros
    // SecretKey is 32 bytes; we write through a raw pointer to prevent
    // the compiler from optimizing away the zeroing.
    let secret_ptr = &keypair.secret as *const _ as *mut u8;
    unsafe {
        core::ptr::write_bytes(secret_ptr, 0, 32);
    }
}

// ---------- HTTP helpers ----------

/// Perform an HTTP POST request with a JSON body.
fn http_post(url: &str, body: &str) -> Result<String> {
    use esp_idf_svc::sys::*;
    use std::ffi::CString;

    let c_url = CString::new(url).map_err(|_| Error::Msg("invalid URL"))?;

    let config = esp_http_client_config_t {
        url: c_url.as_ptr(),
        method: esp_http_client_method_t_HTTP_METHOD_POST,
        timeout_ms: 10_000,
        ..Default::default()
    };

    let client = unsafe { esp_http_client_init(&config) };
    if client.is_null() {
        return Err(Error::Msg("HTTP client init failed"));
    }

    let c_content_type = CString::new("Content-Type").unwrap();
    let c_json = CString::new("application/json").unwrap();
    unsafe {
        esp_http_client_set_header(client, c_content_type.as_ptr(), c_json.as_ptr());
        esp_http_client_set_post_field(client, body.as_ptr() as *const _, body.len() as i32);
    }

    let err = unsafe { esp_http_client_perform(client) };
    let status = unsafe { esp_http_client_get_status_code(client) };

    let mut response = String::new();
    if err == ESP_OK {
        let content_length = unsafe { esp_http_client_get_content_length(client) };
        if content_length > 0 && content_length < 4096 {
            let mut buf = vec![0u8; content_length as usize];
            let read = unsafe {
                esp_http_client_read(client, buf.as_mut_ptr() as *mut _, buf.len() as i32)
            };
            if read > 0 {
                buf.truncate(read as usize);
                response = String::from_utf8_lossy(&buf).to_string();
            }
        }
    }

    unsafe { esp_http_client_cleanup(client) };

    if err != ESP_OK {
        return Err(Error::Owned(format!("HTTP POST failed: {}", err)));
    }

    if status < 200 || status >= 300 {
        return Err(Error::Owned(format!("HTTP POST status: {}", status)));
    }

    Ok(response)
}

/// Perform an HTTP GET request and return the response body.
fn http_get(url: &str) -> Result<String> {
    use esp_idf_svc::sys::*;
    use std::ffi::CString;

    let c_url = CString::new(url).map_err(|_| Error::Msg("invalid URL"))?;

    let config = esp_http_client_config_t {
        url: c_url.as_ptr(),
        method: esp_http_client_method_t_HTTP_METHOD_GET,
        timeout_ms: 10_000,
        ..Default::default()
    };

    let client = unsafe { esp_http_client_init(&config) };
    if client.is_null() {
        return Err(Error::Msg("HTTP client init failed"));
    }

    let err = unsafe { esp_http_client_perform(client) };

    let mut response = String::new();
    if err == ESP_OK {
        let content_length = unsafe { esp_http_client_get_content_length(client) };
        if content_length > 0 && content_length < 4096 {
            let mut buf = vec![0u8; content_length as usize];
            let read = unsafe {
                esp_http_client_read(client, buf.as_mut_ptr() as *mut _, buf.len() as i32)
            };
            if read > 0 {
                buf.truncate(read as usize);
                response = String::from_utf8_lossy(&buf).to_string();
            }
        }
    }

    unsafe { esp_http_client_cleanup(client) };

    if err != ESP_OK {
        return Err(Error::Owned(format!("HTTP GET failed: {}", err)));
    }

    Ok(response)
}

/// Extract the "value" field from a simple OMI JSON response.
///
/// Expects a JSON structure like: `{"value": "base64data"}`
/// or nested: `{"InfoItem": {"value": "base64data"}}`
///
/// Uses minimal parsing to avoid pulling in a JSON parser dependency.
fn extract_omi_value(json: &str) -> Option<String> {
    // Look for "value" key followed by a string value
    let value_key = "\"value\"";
    let pos = json.find(value_key)?;
    let after_key = &json[pos + value_key.len()..];

    // Skip whitespace and colon
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let trimmed = after_colon.trim_start();

    // Handle string value (starts with quote)
    if trimmed.starts_with('"') {
        let content = &trimmed[1..];
        let end = content.find('"')?;
        Some(content[..end].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_omi_value_simple() {
        let json = r#"{"value": "SGVsbG8="}"#;
        assert_eq!(extract_omi_value(json), Some("SGVsbG8=".to_string()));
    }

    #[test]
    fn extract_omi_value_nested() {
        let json = r#"{"InfoItem": {"name": "JoinResponse", "value": "AQID"}}"#;
        assert_eq!(extract_omi_value(json), Some("AQID".to_string()));
    }

    #[test]
    fn extract_omi_value_empty() {
        let json = r#"{"value": ""}"#;
        assert_eq!(extract_omi_value(json), Some("".to_string()));
    }

    #[test]
    fn extract_omi_value_missing() {
        let json = r#"{"status": "ok"}"#;
        assert_eq!(extract_omi_value(json), None);
    }

    #[test]
    fn extract_omi_value_no_string() {
        let json = r#"{"value": 42}"#;
        assert_eq!(extract_omi_value(json), None);
    }
}
