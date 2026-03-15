//! Resolve script text from `javascript://` URLs pointing to MetaData InfoItems.
//!
//! A `javascript://` URL contains an O-DF path that references a MetaData
//! InfoItem in the object tree. The InfoItem's most recent value is read as
//! script source text.
//!
//! A "MetaData InfoItem" is an InfoItem that lives under an Object named
//! `MetaData` in the O-DF hierarchy. For example:
//!
//! ```text
//! /DeviceA/MetaData/calibration  → MetaData InfoItem (valid script target)
//! /DeviceA/Temperature           → regular InfoItem (not a valid target)
//! ```

use crate::odf::{ObjectTree, OmiValue, PathTarget, TreeError};

/// The URL scheme prefix for script references.
const JAVASCRIPT_SCHEME: &str = "javascript://";

/// The special path segment that marks an Object as a MetaData container.
const METADATA_SEGMENT: &str = "MetaData";

/// Errors from resolving a MetaData InfoItem script reference.
#[derive(Debug, PartialEq)]
pub enum ScriptResolveError {
    /// The URL is not a valid `javascript://` URL.
    InvalidUrl(String),
    /// The O-DF path could not be resolved in the tree.
    PathNotFound(String),
    /// The resolved target is not an InfoItem.
    NotInfoItem(String),
    /// The target InfoItem is not under a `MetaData` ancestor Object.
    NotMetaData(String),
    /// The target MetaData InfoItem has no values.
    EmptyValue(String),
    /// The most recent value is not a string.
    NonStringValue(String),
}

impl core::fmt::Display for ScriptResolveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidUrl(msg) => write!(f, "invalid javascript:// URL: {}", msg),
            Self::PathNotFound(msg) => write!(f, "path not found: {}", msg),
            Self::NotInfoItem(path) => write!(f, "target is not an InfoItem: {}", path),
            Self::NotMetaData(path) => {
                write!(f, "target is not a MetaData InfoItem: {}", path)
            }
            Self::EmptyValue(path) => {
                write!(f, "MetaData InfoItem has no values: {}", path)
            }
            Self::NonStringValue(path) => {
                write!(f, "MetaData InfoItem value is not a string: {}", path)
            }
        }
    }
}

impl std::error::Error for ScriptResolveError {}

/// Returns `true` if `url` uses the `javascript://` scheme.
pub fn is_javascript_url(url: &str) -> bool {
    url.starts_with(JAVASCRIPT_SCHEME)
}

/// Extract the O-DF path from a `javascript://` URL.
///
/// Accepts both `javascript:///path` (with authority) and `javascript://path`.
fn parse_javascript_url(url: &str) -> Result<&str, ScriptResolveError> {
    let rest = url
        .strip_prefix(JAVASCRIPT_SCHEME)
        .ok_or_else(|| ScriptResolveError::InvalidUrl("missing javascript:// prefix".into()))?;

    if rest.is_empty() {
        return Err(ScriptResolveError::InvalidUrl("empty path".into()));
    }

    // The path must start with '/' (O-DF paths are absolute)
    if !rest.starts_with('/') {
        return Err(ScriptResolveError::InvalidUrl(format!(
            "path must start with '/': {}",
            rest
        )));
    }

    Ok(rest)
}

/// Check if a path contains a `MetaData` ancestor segment.
///
/// A MetaData InfoItem is one whose path includes a `MetaData` segment
/// somewhere before the final (leaf) segment. For example:
/// - `/Dev/MetaData/script` → true (leaf `script` is under `MetaData`)
/// - `/Dev/MetaData/sub/script` → true
/// - `/Dev/Temperature` → false
/// - `/MetaData` → false (MetaData itself is an Object, not an InfoItem under MetaData)
fn is_metadata_path(path: &str) -> bool {
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    let segments: Vec<&str> = trimmed.split('/').collect();

    // Need at least 3 segments: Object / MetaData / InfoItem
    // (first segment is the root object, one must be MetaData, last is the leaf)
    if segments.len() < 3 {
        return false;
    }

    // Check if any non-last segment is "MetaData"
    segments[..segments.len() - 1]
        .iter()
        .any(|&s| s == METADATA_SEGMENT)
}

