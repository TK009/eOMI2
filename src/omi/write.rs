use std::collections::BTreeMap;

use serde::{Deserialize, Serialize, Serializer};
use serde::ser::SerializeMap;

use crate::odf::{OmiValue, Object};
use super::error::ParseError;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteItem {
    pub path: String,
    pub v: OmiValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<f64>,
}

#[derive(Deserialize)]
struct RawWriteOp {
    path: Option<String>,
    v: Option<OmiValue>,
    t: Option<f64>,
    items: Option<Vec<WriteItem>>,
    objects: Option<BTreeMap<String, Object>>,
}

impl WriteOp {
    pub fn from_value(value: serde_json::Value) -> Result<Self, ParseError> {
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
    }
}
