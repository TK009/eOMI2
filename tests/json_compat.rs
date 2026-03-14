//! Compatibility test suite for lite-json parser and serializer.
//!
//! Verifies:
//! - FR-012: Parser produces identical results to the serde-based parser for all valid OMI messages
//! - SC-004: Parse → serialize → re-parse round-trip preserves all data
//! - SC-005: Parser rejects all malformed inputs that the serde parser rejects
//!
//! These tests run under the `lite-json` feature flag.
//! The `serde_json` dev-dependency is available for constructing expected values
//! and cross-checking serializer output.

#![cfg(feature = "lite-json")]

use std::collections::BTreeMap;

use reconfigurable_device::json::serializer::*;
use reconfigurable_device::json::parser::parse_omi_message;
use reconfigurable_device::odf::{InfoItem, Object, OmiValue, Value, RingBuffer};
use reconfigurable_device::omi::{
    OmiMessage, Operation,
    read::{ReadKind, ReadOp},
    write::{WriteOp, WriteItem},
    delete::DeleteOp,
    cancel::CancelOp,
    response::{ResponseBody, ResponseResult, ItemStatus},
    error::ParseError,
};

// ============================================================================
// Helper: build OMI JSON strings using the lite-json serializer
// ============================================================================

fn serialize_omi(msg: &OmiMessage) -> String {
    let mut w = JsonWriter::new();
    let op_key = match &msg.operation {
        Operation::Read(_) => "read",
        Operation::Write(_) => "write",
        Operation::Delete(_) => "delete",
        Operation::Cancel(_) => "cancel",
        Operation::Response(_) => "response",
    };
    write_omi_envelope(&mut w, &msg.version, msg.ttl, op_key, |w| {
        match &msg.operation {
            Operation::Read(op) => {
                write_read_op(
                    w,
                    op.path.as_deref(),
                    op.rid.as_deref(),
                    op.newest,
                    op.oldest,
                    op.begin,
                    op.end,
                    op.depth,
                    op.interval,
                    op.callback.as_deref(),
                );
            }
            Operation::Write(op) => match op {
                WriteOp::Single { path, v, t } => {
                    write_write_single(w, path, v, *t);
                }
                WriteOp::Batch { items } => {
                    w.begin_object();
                    w.key("items");
                    w.begin_array();
                    for item in items {
                        write_batch_item(w, &item.path, &item.v, item.t);
                    }
                    w.end_array();
                    w.end_object();
                }
                WriteOp::Tree { path, objects } => {
                    write_write_tree(w, path, objects);
                }
            },
            Operation::Delete(op) => {
                write_delete_op(w, &op.path);
            }
            Operation::Cancel(op) => {
                let rids: Vec<&str> = op.rid.iter().map(|s| s.as_str()).collect();
                write_cancel_op(w, &rids);
            }
            Operation::Response(body) => {
                let write_result: Option<Box<dyn Fn(&mut JsonWriter)>> = match &body.result {
                    Some(ResponseResult::Batch(items)) => {
                        let items = items.clone();
                        Some(Box::new(move |w: &mut JsonWriter| {
                            w.begin_array();
                            for item in &items {
                                write_item_status(w, &item.path, item.status, item.desc.as_deref());
                            }
                            w.end_array();
                        }))
                    }
                    Some(ResponseResult::Single(_)) => {
                        // Single results use a placeholder type under lite-json;
                        // proper serialization will be handled by T09.
                        None
                    }
                    None => None,
                };
                write_response_body(
                    w,
                    body.status,
                    body.rid.as_deref(),
                    body.desc.as_deref(),
                    write_result.as_ref().map(|f| f.as_ref()),
                );
            }
        }
    });
    w.into_string()
}

// ============================================================================
// Parse parity tests (FR-012)
//
// Each test provides a known JSON string and verifies that `parse_omi_message`
// produces the expected `OmiMessage`.
// ============================================================================

mod parse_parity {
    use super::*;

    // --- Read operations ---

    #[test]
    fn parse_read_one_time() {
        let json = r#"{"omi":"1.0","ttl":0,"read":{"path":"/DeviceA/Temperature"}}"#;
        let msg = parse_omi_message(json).unwrap();
        assert_eq!(msg.version, "1.0");
        assert_eq!(msg.ttl, 0);
        match &msg.operation {
            Operation::Read(op) => {
                assert_eq!(op.path.as_deref(), Some("/DeviceA/Temperature"));
                assert!(op.rid.is_none());
                assert_eq!(op.kind(), ReadKind::OneTime);
            }
            _ => panic!("expected Read"),
        }
    }

