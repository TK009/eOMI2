// HTTP server helpers.
//
// Pure functions — no ESP deps — so they're testable on the host.

use crate::pages::PageStore;

/// Render the landing page HTML, including links to all stored pages.
pub fn render_landing_page(store: &PageStore) -> String {
    let pages = store.list();
    let list_html = if pages.is_empty() {
        String::from("<p>No pages stored yet.</p>")
    } else {
        let mut s = String::from("<ul>");
        for path in &pages {
            s.push_str("<li><a href=\"");
            s.push_str(path);
            s.push_str("\">");
            s.push_str(path);
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
