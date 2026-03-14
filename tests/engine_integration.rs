#![cfg(feature = "json")]
//! Integration tests for the OMI engine.
//!
//! Unlike the unit tests in `src/omi/engine.rs`, these exercise the full
//! round-trip: **JSON string → parse → engine.process → response → serialize
//! to JSON → verify**.  They also use the real sensor tree from
//! `device::build_sensor_tree()` rather than hand-rolled fixtures.

mod common;

use common::*;
use reconfigurable_device::odf::OmiValue;
use reconfigurable_device::omi::error::ParseError;
use reconfigurable_device::omi::{Engine, OmiMessage};
use serde_json::json;

// ---------------------------------------------------------------------------
// Helpers (file-specific)
// ---------------------------------------------------------------------------

// (All helpers are in common/mod.rs)

// ===========================================================================
// 1.1  Read Operations
// ===========================================================================

#[test]
fn read_root_returns_objects() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(&mut e, r#"{"omi":"1.0","ttl":0,"read":{"path":"/"}}"#);
    assert_eq!(response_status(&resp), 200);
    let result = extract_json_result(&resp);
    assert!(result["System"].is_object(), "root should contain System");

    // Verify via JSON round-trip
    let rt = roundtrip_response_json(&resp);
    assert_eq!(rt["status"], 200);
    assert!(rt["result"]["System"].is_object());
}

#[test]
fn read_object_returns_items() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(&mut e, r#"{"omi":"1.0","ttl":0,"read":{"path":"/System"}}"#);
    assert_eq!(response_status(&resp), 200);
    let result = extract_json_result(&resp);
    assert_eq!(result["id"], "System");
    assert!(result["items"]["FreeHeap"].is_object());
}

#[test]
fn read_infoitem_empty() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/System/FreeHeap"}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values.len(), 0);
}

#[test]
fn read_infoitem_with_values() {
    let mut e = engine_with_sensor_tree();
    // Directly write a value into the sensor item (bypasses writability check).
    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(23.5), Some(1000.0))
        .unwrap();

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/System/FreeHeap"}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].v, OmiValue::Number(23.5));

    // JSON round-trip
    let rt = roundtrip_response_json(&resp);
    assert_eq!(rt["result"]["values"][0]["v"], 23.5);
}

#[test]
fn read_newest_oldest_filters() {
    let mut e = engine_with_sensor_tree();
    for i in 1..=5 {
        e.tree
            .write_value(
                "/System/FreeHeap",
                OmiValue::Number(20.0 + i as f64),
                Some(i as f64 * 100.0),
            )
            .unwrap();
    }

    // newest=2
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/System/FreeHeap","newest":2}}"#,
    );
    assert_eq!(extract_values(&resp).len(), 2);

    // oldest=2
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/System/FreeHeap","oldest":2}}"#,
    );
    assert_eq!(extract_values(&resp).len(), 2);
}

#[test]
fn read_time_range() {
    let mut e = engine_with_sensor_tree();
    for t in [100.0, 200.0, 300.0] {
        e.tree
            .write_value("/System/FreeHeap", OmiValue::Number(t / 10.0), Some(t))
            .unwrap();
    }

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/System/FreeHeap","begin":150,"end":250}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].t, Some(200.0));
}

#[test]
fn read_with_depth() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/System","depth":0}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let result = extract_json_result(&resp);
    assert_eq!(result["id"], "System");
    // depth=0 should omit nested items
    assert!(result.get("items").is_none());
}

#[test]
fn read_with_depth_includes_items() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/System","depth":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let result = extract_json_result(&resp);
    assert_eq!(result["id"], "System");
    // depth=1 should include items
    assert!(result["items"]["FreeHeap"].is_object());
}

#[test]
fn read_nonexistent_path() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/NoSuchThing"}}"#,
    );
    assert_eq!(response_status(&resp), 404);

    // Verify 404 survives JSON serialization round-trip
    let rt = roundtrip_response_json(&resp);
    assert_eq!(rt["status"], 404);
}

// ===========================================================================
// 1.2  Write Operations
// ===========================================================================

