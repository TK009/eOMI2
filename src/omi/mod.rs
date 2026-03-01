pub mod engine;
pub mod error;
pub mod read;
pub mod write;
pub mod delete;
pub mod cancel;
pub mod response;
pub mod subscriptions;

pub use self::engine::Engine;

use serde::{Deserialize, Serialize, Serializer};
use serde::ser::SerializeMap;

use self::error::ParseError;
pub use self::read::{ReadKind, ReadOp};
use self::write::WriteOp;
use self::delete::DeleteOp;
use self::cancel::CancelOp;
use self::response::ResponseBody;
pub use self::write::WriteItem;
pub use self::response::{StatusCode, ResponseResult, ItemStatus, OmiResponse};
pub use self::subscriptions::Delivery;

#[derive(Debug, Clone, PartialEq)]
pub struct OmiMessage {
    pub version: String,
    pub ttl: i64,
    pub operation: Operation,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operation {
    Read(ReadOp),
    Write(WriteOp),
    Delete(DeleteOp),
    Cancel(CancelOp),
    Response(ResponseBody),
}

#[derive(Deserialize)]
struct RawEnvelope {
    omi: Option<String>,
    ttl: Option<i64>,
    read: Option<serde_json::Value>,
    write: Option<serde_json::Value>,
    delete: Option<serde_json::Value>,
    cancel: Option<serde_json::Value>,
    response: Option<serde_json::Value>,
}

impl OmiMessage {
    pub fn parse(json: &str) -> Result<Self, ParseError> {
        let raw: RawEnvelope =
            serde_json::from_str(json).map_err(|e| ParseError::InvalidJson(e.to_string()))?;

        // Validate version
        let version = raw.omi.ok_or(ParseError::MissingField("omi"))?;
        if version != "1.0" {
            return Err(ParseError::UnsupportedVersion(version));
        }

        // Validate ttl
        let ttl = raw.ttl.ok_or(ParseError::MissingField("ttl"))?;

        // Exactly one operation
        let op_count = raw.read.is_some() as usize
            + raw.write.is_some() as usize
            + raw.delete.is_some() as usize
            + raw.cancel.is_some() as usize
            + raw.response.is_some() as usize;

        if op_count != 1 {
            return Err(ParseError::InvalidOperationCount(op_count));
        }

        let operation = if let Some(v) = raw.read {
            Operation::Read(ReadOp::from_value(v)?)
        } else if let Some(v) = raw.write {
            Operation::Write(WriteOp::from_value(v)?)
        } else if let Some(v) = raw.delete {
            Operation::Delete(DeleteOp::from_value(v)?)
        } else if let Some(v) = raw.cancel {
            Operation::Cancel(CancelOp::from_value(v)?)
        } else if let Some(v) = raw.response {
            Operation::Response(ResponseBody::from_value(v)?)
        } else {
            unreachable!()
        };

        Ok(OmiMessage {
            version,
            ttl,
            operation,
        })
    }
}

impl Serialize for OmiMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("omi", &self.version)?;
        map.serialize_entry("ttl", &self.ttl)?;
        match &self.operation {
            Operation::Read(op) => map.serialize_entry("read", op)?,
            Operation::Write(op) => map.serialize_entry("write", op)?,
            Operation::Delete(op) => map.serialize_entry("delete", op)?,
            Operation::Cancel(op) => map.serialize_entry("cancel", op)?,
            Operation::Response(body) => map.serialize_entry("response", body)?,
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "json")]
    mod json {
        use super::*;

        // --- Envelope parsing ---

        #[test]
        fn parse_read_message() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "read": {
                    "path": "/DeviceA/Temperature"
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            assert_eq!(msg.version, "1.0");
            assert_eq!(msg.ttl, 0);
            match &msg.operation {
                Operation::Read(op) => {
                    assert_eq!(op.path.as_deref(), Some("/DeviceA/Temperature"));
                }
                _ => panic!("expected Read"),
            }
        }

        #[test]
        fn parse_write_message() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 10,
                "write": {
                    "path": "/A/B",
                    "v": 42
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            assert_eq!(msg.ttl, 10);
            assert!(matches!(msg.operation, Operation::Write(_)));
        }

        #[test]
        fn parse_delete_message() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "delete": {
                    "path": "/DeviceA"
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            assert!(matches!(msg.operation, Operation::Delete(_)));
        }

        #[test]
        fn parse_cancel_message() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "cancel": {
                    "rid": ["req-1"]
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            assert!(matches!(msg.operation, Operation::Cancel(_)));
        }

        #[test]
        fn parse_response_message() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "response": {
                    "status": 200,
                    "result": { "temperature": 22.5 }
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Response(body) => {
                    assert_eq!(body.status, 200);
                }
                _ => panic!("expected Response"),
            }
        }

        #[test]
        fn reject_missing_omi() {
            let json = r#"{ "ttl": 0, "read": { "path": "/A" } }"#;
            assert_eq!(
                OmiMessage::parse(json).unwrap_err(),
                ParseError::MissingField("omi")
            );
        }

        #[test]
        fn reject_wrong_version() {
            let json = r#"{ "omi": "2.0", "ttl": 0, "read": { "path": "/A" } }"#;
            assert_eq!(
                OmiMessage::parse(json).unwrap_err(),
                ParseError::UnsupportedVersion("2.0".into())
            );
        }

        #[test]
        fn reject_missing_ttl() {
            let json = r#"{ "omi": "1.0", "read": { "path": "/A" } }"#;
            assert_eq!(
                OmiMessage::parse(json).unwrap_err(),
                ParseError::MissingField("ttl")
            );
        }

        #[test]
        fn reject_zero_operations() {
            let json = r#"{ "omi": "1.0", "ttl": 0 }"#;
            assert_eq!(
                OmiMessage::parse(json).unwrap_err(),
                ParseError::InvalidOperationCount(0)
            );
        }

        #[test]
        fn reject_multiple_operations() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "read": { "path": "/A" },
                "delete": { "path": "/B" }
            }"#;
            assert_eq!(
                OmiMessage::parse(json).unwrap_err(),
                ParseError::InvalidOperationCount(2)
            );
        }

        #[test]
        fn reject_invalid_json() {
            let err = OmiMessage::parse("not json").unwrap_err();
            assert!(matches!(err, ParseError::InvalidJson(_)));
        }

        #[test]
        fn negative_ttl_allowed() {
            let json = r#"{
                "omi": "1.0",
                "ttl": -1,
                "read": { "path": "/A" }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            assert_eq!(msg.ttl, -1);
        }

        // --- Serialization ---

        #[test]
        fn serialize_read_message() {
            let msg = OmiMessage {
                version: "1.0".into(),
                ttl: 0,
                operation: Operation::Read(ReadOp {
                    path: Some("/A/B".into()),
                    rid: None,
                    newest: Some(5),
                    oldest: None,
                    begin: None,
                    end: None,
                    depth: None,
                    interval: None,
                    callback: None,
                }),
            };
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["omi"], "1.0");
            assert_eq!(json["ttl"], 0);
            assert_eq!(json["read"]["path"], "/A/B");
            assert_eq!(json["read"]["newest"], 5);
        }

        #[test]
        fn serialize_write_message() {
            let msg = OmiMessage {
                version: "1.0".into(),
                ttl: 10,
                operation: Operation::Write(crate::omi::write::WriteOp::Single {
                    path: "/A/B".into(),
                    v: crate::odf::OmiValue::Number(42.0),
                    t: None,
                }),
            };
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["omi"], "1.0");
            assert_eq!(json["ttl"], 10);
            assert_eq!(json["write"]["path"], "/A/B");
            assert_eq!(json["write"]["v"], 42.0);
        }

        #[test]
        fn serialize_delete_message() {
            let msg = OmiMessage {
                version: "1.0".into(),
                ttl: 0,
                operation: Operation::Delete(DeleteOp {
                    path: "/DeviceA".into(),
                }),
            };
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["delete"]["path"], "/DeviceA");
        }

        #[test]
        fn serialize_cancel_message() {
            let msg = OmiMessage {
                version: "1.0".into(),
                ttl: 0,
                operation: Operation::Cancel(CancelOp {
                    rid: vec!["req-1".into()],
                }),
            };
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["cancel"]["rid"][0], "req-1");
        }

        // --- Round-trip tests ---

        #[test]
        fn roundtrip_read() {
            let msg = OmiMessage {
                version: "1.0".into(),
                ttl: 5,
                operation: Operation::Read(ReadOp {
                    path: Some("/DeviceA/Temperature".into()),
                    rid: None,
                    newest: Some(10),
                    oldest: None,
                    begin: None,
                    end: Some(2000.0),
                    depth: Some(3),
                    interval: None,
                    callback: None,
                }),
            };
            let json_str = serde_json::to_string(&msg).unwrap();
            let msg2 = OmiMessage::parse(&json_str).unwrap();
            assert_eq!(msg, msg2);
        }

        #[test]
        fn roundtrip_write_single() {
            let msg = OmiMessage {
                version: "1.0".into(),
                ttl: 0,
                operation: Operation::Write(crate::omi::write::WriteOp::Single {
                    path: "/A/B".into(),
                    v: crate::odf::OmiValue::Str("hello".into()),
                    t: Some(1000.0),
                }),
            };
            let json_str = serde_json::to_string(&msg).unwrap();
            let msg2 = OmiMessage::parse(&json_str).unwrap();
            assert_eq!(msg, msg2);
        }

        #[test]
        fn roundtrip_delete() {
            let msg = OmiMessage {
                version: "1.0".into(),
                ttl: 0,
                operation: Operation::Delete(DeleteOp {
                    path: "/DeviceA/Temp".into(),
                }),
            };
            let json_str = serde_json::to_string(&msg).unwrap();
            let msg2 = OmiMessage::parse(&json_str).unwrap();
            assert_eq!(msg, msg2);
        }

        #[test]
        fn roundtrip_cancel() {
            let msg = OmiMessage {
                version: "1.0".into(),
                ttl: 0,
                operation: Operation::Cancel(CancelOp {
                    rid: vec!["a".into(), "b".into()],
                }),
            };
            let json_str = serde_json::to_string(&msg).unwrap();
            let msg2 = OmiMessage::parse(&json_str).unwrap();
            assert_eq!(msg, msg2);
        }

        #[test]
        fn roundtrip_response() {
            let msg = OmiResponse::ok(serde_json::json!({"x": 1}));
            let json_str = serde_json::to_string(&msg).unwrap();
            let msg2 = OmiMessage::parse(&json_str).unwrap();
            assert_eq!(msg2.version, "1.0");
            assert_eq!(msg2.ttl, 0);
            match msg2.operation {
                Operation::Response(body) => assert_eq!(body.status, 200),
                _ => panic!("expected Response"),
            }
        }

        // --- Spec-style example messages ---

        #[test]
        fn spec_read_one_time() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "read": {
                    "path": "/OMI-Lite/SmartHouse/Floor1/Room101/Temperature",
                    "newest": 1
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Read(op) => {
                    assert_eq!(
                        op.path.as_deref(),
                        Some("/OMI-Lite/SmartHouse/Floor1/Room101/Temperature")
                    );
                    assert_eq!(op.newest, Some(1));
                    assert_eq!(op.kind(), ReadKind::OneTime);
                }
                _ => panic!("expected Read"),
            }
        }

        #[test]
        fn spec_read_subscription() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 60,
                "read": {
                    "path": "/OMI-Lite/SmartHouse/Floor1/Room101/Temperature",
                    "interval": 10.0,
                    "callback": "http://client.example.com/omi"
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Read(op) => {
                    assert_eq!(op.interval, Some(10.0));
                    assert_eq!(op.kind(), ReadKind::Subscription);
                }
                _ => panic!("expected Read"),
            }
        }

        #[test]
        fn spec_read_poll() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "read": {
                    "rid": "sub-abc-123"
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Read(op) => {
                    assert_eq!(op.rid.as_deref(), Some("sub-abc-123"));
                    assert_eq!(op.kind(), ReadKind::Poll);
                }
                _ => panic!("expected Read"),
            }
        }

        #[test]
        fn spec_write_single_value() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 10,
                "write": {
                    "path": "/OMI-Lite/SmartHouse/Floor1/Room101/Temperature",
                    "v": 22.5,
                    "t": 1700000000.0
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Write(crate::omi::write::WriteOp::Single { path, v, t }) => {
                    assert_eq!(
                        path,
                        "/OMI-Lite/SmartHouse/Floor1/Room101/Temperature"
                    );
                    assert_eq!(*v, crate::odf::OmiValue::Number(22.5));
                    assert_eq!(*t, Some(1700000000.0));
                }
                _ => panic!("expected Write Single"),
            }
        }

        #[test]
        fn spec_write_batch() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 10,
                "write": {
                    "items": [
                        { "path": "/House/Room1/Temp", "v": 22.5 },
                        { "path": "/House/Room1/Humidity", "v": 45 }
                    ]
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Write(crate::omi::write::WriteOp::Batch { items }) => {
                    assert_eq!(items.len(), 2);
                }
                _ => panic!("expected Write Batch"),
            }
        }

        #[test]
        fn spec_write_tree() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 10,
                "write": {
                    "path": "/",
                    "objects": {
                        "SmartHouse": {
                            "id": "SmartHouse",
                            "type": "omi:building"
                        }
                    }
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Write(crate::omi::write::WriteOp::Tree { path, objects }) => {
                    assert_eq!(path, "/");
                    assert!(objects.contains_key("SmartHouse"));
                }
                _ => panic!("expected Write Tree"),
            }
        }

        #[test]
        fn spec_delete() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "delete": {
                    "path": "/OMI-Lite/SmartHouse/Floor1/Room101/Temperature"
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Delete(op) => {
                    assert_eq!(
                        op.path,
                        "/OMI-Lite/SmartHouse/Floor1/Room101/Temperature"
                    );
                }
                _ => panic!("expected Delete"),
            }
        }

        #[test]
        fn spec_cancel() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "cancel": {
                    "rid": ["sub-abc-123"]
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Cancel(op) => {
                    assert_eq!(op.rid, vec!["sub-abc-123"]);
                }
                _ => panic!("expected Cancel"),
            }
        }

        #[test]
        fn spec_response_ok() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "response": {
                    "status": 200,
                    "result": {
                        "path": "/SmartHouse/Floor1/Room101/Temperature",
                        "values": [{"v": 22.5, "t": 1700000000.0}]
                    }
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Response(body) => {
                    assert_eq!(body.status, 200);
                    assert!(body.result.is_some());
                }
                _ => panic!("expected Response"),
            }
        }

        #[test]
        fn spec_response_not_found() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "response": {
                    "status": 404,
                    "desc": "Path not found: /Missing/Path"
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Response(body) => {
                    assert_eq!(body.status, 404);
                    assert_eq!(body.desc.as_deref(), Some("Path not found: /Missing/Path"));
                }
                _ => panic!("expected Response"),
            }
        }

        #[test]
        fn spec_response_batch() {
            let json = r#"{
                "omi": "1.0",
                "ttl": 0,
                "response": {
                    "status": 200,
                    "result": [
                        { "path": "/A/B", "status": 200 },
                        { "path": "/A/C", "status": 404, "desc": "not found" }
                    ]
                }
            }"#;
            let msg = OmiMessage::parse(json).unwrap();
            match &msg.operation {
                Operation::Response(body) => {
                    assert_eq!(body.status, 200);
                    match &body.result {
                        Some(ResponseResult::Batch(items)) => {
                            assert_eq!(items.len(), 2);
                            assert_eq!(items[0].status, 200);
                            assert_eq!(items[1].status, 404);
                        }
                        _ => panic!("expected Batch result"),
                    }
                }
                _ => panic!("expected Response"),
            }
        }
    }
}
