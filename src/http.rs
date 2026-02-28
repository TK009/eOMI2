// HTTP server helpers.
//
// Pure functions — no ESP deps — so they're testable on the host.

use crate::pages::PageStore;

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
}
