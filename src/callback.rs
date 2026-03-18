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

/// Outcome of a callback delivery attempt.
#[derive(Debug, PartialEq)]
pub enum DeliveryOutcome {
    /// Delivered on first attempt.
    Delivered,
    /// Failed first attempt, succeeded on retry.
    Retried,
    /// Failed both attempts; contains the two error strings.
    Dropped(String, String),
}

/// Build the expected HTTP headers for a callback POST.
///
/// Returns `[("Content-Type", "application/json"), ("Content-Length", "<len>")]`.
pub fn callback_headers(body_len: usize) -> [(&'static str, String); 2] {
    [
        ("Content-Type", "application/json".to_string()),
        ("Content-Length", body_len.to_string()),
    ]
}

/// Check whether an HTTP status code indicates success (2xx).
pub fn is_success_status(status: u16) -> bool {
    (200..300).contains(&status)
}

/// Deliver a callback with 1 retry, using the provided `poster` for the
/// actual HTTP call. Returns the delivery outcome.
pub fn deliver_with_poster<F>(poster: &mut F) -> DeliveryOutcome
where
    F: FnMut() -> Result<(), String>,
{
    match poster() {
        Ok(()) => DeliveryOutcome::Delivered,
        Err(e1) => match poster() {
            Ok(()) => DeliveryOutcome::Retried,
            Err(e2) => DeliveryOutcome::Dropped(e1, e2),
        },
    }
}

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
    if is_success_status(status) {
        Ok(())
    } else {
        Err(format!("HTTP {status}"))
    }
}

/// Deliver a callback POST with 1 retry on failure.
#[cfg(feature = "esp")]
pub fn deliver_callback(url: &str, body: &[u8], rid: &str) {
    let mut poster = || post_callback(url, body);
    match deliver_with_poster(&mut poster) {
        DeliveryOutcome::Delivered => {
            info!("Callback delivered: rid={rid}, url={url}");
        }
        DeliveryOutcome::Retried => {
            info!("Callback delivered on retry: rid={rid}, url={url}");
        }
        DeliveryOutcome::Dropped(e1, _e2) => {
            warn!("Callback failed after retry: rid={rid}, url={url}, err={e1}; dropping");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Status code checking ──────────────────────────────────────────

    #[test]
    fn status_200_is_success() {
        assert!(is_success_status(200));
    }

    #[test]
    fn status_201_is_success() {
        assert!(is_success_status(201));
    }

    #[test]
    fn status_299_is_success() {
        assert!(is_success_status(299));
    }

    #[test]
    fn status_199_is_not_success() {
        assert!(!is_success_status(199));
    }

    #[test]
    fn status_300_is_not_success() {
        assert!(!is_success_status(300));
    }

    #[test]
    fn status_404_is_not_success() {
        assert!(!is_success_status(404));
    }

    #[test]
    fn status_500_is_not_success() {
        assert!(!is_success_status(500));
    }

    // ── Header formatting ─────────────────────────────────────────────

    #[test]
    fn headers_content_type_is_json() {
        let hdrs = callback_headers(42);
        assert_eq!(hdrs[0].0, "Content-Type");
        assert_eq!(hdrs[0].1, "application/json");
    }

    #[test]
    fn headers_content_length_matches_body() {
        let hdrs = callback_headers(128);
        assert_eq!(hdrs[1].0, "Content-Length");
        assert_eq!(hdrs[1].1, "128");
    }

    #[test]
    fn headers_zero_length_body() {
        let hdrs = callback_headers(0);
        assert_eq!(hdrs[1].1, "0");
    }

    // ── Retry logic (deliver_with_poster) ─────────────────────────────

    #[test]
    fn deliver_succeeds_first_attempt() {
        let mut poster = || Ok(());
        assert_eq!(deliver_with_poster(&mut poster), DeliveryOutcome::Delivered);
    }

    #[test]
    fn deliver_retries_on_first_failure() {
        let mut calls = 0u32;
        let mut poster = || {
            calls += 1;
            if calls == 1 {
                Err("timeout".to_string())
            } else {
                Ok(())
            }
        };
        assert_eq!(deliver_with_poster(&mut poster), DeliveryOutcome::Retried);
        assert_eq!(calls, 2);
    }

    #[test]
    fn deliver_drops_after_two_failures() {
        let mut calls = 0u32;
        let mut poster = || {
            calls += 1;
            Err(format!("err-{calls}"))
        };
        let outcome = deliver_with_poster(&mut poster);
        assert_eq!(
            outcome,
            DeliveryOutcome::Dropped("err-1".to_string(), "err-2".to_string())
        );
        assert_eq!(calls, 2);
    }

    #[test]
    fn deliver_calls_poster_exactly_once_on_success() {
        let mut calls = 0u32;
        let mut poster = || {
            calls += 1;
            Ok(())
        };
        deliver_with_poster(&mut poster);
        assert_eq!(calls, 1);
    }

    #[test]
    fn deliver_calls_poster_exactly_twice_on_retry() {
        let mut calls = 0u32;
        let mut poster = || {
            calls += 1;
            if calls == 1 { Err("fail".into()) } else { Ok(()) }
        };
        deliver_with_poster(&mut poster);
        assert_eq!(calls, 2);
    }

    #[test]
    fn deliver_calls_poster_exactly_twice_on_drop() {
        let mut calls = 0u32;
        let mut poster = || {
            calls += 1;
            Err("fail".to_string())
        };
        deliver_with_poster(&mut poster);
        assert_eq!(calls, 2);
    }

    #[test]
    fn deliver_preserves_error_messages() {
        let mut poster = || Err("connection refused".to_string());
        match deliver_with_poster(&mut poster) {
            DeliveryOutcome::Dropped(e1, e2) => {
                assert_eq!(e1, "connection refused");
                assert_eq!(e2, "connection refused");
            }
            other => panic!("expected Dropped, got {other:?}"),
        }
    }

    #[test]
    fn deliver_captures_different_errors() {
        let mut calls = 0u32;
        let mut poster = || {
            calls += 1;
            match calls {
                1 => Err("timeout".to_string()),
                _ => Err("connection reset".to_string()),
            }
        };
        match deliver_with_poster(&mut poster) {
            DeliveryOutcome::Dropped(e1, e2) => {
                assert_eq!(e1, "timeout");
                assert_eq!(e2, "connection reset");
            }
            other => panic!("expected Dropped, got {other:?}"),
        }
    }
}