    #[test]
    fn parse_read_with_newest() {
        let json = r#"{"omi":"1.0","ttl":0,"read":{"path":"/A/B","newest":5}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Read(op) => {
                assert_eq!(op.path.as_deref(), Some("/A/B"));
                assert_eq!(op.newest, Some(5));
            }
            _ => panic!("expected Read"),
        }
    }

    #[test]
    fn parse_read_all_fields() {
        let json = r#"{
            "omi": "1.0", "ttl": 5,
            "read": {
                "path": "/DeviceA/Temperature",
                "newest": 10, "oldest": 2,
                "begin": 1000.0, "end": 2000.0,
                "depth": 3
            }
        }"#;
        let msg = parse_omi_message(json).unwrap();
        assert_eq!(msg.ttl, 5);
        match &msg.operation {
            Operation::Read(op) => {
                assert_eq!(op.path.as_deref(), Some("/DeviceA/Temperature"));
                assert_eq!(op.newest, Some(10));
                assert_eq!(op.oldest, Some(2));
                assert_eq!(op.begin, Some(1000.0));
                assert_eq!(op.end, Some(2000.0));
                assert_eq!(op.depth, Some(3));
                assert_eq!(op.kind(), ReadKind::OneTime);
            }
            _ => panic!("expected Read"),
        }
    }

    #[test]
    fn parse_read_subscription() {
        let json = r#"{
            "omi": "1.0", "ttl": 60,
            "read": {
                "path": "/DeviceA/Temperature",
                "interval": 10.0,
                "callback": "http://client.example.com/omi"
            }
        }"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Read(op) => {
                assert_eq!(op.interval, Some(10.0));
                assert_eq!(op.callback.as_deref(), Some("http://client.example.com/omi"));
                assert_eq!(op.kind(), ReadKind::Subscription);
            }
            _ => panic!("expected Read"),
        }
    }

    #[test]
    fn parse_read_poll() {
        let json = r#"{"omi":"1.0","ttl":0,"read":{"rid":"sub-abc-123"}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Read(op) => {
                assert_eq!(op.rid.as_deref(), Some("sub-abc-123"));
                assert_eq!(op.kind(), ReadKind::Poll);
            }
            _ => panic!("expected Read"),
        }
    }

    // --- Write operations ---

    #[test]
    fn parse_write_single_number() {
        let json = r#"{"omi":"1.0","ttl":10,"write":{"path":"/A/B","v":42.0}}"#;
        let msg = parse_omi_message(json).unwrap();
        assert_eq!(msg.ttl, 10);
        match &msg.operation {
            Operation::Write(WriteOp::Single { path, v, t }) => {
                assert_eq!(path, "/A/B");
                assert_eq!(*v, OmiValue::Number(42.0));
                assert!(t.is_none());
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_write_single_string() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":"hello"}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Str("hello".into()));
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_write_single_bool() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":true}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Bool(true));
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_write_single_null() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":null}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Null);
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_write_single_with_timestamp() {
        let json = r#"{"omi":"1.0","ttl":10,"write":{"path":"/A/B","v":22.5,"t":1700000000.0}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { path, v, t }) => {
                assert_eq!(path, "/A/B");
                assert_eq!(*v, OmiValue::Number(22.5));
                assert_eq!(*t, Some(1700000000.0));
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_write_batch() {
        let json = r#"{
            "omi": "1.0", "ttl": 10,
            "write": {
                "items": [
                    {"path": "/House/Room1/Temp", "v": 22.5},
                    {"path": "/House/Room1/Humidity", "v": 45}
                ]
            }
        }"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Batch { items }) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].path, "/House/Room1/Temp");
                assert_eq!(items[0].v, OmiValue::Number(22.5));
                assert_eq!(items[1].path, "/House/Room1/Humidity");
                assert_eq!(items[1].v, OmiValue::Number(45.0));
            }
            _ => panic!("expected Write Batch"),
        }
    }

    #[test]
    fn parse_write_tree() {
        let json = r#"{
            "omi": "1.0", "ttl": 10,
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
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Tree { path, objects }) => {
                assert_eq!(path, "/");
                assert!(objects.contains_key("SmartHouse"));
                let obj = &objects["SmartHouse"];
                assert_eq!(obj.id, "SmartHouse");
                assert_eq!(obj.type_uri.as_deref(), Some("omi:building"));
            }
            _ => panic!("expected Write Tree"),
        }
    }

    // --- Delete operations ---

    #[test]
    fn parse_delete() {
        let json = r#"{"omi":"1.0","ttl":0,"delete":{"path":"/DeviceA/Temperature"}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Delete(op) => {
                assert_eq!(op.path, "/DeviceA/Temperature");
            }
            _ => panic!("expected Delete"),
        }
    }

    // --- Cancel operations ---

    #[test]
    fn parse_cancel() {
        let json = r#"{"omi":"1.0","ttl":0,"cancel":{"rid":["sub-abc-123"]}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Cancel(op) => {
                assert_eq!(op.rid, vec!["sub-abc-123"]);
            }
            _ => panic!("expected Cancel"),
        }
    }

    #[test]
    fn parse_cancel_multiple_rids() {
        let json = r#"{"omi":"1.0","ttl":0,"cancel":{"rid":["a","b","c"]}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Cancel(op) => {
                assert_eq!(op.rid, vec!["a", "b", "c"]);
            }
            _ => panic!("expected Cancel"),
        }
    }

    // --- Response operations ---

    #[test]
    fn parse_response_ok() {
        let json = r#"{
            "omi": "1.0", "ttl": 0,
            "response": {
                "status": 200,
                "result": {"path": "/A/Temp", "values": [{"v": 22.5, "t": 1700000000.0}]}
            }
        }"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 200);
                assert!(body.result.is_some());
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn parse_response_not_found() {
        let json = r#"{
            "omi": "1.0", "ttl": 0,
            "response": {
                "status": 404,
                "desc": "Path not found: /Missing/Path"
            }
        }"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 404);
                assert_eq!(body.desc.as_deref(), Some("Path not found: /Missing/Path"));
                assert!(body.result.is_none());
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn parse_response_with_rid() {
        let json = r#"{
            "omi": "1.0", "ttl": 0,
            "response": {
                "status": 200,
                "rid": "sub-1"
            }
        }"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 200);
                assert_eq!(body.rid.as_deref(), Some("sub-1"));
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn parse_response_batch() {
        let json = r#"{
            "omi": "1.0", "ttl": 0,
            "response": {
                "status": 200,
                "result": [
                    {"path": "/A/B", "status": 200},
                    {"path": "/A/C", "status": 404, "desc": "not found"}
                ]
            }
        }"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 200);
                match &body.result {
                    Some(ResponseResult::Batch(items)) => {
                        assert_eq!(items.len(), 2);
                        assert_eq!(items[0].path, "/A/B");
                        assert_eq!(items[0].status, 200);
                        assert_eq!(items[1].path, "/A/C");
                        assert_eq!(items[1].status, 404);
                        assert_eq!(items[1].desc.as_deref(), Some("not found"));
                    }
                    _ => panic!("expected Batch result"),
                }
            }
            _ => panic!("expected Response"),
        }
    }

    // --- Negative TTL ---

    #[test]
    fn negative_ttl_allowed() {
        let json = r#"{"omi":"1.0","ttl":-1,"read":{"path":"/A"}}"#;
        let msg = parse_omi_message(json).unwrap();
        assert_eq!(msg.ttl, -1);
    }

    // --- Unknown fields (FR-007) ---

    #[test]
    fn ignore_unknown_envelope_fields() {
        let json = r#"{"omi":"1.0","ttl":0,"extra":"ignored","read":{"path":"/A"}}"#;
        let msg = parse_omi_message(json).unwrap();
        assert!(matches!(msg.operation, Operation::Read(_)));
    }

    #[test]
    fn ignore_unknown_operation_fields() {
        let json = r#"{"omi":"1.0","ttl":0,"read":{"path":"/A","unknown_field":42}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Read(op) => assert_eq!(op.path.as_deref(), Some("/A")),
            _ => panic!("expected Read"),
        }
    }

    // --- Integer value parsing ---

    #[test]
    fn parse_write_integer_value() {
        // JSON integer 42 should parse as OmiValue::Number(42.0)
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":42}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Number(42.0));
            }
            _ => panic!("expected Write Single"),
        }
    }
}

