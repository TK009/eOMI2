/// Callback URL scheme detection and path extraction.
///
/// Subscription callbacks can target either HTTP endpoints or local
/// `javascript://` URLs that route to the embedded script engine.
/// Recognised callback URL schemes.
#[derive(Debug, Clone, PartialEq)]
pub enum CallbackScheme {
    /// HTTP or HTTPS remote endpoint.
    Http,
    /// Local script execution via `javascript://` pseudo-protocol.
    /// Contains the extracted O-DF path (e.g. `Objects/Device/MetaData/script`).
    JavaScript { path: String },
}

/// The `javascript://` scheme prefix (case-insensitive match).
const JS_PREFIX: &str = "javascript://";

/// Detect the scheme of a callback URL and extract scheme-specific data.
///
/// - `javascript://Objects/Foo/Bar` → `CallbackScheme::JavaScript { path: "Objects/Foo/Bar" }`
/// - `http://...` or `https://...`  → `CallbackScheme::Http`
/// - Anything else                  → `None`
pub fn parse_callback_url(url: &str) -> Option<CallbackScheme> {
    // Case-insensitive prefix check without allocating.
    if url.len() >= JS_PREFIX.len()
        && url.as_bytes()[..JS_PREFIX.len()].eq_ignore_ascii_case(JS_PREFIX.as_bytes())
    {
        let raw_path = &url[JS_PREFIX.len()..];
        // Strip optional leading slash so both `javascript:///Objects/...`
        // and `javascript://Objects/...` produce the same path.
        let path = raw_path.strip_prefix('/').unwrap_or(raw_path);
        Some(CallbackScheme::JavaScript {
            path: path.into(),
        })
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Some(CallbackScheme::Http)
    } else {
        None
    }
}

/// Returns `true` if `url` uses the `javascript://` scheme.
pub fn is_javascript_callback(url: &str) -> bool {
    url.len() >= JS_PREFIX.len()
        && url.as_bytes()[..JS_PREFIX.len()].eq_ignore_ascii_case(JS_PREFIX.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_javascript_url() {
        let scheme = parse_callback_url("javascript://Objects/Device/MetaData/script");
        assert_eq!(
            scheme,
            Some(CallbackScheme::JavaScript {
                path: "Objects/Device/MetaData/script".into()
            })
        );
    }

    #[test]
    fn parse_javascript_url_triple_slash() {
        // Three slashes: authority is empty, path starts with `/`.
        let scheme = parse_callback_url("javascript:///Objects/Device/MetaData/script");
        assert_eq!(
            scheme,
            Some(CallbackScheme::JavaScript {
                path: "Objects/Device/MetaData/script".into()
            })
        );
    }

    #[test]
    fn parse_javascript_url_empty_path() {
        let scheme = parse_callback_url("javascript://");
        assert_eq!(
            scheme,
            Some(CallbackScheme::JavaScript {
                path: "".into()
            })
        );
    }

    #[test]
    fn parse_javascript_case_insensitive() {
        let scheme = parse_callback_url("JavaScript://Some/Path");
        assert_eq!(
            scheme,
            Some(CallbackScheme::JavaScript {
                path: "Some/Path".into()
            })
        );
    }

    #[test]
    fn parse_http_url() {
        assert_eq!(
            parse_callback_url("http://192.168.1.1/callback"),
            Some(CallbackScheme::Http)
        );
    }

    #[test]
    fn parse_https_url() {
        assert_eq!(
            parse_callback_url("https://example.com/cb"),
            Some(CallbackScheme::Http)
        );
    }

    #[test]
    fn parse_unknown_scheme() {
        assert_eq!(parse_callback_url("ftp://files/data"), None);
    }

    #[test]
    fn parse_empty_string() {
        assert_eq!(parse_callback_url(""), None);
    }

    #[test]
    fn parse_garbage() {
        assert_eq!(parse_callback_url("not-a-url"), None);
    }

    #[test]
    fn is_javascript_true() {
        assert!(is_javascript_callback("javascript://Objects/X"));
    }

    #[test]
    fn is_javascript_false_http() {
        assert!(!is_javascript_callback("http://example.com"));
    }

    #[test]
    fn is_javascript_false_empty() {
        assert!(!is_javascript_callback(""));
    }

    #[test]
    fn is_javascript_case_insensitive() {
        assert!(is_javascript_callback("JAVASCRIPT://Foo"));
    }
}
