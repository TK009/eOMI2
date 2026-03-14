use std::collections::BTreeMap;

#[cfg(feature = "json")]
use serde::{Deserialize, Serialize, Serializer};
#[cfg(feature = "json")]
use serde::ser::SerializeMap;

use crate::odf::{OmiValue, Object};
use super::error::ParseError;

/// Maximum nesting depth for Object trees in write operations.
/// Limits recursive serde deserialization to stay within the HTTP thread stack.
/// NOTE: If you change this value, also update `HTTP_THREAD_STACK` in `server.rs`.
pub(crate) const MAX_OBJECT_DEPTH: usize = 8;

/// Count the maximum Object nesting depth in a parsed objects map, iteratively.
///
/// Returns 0 if the map is empty.
pub(crate) fn parsed_object_tree_depth(objects: &BTreeMap<String, Object>) -> usize {
    let mut max_depth: usize = 0;
    let mut stack: Vec<(&Object, usize)> = Vec::new();
    for obj in objects.values() {
        stack.push((obj, 1));
    }
    while let Some((obj, depth)) = stack.pop() {
        if depth > max_depth {
            max_depth = depth;
        }
        if let Some(children) = &obj.objects {
            for child in children.values() {
                stack.push((child, depth + 1));
            }
        }
    }
    max_depth
}

#[derive(Debug, Clone, PartialEq)]
pub enum WriteOp {
    Single {
        path: String,
        v: OmiValue,
        t: Option<f64>,
    },
    Batch {
        items: Vec<WriteItem>,
    },
    Tree {
        path: String,
        objects: BTreeMap<String, Object>,
    },
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(Serialize, Deserialize))]
pub struct WriteItem {
    pub path: String,
    pub v: OmiValue,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub t: Option<f64>,
}

#[cfg(feature = "json")]
#[derive(Deserialize)]
struct RawWriteOp {
    path: Option<String>,
    v: Option<OmiValue>,
    t: Option<f64>,
    items: Option<Vec<WriteItem>>,
    objects: Option<BTreeMap<String, Object>>,
}

#[cfg(feature = "json")]
/// Count the maximum Object nesting depth in a JSON `"objects"` map, iteratively.
///
/// Expects `value` to be the top-level `"objects"` map (`{ "Id": { "id": ..., "objects": ... }, ... }`).
/// Returns 0 if `value` is not an object.
fn object_nesting_depth(value: &serde_json::Value) -> usize {
    let top_map = match value.as_object() {
        Some(m) => m,
        None => return 0,
    };

    let mut max_depth: usize = 0;
    // Explicit stack: (json_value_of_an_Object, depth)
    let mut stack: Vec<(&serde_json::Value, usize)> = Vec::new();
    for obj in top_map.values() {
        stack.push((obj, 1));
    }

    while let Some((v, depth)) = stack.pop() {
        if depth > max_depth {
            max_depth = depth;
        }
        if let Some(map) = v.as_object() {
            if let Some(serde_json::Value::Object(children)) = map.get("objects") {
                for child in children.values() {
                    stack.push((child, depth + 1));
                }
            }
        }
    }

    max_depth
}

#[cfg(feature = "json")]
impl WriteOp {
    #[cfg(feature = "json")]
    pub fn from_value(value: serde_json::Value) -> Result<Self, ParseError> {
        // Guard against deeply nested object trees that would overflow the stack
        // during recursive serde deserialization.
        if let Some(objects) = value.get("objects") {
            let depth = object_nesting_depth(objects);
            if depth > MAX_OBJECT_DEPTH {
                return Err(ParseError::InvalidField {
                    field: "objects",
                    reason: format!(
                        "nesting depth {} exceeds maximum of {}",
                        depth, MAX_OBJECT_DEPTH
                    ),
                });
            }
        }

        let raw: RawWriteOp = serde_json::from_value(value)
            .map_err(|e| ParseError::InvalidJson(e.to_string()))?;

        let has_v = raw.v.is_some();
        let has_items = raw.items.is_some();
        let has_objects = raw.objects.is_some();

        let form_count = has_v as u8 + has_items as u8 + has_objects as u8;

        if form_count == 0 {
            return Err(ParseError::MissingField("v, items, or objects"));
        }
        if has_v && has_items {
            return Err(ParseError::MutuallyExclusive("v", "items"));
        }
        if has_v && has_objects {
            return Err(ParseError::MutuallyExclusive("v", "objects"));
        }
        if has_items && has_objects {
            return Err(ParseError::MutuallyExclusive("items", "objects"));
        }

        if has_v {
            let path = raw.path.ok_or(ParseError::MissingField("path"))?;
            Ok(WriteOp::Single {
                path,
                v: raw.v.unwrap(),
                t: raw.t,
            })
        } else if has_items {
            let items = raw.items.unwrap();
            if items.is_empty() {
                return Err(ParseError::InvalidField {
                    field: "items",
                    reason: "items array must not be empty".into(),
                });
            }
            Ok(WriteOp::Batch { items })
        } else {
            let path = raw.path.ok_or(ParseError::MissingField("path"))?;
            Ok(WriteOp::Tree {
                path,
                objects: raw.objects.unwrap(),
            })
        }
    }
}