// ============================================================================
// Error parity tests (SC-005)
//
// The lite-json parser must reject the same malformed inputs as the serde
// parser, with equivalent or better error descriptions.
// ============================================================================

mod error_parity {
    use super::*;

    #[test]
    fn reject_invalid_json() {
        let err = parse_omi_message("not json").unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn reject_empty_input() {
        let err = parse_omi_message("").unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn reject_whitespace_only() {
        let err = parse_omi_message("   ").unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn reject_truncated_json() {
        let err = parse_omi_message(r##"{"omi":"1.0","ttl":"##).unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn reject_missing_omi() {
        let json = r#"{"ttl":0,"read":{"path":"/A"}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::MissingField("omi")
        );
    }

    #[test]
    fn reject_wrong_version() {
        let json = r#"{"omi":"2.0","ttl":0,"read":{"path":"/A"}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::UnsupportedVersion("2.0".into())
        );
    }

    #[test]
    fn reject_missing_ttl() {
        let json = r#"{"omi":"1.0","read":{"path":"/A"}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::MissingField("ttl")
        );
    }

    #[test]
    fn reject_zero_operations() {
        let json = r#"{"omi":"1.0","ttl":0}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::InvalidOperationCount(0)
        );
    }

    #[test]
    fn reject_multiple_operations() {
        let json = r#"{"omi":"1.0","ttl":0,"read":{"path":"/A"},"delete":{"path":"/B"}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::InvalidOperationCount(2)
        );
    }

    // --- Read validation ---

    #[test]
    fn reject_read_both_path_and_rid() {
        let json = r#"{"omi":"1.0","ttl":0,"read":{"path":"/A","rid":"req-1"}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::MutuallyExclusive("path", "rid")
        );
    }

    #[test]
    fn reject_read_neither_path_nor_rid() {
        let json = r#"{"omi":"1.0","ttl":0,"read":{}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::MissingField("path or rid")
        );
    }

    // --- Write validation ---

    #[test]
    fn reject_write_no_form() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B"}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::MissingField("v, items, or objects")
        );
    }

    #[test]
    fn reject_write_v_and_items() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A","v":1,"items":[{"path":"/B","v":2}]}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::MutuallyExclusive("v", "items")
        );
    }

    #[test]
    fn reject_write_v_and_objects() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A","v":1,"objects":{"X":{"id":"X"}}}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::MutuallyExclusive("v", "objects")
        );
    }

    #[test]
    fn reject_write_items_and_objects() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"items":[{"path":"/A","v":1}],"objects":{"X":{"id":"X"}}}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::MutuallyExclusive("items", "objects")
        );
    }

    #[test]
    fn reject_write_single_without_path() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"v":42}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::MissingField("path")
        );
    }

    #[test]
    fn reject_write_empty_items() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"items":[]}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::InvalidField {
                field: "items",
                reason: "items array must not be empty".into(),
            }
        );
    }

    // --- Delete validation ---

    #[test]
    fn reject_delete_root() {
        let json = r#"{"omi":"1.0","ttl":0,"delete":{"path":"/"}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::InvalidField {
                field: "path",
                reason: "cannot delete root '/'".into(),
            }
        );
    }

    #[test]
    fn reject_delete_no_leading_slash() {
        let json = r#"{"omi":"1.0","ttl":0,"delete":{"path":"DeviceA"}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::InvalidField {
                field: "path",
                reason: "must start with '/'".into(),
            }
        );
    }

    // --- Cancel validation ---

    #[test]
    fn reject_cancel_empty_rid() {
        let json = r#"{"omi":"1.0","ttl":0,"cancel":{"rid":[]}}"#;
        assert_eq!(
            parse_omi_message(json).unwrap_err(),
            ParseError::InvalidField {
                field: "rid",
                reason: "rid array must not be empty".into(),
            }
        );
    }
}

