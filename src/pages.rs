// In-memory storage for user-uploaded HTML pages.
//
// Pages are stored as raw bytes with an optional compression flag.
// When `compressed` is true the bytes are gzip-encoded; the GET handler
// serves them with `Content-Encoding: gzip` (zero CPU passthrough).
//
// Platform-independent (no ESP deps), fully unit-testable on host.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::psram::PsramBytes;

const DEFAULT_MAX_TOTAL: usize = 100 * 1024; // 100 KB
const MAX_SINGLE_PAGE: usize = 64 * 1024; // 64 KB

#[derive(Debug, PartialEq)]
pub enum PageError {
    InvalidPath,
    ReservedPath,
    PageTooLarge,
    StorageFull,
    NotFound,
}

/// A stored page: raw bytes plus a flag indicating gzip compression.
struct StoredPage {
    data: PsramBytes,
    compressed: bool,
}

pub struct PageStore {
    pages: BTreeMap<String, StoredPage>,
    /// Tracks total heap usage: path keys + page content bytes.
    total_bytes: usize,
    max_total_bytes: usize,
}

impl PageStore {
    pub fn new() -> Self {
        Self {
            pages: BTreeMap::new(),
            total_bytes: 0,
            max_total_bytes: DEFAULT_MAX_TOTAL,
        }
    }

    pub fn with_capacity(max_bytes: usize) -> Self {
        Self {
            pages: BTreeMap::new(),
            total_bytes: 0,
            max_total_bytes: max_bytes,
        }
    }

    /// Validate that the path is safe and not reserved.
    fn validate_path(path: &str) -> Result<(), PageError> {
        if !path.starts_with('/') {
            return Err(PageError::InvalidPath);
        }
        if path == "/" {
            return Err(PageError::ReservedPath);
        }
        // Reserved prefixes (exact match or with trailing slash)
        for prefix in &["/omi", "/odf", "/Objects"] {
            if path == *prefix || path.starts_with(&format!("{}/", prefix)) {
                return Err(PageError::ReservedPath);
            }
        }
        // Reject ".", ".." segments and empty segments
        for segment in path[1..].split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return Err(PageError::InvalidPath);
            }
        }
        Ok(())
    }

    /// Store an HTML page at the given path (uncompressed). Replaces any existing page.
    pub fn store(&mut self, path: &str, html: &str) -> Result<(), PageError> {
        self.store_bytes(path, html.as_bytes(), false)
    }

    /// Store raw bytes at the given path, with an optional compression flag.
    ///
    /// When `compressed` is true the bytes are assumed to be gzip-encoded and
    /// will be served back with `Content-Encoding: gzip`.
    pub fn store_bytes(&mut self, path: &str, data: &[u8], compressed: bool) -> Result<(), PageError> {
        Self::validate_path(path)?;

        if data.len() > MAX_SINGLE_PAGE {
            return Err(PageError::PageTooLarge);
        }

        // Reclaim old size if replacing (account for both path key and content)
        let is_new = !self.pages.contains_key(path);
        let old_size = self.pages.get(path).map(|p| p.data.len()).unwrap_or(0);
        let key_cost = if is_new { path.len() } else { 0 };
        let new_total = self.total_bytes - old_size + data.len() + key_cost;

        if new_total > self.max_total_bytes {
            return Err(PageError::StorageFull);
        }

        self.total_bytes = new_total;
        self.pages.insert(
            String::from(path),
            StoredPage {
                data: PsramBytes::from_bytes(data),
                compressed,
            },
        );
        Ok(())
    }

    /// Retrieve a page. Returns `(bytes, is_compressed)`.
    pub fn get(&self, path: &str) -> Option<(&[u8], bool)> {
        self.pages.get(path).map(|p| (p.data.as_bytes(), p.compressed))
    }

    /// Return sorted list of all stored paths.
    pub fn list(&self) -> Vec<&str> {
        self.pages.keys().map(|k| k.as_str()).collect()
    }

    pub fn remove(&mut self, path: &str) -> Result<(), PageError> {
        match self.pages.remove(path) {
            Some(page) => {
                self.total_bytes -= page.data.len() + path.len();
                Ok(())
            }
            None => Err(PageError::NotFound),
        }
    }
}