#[test]
fn write_new_path_creates_item() {
    let mut e = engine_with_sensor_tree();

    // Write via JSON
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/MyObj/MyItem","v":42}}"#,
    );
    assert_eq!(response_status(&resp), 201);

    // Read back via JSON
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/MyObj/MyItem","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(42.0));
}

#[test]
fn write_new_then_update() {
    let mut e = engine_with_sensor_tree();

    // First write — creates (201)
    let r1 = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Act/Switch","v":true}}"#,
    );
    assert_eq!(response_status(&r1), 201);

    // Second write — updates (200)
    let r2 = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Act/Switch","v":false}}"#,
    );
    assert_eq!(response_status(&r2), 200);
}

#[test]
fn write_read_only_rejected() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/System/FreeHeap","v":99}}"#,
    );
    assert_eq!(response_status(&resp), 403);
}

#[test]
fn write_batch_mixed_results() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{
            "omi":"1.0","ttl":10,
            "write":{
                "items":[
                    {"path":"/NewObj/Item1","v":1},
                    {"path":"/System/FreeHeap","v":99}
                ]
            }
        }"#,
    );
    assert_eq!(response_status(&resp), 200);
    let batch = response_batch(&resp);
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0].status, 201); // new path created
    assert_eq!(batch[1].status, 403); // sensor item not writable

    // Verify batch survives JSON round-trip
    let rt = roundtrip_response_json(&resp);
    let items = rt["result"].as_array().unwrap();
    assert_eq!(items[0]["status"], 201);
    assert_eq!(items[1]["status"], 403);
}

#[test]
fn write_tree_merges_objects() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{
            "omi":"1.0","ttl":10,
            "write":{
                "path":"/",
                "objects":{
                    "Garage":{"id":"Garage"}
                }
            }
        }"#,
    );
    assert_eq!(response_status(&resp), 200);

    // New object should be readable
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Garage"}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    assert_eq!(extract_json_result(&resp)["id"], "Garage");

    // Original sensor tree should still exist
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/System"}}"#,
    );
    assert_eq!(response_status(&resp), 200);
}

#[test]
fn write_then_read_roundtrip_all_types() {
    let mut e = Engine::new();

    // Note: JSON `"v": null` is indistinguishable from absent `v` in the
    // parser (Option<OmiValue>), so OmiValue::Null is not testable via JSON.
    let cases: &[(&str, &str, OmiValue, serde_json::Value)] = &[
        (
            r#"{"omi":"1.0","ttl":10,"write":{"path":"/T/Str","v":"hello"}}"#,
            "/T/Str",
            OmiValue::Str("hello".into()),
            json!("hello"),
        ),
        (
            r#"{"omi":"1.0","ttl":10,"write":{"path":"/T/Num","v":3.14}}"#,
            "/T/Num",
            OmiValue::Number(3.14),
            json!(3.14),
        ),
        (
            r#"{"omi":"1.0","ttl":10,"write":{"path":"/T/Bool","v":true}}"#,
            "/T/Bool",
            OmiValue::Bool(true),
            json!(true),
        ),
    ];

    for (write_json, _, _, _) in cases {
        parse_and_process(&mut e, write_json);
    }

    // Read each back and verify the value survives the full round-trip
    for (_, path, expected_omi, expected_json) in cases {
        let read_json = format!(
            r#"{{"omi":"1.0","ttl":0,"read":{{"path":"{}","newest":1}}}}"#,
            path
        );
        let resp = parse_and_process(&mut e, &read_json);
        assert_eq!(response_status(&resp), 200);
        let values = extract_values(&resp);
        assert_eq!(
            values[0].v, *expected_omi,
            "round-trip mismatch for path {}",
            path
        );

        // Also verify via serialization round-trip
        let rt = roundtrip_response_json(&resp);
        assert_eq!(rt["result"]["values"][0]["v"], *expected_json);
    }
}

// ===========================================================================
// 1.3  Delete Operations
// ===========================================================================

