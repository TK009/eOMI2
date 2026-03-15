//! OMI message round-trip tests (SC-004).
//!
//! Construct OmiMessage → serialize (lite-json) → parse → compare.
//! Proves data survives the full serialize/parse cycle at the message level.

#![cfg(feature = "lite-json")]

use std::collections::BTreeMap;

use reconfigurable_device::json::serializer::ToJson;
use reconfigurable_device::json::parser::parse_omi_message;
use reconfigurable_device::odf::{Object, OmiValue};
use reconfigurable_device::omi::{
    OmiMessage, Operation,
    read::ReadOp,
    write::{WriteOp, WriteItem},
    delete::DeleteOp,
    cancel::CancelOp,
    response::{ResponseBody, ResponseResult, ItemStatus},
};

/// Serialize an OmiMessage, then parse it back.
fn roundtrip(msg: &OmiMessage) -> OmiMessage {
    let json = msg.to_json_string();
    parse_omi_message(&json).unwrap_or_else(|e| {
        panic!("round-trip parse failed: {e}\nJSON: {json}")
    })
}

// ---- Read operations ----

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

// ---- Write operations ----

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
fn roundtrip_write_single_string() {
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
fn roundtrip_write_single_bool() {
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
fn roundtrip_write_single_null() {
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

// ---- Delete and Cancel ----

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

// ---- Response operations ----

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
