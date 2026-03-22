// WSOP Gateway role — hidden AP + OMI InfoItems (FR-120 through FR-126).
//
// Provisioned devices run a hidden AP `_eomi_onboard` alongside their STA
// connection. Joiners connect to this AP and write JoinRequest/Approval
// InfoItems. The gateway validates, queues, and processes these requests.
//
// Host-testable core logic is platform-independent. ESP-specific AP code
// is gated behind `#[cfg(feature = "esp")]`.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use super::crypto::{self, VerifyCode};
use super::protocol::{
    self, JoinRequest, JoinResponse, WifiCredentials, SecurityType,
    STATUS_APPROVED, STATUS_DENIED,
};

/// O-DF paths for gateway InfoItems (FR-121).
pub const PATH_JOIN_REQUEST: &str = "/OnboardingGateway/JoinRequest";
pub const PATH_JOIN_RESPONSE: &str = "/OnboardingGateway/JoinResponse";
pub const PATH_PENDING_REQUESTS: &str = "/OnboardingGateway/PendingRequests";
pub const PATH_APPROVAL: &str = "/OnboardingGateway/Approval";

/// Human-readable color names for verification display.
const COLOR_NAMES: [&str; 8] = [
    "Red", "Green", "Blue", "Yellow", "Cyan", "Magenta", "White", "Orange",
];

/// Map verification byte to color name (top 3 bits → 0..7).
fn verify_color_name(byte: u8) -> &'static str {
    COLOR_NAMES[(byte >> 5) as usize]
}

/// Map verification byte to digit (mod 10).
fn verify_digit(byte: u8) -> u8 {
    byte % 10
}

/// SSID for the hidden onboarding AP (FR-120).
pub const ONBOARD_SSID: &str = "_eomi_onboard";

/// Maximum simultaneous pending requests (FR-125).
const MAX_PENDING: usize = 4;

/// Default approval timeout in seconds (FR-124).
const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 60;

/// Maximum timestamp drift in seconds (FR-122).
const MAX_TIMESTAMP_DRIFT_SECS: u32 = 300;

/// A pending join request awaiting owner approval.
#[derive(Debug, Clone)]
pub struct PendingRequest {
    pub mac: [u8; 6],
    pub name: String,
    pub pubkey: [u8; 32],
    pub nonce: [u8; 8],
    pub verify_color: &'static str,
    pub verify_digit: u8,
    pub queued_at_secs: f64,
}

impl PendingRequest {
    /// Seconds remaining before auto-denial, given current time.
    pub fn remaining_secs(&self, now_secs: f64, timeout: u64) -> f64 {
        let deadline = self.queued_at_secs + timeout as f64;
        (deadline - now_secs).max(0.0)
    }
}

/// Gateway state machine — manages pending onboarding requests.
pub struct GatewayState {
    pending: Vec<PendingRequest>,
    approval_timeout_secs: u64,
}

/// Result of processing a join request write.
#[derive(Debug, PartialEq)]
pub enum JoinResult {
    /// Request queued; PendingRequests InfoItem should be updated.
    Queued,
    /// Request rejected (bad timestamp, queue full, duplicate MAC).
    Rejected,
}

/// Result of processing an approval write.
#[derive(Debug)]
pub enum ApprovalResult {
    /// Credentials encrypted; write base64 ciphertext to JoinResponse.
    Approved {
        nonce_echo: [u8; 8],
        response_b64: String,
    },
    /// MAC not found in pending queue.
    NotFound,
}

/// Result of ticking timeouts.
#[derive(Debug)]
pub struct TimeoutResult {
    /// Denial responses to write to JoinResponse (one per expired request).
    pub denials: Vec<DenialResponse>,
}

#[derive(Debug)]
pub struct DenialResponse {
    pub nonce_echo: [u8; 8],
    pub response_b64: String,
}