// ============================================================================
// Serialize parity tests
//
// Verify the lite-json serializer produces output equivalent to serde_json
// for all OMI message types. We serialize with JsonWriter and validate
// the result by parsing it back with serde_json (always available as dev-dep).
// ============================================================================

mod serialize_parity {
    use super::*;

    /// Parse a JSON string with serde_json for cross-checking.
    fn parse_json(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap_or_else(|e| panic!("invalid JSON: {e}\n{s}"))
    }

    #[test]
    fn serialize_read_one_time() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "read", |w| {
            write_read_op(w, Some("/DeviceA/Temp"), None, Some(1), None, None, None, None, None, None);
        });
        let v = parse_json(&w.into_string());
        assert_eq!(v["omi"], "1.0");
        assert_eq!(v["ttl"], 0);
        assert_eq!(v["read"]["path"], "/DeviceA/Temp");
        assert_eq!(v["read"]["newest"], 1);
        assert!(v["read"].get("rid").is_none());
    }

    #[test]
    fn serialize_read_subscription() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 60, "read", |w| {
            write_read_op(
                w,
                Some("/DeviceA/Temp"),
                None,
                None, None, None, None, None,
                Some(10.0),
                Some("http://example.com/cb"),
            );
        });
        let v = parse_json(&w.into_string());
        assert_eq!(v["read"]["interval"], 10.0);
        assert_eq!(v["read"]["callback"], "http://example.com/cb");
    }

    #[test]
    fn serialize_read_all_fields() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 5, "read", |w| {
            write_read_op(
                w,
                Some("/A/B"), None,
                Some(10), Some(2),
                Some(1000.0), Some(2000.0),
                Some(3), Some(5.0),
                Some("http://cb"),
            );
        });
        let v = parse_json(&w.into_string());
        assert_eq!(v["read"]["path"], "/A/B");
        assert_eq!(v["read"]["newest"], 10);
        assert_eq!(v["read"]["oldest"], 2);
        assert_eq!(v["read"]["begin"], 1000.0);
        assert_eq!(v["read"]["end"], 2000.0);
        assert_eq!(v["read"]["depth"], 3);
        assert_eq!(v["read"]["interval"], 5.0);
        assert_eq!(v["read"]["callback"], "http://cb");
    }

    #[test]
    fn serialize_write_single() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 10, "write", |w| {
            write_write_single(w, "/A/B", &OmiValue::Number(42.0), None);
        });
        let v = parse_json(&w.into_string());
        assert_eq!(v["write"]["path"], "/A/B");
        assert_eq!(v["write"]["v"], 42.0);
        assert!(v["write"].get("t").is_none());
    }

    #[test]
    fn serialize_write_single_with_timestamp() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 10, "write", |w| {
            write_write_single(w, "/A/B", &OmiValue::Bool(true), Some(1000.0));
        });
        let v = parse_json(&w.into_string());
        assert_eq!(v["write"]["v"], true);
        assert_eq!(v["write"]["t"], 1000.0);
    }

    #[test]
    fn serialize_write_batch() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 10, "write", |w| {
            w.begin_object();
            w.key("items");
            w.begin_array();
            write_batch_item(w, "/A/B", &OmiValue::Number(1.0), None);
            write_batch_item(w, "/C/D", &OmiValue::Str("x".into()), Some(2.0));
            w.end_array();
            w.end_object();
        });
        let v = parse_json(&w.into_string());
        let items = v["write"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["path"], "/A/B");
        assert_eq!(items[0]["v"], 1.0);
        assert_eq!(items[1]["path"], "/C/D");
        assert_eq!(items[1]["v"], "x");
        assert_eq!(items[1]["t"], 2.0);
    }

    #[test]
    fn serialize_write_tree() {
        let mut objects = BTreeMap::new();
        objects.insert("Dev".into(), Object::new("Dev"));
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 10, "write", |w| {
            write_write_tree(w, "/", &objects);
        });
        let v = parse_json(&w.into_string());
        assert_eq!(v["write"]["path"], "/");
        assert!(v["write"]["objects"]["Dev"].is_object());
        assert_eq!(v["write"]["objects"]["Dev"]["id"], "Dev");
    }

    #[test]
    fn serialize_delete() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "delete", |w| {
            write_delete_op(w, "/DeviceA");
        });
        let v = parse_json(&w.into_string());
        assert_eq!(v["delete"]["path"], "/DeviceA");
    }

    #[test]
    fn serialize_cancel() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "cancel", |w| {
            write_cancel_op(w, &["sub-1", "sub-2"]);
        });
        let v = parse_json(&w.into_string());
        let rids = v["cancel"]["rid"].as_array().unwrap();
        assert_eq!(rids.len(), 2);
        assert_eq!(rids[0], "sub-1");
        assert_eq!(rids[1], "sub-2");
    }

    #[test]
    fn serialize_response_status_only() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "response", |w| {
            write_response_body(w, 404, None, Some("not found"), None);
        });
        let v = parse_json(&w.into_string());
        assert_eq!(v["response"]["status"], 404);
        assert_eq!(v["response"]["desc"], "not found");
        assert!(v["response"].get("result").is_none());
    }

    #[test]
    fn serialize_response_with_rid() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "response", |w| {
            write_response_body(w, 200, Some("req-1"), None, None);
        });
        let v = parse_json(&w.into_string());
        assert_eq!(v["response"]["rid"], "req-1");
    }

    #[test]
    fn serialize_response_batch() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "response", |w| {
            write_response_body(w, 200, None, None, Some(&|w| {
                w.begin_array();
                write_item_status(w, "/A/B", 200, None);
                write_item_status(w, "/A/C", 404, Some("gone"));
                w.end_array();
            }));
        });
        let v = parse_json(&w.into_string());
        let result = v["response"]["result"].as_array().unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["path"], "/A/B");
        assert_eq!(result[0]["status"], 200);
        assert_eq!(result[1]["desc"], "gone");
    }

    // --- OmiValue serialization ---

    #[test]
    fn serialize_omi_values() {
        assert_eq!(OmiValue::Null.to_json_string(), "null");
        assert_eq!(OmiValue::Bool(true).to_json_string(), "true");
        assert_eq!(OmiValue::Bool(false).to_json_string(), "false");
        assert_eq!(OmiValue::Str("hello".into()).to_json_string(), r#""hello""#);
    }

    #[test]
    fn serialize_omi_number() {
        let s = OmiValue::Number(42.5).to_json_string();
        let v: f64 = serde_json::from_str(&s).unwrap();
        assert_eq!(v, 42.5);
    }

    // --- String escaping ---

    #[test]
    fn serialize_string_with_escapes() {
        let s = OmiValue::Str("say \"hello\" \\world\nnewline".into()).to_json_string();
        let v: String = serde_json::from_str(&s).unwrap();
        assert_eq!(v, "say \"hello\" \\world\nnewline");
    }

    #[test]
    fn serialize_string_with_control_chars() {
        let s = OmiValue::Str("\x08\x0C\t\r".into()).to_json_string();
        let v: String = serde_json::from_str(&s).unwrap();
        assert_eq!(v, "\x08\x0C\t\r");
    }

    #[test]
    fn serialize_string_utf8() {
        let s = OmiValue::Str("héllo 日本語".into()).to_json_string();
        let v: String = serde_json::from_str(&s).unwrap();
        assert_eq!(v, "héllo 日本語");
    }
}