#[cfg(feature = "json")]
impl Serialize for WriteOp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            WriteOp::Single { path, v, t } => {
                let len = 2 + t.is_some() as usize;
                let mut map = serializer.serialize_map(Some(len))?;
                map.serialize_entry("path", path)?;
                map.serialize_entry("v", v)?;
                if let Some(t) = t {
                    map.serialize_entry("t", t)?;
                }
                map.end()
            }
            WriteOp::Batch { items } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("items", items)?;
                map.end()
            }
            WriteOp::Tree { path, objects } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("path", path)?;
                map.serialize_entry("objects", objects)?;
                map.end()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odf::OmiValue;

    #[test]
    fn write_item_fields() {
        let item = WriteItem {
            path: "/A/B".into(),
            v: OmiValue::Number(42.0),
            t: Some(1000.0),
        };
        assert_eq!(item.path, "/A/B");
        assert_eq!(item.v, OmiValue::Number(42.0));
        assert_eq!(item.t, Some(1000.0));
    }

    #[cfg(feature = "json")]
    mod json {
        use super::*;

        #[test]
        fn from_value_single() {
            let v = serde_json::json!({
                "path": "/DeviceA/Temperature",
                "v": 22.5
            });
            let op = WriteOp::from_value(v).unwrap();
            match op {
                WriteOp::Single { path, v, t } => {
                    assert_eq!(path, "/DeviceA/Temperature");
                    assert_eq!(v, OmiValue::Number(22.5));
                    assert!(t.is_none());
                }
                _ => panic!("expected Single"),
            }
        }

        #[test]
        fn from_value_single_with_timestamp() {
            let v = serde_json::json!({
                "path": "/A/B",
                "v": true,
                "t": 1000.0
            });
            let op = WriteOp::from_value(v).unwrap();
            match op {
                WriteOp::Single { path, v, t } => {
                    assert_eq!(path, "/A/B");
                    assert_eq!(v, OmiValue::Bool(true));
                    assert_eq!(t, Some(1000.0));
                }
                _ => panic!("expected Single"),
            }
        }

        #[test]
        fn from_value_batch() {
            let v = serde_json::json!({
                "items": [
                    { "path": "/A/B", "v": 1.0 },
                    { "path": "/A/C", "v": "hello" }
                ]
            });
            let op = WriteOp::from_value(v).unwrap();
            match op {
                WriteOp::Batch { items } => {
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].path, "/A/B");
                    assert_eq!(items[1].v, OmiValue::Str("hello".into()));
                }
                _ => panic!("expected Batch"),
            }
        }

        #[test]
        fn from_value_tree() {
            let v = serde_json::json!({
                "path": "/",
                "objects": {
                    "DeviceA": {
                        "id": "DeviceA"
                    }
                }
            });
            let op = WriteOp::from_value(v).unwrap();
            match op {
                WriteOp::Tree { path, objects } => {
                    assert_eq!(path, "/");
                    assert!(objects.contains_key("DeviceA"));
                }
                _ => panic!("expected Tree"),
            }
        }

        #[test]
        fn reject_v_and_items() {
            let v = serde_json::json!({
                "path": "/A/B",
                "v": 1.0,
                "items": [{ "path": "/C/D", "v": 2.0 }]
            });
            assert_eq!(
                WriteOp::from_value(v).unwrap_err(),
                ParseError::MutuallyExclusive("v", "items")
            );
        }

        #[test]
        fn reject_v_and_objects() {
            let v = serde_json::json!({
                "path": "/A",
                "v": 1.0,
                "objects": { "X": { "id": "X" } }
            });
            assert_eq!(
                WriteOp::from_value(v).unwrap_err(),
                ParseError::MutuallyExclusive("v", "objects")
            );
        }

        #[test]
        fn reject_items_and_objects() {
            let v = serde_json::json!({
                "items": [{ "path": "/A/B", "v": 1.0 }],
                "objects": { "X": { "id": "X" } }
            });
            assert_eq!(
                WriteOp::from_value(v).unwrap_err(),
                ParseError::MutuallyExclusive("items", "objects")
            );
        }

        #[test]
        fn reject_no_form() {
            let v = serde_json::json!({ "path": "/A/B" });
            assert_eq!(
                WriteOp::from_value(v).unwrap_err(),
                ParseError::MissingField("v, items, or objects")
            );
        }

        #[test]
        fn reject_single_without_path() {
            let v = serde_json::json!({ "v": 42 });
            assert_eq!(
                WriteOp::from_value(v).unwrap_err(),
                ParseError::MissingField("path")
            );
        }

        #[test]
        fn reject_empty_items() {
            let v = serde_json::json!({ "items": [] });
            assert_eq!(
                WriteOp::from_value(v).unwrap_err(),
                ParseError::InvalidField {
                    field: "items",
                    reason: "items array must not be empty".into(),
                }
            );
        }

        #[test]
        fn serialize_single() {
            let op = WriteOp::Single {
                path: "/A/B".into(),
                v: OmiValue::Number(42.0),
                t: None,
            };
            let json = serde_json::to_value(&op).unwrap();
            assert_eq!(json["path"], "/A/B");
            assert_eq!(json["v"], 42.0);
            assert!(json.get("t").is_none());
        }

        #[test]
        fn serialize_single_with_timestamp() {
            let op = WriteOp::Single {
                path: "/A/B".into(),
                v: OmiValue::Bool(true),
                t: Some(1000.0),
            };
            let json = serde_json::to_value(&op).unwrap();
            assert_eq!(json["path"], "/A/B");
            assert_eq!(json["v"], true);
            assert_eq!(json["t"], 1000.0);
        }

        #[test]
        fn serialize_batch() {
            let op = WriteOp::Batch {
                items: vec![
                    WriteItem { path: "/A/B".into(), v: OmiValue::Number(1.0), t: None },
                    WriteItem { path: "/C/D".into(), v: OmiValue::Str("x".into()), t: Some(2.0) },
                ],
            };
            let json = serde_json::to_value(&op).unwrap();
            let items = json["items"].as_array().unwrap();
            assert_eq!(items.len(), 2);
            assert_eq!(items[0]["path"], "/A/B");
            assert_eq!(items[1]["t"], 2.0);
        }

        #[test]
        fn serialize_tree() {
            let mut objects = BTreeMap::new();
            objects.insert("Dev".into(), Object::new("Dev"));
            let op = WriteOp::Tree {
                path: "/".into(),
                objects,
            };
            let json = serde_json::to_value(&op).unwrap();
            assert_eq!(json["path"], "/");
            assert!(json["objects"]["Dev"].is_object());
        }

        #[test]
        fn serialize_roundtrip_single() {
            let op = WriteOp::Single {
                path: "/A/B".into(),
                v: OmiValue::Number(3.14),
                t: Some(500.0),
            };
            let json = serde_json::to_value(&op).unwrap();
            let op2 = WriteOp::from_value(json).unwrap();
            assert_eq!(op, op2);
        }

        #[test]
        fn serialize_roundtrip_batch() {
            let op = WriteOp::Batch {
                items: vec![
                    WriteItem { path: "/X/Y".into(), v: OmiValue::Str("hi".into()), t: None },
                ],
            };
            let json = serde_json::to_value(&op).unwrap();
            let op2 = WriteOp::from_value(json).unwrap();
            assert_eq!(op, op2);
        }

        /// Build a tree write JSON value with `depth` levels of Object nesting.
        fn make_nested_tree(depth: usize) -> serde_json::Value {
            assert!(depth >= 1);
            let mut current = serde_json::json!({"id": format!("L{}", depth)});
            for i in (1..depth).rev() {
                let id = format!("L{}", i);
                let child_id = format!("L{}", i + 1);
                current = serde_json::json!({
                    "id": id,
                    "objects": { child_id: current }
                });
            }
            serde_json::json!({
                "path": "/",
                "objects": { "L1": current }
            })
        }

        #[test]
        fn object_nesting_depth_flat() {
            let objects = serde_json::json!({"A": {"id": "A"}, "B": {"id": "B"}});
            assert_eq!(object_nesting_depth(&objects), 1);
        }

        #[test]
        fn object_nesting_depth_nested() {
            let objects = serde_json::json!({
                "A": {
                    "id": "A",
                    "objects": {
                        "B": {
                            "id": "B",
                            "objects": {
                                "C": { "id": "C" }
                            }
                        }
                    }
                }
            });
            assert_eq!(object_nesting_depth(&objects), 3);
        }

        #[test]
        fn object_nesting_depth_non_object() {
            assert_eq!(object_nesting_depth(&serde_json::json!(42)), 0);
            assert_eq!(object_nesting_depth(&serde_json::json!(null)), 0);
        }

        #[test]
        fn from_value_tree_depth_at_limit() {
            let v = make_nested_tree(MAX_OBJECT_DEPTH);
            let op = WriteOp::from_value(v).unwrap();
            match op {
                WriteOp::Tree { path, .. } => assert_eq!(path, "/"),
                _ => panic!("expected Tree"),
            }
        }

        #[test]
        fn from_value_tree_depth_exceeded() {
            let v = make_nested_tree(MAX_OBJECT_DEPTH + 1);
            let err = WriteOp::from_value(v).unwrap_err();
            match err {
                ParseError::InvalidField { field, reason } => {
                    assert_eq!(field, "objects");
                    assert!(reason.contains("nesting depth"), "{}", reason);
                }
                _ => panic!("expected InvalidField, got {:?}", err),
            }
        }
    }
}