impl GatewayState {
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(MAX_PENDING),
            approval_timeout_secs: DEFAULT_APPROVAL_TIMEOUT_SECS,
        }
    }

    /// Number of currently pending requests.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Process a JoinRequest write from the OMI tree.
    ///
    /// Decodes base64 → wire bytes → JoinRequest, validates timestamp,
    /// computes verify code, and queues the request.
    pub fn process_join_request(
        &mut self,
        b64_value: &str,
        gateway_time_secs: u32,
        now_secs: f64,
    ) -> JoinResult {
        // Decode base64 → wire bytes
        let wire_bytes = match protocol::base64_decode(b64_value) {
            Ok(b) => b,
            Err(_) => return JoinResult::Rejected,
        };

        // Deserialize
        let req = match JoinRequest::deserialize(&wire_bytes) {
            Ok(r) => r,
            Err(_) => return JoinResult::Rejected,
        };

        // FR-122: validate timestamp ±300s
        let drift = if req.timestamp > gateway_time_secs {
            req.timestamp - gateway_time_secs
        } else {
            gateway_time_secs - req.timestamp
        };
        if drift > MAX_TIMESTAMP_DRIFT_SECS {
            return JoinResult::Rejected;
        }

        // FR-125: check queue capacity
        if self.pending.len() >= MAX_PENDING {
            return JoinResult::Rejected;
        }

        // Deduplicate by MAC — replace existing entry
        self.pending.retain(|p| p.mac != req.mac);

        // FR-123: compute verify code
        let vcode = VerifyCode::from_pubkey(&req.pubkey);
        let verify_color = verify_color_name(vcode.byte);
        let verify_digit = verify_digit(vcode.byte);

        self.pending.push(PendingRequest {
            mac: req.mac,
            name: req.name,
            pubkey: req.pubkey,
            nonce: req.nonce,
            verify_color,
            verify_digit,
            queued_at_secs: now_secs,
        });

        JoinResult::Queued
    }

    /// Build the PendingRequests JSON value for the OMI InfoItem (FR-123).
    pub fn pending_requests_json(&self, now_secs: f64) -> String {
        let mut json = String::from("[");
        for (i, p) in self.pending.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            let remaining = p.remaining_secs(now_secs, self.approval_timeout_secs) as u32;
            let mac_str = format!(
                "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                p.mac[0], p.mac[1], p.mac[2], p.mac[3], p.mac[4], p.mac[5]
            );
            json.push_str(&format!(
                "{{\"mac\":\"{}\",\"name\":\"{}\",\"color\":\"{}\",\"digit\":{},\"remaining\":{}}}",
                mac_str, p.name, p.verify_color, p.verify_digit, remaining
            ));
        }
        json.push(']');
        json
    }

    /// Process an Approval write from the OMI tree (FR-126).
    ///
    /// Expects JSON: `{"mac": "AA:BB:CC:DD:EE:FF", "action": "approve"}`
    /// Returns encrypted credentials to write to JoinResponse.
    pub fn process_approval(
        &mut self,
        approval_json: &str,
        wifi_ssid: &str,
        wifi_password: &str,
    ) -> ApprovalResult {
        // Parse MAC from approval JSON (minimal parser — no serde)
        let mac = match parse_approval_mac(approval_json) {
            Some(m) => m,
            None => return ApprovalResult::NotFound,
        };

        let action = parse_approval_action(approval_json);

        // Find and remove the pending request
        let idx = match self.pending.iter().position(|p| p.mac == mac) {
            Some(i) => i,
            None => return ApprovalResult::NotFound,
        };
        let pending = self.pending.remove(idx);

        if action.as_deref() != Some("approve") {
            // Denial
            let response = JoinResponse {
                nonce_echo: pending.nonce,
                status: STATUS_DENIED,
                ciphertext: Vec::new(),
            };
            return ApprovalResult::Approved {
                nonce_echo: pending.nonce,
                response_b64: protocol::base64_encode(&response.serialize()),
            };
        }

        // FR-126: serialize credentials, encrypt with joiner pubkey
        let creds = WifiCredentials {
            ssid: String::from(wifi_ssid),
            security_type: SecurityType::Wpa2Psk,
            credential: String::from(wifi_password),
        };
        let creds_bytes = match creds.serialize() {
            Ok(b) => b,
            Err(_) => {
                // Shouldn't happen with valid WiFi creds, but be safe
                let response = JoinResponse {
                    nonce_echo: pending.nonce,
                    status: STATUS_DENIED,
                    ciphertext: Vec::new(),
                };
                return ApprovalResult::Approved {
                    nonce_echo: pending.nonce,
                    response_b64: protocol::base64_encode(&response.serialize()),
                };
            }
        };

        let recipient_pubkey = crypto_box::PublicKey::from(pending.pubkey);
        let sealed = crypto::seal(&creds_bytes, &recipient_pubkey);

        let response = JoinResponse {
            nonce_echo: pending.nonce,
            status: STATUS_APPROVED,
            ciphertext: sealed,
        };

        ApprovalResult::Approved {
            nonce_echo: pending.nonce,
            response_b64: protocol::base64_encode(&response.serialize()),
        }
    }

    /// Tick timeouts — auto-deny expired requests (FR-124).
    pub fn tick_timeouts(&mut self, now_secs: f64) -> TimeoutResult {
        let mut denials = Vec::new();
        let timeout = self.approval_timeout_secs;

        self.pending.retain(|p| {
            if p.remaining_secs(now_secs, timeout) <= 0.0 {
                let response = JoinResponse {
                    nonce_echo: p.nonce,
                    status: STATUS_DENIED,
                    ciphertext: Vec::new(),
                };
                denials.push(DenialResponse {
                    nonce_echo: p.nonce,
                    response_b64: protocol::base64_encode(&response.serialize()),
                });
                false
            } else {
                true
            }
        });

        TimeoutResult { denials }
    }
}