/// Resolve a `javascript://` URL to script text from a MetaData InfoItem.
///
/// 1. Parses the URL to extract the O-DF path
/// 2. Resolves the path in the object tree
/// 3. Validates the target is a MetaData InfoItem (not a regular InfoItem)
/// 4. Reads the most recent value as script text
///
/// # Errors
///
/// Returns [`ScriptResolveError`] for:
/// - Malformed URLs
/// - Missing paths
/// - Non-MetaData targets
/// - Empty values
/// - Non-string values
pub fn resolve_script_url(tree: &ObjectTree, url: &str) -> Result<String, ScriptResolveError> {
    let path = parse_javascript_url(url)?;

    // Resolve the path in the tree
    let target = tree.resolve(path).map_err(|e| match e {
        TreeError::NotFound(msg) => ScriptResolveError::PathNotFound(msg),
        TreeError::InvalidPath(msg) => ScriptResolveError::InvalidUrl(msg),
        TreeError::Forbidden(msg) => ScriptResolveError::PathNotFound(msg),
        #[cfg(feature = "json")]
        TreeError::SerializationError(msg) => ScriptResolveError::PathNotFound(msg),
    })?;

    // Must resolve to an InfoItem
    let item = match target {
        PathTarget::InfoItem(item) => item,
        PathTarget::Object(_) | PathTarget::Root(_) => {
            return Err(ScriptResolveError::NotInfoItem(path.into()));
        }
    };

    // Must be a MetaData InfoItem (under a MetaData ancestor)
    if !is_metadata_path(path) {
        return Err(ScriptResolveError::NotMetaData(path.into()));
    }

    // Read the most recent value
    let values = item.query_values(Some(1), None, None, None);
    let newest = values
        .first()
        .ok_or_else(|| ScriptResolveError::EmptyValue(path.into()))?;

    // Value must be a string (script text)
    match &newest.v {
        OmiValue::Str(s) if s.is_empty() => Err(ScriptResolveError::EmptyValue(path.into())),
        OmiValue::Str(s) => Ok(s.clone()),
        _ => Err(ScriptResolveError::NonStringValue(path.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odf::{InfoItem, Object, ObjectTree, OmiValue};

    // --- is_javascript_url ---

    #[test]
    fn detects_javascript_url() {
        assert!(is_javascript_url("javascript:///Dev/MetaData/script"));
        assert!(is_javascript_url("javascript://"));
        assert!(!is_javascript_url("http://example.com"));
        assert!(!is_javascript_url("event.value * 2"));
        assert!(!is_javascript_url(""));
    }

    // --- parse_javascript_url ---

    #[test]
    fn parse_valid_url() {
        let path = parse_javascript_url("javascript:///Dev/MetaData/script").unwrap();
        assert_eq!(path, "/Dev/MetaData/script");
    }

    #[test]
    fn parse_url_deep_path() {
        let path =
            parse_javascript_url("javascript:///A/B/MetaData/C/script").unwrap();
        assert_eq!(path, "/A/B/MetaData/C/script");
    }

    #[test]
    fn parse_url_empty_path_errors() {
        assert_eq!(
            parse_javascript_url("javascript://"),
            Err(ScriptResolveError::InvalidUrl("empty path".into()))
        );
    }

    #[test]
    fn parse_url_no_leading_slash_errors() {
        assert_eq!(
            parse_javascript_url("javascript://Dev/MetaData/x"),
            Err(ScriptResolveError::InvalidUrl(
                "path must start with '/': Dev/MetaData/x".into()
            ))
        );
    }

    #[test]
    fn parse_url_wrong_scheme_errors() {
        assert!(parse_javascript_url("http:///path").is_err());
    }

    // --- is_metadata_path ---

    #[test]
    fn metadata_path_detected() {
        assert!(is_metadata_path("/Dev/MetaData/script"));
        assert!(is_metadata_path("/Dev/MetaData/sub/script"));
        assert!(is_metadata_path("/A/B/MetaData/C"));
    }

    #[test]
    fn non_metadata_path_rejected() {
        assert!(!is_metadata_path("/Dev/Temperature"));
        assert!(!is_metadata_path("/MetaData"));
        assert!(!is_metadata_path("/Dev/MetaData")); // MetaData is the last segment (Object)
        assert!(!is_metadata_path("/"));
    }

    // --- resolve_script_url ---

    /// Build a tree with a MetaData InfoItem containing script text.
    fn tree_with_metadata_script(script: &str) -> ObjectTree {
        let mut tree = ObjectTree::new();

        let mut dev = Object::new("Dev");
        let mut meta_obj = Object::new("MetaData");

        let mut script_item = InfoItem::new(10);
        script_item.add_value(OmiValue::Str(script.into()), Some(1000.0));
        meta_obj.add_item("onread".into(), script_item);

        dev.add_child(meta_obj);

        // Also add a regular InfoItem
        let mut temp = InfoItem::new(10);
        temp.add_value(OmiValue::Number(22.5), Some(1000.0));
        dev.add_item("Temperature".into(), temp);

        tree.insert_root(dev);
        tree
    }

    #[test]
    fn resolve_valid_metadata_script() {
        let tree = tree_with_metadata_script("event.value * 0.01 - 40");
        let script =
            resolve_script_url(&tree, "javascript:///Dev/MetaData/onread").unwrap();
        assert_eq!(script, "event.value * 0.01 - 40");
    }

    #[test]
    fn resolve_returns_newest_value() {
        let mut tree = ObjectTree::new();
        let mut dev = Object::new("Dev");
        let mut meta_obj = Object::new("MetaData");

        let mut script_item = InfoItem::new(10);
        script_item.add_value(OmiValue::Str("old script".into()), Some(1.0));
        script_item.add_value(OmiValue::Str("new script".into()), Some(2.0));
        meta_obj.add_item("handler".into(), script_item);

        dev.add_child(meta_obj);
        tree.insert_root(dev);

        let script =
            resolve_script_url(&tree, "javascript:///Dev/MetaData/handler").unwrap();
        assert_eq!(script, "new script");
    }

    #[test]
    fn resolve_missing_path_errors() {
        let tree = tree_with_metadata_script("x");
        let err =
            resolve_script_url(&tree, "javascript:///Missing/MetaData/script").unwrap_err();
        assert!(matches!(err, ScriptResolveError::PathNotFound(_)));
    }

    #[test]
    fn resolve_regular_infoitem_errors() {
        let tree = tree_with_metadata_script("x");
        let err =
            resolve_script_url(&tree, "javascript:///Dev/Temperature").unwrap_err();
        assert!(matches!(err, ScriptResolveError::NotMetaData(_)));
    }

    #[test]
    fn resolve_object_target_errors() {
        let tree = tree_with_metadata_script("x");
        let err =
            resolve_script_url(&tree, "javascript:///Dev/MetaData").unwrap_err();
        assert!(matches!(err, ScriptResolveError::NotInfoItem(_)));
    }

    #[test]
    fn resolve_empty_value_errors() {
        let mut tree = ObjectTree::new();
        let mut dev = Object::new("Dev");
        let mut meta_obj = Object::new("MetaData");

        // InfoItem with no values
        let script_item = InfoItem::new(10);
        meta_obj.add_item("empty".into(), script_item);

        dev.add_child(meta_obj);
        tree.insert_root(dev);

        let err =
            resolve_script_url(&tree, "javascript:///Dev/MetaData/empty").unwrap_err();
        assert!(matches!(err, ScriptResolveError::EmptyValue(_)));
    }

    #[test]
    fn resolve_empty_string_value_errors() {
        let mut tree = ObjectTree::new();
        let mut dev = Object::new("Dev");
        let mut meta_obj = Object::new("MetaData");

        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Str(String::new()), Some(1.0));
        meta_obj.add_item("blank".into(), item);

        dev.add_child(meta_obj);
        tree.insert_root(dev);

        let err =
            resolve_script_url(&tree, "javascript:///Dev/MetaData/blank").unwrap_err();
        assert!(matches!(err, ScriptResolveError::EmptyValue(_)));
    }

    #[test]
    fn resolve_non_string_value_errors() {
        let mut tree = ObjectTree::new();
        let mut dev = Object::new("Dev");
        let mut meta_obj = Object::new("MetaData");

        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Number(42.0), Some(1.0));
        meta_obj.add_item("numeric".into(), item);

        dev.add_child(meta_obj);
        tree.insert_root(dev);

        let err =
            resolve_script_url(&tree, "javascript:///Dev/MetaData/numeric").unwrap_err();
        assert!(matches!(err, ScriptResolveError::NonStringValue(_)));
    }

    #[test]
    fn resolve_deep_metadata_path() {
        let mut tree = ObjectTree::new();
        let mut root = Object::new("Building");
        let mut floor = Object::new("Floor1");
        let mut meta_obj = Object::new("MetaData");

        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Str("deep script".into()), Some(1.0));
        meta_obj.add_item("handler".into(), item);

        floor.add_child(meta_obj);
        root.add_child(floor);
        tree.insert_root(root);

        let script = resolve_script_url(
            &tree,
            "javascript:///Building/Floor1/MetaData/handler",
        )
        .unwrap();
        assert_eq!(script, "deep script");
    }

    #[test]
    fn resolve_invalid_url_errors() {
        let tree = ObjectTree::new();
        let err = resolve_script_url(&tree, "not-a-url").unwrap_err();
        assert!(matches!(err, ScriptResolveError::InvalidUrl(_)));
    }

    // --- Display ---

    #[test]
    fn display_all_variants() {
        let variants = vec![
            ScriptResolveError::InvalidUrl("bad".into()),
            ScriptResolveError::PathNotFound("missing".into()),
            ScriptResolveError::NotInfoItem("/Dev".into()),
            ScriptResolveError::NotMetaData("/Dev/Temp".into()),
            ScriptResolveError::EmptyValue("/Dev/MetaData/x".into()),
            ScriptResolveError::NonStringValue("/Dev/MetaData/x".into()),
        ];
        for v in &variants {
            let msg = v.to_string();
            assert!(!msg.is_empty());
        }
    }

    #[test]
    fn implements_std_error() {
        let e: Box<dyn std::error::Error> =
            Box::new(ScriptResolveError::InvalidUrl("test".into()));
        assert!(e.source().is_none());
    }
}
