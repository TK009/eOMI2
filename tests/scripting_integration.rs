//! Integration tests for the scripting engine + ODF interaction.
//!
//! These exercise the full JSON round-trip for onwrite scripts:
//! **JSON string → parse → engine.process → response → serialize → verify**.
//! Script attachment uses `tree.resolve_mut()` directly because there is no
//! JSON API for setting item metadata.

#![cfg(feature = "scripting")]

mod common;

use std::collections::BTreeMap;

use common::*;
use reconfigurable_device::odf::{OmiValue, PathTargetMut};
use reconfigurable_device::omi::Engine;
use reconfigurable_device::scripting::bindings::MAX_SCRIPT_DEPTH;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an engine and assert the script engine initialized successfully.
///
/// If the scripting feature is misconfigured, this panics immediately rather
/// than letting tests pass vacuously.
fn engine() -> Engine {
    let e = Engine::new();
    assert!(
        e.has_script_engine(),
        "ScriptEngine failed to initialise — scripting tests would be meaningless"
    );
    e
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
// 1.  Script reads written value
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
// 2.  Script triggers cascading write (C→F conversion)
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
// 3.  Script error does not block write
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

// ===========================================================================
// 4.  Cascading depth limit prevents runaway scripts
// ===========================================================================

#[test]
fn cascading_depth_limit_stops_chain() {
    let mut e = engine();

    // Create a chain of items: /Chain/L0 → /Chain/L1 → ... → /Chain/L{MAX+1}
    // Each item's onwrite copies to the next.
    let depth = MAX_SCRIPT_DEPTH as usize;
    let chain_len = depth + 2; // enough items to exceed the limit

    for i in 0..chain_len {
        let path = format!("/Chain/L{i}");
        let json = format!(
            r#"{{"omi":"1.0","ttl":10,"write":{{"path":"{}","v":-1}}}}"#,
            path
        );
        parse_and_process(&mut e, &json);
    }

    // Wire each item to forward to the next
    for i in 0..chain_len - 1 {
        let script = format!("odf.writeItem(event.value, '/Chain/L{}');", i + 1);
        set_onwrite(&mut e, &format!("/Chain/L{i}"), &script);
    }

    // Trigger the chain by writing to L0
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Chain/L0","v":99}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // Items within the depth limit should have been updated
    for i in 0..depth {
        let json = format!(
            r#"{{"omi":"1.0","ttl":0,"read":{{"path":"/Chain/L{}","newest":1}}}}"#,
            i
        );
        let resp = parse_and_process(&mut e, &json);
        assert_eq!(response_status(&resp), 200);
        let values = response_result(&resp)["values"].as_array().unwrap();
        assert_eq!(
            values[0]["v"], 99.0,
            "/Chain/L{i} should have been updated (within depth limit)"
        );
    }

    // Items beyond the depth limit should retain initial value (-1)
    for i in depth..chain_len {
        let json = format!(
            r#"{{"omi":"1.0","ttl":0,"read":{{"path":"/Chain/L{}","newest":1}}}}"#,
            i
        );
        let resp = parse_and_process(&mut e, &json);
        assert_eq!(response_status(&resp), 200);
        let values = response_result(&resp)["values"].as_array().unwrap();
        assert_eq!(
            values[0]["v"], -1.0,
            "/Chain/L{i} should NOT have been updated (beyond depth limit)"
        );
    }
}