#[test]
fn delete_existing_object() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"delete":{"path":"/System"}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // Verify it's gone
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/System"}}"#,
    );
    assert_eq!(response_status(&resp), 404);
}

#[test]
fn delete_root_forbidden() {
    // The parser itself rejects delete of root before it reaches the engine.
    let err = OmiMessage::parse(r#"{"omi":"1.0","ttl":0,"delete":{"path":"/"}}"#).unwrap_err();
    assert!(
        matches!(err, ParseError::InvalidField { field: "path", .. }),
        "expected InvalidField for root delete, got {:?}",
        err
    );
}

#[test]
fn delete_nonexistent() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"delete":{"path":"/Ghost"}}"#,
    );
    assert_eq!(response_status(&resp), 404);
}

// ===========================================================================
// 1.4  Subscriptions & Cancel
// ===========================================================================

#[test]
fn subscribe_poll_returns_rid() {
    let mut e = engine_with_sensor_tree();
    // interval triggers Subscription kind; no callback + no ws_session → Poll target
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":5.0}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp);
    assert!(!rid.is_empty(), "subscription should return a non-empty rid");

    // Verify rid survives JSON round-trip
    let rt = roundtrip_response_json(&resp);
    assert_eq!(rt["rid"], rid);
}

#[test]
fn subscribe_requires_positive_ttl() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/System/FreeHeap","interval":5.0}}"#,
    );
    assert_eq!(response_status(&resp), 400);
}

#[test]
fn subscribe_then_poll_interval() {
    let mut e = engine_with_sensor_tree();

    // Write a value so the interval tick has something to return
    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(22.0), Some(1000.0))
        .unwrap();

    // Create poll subscription with 5s interval
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":5.0}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_owned();

    // Tick the engine past the interval trigger time (created at now=0, triggers at 0+5=5)
    e.tick(6.0);

    // Poll for buffered results
    let poll_json = format!(
        r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#,
        rid
    );
    let msg = OmiMessage::parse(&poll_json).expect("poll JSON should parse");
    let (resp, _) = e.process(msg, 6.0, None);
    assert_eq!(response_status(&resp), 200);
    assert_eq!(extract_read_path(&resp), "/System/FreeHeap");
}

#[test]
fn cancel_active_subscription() {
    let mut e = engine_with_sensor_tree();

    // Create a subscription
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":5.0}}"#,
    );
    let rid = response_rid(&resp).to_owned();

    // Cancel it
    let cancel_json = format!(
        r#"{{"omi":"1.0","ttl":0,"cancel":{{"rid":["{}"]}}}}"#,
        rid
    );
    let resp = parse_and_process(&mut e, &cancel_json);
    assert_eq!(response_status(&resp), 200);

    // Polling the cancelled rid should return 404
    let poll_json = format!(
        r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#,
        rid
    );
    let msg = OmiMessage::parse(&poll_json).expect("poll JSON should parse");
    let (resp, _) = e.process(msg, 1.0, None);
    assert_eq!(response_status(&resp), 404);
}

#[test]
fn cancel_nonexistent_rid() {
    let mut e = engine_with_sensor_tree();
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"cancel":{"rid":["rid-999"]}}"#,
    );
    // Cancel is idempotent — always 200 even for unknown rids
    assert_eq!(response_status(&resp), 200);
}

// ===========================================================================
// 1.5  Error Handling (parse-level)
// ===========================================================================

#[test]
fn malformed_json_rejected() {
    let err = OmiMessage::parse("not json").unwrap_err();
    assert!(matches!(err, ParseError::InvalidJson(_)));
}

#[test]
fn missing_operation_rejected() {
    let err = OmiMessage::parse(r#"{"omi":"1.0","ttl":0}"#).unwrap_err();
    assert_eq!(err, ParseError::InvalidOperationCount(0));
}

#[test]
fn wrong_version_rejected() {
    let err =
        OmiMessage::parse(r#"{"omi":"2.0","ttl":0,"read":{"path":"/"}}"#).unwrap_err();
    assert_eq!(err, ParseError::UnsupportedVersion("2.0".into()));
}
