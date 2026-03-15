#[cfg(feature = "lite-json")]
pub mod engine;
pub mod error;
pub mod read;
pub mod write;
pub mod delete;
pub mod cancel;
pub mod response;
#[cfg(feature = "lite-json")]
pub mod subscriptions;

#[cfg(feature = "lite-json")]
pub use self::engine::Engine;

use self::error::ParseError;
pub use self::read::{ReadKind, ReadOp};
use self::write::WriteOp;
use self::delete::DeleteOp;
use self::cancel::CancelOp;
use self::response::ResponseBody;
pub use self::write::WriteItem;
pub use self::response::{StatusCode, ResponseResult, ResultPayload, ItemStatus, OmiResponse};
#[cfg(feature = "lite-json")]
pub use self::subscriptions::{Delivery, SessionId};

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

#[cfg(feature = "lite-json")]
mod lite_parse {
    use super::*;

    impl OmiMessage {
        pub fn parse(json: &str) -> Result<Self, ParseError> {
            crate::json::parser::parse_omi_message(json)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "lite-json")]
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