// ============================================================================
// Round-trip tests (SC-004)
//
// Construct OmiMessage → serialize → parse → compare.
// Proves data survives the full cycle.
// ============================================================================

mod round_trip {
    use super::*;

    /// Serialize an OmiMessage using the lite-json serializer, then parse it back.
    fn roundtrip(msg: &OmiMessage) -> OmiMessage {
        let json = serialize_omi(msg);
        parse_omi_message(&json).unwrap_or_else(|e| {
            panic!("round-trip parse failed: {e}\nJSON: {json}")
        })
    }

    #[test]
    fn roundtrip_read_one_time() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Read(ReadOp {
                path: Some("/DeviceA/Temperature".into()),
                rid: None,
                newest: Some(1),
                oldest: None,
                begin: None,
                end: None,
                depth: None,
                interval: None,
                callback: None,
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_read_all_fields() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 5,
            operation: Operation::Read(ReadOp {
                path: Some("/DeviceA/Temperature".into()),
                rid: None,
                newest: Some(10),
                oldest: Some(2),
                begin: Some(1000.0),
                end: Some(2000.0),
                depth: Some(3),
                interval: None,
                callback: None,
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_read_subscription() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 60,
            operation: Operation::Read(ReadOp {
                path: Some("/A".into()),
                rid: None,
                newest: None,
                oldest: None,
                begin: None,
                end: None,
                depth: None,
                interval: Some(10.0),
                callback: Some("http://example.com/cb".into()),
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_read_poll() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Read(ReadOp {
                path: None,
                rid: Some("sub-abc-123".into()),
                newest: None,
                oldest: None,
                begin: None,
                end: None,
                depth: None,
                interval: None,
                callback: None,
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_write_single() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 10,
            operation: Operation::Write(WriteOp::Single {
                path: "/A/B".into(),
                v: OmiValue::Number(22.5),
                t: Some(1700000000.0),
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_write_single_string_value() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Write(WriteOp::Single {
                path: "/A/B".into(),
                v: OmiValue::Str("hello world".into()),
                t: None,
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_write_single_bool_value() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Write(WriteOp::Single {
                path: "/A/B".into(),
                v: OmiValue::Bool(false),
                t: None,
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_write_single_null_value() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Write(WriteOp::Single {
                path: "/A/B".into(),
                v: OmiValue::Null,
                t: None,
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_write_batch() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 10,
            operation: Operation::Write(WriteOp::Batch {
                items: vec![
                    WriteItem { path: "/A/B".into(), v: OmiValue::Number(1.0), t: None },
                    WriteItem { path: "/C/D".into(), v: OmiValue::Str("x".into()), t: Some(2.0) },
                ],
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_write_tree() {
        let mut objects = BTreeMap::new();
        let mut obj = Object::new("SmartHouse");
        obj.type_uri = Some("omi:building".into());
        objects.insert("SmartHouse".into(), obj);

        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 10,
            operation: Operation::Write(WriteOp::Tree {
                path: "/".into(),
                objects,
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
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
        assert_eq!(roundtrip(&msg), msg);
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
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_response_status_only() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Response(ResponseBody {
                status: 404,
                rid: None,
                desc: Some("not found".into()),
                result: None,
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_response_batch() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Response(ResponseBody {
                status: 200,
                rid: None,
                desc: None,
                result: Some(ResponseResult::Batch(vec![
                    ItemStatus { path: "/A".into(), status: 200, desc: None },
                    ItemStatus { path: "/B".into(), status: 404, desc: Some("gone".into()) },
                ])),
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }

    #[test]
    fn roundtrip_negative_ttl() {
        let msg = OmiMessage {
            version: "1.0".into(),
            ttl: -1,
            operation: Operation::Read(ReadOp {
                path: Some("/A".into()),
                rid: None,
                newest: None,
                oldest: None,
                begin: None,
                end: None,
                depth: None,
                interval: None,
                callback: None,
            }),
        };
        assert_eq!(roundtrip(&msg), msg);
    }
}

// ============================================================================
// Edge case tests
//
// Cover boundary conditions from the spec's edge case list.
// ============================================================================

mod edge_cases {
    use super::*;

    // --- String escape handling (FR-005) ---

    #[test]
    fn parse_string_with_escaped_quotes() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":"say \"hello\""}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Str("say \"hello\"".into()));
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_string_with_backslash() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":"back\\slash"}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Str("back\\slash".into()));
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_string_with_newlines() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":"line1\nline2\ttab"}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Str("line1\nline2\ttab".into()));
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_string_with_unicode_escape() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":"\u0041\u0042"}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Str("AB".into()));
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_string_with_slash_escape() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":"a\/b"}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Str("a/b".into()));
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_string_with_utf8() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":"héllo 日本語"}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Str("héllo 日本語".into()));
            }
            _ => panic!("expected Write Single"),
        }
    }

    // --- Numeric edge cases ---

    #[test]
    fn parse_zero_value() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A","v":0}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Number(0.0));
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_negative_number() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A","v":-42.5}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Number(-42.5));
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn parse_scientific_notation() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A","v":1.5e10}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Number(1.5e10));
            }
            _ => panic!("expected Write Single"),
        }
    }

    // --- Whitespace handling ---

    #[test]
    fn parse_with_extra_whitespace() {
        let json = r#"  {  "omi" : "1.0" , "ttl" : 0 , "read" : { "path" : "/A" }  }  "#;
        let msg = parse_omi_message(json).unwrap();
        assert!(matches!(msg.operation, Operation::Read(_)));
    }

    // --- Duplicate keys (last-value-wins per serde_json) ---

    #[test]
    fn duplicate_keys_last_wins() {
        let json = r#"{"omi":"1.0","ttl":0,"ttl":5,"read":{"path":"/A"}}"#;
        let msg = parse_omi_message(json).unwrap();
        assert_eq!(msg.ttl, 5);
    }

    // --- Deep nesting limit ---

    #[test]
    fn reject_excessive_object_nesting() {
        // Build a write-tree with nesting > MAX_OBJECT_DEPTH (8)
        let mut inner = r#"{"id":"L9"}"#.to_string();
        for i in (1..=8).rev() {
            inner = format!(r#"{{"id":"L{i}","objects":{{"L{}":{inner}}}}}"#, i + 1);
        }
        let json = format!(
            r#"{{"omi":"1.0","ttl":0,"write":{{"path":"/","objects":{{"L1":{inner}}}}}}}"#
        );
        let err = parse_omi_message(&json).unwrap_err();
        match err {
            ParseError::InvalidField { field, reason } => {
                assert_eq!(field, "objects");
                assert!(reason.contains("nesting depth"), "{}", reason);
            }
            _ => panic!("expected InvalidField for nesting depth, got {:?}", err),
        }
    }

    // --- Response with all optional fields ---

    #[test]
    fn parse_response_all_fields() {
        let json = r#"{
            "omi": "1.0", "ttl": 0,
            "response": {
                "status": 200,
                "rid": "req-42",
                "desc": "OK",
                "result": [{"path": "/A", "status": 200, "desc": "ok"}]
            }
        }"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 200);
                assert_eq!(body.rid.as_deref(), Some("req-42"));
                assert_eq!(body.desc.as_deref(), Some("OK"));
                match &body.result {
                    Some(ResponseResult::Batch(items)) => {
                        assert_eq!(items.len(), 1);
                        assert_eq!(items[0].desc.as_deref(), Some("ok"));
                    }
                    _ => panic!("expected Batch result"),
                }
            }
            _ => panic!("expected Response"),
        }
    }

    // --- Empty string values ---

    #[test]
    fn parse_empty_string_value() {
        let json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/A/B","v":""}}"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { v, .. }) => {
                assert_eq!(*v, OmiValue::Str("".into()));
            }
            _ => panic!("expected Write Single"),
        }
    }

    // --- Large TTL values ---

    #[test]
    fn parse_large_ttl() {
        let json = r#"{"omi":"1.0","ttl":2147483647,"read":{"path":"/A"}}"#;
        let msg = parse_omi_message(json).unwrap();
        assert_eq!(msg.ttl, 2147483647);
    }

    // --- Nested write tree with items ---

    #[test]
    fn parse_write_tree_with_nested_objects_and_items() {
        let json = r#"{
            "omi": "1.0", "ttl": 10,
            "write": {
                "path": "/",
                "objects": {
                    "House": {
                        "id": "House",
                        "objects": {
                            "Room1": {
                                "id": "Room1",
                                "items": {
                                    "Temp": {
                                        "values": [{"v": 22.5, "t": 1000.0}]
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"#;
        let msg = parse_omi_message(json).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Tree { path, objects }) => {
                assert_eq!(path, "/");
                let house = &objects["House"];
                assert_eq!(house.id, "House");
                let room = house.objects.as_ref().unwrap().get("Room1").unwrap();
                assert_eq!(room.id, "Room1");
                let temp = room.items.as_ref().unwrap().get("Temp").unwrap();
                assert!(!temp.values.is_empty());
            }
            _ => panic!("expected Write Tree"),
        }
    }
}

// ============================================================================
// O-DF serialization tests
//
// Verify that ToJson impls for ODF types produce valid, correct JSON.
// ============================================================================

mod odf_serialization {
    use super::*;

    fn parse_json(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap_or_else(|e| panic!("invalid JSON: {e}\n{s}"))
    }

    #[test]
    fn value_with_timestamp() {
        let val = Value::new(OmiValue::Number(22.5), Some(1700000000.0));
        let s = val.to_json_string();
        let v = parse_json(&s);
        assert_eq!(v["v"], 22.5);
        assert_eq!(v["t"], 1700000000.0);
    }

    #[test]
    fn value_without_timestamp() {
        let val = Value::new(OmiValue::Bool(true), None);
        let s = val.to_json_string();
        let v = parse_json(&s);
        assert_eq!(v["v"], true);
        assert!(v.get("t").is_none());
    }

    #[test]
    fn value_null() {
        let val = Value::new(OmiValue::Null, None);
        let s = val.to_json_string();
        let v = parse_json(&s);
        assert!(v["v"].is_null());
    }

    #[test]
    fn ring_buffer_newest_first() {
        let mut rb = RingBuffer::new(3);
        rb.push(Value::new(OmiValue::Number(1.0), Some(100.0)));
        rb.push(Value::new(OmiValue::Number(2.0), Some(200.0)));
        rb.push(Value::new(OmiValue::Number(3.0), Some(300.0)));
        let s = rb.to_json_string();
        let v = parse_json(&s);
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        // Newest first (FR-010)
        assert_eq!(arr[0]["v"], 3.0);
        assert_eq!(arr[1]["v"], 2.0);
        assert_eq!(arr[2]["v"], 1.0);
    }

    #[test]
    fn ring_buffer_empty() {
        let rb = RingBuffer::new(3);
        let s = rb.to_json_string();
        assert_eq!(s, "[]");
    }

    #[test]
    fn info_item_minimal() {
        let item = InfoItem::new(10);
        let s = item.to_json_string();
        let v = parse_json(&s);
        assert!(v.is_object());
        // Empty item: no type, no desc, no meta, no values
        assert!(v.get("type").is_none());
        assert!(v.get("desc").is_none());
        assert!(v.get("meta").is_none());
        assert!(v.get("values").is_none());
    }

    #[test]
    fn info_item_with_all_fields() {
        let mut item = InfoItem::new(10);
        item.type_uri = Some("omi:temperature".into());
        item.desc = Some("Room temperature".into());
        let mut meta = BTreeMap::new();
        meta.insert("writable".into(), OmiValue::Bool(true));
        meta.insert("unit".into(), OmiValue::Str("°C".into()));
        item.meta = Some(meta);
        item.values.push(Value::new(OmiValue::Number(22.5), Some(1000.0)));

        let s = item.to_json_string();
        let v = parse_json(&s);
        assert_eq!(v["type"], "omi:temperature");
        assert_eq!(v["desc"], "Room temperature");
        assert_eq!(v["meta"]["writable"], true);
        assert_eq!(v["meta"]["unit"], "°C");
        let vals = v["values"].as_array().unwrap();
        assert_eq!(vals.len(), 1);
        assert_eq!(vals[0]["v"], 22.5);
    }

    #[test]
    fn object_minimal() {
        let obj = Object::new("DeviceA");
        let s = obj.to_json_string();
        let v = parse_json(&s);
        assert_eq!(v["id"], "DeviceA");
        assert!(v.get("type").is_none());
        assert!(v.get("desc").is_none());
        assert!(v.get("items").is_none());
        assert!(v.get("objects").is_none());
    }

    #[test]
    fn object_with_type_and_desc() {
        let mut obj = Object::new("House");
        obj.type_uri = Some("omi:building".into());
        obj.desc = Some("Main building".into());
        let s = obj.to_json_string();
        let v = parse_json(&s);
        assert_eq!(v["id"], "House");
        assert_eq!(v["type"], "omi:building");
        assert_eq!(v["desc"], "Main building");
    }

    #[test]
    fn object_with_nested_children() {
        let mut room = Object::new("Room1");
        let mut temp = InfoItem::new(5);
        temp.values.push(Value::new(OmiValue::Number(22.5), None));
        room.add_item("Temperature".into(), temp);

        let mut house = Object::new("House");
        house.add_child(room);

        let s = house.to_json_string();
        let v = parse_json(&s);
        assert_eq!(v["id"], "House");
        let room_v = &v["objects"]["Room1"];
        assert_eq!(room_v["id"], "Room1");
        let temp_v = &room_v["items"]["Temperature"];
        assert_eq!(temp_v["values"][0]["v"], 22.5);
    }

    #[test]
    fn object_depth_limited() {
        let inner = Object::new("Inner");
        let mut outer = Object::new("Outer");
        outer.add_child(inner);

        // Depth 0: only id/type/desc, no children
        let mut w = JsonWriter::new();
        outer.write_json_with_depth(&mut w, 0);
        let v = parse_json(&w.into_string());
        assert_eq!(v["id"], "Outer");
        assert!(v.get("objects").is_none());

        // Depth 1: include children as shells
        let mut w = JsonWriter::new();
        outer.write_json_with_depth(&mut w, 1);
        let v = parse_json(&w.into_string());
        assert_eq!(v["objects"]["Inner"]["id"], "Inner");
    }
}