impl Default for PageStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_retrieve() {
        let mut store = PageStore::new();
        store.store("/hello", "<h1>Hi</h1>").unwrap();
        let (data, compressed) = store.get("/hello").unwrap();
        assert_eq!(data, b"<h1>Hi</h1>");
        assert!(!compressed);
    }

    #[test]
    fn store_compressed_bytes() {
        let mut store = PageStore::new();
        let fake_gzip = b"\x1f\x8b\x08compressed-data";
        store.store_bytes("/gz", fake_gzip, true).unwrap();
        let (data, compressed) = store.get("/gz").unwrap();
        assert_eq!(data, fake_gzip);
        assert!(compressed);
    }

    #[test]
    fn replace_plain_with_compressed() {
        let mut store = PageStore::new();
        store.store("/page", "<h1>Hi</h1>").unwrap();
        let (_, compressed) = store.get("/page").unwrap();
        assert!(!compressed);

        let gzip_data = b"\x1f\x8b\x08short";
        store.store_bytes("/page", gzip_data, true).unwrap();
        let (data, compressed) = store.get("/page").unwrap();
        assert_eq!(data, gzip_data);
        assert!(compressed);
    }

    #[test]
    fn reserved_root_rejected() {
        let mut store = PageStore::new();
        assert_eq!(store.store("/", "<h1>X</h1>"), Err(PageError::ReservedPath));
    }

    #[test]
    fn reserved_omi_rejected() {
        let mut store = PageStore::new();
        assert_eq!(
            store.store("/omi/test", "<h1>X</h1>"),
            Err(PageError::ReservedPath)
        );
        assert_eq!(
            store.store("/omi", "<h1>X</h1>"),
            Err(PageError::ReservedPath)
        );
    }

    #[test]
    fn reserved_odf_rejected() {
        let mut store = PageStore::new();
        assert_eq!(
            store.store("/odf/foo", "<h1>X</h1>"),
            Err(PageError::ReservedPath)
        );
        assert_eq!(
            store.store("/odf", "<h1>X</h1>"),
            Err(PageError::ReservedPath)
        );
    }

    #[test]
    fn reserved_objects_rejected() {
        let mut store = PageStore::new();
        assert_eq!(
            store.store("/Objects/1", "<h1>X</h1>"),
            Err(PageError::ReservedPath)
        );
        assert_eq!(
            store.store("/Objects", "<h1>X</h1>"),
            Err(PageError::ReservedPath)
        );
    }

    #[test]
    fn non_reserved_prefix_allowed() {
        let mut store = PageStore::new();
        // "/omission" should NOT be blocked — it's not "/omi" or "/omi/*"
        store.store("/omission", "<h1>X</h1>").unwrap();
        let (data, _) = store.get("/omission").unwrap();
        assert_eq!(data, b"<h1>X</h1>");
    }

    #[test]
    fn invalid_path_no_slash() {
        let mut store = PageStore::new();
        assert_eq!(
            store.store("hello", "<h1>X</h1>"),
            Err(PageError::InvalidPath)
        );
    }

    #[test]
    fn invalid_path_dotdot() {
        let mut store = PageStore::new();
        assert_eq!(
            store.store("/foo/../bar", "<h1>X</h1>"),
            Err(PageError::InvalidPath)
        );
    }

    #[test]
    fn invalid_path_empty_segment() {
        let mut store = PageStore::new();
        assert_eq!(
            store.store("/foo//bar", "<h1>X</h1>"),
            Err(PageError::InvalidPath)
        );
    }

    #[test]
    fn replace_updates_total_bytes() {
        let mut store = PageStore::with_capacity(200);
        // total_bytes includes path key ("/a" = 2) + value
        store.store("/a", "aaaa").unwrap(); // 2 + 4 = 6
        assert_eq!(store.total_bytes, 6);
        store.store("/a", "bb").unwrap(); // key already exists, so 6 - 4 + 2 = 4
        assert_eq!(store.total_bytes, 4);
        let (data, _) = store.get("/a").unwrap();
        assert_eq!(data, b"bb");
    }

    #[test]
    fn storage_cap_enforced() {
        // /a = 2 key + 3 value = 5, /b = 2 key + 3 value = 5, total = 10
        let mut store = PageStore::with_capacity(10);
        store.store("/a", "123").unwrap();
        store.store("/b", "123").unwrap();
        assert_eq!(
            store.store("/c", "1"),
            Err(PageError::StorageFull)
        );
    }

    #[test]
    fn single_page_too_large() {
        let mut store = PageStore::new();
        let big = "x".repeat(MAX_SINGLE_PAGE + 1);
        assert_eq!(store.store("/big", &big), Err(PageError::PageTooLarge));
    }

    #[test]
    fn list_sorted() {
        let mut store = PageStore::new();
        store.store("/z", "z").unwrap();
        store.store("/a", "a").unwrap();
        store.store("/m", "m").unwrap();
        assert_eq!(store.list(), vec!["/a", "/m", "/z"]);
    }

    #[test]
    fn remove_frees_space() {
        // /a = 2 key + 2 value = 4, /b = 2 key + 2 value = 4, total = 8
        let mut store = PageStore::with_capacity(8);
        store.store("/a", "12").unwrap();
        store.store("/b", "12").unwrap();
        assert_eq!(store.total_bytes, 8);
        store.remove("/a").unwrap();
        assert_eq!(store.total_bytes, 4);
        assert!(store.get("/a").is_none());
        // Now there's room again
        store.store("/c", "12").unwrap();
    }

    #[test]
    fn remove_not_found() {
        let mut store = PageStore::new();
        assert_eq!(store.remove("/nope"), Err(PageError::NotFound));
    }

    #[test]
    fn invalid_path_dot_segment() {
        let mut store = PageStore::new();
        assert_eq!(
            store.store("/foo/./bar", "<h1>X</h1>"),
            Err(PageError::InvalidPath)
        );
    }

    #[test]
    fn total_bytes_includes_path_key() {
        let mut store = PageStore::with_capacity(100);
        // "/hello" = 6 bytes key + "x" = 1 byte value = 7 total
        store.store("/hello", "x").unwrap();
        assert_eq!(store.total_bytes, 7);
        // Removing frees both key and value
        store.remove("/hello").unwrap();
        assert_eq!(store.total_bytes, 0);
    }

    #[test]
    fn store_bytes_size_tracking() {
        let mut store = PageStore::with_capacity(100);
        let data = b"\x1f\x8b\x08abcde";
        store.store_bytes("/p", data, true).unwrap();
        // "/p" = 2 key + 8 bytes data = 10
        assert_eq!(store.total_bytes, 10);
    }
}