// ---------- Minimal JSON parsing helpers ----------

/// Extract MAC address from approval JSON. Returns 6-byte MAC or None.
fn parse_approval_mac(json: &str) -> Option<[u8; 6]> {
    // Find "mac" key value
    let mac_str = extract_json_string(json, "mac")?;
    parse_mac_str(&mac_str)
}

/// Extract action string from approval JSON.
fn parse_approval_action(json: &str) -> Option<String> {
    extract_json_string(json, "action")
}

/// Minimal JSON string value extractor for a given key.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\"", key);
    let key_pos = json.find(&pattern)?;
    let after_key = &json[key_pos + pattern.len()..];
    // Skip whitespace and colon
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let after_ws = after_colon.trim_start();
    // Expect opening quote
    let after_quote = after_ws.strip_prefix('"')?;
    // Find closing quote (no escape handling needed for MAC/action values)
    let end_quote = after_quote.find('"')?;
    Some(after_quote[..end_quote].to_string())
}

/// Parse "AA:BB:CC:DD:EE:FF" to [u8; 6].
fn parse_mac_str(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

// ---------- O-DF tree registration ----------

/// Build the OnboardingGateway object for the O-DF tree (FR-121).
///
/// Returns a map with a single `OnboardingGateway` object containing
/// JoinRequest (writable), JoinResponse (read-only), PendingRequests
/// (read-only), and Approval (writable) InfoItems.
#[cfg(feature = "std")]
pub fn build_gateway_tree() -> std::collections::BTreeMap<String, crate::odf::Object> {
    use crate::odf::{InfoItem, Object, OmiValue};
    use std::collections::BTreeMap;

    let mut gw = Object::new("OnboardingGateway");
    gw.type_uri = Some("omi:wsop:gateway".into());

    // JoinRequest — writable by joiners
    let mut join_req = InfoItem::new(4);
    let mut meta = BTreeMap::new();
    meta.insert("writable".into(), OmiValue::Bool(true));
    join_req.meta = Some(meta);
    gw.add_item("JoinRequest".into(), join_req);

    // JoinResponse — gateway writes, joiners read
    let join_resp = InfoItem::new(4);
    gw.add_item("JoinResponse".into(), join_resp);

    // PendingRequests — readable JSON for owner
    let pending = InfoItem::new(1);
    gw.add_item("PendingRequests".into(), pending);

    // Approval — writable by owner
    let mut approval = InfoItem::new(4);
    let mut meta = BTreeMap::new();
    meta.insert("writable".into(), OmiValue::Bool(true));
    approval.meta = Some(meta);
    gw.add_item("Approval".into(), approval);

    let mut map = BTreeMap::new();
    map.insert("OnboardingGateway".into(), gw);
    map
}

// ---------- ESP-specific: hidden AP ----------

#[cfg(feature = "esp")]
mod esp_impl {
    use esp_idf_svc::wifi::{
        AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration,
        Configuration, EspWifi,
    };
    use log::info;

    use super::ONBOARD_SSID;

    /// Start the hidden onboarding AP alongside the existing STA connection (FR-120).
    ///
    /// Uses AP+STA mixed mode with the hidden AP on the same channel as STA.
    /// The STA connection is preserved.
    pub fn start_hidden_ap(
        wifi: &mut BlockingWifi<EspWifi<'static>>,
        sta_channel: u8,
    ) -> crate::error::Result<()> {
        // Get current STA configuration to preserve it
        let current_config = wifi.get_configuration()?;
        let sta_config = match current_config {
            Configuration::Client(sta) => sta,
            Configuration::Mixed(sta, _) => sta,
            _ => ClientConfiguration::default(),
        };

        let ap_config = AccessPointConfiguration {
            ssid: ONBOARD_SSID
                .try_into()
                .map_err(|_| crate::error::Error::Msg("onboard SSID too long"))?,
            auth_method: AuthMethod::None,
            channel: sta_channel,
            ssid_hidden: true,
            max_connections: 4,
            ..Default::default()
        };

        info!(
            "WSOP gateway: starting hidden AP '{}' on channel {}",
            ONBOARD_SSID, sta_channel
        );

        wifi.set_configuration(&Configuration::Mixed(sta_config, ap_config))?;
        wifi.start()?;
        wifi.wait_netif_up()?;

        info!("WSOP gateway: hidden AP active");
        Ok(())
    }

    /// Stop the hidden AP, returning to STA-only mode.
    pub fn stop_hidden_ap(
        wifi: &mut BlockingWifi<EspWifi<'static>>,
    ) -> crate::error::Result<()> {
        let current_config = wifi.get_configuration()?;
        let sta_config = match current_config {
            Configuration::Mixed(sta, _) => sta,
            Configuration::Client(sta) => sta,
            _ => ClientConfiguration::default(),
        };

        wifi.set_configuration(&Configuration::Client(sta_config))?;
        wifi.start()?;

        info!("WSOP gateway: hidden AP stopped");
        Ok(())
    }
}

#[cfg(feature = "esp")]
pub use esp_impl::{start_hidden_ap, stop_hidden_ap};

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wsop::protocol::{self, JoinRequest};

    fn make_join_request_b64(name: &str, mac: [u8; 6], timestamp: u32) -> String {
        let kp = crypto::Keypair::generate();
        let req = JoinRequest {
            name: String::from(name),
            mac,
            pubkey: kp.public_bytes(),
            nonce: [0x42; 8],
            timestamp,
        };
        protocol::base64_encode(&req.serialize().unwrap())
    }

    fn make_join_request_b64_with_key(
        name: &str,
        mac: [u8; 6],
        timestamp: u32,
        pubkey: [u8; 32],
        nonce: [u8; 8],
    ) -> String {
        let req = JoinRequest {
            name: String::from(name),
            mac,
            pubkey,
            nonce,
            timestamp,
        };
        protocol::base64_encode(&req.serialize().unwrap())
    }

    #[test]
    fn queue_join_request() {
        let mut gw = GatewayState::new();
        let b64 = make_join_request_b64("sensor-1", [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x01], 1000);
        let result = gw.process_join_request(&b64, 1000, 100.0);
        assert_eq!(result, JoinResult::Queued);
        assert_eq!(gw.pending_count(), 1);
    }

    #[test]
    fn reject_bad_timestamp() {
        let mut gw = GatewayState::new();
        let b64 = make_join_request_b64("sensor-1", [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x01], 1000);
        // Gateway time is 2000, drift = 1000 > 300
        let result = gw.process_join_request(&b64, 2000, 100.0);
        assert_eq!(result, JoinResult::Rejected);
        assert_eq!(gw.pending_count(), 0);
    }

    #[test]
    fn reject_when_queue_full() {
        let mut gw = GatewayState::new();
        for i in 0..4u8 {
            let b64 = make_join_request_b64(
                "sensor",
                [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, i],
                1000,
            );
            assert_eq!(gw.process_join_request(&b64, 1000, 100.0), JoinResult::Queued);
        }
        // 5th request should be rejected
        let b64 = make_join_request_b64("sensor", [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], 1000);
        assert_eq!(gw.process_join_request(&b64, 1000, 100.0), JoinResult::Rejected);
    }

    #[test]
    fn duplicate_mac_replaces() {
        let mut gw = GatewayState::new();
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let b64_1 = make_join_request_b64("first", mac, 1000);
        let b64_2 = make_join_request_b64("second", mac, 1001);

        gw.process_join_request(&b64_1, 1000, 100.0);
        gw.process_join_request(&b64_2, 1001, 101.0);

        assert_eq!(gw.pending_count(), 1);
        assert_eq!(gw.pending[0].name, "second");
    }

    #[test]
    fn pending_requests_json_format() {
        let mut gw = GatewayState::new();
        let b64 = make_join_request_b64("sensor-1", [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x01], 1000);
        gw.process_join_request(&b64, 1000, 100.0);

        let json = gw.pending_requests_json(100.0);
        assert!(json.starts_with('['));
        assert!(json.ends_with(']'));
        assert!(json.contains("\"mac\":\"AA:BB:CC:DD:EE:01\""));
        assert!(json.contains("\"name\":\"sensor-1\""));
        assert!(json.contains("\"remaining\":60"));
    }

    #[test]
    fn approval_encrypts_credentials() {
        let mut gw = GatewayState::new();
        let device_kp = crypto::Keypair::generate();
        let nonce = [0x42; 8];
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];

        let b64 = make_join_request_b64_with_key(
            "sensor", mac, 1000, device_kp.public_bytes(), nonce,
        );
        gw.process_join_request(&b64, 1000, 100.0);

        let approval = r#"{"mac": "AA:BB:CC:DD:EE:FF", "action": "approve"}"#;
        let result = gw.process_approval(approval, "HomeNet", "secret123");

        match result {
            ApprovalResult::Approved { nonce_echo, response_b64 } => {
                assert_eq!(nonce_echo, nonce);
                // Decode and verify the response
                let resp_bytes = protocol::base64_decode(&response_b64).unwrap();
                let resp = JoinResponse::deserialize(&resp_bytes).unwrap();
                assert_eq!(resp.status, STATUS_APPROVED);
                assert_eq!(resp.nonce_echo, nonce);
                // Device should be able to decrypt
                let plaintext = crypto::seal_open(&resp.ciphertext, &device_kp.secret).unwrap();
                let creds = WifiCredentials::deserialize(&plaintext).unwrap();
                assert_eq!(creds.ssid, "HomeNet");
                assert_eq!(creds.credential, "secret123");
                assert_eq!(creds.security_type, SecurityType::Wpa2Psk);
            }
            _ => panic!("expected Approved"),
        }

        assert_eq!(gw.pending_count(), 0);
    }

    #[test]
    fn approval_not_found() {
        let mut gw = GatewayState::new();
        let approval = r#"{"mac": "AA:BB:CC:DD:EE:FF", "action": "approve"}"#;
        assert!(matches!(gw.process_approval(approval, "net", "pass"), ApprovalResult::NotFound));
    }

    #[test]
    fn timeout_auto_denies() {
        let mut gw = GatewayState::new();
        let b64 = make_join_request_b64("sensor", [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x01], 1000);
        gw.process_join_request(&b64, 1000, 100.0);
        assert_eq!(gw.pending_count(), 1);

        // Tick at 161s (past 60s timeout from queued_at=100)
        let result = gw.tick_timeouts(161.0);
        assert_eq!(result.denials.len(), 1);
        assert_eq!(gw.pending_count(), 0);

        // Verify denial response
        let resp_bytes = protocol::base64_decode(&result.denials[0].response_b64).unwrap();
        let resp = JoinResponse::deserialize(&resp_bytes).unwrap();
        assert_eq!(resp.status, STATUS_DENIED);
    }

    #[test]
    fn timeout_does_not_expire_fresh() {
        let mut gw = GatewayState::new();
        let b64 = make_join_request_b64("sensor", [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x01], 1000);
        gw.process_join_request(&b64, 1000, 100.0);

        // Tick at 130s (only 30s elapsed, not expired)
        let result = gw.tick_timeouts(130.0);
        assert_eq!(result.denials.len(), 0);
        assert_eq!(gw.pending_count(), 1);
    }

    #[test]
    fn reject_invalid_base64() {
        let mut gw = GatewayState::new();
        assert_eq!(
            gw.process_join_request("not-valid-base64!!!", 1000, 100.0),
            JoinResult::Rejected,
        );
    }

    #[test]
    fn parse_mac_str_valid() {
        assert_eq!(
            parse_mac_str("AA:BB:CC:DD:EE:FF"),
            Some([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]),
        );
    }

    #[test]
    fn parse_mac_str_lowercase() {
        assert_eq!(
            parse_mac_str("aa:bb:cc:dd:ee:ff"),
            Some([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]),
        );
    }

    #[test]
    fn parse_mac_str_invalid() {
        assert_eq!(parse_mac_str("not-a-mac"), None);
        assert_eq!(parse_mac_str("AA:BB"), None);
    }

    #[test]
    fn extract_json_string_works() {
        let json = r#"{"mac": "AA:BB:CC:DD:EE:FF", "action": "approve"}"#;
        assert_eq!(
            extract_json_string(json, "mac"),
            Some("AA:BB:CC:DD:EE:FF".into()),
        );
        assert_eq!(
            extract_json_string(json, "action"),
            Some("approve".into()),
        );
        assert_eq!(extract_json_string(json, "missing"), None);
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_gateway_tree_has_correct_items() {
        let tree = build_gateway_tree();
        let gw = tree.get("OnboardingGateway").unwrap();

        let items = gw.items.as_ref().unwrap();
        assert!(items.contains_key("JoinRequest"));
        assert!(items.contains_key("JoinResponse"));
        assert!(items.contains_key("PendingRequests"));
        assert!(items.contains_key("Approval"));

        // JoinRequest and Approval must be writable
        assert!(items["JoinRequest"].is_writable());
        assert!(items["Approval"].is_writable());

        // JoinResponse and PendingRequests must NOT be writable
        assert!(!items["JoinResponse"].is_writable());
        assert!(!items["PendingRequests"].is_writable());
    }
}
