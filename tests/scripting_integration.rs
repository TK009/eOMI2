//! Integration tests for the scripting engine + ODF interaction.
//!
//! These exercise the full JSON round-trip for onwrite scripts:
//! **JSON string → parse → engine.process → response → serialize → verify**.
//! Script attachment uses `tree.resolve_mut()` directly because there is no
//! JSON API for setting item metadata.

#![cfg(feature = "scripting")]

use std::collections::BTreeMap;

use reconfigurable_device::odf::{OmiValue, PathTargetMut};
use reconfigurable_device::omi::{Engine, OmiMessage, ResponseResult};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn engine() -> Engine {
    Engine::new()
}

fn parse_and_process(engine: &mut Engine, json: &str) -> OmiMessage {
    let msg = OmiMessage::parse(json).expect("request JSON should parse");
    engine.process(msg, 0.0, None)
}

fn response_status(resp: &OmiMessage) -> u16 {
    match &resp.operation {
        reconfigurable_device::omi::Operation::Response(body) => body.status,
        _ => panic!("expected Response"),
    }
}

fn response_result(resp: &OmiMessage) -> &serde_json::Value {
    match &resp.operation {
        reconfigurable_device::omi::Operation::Response(body) => match &body.result {
            Some(ResponseResult::Single(v)) => v,
            other => panic!("expected Single result, got {:?}", other),
        },
        _ => panic!("expected Response"),
    }
}

fn roundtrip_response_json(resp: &OmiMessage) -> serde_json::Value {
    let json_str = serde_json::to_string(resp).expect("response should serialize");
    let v: serde_json::Value = serde_json::from_str(&json_str).expect("should re-parse");
    v["response"].clone()
}

/// Attach an onwrite script to a writable InfoItem (via direct tree mutation).
fn set_onwrite(e: &mut Engine, path: &str, script: &str) {
    if let Ok(PathTargetMut::InfoItem(item)) = e.tree.resolve_mut(path) {
        let meta = item.meta.get_or_insert_with(BTreeMap::new);
        meta.insert("onwrite".into(), OmiValue::Str(script.into()));
    } else {
        panic!("set_onwrite: path {path} is not a writable InfoItem");
    }
}

// ===========================================================================
// 4.1  Script reads written value
// ===========================================================================

#[test]
fn script_reads_written_value() {
    let mut e = engine();

    // Create /Dev/Src and /Dev/Dst via JSON writes
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Src","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Dst","v":0}}"#,
    );

    // Attach onwrite script: copy value to /Dev/Dst
    set_onwrite(
        &mut e,
        "/Dev/Src",
        "odf.writeItem(event.value, '/Dev/Dst');",
    );

    // Write 42.0 to /Dev/Src via JSON
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Src","v":42}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // Read /Dev/Dst via JSON — should have the copied value
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Dst","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = response_result(&resp)["values"].as_array().unwrap();
    assert_eq!(values[0]["v"], 42.0);

    // JSON round-trip
    let rt = roundtrip_response_json(&resp);
    assert_eq!(rt["result"]["values"][0]["v"], 42.0);
}

// ===========================================================================
// 4.2  Script triggers cascading write (C→F conversion)
// ===========================================================================

#[test]
fn script_triggers_cascading_write() {
    let mut e = engine();

    // Create /Dev/TempC and /Dev/TempF via JSON writes
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/TempC","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/TempF","v":0}}"#,
    );

    // Attach C→F conversion script
    set_onwrite(
        &mut e,
        "/Dev/TempC",
        "odf.writeItem(event.value * 9 / 5 + 32, '/Dev/TempF');",
    );

    // Write 100°C (boiling point)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/TempC","v":100}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // Read /Dev/TempF — should be 212°F
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/TempF","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = response_result(&resp)["values"].as_array().unwrap();
    assert_eq!(values[0]["v"], 212.0);

    // JSON round-trip
    let rt = roundtrip_response_json(&resp);
    assert_eq!(rt["result"]["values"][0]["v"], 212.0);
}

// ===========================================================================
// 4.3  Script error does not block write
// ===========================================================================

#[test]
fn script_error_does_not_block_write() {
    let mut e = engine();

    // Create /Dev/Item via JSON write
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":0}}"#,
    );

    // Attach a broken script
    set_onwrite(&mut e, "/Dev/Item", "this is not valid javascript!!!");

    // Write 7.0 — should succeed despite script error
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":7}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // Read back — value should be written
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = response_result(&resp)["values"].as_array().unwrap();
    assert_eq!(values[0]["v"], 7.0);

    // JSON round-trip
    let rt = roundtrip_response_json(&resp);
    assert_eq!(rt["result"]["values"][0]["v"], 7.0);
}
