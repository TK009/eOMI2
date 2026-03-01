// HTTP server helpers.
//
// Pure functions — no ESP deps — so they're testable on the host.

use crate::pages::PageStore;

// ---------------------------------------------------------------------------
// URI / query-string helpers
// ---------------------------------------------------------------------------

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
/// Returns `(odf_path, has_trailing_slash)`.
///
/// Examples:
/// - `/omi`        → `("/", true)`
/// - `/omi/`       → `("/", true)`
/// - `/omi/DevA/`  → `("/DevA", true)`
/// - `/omi/DevA/T` → `("/DevA/T", false)`
pub fn omi_uri_to_odf_path(uri_path: &str) -> (&str, bool) {
    // Strip the "/omi" prefix
    let rest = if uri_path.starts_with("/omi") {
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
}
