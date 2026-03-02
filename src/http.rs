// HTTP server helpers.
//
// Pure functions — no ESP deps — so they're testable on the host.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::omi::{OmiMessage, Operation, ReadOp};
use crate::omi::read::ReadKind;
use crate::pages::PageStore;

// ---------------------------------------------------------------------------
// Time helpers
// ---------------------------------------------------------------------------

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// Body / response helpers (platform-independent)
// ---------------------------------------------------------------------------

/// Body reading error — empty, too large, or I/O failure.
#[derive(Debug)]
pub enum BodyError {
    Empty,
    TooLarge,
    ReadFailed,
}

/// Check if an OMI response indicates a successful write (status 200 or 201).
pub fn is_successful_write_response(resp: &OmiMessage) -> bool {
    if let Operation::Response(body) = &resp.operation {
        body.status == 200 || body.status == 201
    } else {
        false
    }
}

/// Check if an OMI operation mutates state (write, delete, cancel, or subscription).
pub fn is_mutating_operation(op: &Operation) -> bool {
    match op {
        Operation::Write(_) | Operation::Delete(_) | Operation::Cancel(_) => true,
        Operation::Read(read_op) => read_op.kind() == ReadKind::Subscription,
        Operation::Response(_) => false,
    }
}

// ---------------------------------------------------------------------------
// URI / query-string helpers
// ---------------------------------------------------------------------------

/// Strip query string from URI, returning just the path.
pub fn uri_path(uri: &str) -> &str {
    uri.split('?').next().unwrap_or(uri)
}

/// Extract the query string after `?`, if present.
pub fn uri_query(uri: &str) -> Option<&str> {
    uri.find('?').map(|pos| &uri[pos + 1..])
}

/// Split a query string `key=value&key2=value2` into borrowed pairs.
/// Entries without `=` are silently skipped.
pub fn parse_query_params(query: &str) -> Vec<(&str, &str)> {
    query
        .split('&')
        .filter_map(|pair| {
            let mut it = pair.splitn(2, '=');
            let key = it.next()?;
            let val = it.next()?;
            if key.is_empty() { return None; }
            Some((key, val))
        })
        .collect()
}

/// Parsed read-parameters extracted from the query string of an OMI GET.
#[derive(Debug, Default, PartialEq)]
pub struct OmiReadParams {
    pub newest: Option<u64>,
    pub oldest: Option<u64>,
    pub begin: Option<f64>,
    pub end: Option<f64>,
    pub depth: Option<u64>,
}

impl OmiReadParams {
    /// Parse from a raw query string. Invalid values are silently ignored.
    pub fn from_query(query: &str) -> Self {
        let mut p = Self::default();
        for (k, v) in parse_query_params(query) {
            match k {
                "newest" => p.newest = v.parse().ok(),
                "oldest" => p.oldest = v.parse().ok(),
                "begin" => p.begin = v.parse().ok(),
                "end" => p.end = v.parse().ok(),
                "depth" => p.depth = v.parse().ok(),
                _ => {}
            }
        }
        p
    }
}

/// Convert an HTTP URI path to an O-DF path by stripping the `/omi` prefix.
///
/// Paths without the `/omi` prefix are returned as-is.
///
/// Returns `(odf_path, has_trailing_slash)`.
///
/// Examples:
/// - `/omi`        → `("/", true)`
/// - `/omi/`       → `("/", true)`
/// - `/omi/DevA/`  → `("/DevA", true)`
/// - `/omi/DevA/T` → `("/DevA/T", false)`
/// - `/other`      → `("/other", false)`
pub fn omi_uri_to_odf_path(uri_path: &str) -> (&str, bool) {
    // Strip the "/omi" prefix (exact match only, not "/omission" etc.)
    let rest = if uri_path == "/omi" {
        ""
    } else if uri_path.starts_with("/omi/") {
        &uri_path[4..]
    } else {
        uri_path
    };

    if rest.is_empty() || rest == "/" {
        return ("/", true);
    }

    let trailing = rest.ends_with('/');
    let trimmed = rest.trim_end_matches('/');
    if trimmed.is_empty() {
        ("/", true)
    } else {
        (trimmed, trailing)
    }
}

/// Build a ReadOp from an O-DF path and parsed query parameters.
pub fn build_read_op(odf_path: &str, params: &OmiReadParams) -> OmiMessage {
    OmiMessage {
        version: "1.0".into(),
        ttl: 0,
        operation: Operation::Read(ReadOp {
            path: Some(odf_path.into()),
            rid: None,
            newest: params.newest,
            oldest: params.oldest,
            begin: params.begin,
            end: params.end,
            depth: params.depth,
            interval: None,
            callback: None,
        }),
    }
}

