// HTTP callback delivery for subscription events — ESP-only.
//
// Uses ESP-IDF's built-in HTTP client to POST JSON to callback URLs.
// Fire-and-forget with 1 retry on failure, 5-second timeout per attempt.
// Fresh connection per request (no pooling; LAN callbacks are infrequent).
//
// All code is gated behind #[cfg(feature = "esp")].

#[cfg(feature = "esp")]
use embedded_svc::http::client::Client as HttpClient;
#[cfg(feature = "esp")]
use embedded_svc::io::Write;
#[cfg(feature = "esp")]
use esp_idf_svc::http::client::{Configuration as HttpClientConfig, EspHttpConnection};
#[cfg(feature = "esp")]
use log::{info, warn};

#[cfg(feature = "esp")]
const CALLBACK_TIMEOUT_MS: u64 = 5000;

/// POST `body` to `url` with Content-Type: application/json.
#[cfg(feature = "esp")]
fn post_callback(url: &str, body: &[u8]) -> Result<(), String> {
    let config = HttpClientConfig {
        timeout: Some(std::time::Duration::from_millis(CALLBACK_TIMEOUT_MS)),
        ..Default::default()
    };

    let conn = EspHttpConnection::new(&config)
        .map_err(|e| format!("connection: {e}"))?;

    let content_length = body.len().to_string();
    let headers = [
        ("Content-Type", "application/json"),
        ("Content-Length", &*content_length),
    ];

    let mut client = HttpClient::wrap(conn);

    let mut req = client.post(url, &headers)
        .map_err(|e| format!("request: {e}"))?;
    req.write_all(body)
        .map_err(|e| format!("write: {e}"))?;
    req.flush()
        .map_err(|e| format!("flush: {e}"))?;
    let resp = req.submit()
        .map_err(|e| format!("submit: {e}"))?;
    let status = resp.status();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("HTTP {status}"))
    }
}

/// Deliver a callback POST with 1 retry on failure.
#[cfg(feature = "esp")]
pub fn deliver_callback(url: &str, body: &[u8], rid: &str) {
    match post_callback(url, body) {
        Ok(()) => {
            info!("Callback delivered: rid={rid}, url={url}");
        }
        Err(e) => {
            warn!("Callback failed (attempt 1): rid={rid}, url={url}, err={e}; retrying");
            match post_callback(url, body) {
                Ok(()) => {
                    info!("Callback delivered on retry: rid={rid}, url={url}");
                }
                Err(e2) => {
                    warn!("Callback failed (attempt 2): rid={rid}, url={url}, err={e2}; dropping");
                }
            }
        }
    }
}
