// HTTP callback delivery for subscription events — ESP-only.
//
// Uses ESP-IDF's built-in HTTP client to POST JSON to callback URLs.
// Fire-and-forget with 1 retry on failure, 5-second timeout per attempt.
// Fresh connection per request (no pooling; LAN callbacks are infrequent).

use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::io::Write;
use esp_idf_svc::http::client::{Configuration as HttpClientConfig, EspHttpConnection};
use log::{info, warn};

const CALLBACK_TIMEOUT_MS: u64 = 5000;

/// Result of a single POST attempt.
enum CallbackResult {
    Ok,
    Failed(String),
}

/// POST `body` to `url` with Content-Type: application/json.
fn post_callback(url: &str, body: &[u8]) -> CallbackResult {
    let config = HttpClientConfig {
        timeout: Some(std::time::Duration::from_millis(CALLBACK_TIMEOUT_MS)),
        ..Default::default()
    };

    let conn = match EspHttpConnection::new(&config) {
        Ok(c) => c,
        Err(e) => return CallbackResult::Failed(format!("connection: {e}")),
    };

    let content_length = body.len().to_string();
    let headers = [
        ("Content-Type", "application/json"),
        ("Content-Length", &*content_length),
    ];

    let mut client = HttpClient::wrap(conn);

    match client.post(url, &headers) {
        Ok(mut req) => {
            if let Err(e) = req.write_all(body) {
                return CallbackResult::Failed(format!("write: {e}"));
            }
            if let Err(e) = req.flush() {
                return CallbackResult::Failed(format!("flush: {e}"));
            }
            match req.submit() {
                Ok(resp) => {
                    let status = resp.status();
                    if (200..300).contains(&(status as i32)) {
                        CallbackResult::Ok
                    } else {
                        CallbackResult::Failed(format!("HTTP {status}"))
                    }
                }
                Err(e) => CallbackResult::Failed(format!("submit: {e}")),
            }
        }
        Err(e) => CallbackResult::Failed(format!("request: {e}")),
    }
}

/// Deliver a callback POST with 1 retry on failure.
pub fn deliver_callback(url: &str, body: &[u8], rid: &str) {
    match post_callback(url, body) {
        CallbackResult::Ok => {
            info!("Callback delivered: rid={rid}, url={url}");
        }
        CallbackResult::Failed(e) => {
            warn!("Callback failed (attempt 1): rid={rid}, url={url}, err={e}; retrying");
            match post_callback(url, body) {
                CallbackResult::Ok => {
                    info!("Callback delivered on retry: rid={rid}, url={url}");
                }
                CallbackResult::Failed(e2) => {
                    warn!("Callback failed (attempt 2): rid={rid}, url={url}, err={e2}; dropping");
                }
            }
        }
    }
}