/// Escape HTML special characters to prevent XSS.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// Render the landing page HTML, including links to all stored pages.
pub fn render_landing_page(store: &PageStore) -> String {
    let pages = store.list();
    let list_html = if pages.is_empty() {
        String::from("<p>No pages stored yet.</p>")
    } else {
        let mut s = String::from("<ul>");
        for path in &pages {
            let escaped = html_escape(path);
            s.push_str("<li><a href=\"");
            s.push_str(&escaped);
            s.push_str("\">");
            s.push_str(&escaped);
            s.push_str("</a></li>");
        }
        s.push_str("</ul>");
        s
    };

    format!(
        "<!DOCTYPE html>\
        <html><body>\
        <h1>Reconfigurable Device</h1>\
        <p>Status: running</p>\
        <p>PATCH HTML to any path to store a page.</p>\
        <h2>Stored pages</h2>\
        {}\
        </body></html>",
        list_html
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_special_chars() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape("x\"y"), "x&quot;y");
        assert_eq!(html_escape("a'b"), "a&#x27;b");
        assert_eq!(html_escape("/normal/path"), "/normal/path");
    }

    #[test]
    fn landing_page_empty() {
        let store = PageStore::new();
        let html = render_landing_page(&store);
        assert!(html.contains("No pages stored yet."));
        assert!(html.contains("Reconfigurable Device"));
    }

    #[test]
    fn landing_page_with_pages() {
        let mut store = PageStore::new();
        store.store("/hello", "<h1>Hi</h1>").unwrap();
        store.store("/about", "<p>About</p>").unwrap();
        let html = render_landing_page(&store);
        assert!(html.contains("<a href=\"/about\">/about</a>"));
        assert!(html.contains("<a href=\"/hello\">/hello</a>"));
        assert!(!html.contains("No pages stored yet."));
    }

    // --- URI / query helpers ---

    #[test]
    fn uri_query_present() {
        assert_eq!(uri_query("/omi/Dev?newest=1"), Some("newest=1"));
    }

    #[test]
    fn uri_query_absent() {
        assert_eq!(uri_query("/omi/Dev"), None);
    }

    #[test]
    fn uri_query_empty_after_question() {
        assert_eq!(uri_query("/omi?"), Some(""));
    }

    #[test]
    fn parse_query_basic() {
        let pairs = parse_query_params("newest=1&oldest=5");
        assert_eq!(pairs, vec![("newest", "1"), ("oldest", "5")]);
    }

    #[test]
    fn parse_query_skips_bare_key() {
        let pairs = parse_query_params("foo&bar=2");
        assert_eq!(pairs, vec![("bar", "2")]);
    }

    #[test]
    fn parse_query_empty_value() {
        let pairs = parse_query_params("key=");
        assert_eq!(pairs, vec![("key", "")]);
    }

    #[test]
    fn parse_query_empty_string() {
        let pairs = parse_query_params("");
        assert!(pairs.is_empty());
    }

    // --- OmiReadParams ---

    #[test]
    fn omi_read_params_all_fields() {
        let p = OmiReadParams::from_query("newest=3&oldest=1&begin=100.0&end=200.0&depth=2");
        assert_eq!(p.newest, Some(3));
        assert_eq!(p.oldest, Some(1));
        assert_eq!(p.begin, Some(100.0));
        assert_eq!(p.end, Some(200.0));
        assert_eq!(p.depth, Some(2));
    }

    #[test]
    fn omi_read_params_invalid_ignored() {
        let p = OmiReadParams::from_query("newest=abc&depth=-1");
        assert_eq!(p.newest, None);
        assert_eq!(p.depth, None); // u64 can't be negative
    }

    #[test]
    fn omi_read_params_empty() {
        let p = OmiReadParams::from_query("");
        assert_eq!(p, OmiReadParams::default());
    }

    #[test]
    fn omi_read_params_unknown_keys_ignored() {
        let p = OmiReadParams::from_query("foo=bar&newest=2");
        assert_eq!(p.newest, Some(2));
        assert_eq!(p.oldest, None);
    }

    // --- omi_uri_to_odf_path ---

    #[test]
    fn uri_to_path_root() {
        assert_eq!(omi_uri_to_odf_path("/omi"), ("/", true));
        assert_eq!(omi_uri_to_odf_path("/omi/"), ("/", true));
    }

    #[test]
    fn uri_to_path_object() {
        assert_eq!(omi_uri_to_odf_path("/omi/DeviceA/"), ("/DeviceA", true));
    }

    #[test]
    fn uri_to_path_infoitem() {
        assert_eq!(omi_uri_to_odf_path("/omi/DeviceA/Temp"), ("/DeviceA/Temp", false));
    }

    #[test]
    fn uri_to_path_deep() {
        assert_eq!(
            omi_uri_to_odf_path("/omi/House/Floor1/Room101/Temp"),
            ("/House/Floor1/Room101/Temp", false)
        );
    }

    #[test]
    fn uri_to_path_no_false_prefix_match() {
        // "/omission" should NOT be treated as an /omi path
        assert_eq!(omi_uri_to_odf_path("/omission"), ("/omission", false));
        assert_eq!(omi_uri_to_odf_path("/omitted/data"), ("/omitted/data", false));
    }

    // --- uri_path ---

    #[test]
    fn uri_path_strips_query() {
        assert_eq!(uri_path("/omi/Dev?newest=1"), "/omi/Dev");
    }

    #[test]
    fn uri_path_no_query() {
        assert_eq!(uri_path("/omi/Dev"), "/omi/Dev");
    }

    #[test]
    fn uri_path_empty_query() {
        assert_eq!(uri_path("/omi?"), "/omi");
    }

    #[test]
    fn uri_path_empty_string() {
        assert_eq!(uri_path(""), "");
    }

    // --- is_successful_write_response ---

    #[test]
    fn successful_write_200() {
        use crate::omi::OmiResponse;
        let msg = OmiResponse::ok(serde_json::json!(null));
        assert!(is_successful_write_response(&msg));
    }

    #[test]
    fn successful_write_201() {
        use crate::omi::OmiResponse;
        let msg = OmiResponse::created();
        assert!(is_successful_write_response(&msg));
    }

    #[test]
    fn non_success_response() {
        use crate::omi::OmiResponse;
        let msg = OmiResponse::not_found("/Missing");
        assert!(!is_successful_write_response(&msg));
    }

    #[test]
    fn non_response_operation() {
        let msg = build_read_op("/Foo", &OmiReadParams::default());
        assert!(!is_successful_write_response(&msg));
    }

    // --- is_mutating_operation ---

    #[test]
    fn mutating_write_single() {
        let op = Operation::Write(crate::omi::write::WriteOp::Single {
            path: "/A/B".into(),
            v: crate::odf::OmiValue::Number(1.0),
            t: None,
        });
        assert!(is_mutating_operation(&op));
    }

    #[test]
    fn mutating_delete() {
        let op = Operation::Delete(crate::omi::delete::DeleteOp {
            path: "/A".into(),
        });
        assert!(is_mutating_operation(&op));
    }

    #[test]
    fn mutating_cancel() {
        let op = Operation::Cancel(crate::omi::cancel::CancelOp {
            rid: vec!["req-1".into()],
        });
        assert!(is_mutating_operation(&op));
    }

    #[test]
    fn mutating_subscription() {
        let op = Operation::Read(ReadOp {
            path: Some("/A/B".into()),
            rid: None,
            newest: None,
            oldest: None,
            begin: None,
            end: None,
            depth: None,
            interval: Some(10.0),
            callback: None,
        });
        assert!(is_mutating_operation(&op));
    }

    #[test]
    fn non_mutating_read() {
        let op = Operation::Read(ReadOp {
            path: Some("/A/B".into()),
            rid: None,
            newest: Some(1),
            oldest: None,
            begin: None,
            end: None,
            depth: None,
            interval: None,
            callback: None,
        });
        assert!(!is_mutating_operation(&op));
    }

    #[test]
    fn non_mutating_poll() {
        let op = Operation::Read(ReadOp {
            path: None,
            rid: Some("req-1".into()),
            newest: None,
            oldest: None,
            begin: None,
            end: None,
            depth: None,
            interval: None,
            callback: None,
        });
        assert!(!is_mutating_operation(&op));
    }

    #[test]
    fn non_mutating_response() {
        use crate::omi::OmiResponse;
        let msg = OmiResponse::ok(serde_json::json!(null));
        assert!(!is_mutating_operation(&msg.operation));
    }

    // --- build_read_op ---

    #[test]
    fn build_read_op_defaults() {
        let msg = build_read_op("/Sensor/Temp", &OmiReadParams::default());
        match &msg.operation {
            Operation::Read(op) => {
                assert_eq!(op.path.as_deref(), Some("/Sensor/Temp"));
                assert_eq!(op.newest, None);
                assert_eq!(op.oldest, None);
                assert_eq!(op.begin, None);
                assert_eq!(op.end, None);
                assert_eq!(op.depth, None);
            }
            _ => panic!("expected Read"),
        }
    }

    #[test]
    fn build_read_op_with_params() {
        let params = OmiReadParams {
            newest: Some(5),
            oldest: Some(1),
            begin: Some(100.0),
            end: Some(200.0),
            depth: Some(3),
        };
        let msg = build_read_op("/A/B", &params);
        match &msg.operation {
            Operation::Read(op) => {
                assert_eq!(op.newest, Some(5));
                assert_eq!(op.oldest, Some(1));
                assert_eq!(op.begin, Some(100.0));
                assert_eq!(op.end, Some(200.0));
                assert_eq!(op.depth, Some(3));
            }
            _ => panic!("expected Read"),
        }
    }
}
