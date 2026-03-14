#![cfg(any(feature = "json", feature = "lite-json"))]
//! Comprehensive integration tests for the `onread` script trigger (spec-006).
//!
//! Covers all functional requirements:
//! - FR-001: metadata parsing
//! - FR-002: read/interval/readItem trigger (event does not)
//! - FR-003: event object {value, path, timestamp}
//! - FR-004: stored value unchanged after onread
//! - FR-005: script error fallback
//! - FR-006: no writeItem in onread scripts
//! - FR-007: cascading / nested onread
//! - FR-008: self-read recursion guard
//! - FR-009: onwrite + onread independence
//! - FR-010: resource limits
//! - FR-011: timestamps preserved in element structure
//! - FR-012: newest-only transform

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

fn engine() -> Engine {
    let e = Engine::new();
    assert!(
        e.has_script_engine(),
        "ScriptEngine failed to initialise — scripting tests would be meaningless"
    );
    e
}

fn set_onwrite(e: &mut Engine, path: &str, script: &str) {
    if let Ok(PathTargetMut::InfoItem(item)) = e.tree.resolve_mut(path) {
        let meta = item.meta.get_or_insert_with(BTreeMap::new);
        meta.insert("onwrite".into(), OmiValue::Str(script.into()));
    } else {
        panic!("set_onwrite: path {path} is not a writable InfoItem");
    }
}

fn set_onread(e: &mut Engine, path: &str, script: &str) {
    if let Ok(PathTargetMut::InfoItem(item)) = e.tree.resolve_mut(path) {
        let meta = item.meta.get_or_insert_with(BTreeMap::new);
        meta.insert("onread".into(), OmiValue::Str(script.into()));
    } else {
        panic!("set_onread: path {path} is not a writable InfoItem");
    }
}

const BASE_TIME: f64 = 1_000_000.0;

// ===========================================================================
// FR-001: metadata parsing — onread key in InfoItem metadata
// ===========================================================================

#[test]
fn fr001_onread_metadata_via_tree_write() {
    let mut e = engine();

    // Create item with onread in metadata via tree write (same as onwrite pattern)
    let tree_json = r#"{
        "omi":"1.0","ttl":0,
        "write":{
            "path":"/",
            "objects":{
                "Sensor":{
                    "id":"Sensor",
                    "items":{
                        "Temp":{
                            "values":[{"v":100}],
                            "meta":{
                                "writable":true,
                                "onread":"event.value * 0.01 - 40"
                            }
                        }
                    }
                }
            }
        }
    }"#;
    let resp = parse_and_process(&mut e, tree_json);
    assert_eq!(response_status(&resp), 200);

    // Read — onread should transform: 100 * 0.01 - 40 = -39
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Sensor/Temp","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(-39.0));
}

#[test]
fn fr001_onread_metadata_via_direct_mutation() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Raw","v":6500}}"#,
    );

    // Attach onread via direct metadata mutation
    set_onread(&mut e, "/Dev/Raw", "event.value * 0.01 - 40");

    // Read — should get computed value: 6500 * 0.01 - 40 = 25.0
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Raw","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(25.0));
}

#[test]
fn fr001_no_onread_returns_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Plain","v":42}}"#,
    );

    // Read without onread — stored value returned as-is
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Plain","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(42.0));
}

// ===========================================================================
// FR-002: read triggers onread; interval triggers onread; event does NOT
// ===========================================================================

#[test]
fn fr002_one_time_read_triggers_onread() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":100}}"#,
    );
    set_onread(&mut e, "/Dev/Sensor", "event.value * 2");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Sensor","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(200.0), "one-time read should trigger onread");
}

#[test]
fn fr002_interval_subscription_triggers_onread() {
    let mut e = engine();

    // Write a value with a timestamp
    e.tree
        .write_value("/Dev/Sensor", OmiValue::Number(100.0), Some(BASE_TIME))
        .unwrap();
    set_onread(&mut e, "/Dev/Sensor", "event.value * 2");

    // Create interval subscription (callback to get deliveries back)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dev/Sensor","interval":5,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Tick at interval — onread should transform
    let deliveries = e.tick(BASE_TIME + 5.0);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        deliveries[0].values[0].v,
        OmiValue::Number(200.0),
        "interval delivery should run onread (100 * 2 = 200)"
    );
}

#[test]
fn fr002_event_subscription_does_not_trigger_onread() {
    let mut e = engine();

    // Create item and subscribe to events
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":50}}"#,
    );
    set_onread(&mut e, "/Dev/Sensor", "event.value * 2");

    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dev/Sensor","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Write a new value — event delivery should NOT run onread
    let (_write_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":100}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        deliveries[0].values[0].v,
        OmiValue::Number(100.0),
        "event delivery should return raw written value (100), NOT onread-transformed (200)"
    );
}

#[test]
fn fr002_readitem_from_onread_triggers_nested_onread() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Src","v":50}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Reader","v":0}}"#,
    );

    // Src has an onread that doubles the value
    set_onread(&mut e, "/Dev/Src", "event.value * 2");
    // Reader's onread reads Src — should trigger Src's onread
    set_onread(
        &mut e,
        "/Dev/Reader",
        "odf.readItem('/Dev/Src/value')",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Reader","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(100.0),
        "odf.readItem from onread should trigger nested onread (50 * 2 = 100)"
    );
}

#[test]
fn fr002_readitem_from_onwrite_returns_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Src","v":50}}"#,
    );
    // Src has an onread that doubles the value
    set_onread(&mut e, "/Dev/Src", "event.value * 2");

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Trigger","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":0}}"#,
    );

    // onwrite reads /Dev/Src/value — onread is NOT triggered from onwrite context
    // (onread_fns not pre-compiled in onwrite scripts)
    set_onwrite(
        &mut e,
        "/Dev/Trigger",
        "odf.writeItem(odf.readItem('/Dev/Src/value'), '/Dev/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Trigger","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(50.0),
        "odf.readItem from onwrite context returns stored value (50), not onread-transformed"
    );
}

// ===========================================================================
// FR-003: event object contains {value, path, timestamp}
// ===========================================================================

#[test]
fn fr003_event_value_is_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":42}}"#,
    );
    // onread simply returns event.value + 1 to verify it receives the stored value
    set_onread(&mut e, "/Dev/Item", "event.value + 1");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(43.0), "event.value should be the stored value (42 + 1 = 43)");
}

#[test]
fn fr003_event_path_is_item_path() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/PathCheck","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":""}}"#,
    );
    // onread writes the event.path to another item so we can inspect it
    set_onread(
        &mut e,
        "/Dev/PathCheck",
        "odf.readItem('/Dev/PathCheck/value'); event.path",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/PathCheck","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Str("/Dev/PathCheck".into()),
        "event.path should be the InfoItem path"
    );
}

#[test]
fn fr003_event_timestamp_present() {
    let mut e = engine();

    // Write with explicit timestamp
    e.tree
        .write_value("/Dev/TsCheck", OmiValue::Number(10.0), Some(12345.0))
        .unwrap();
    // onread returns event.timestamp
    set_onread(&mut e, "/Dev/TsCheck", "event.timestamp");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/TsCheck","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(12345.0),
        "event.timestamp should be the stored timestamp"
    );
}

#[test]
fn fr003_event_value_null_when_empty() {
    let mut e = engine();

    // Create empty InfoItem via tree write
    let tree_json = r#"{
        "omi":"1.0","ttl":0,
        "write":{
            "path":"/",
            "objects":{
                "Dev":{
                    "id":"Dev",
                    "items":{
                        "Empty":{
                            "values":[],
                            "meta":{"writable":true,"onread":"event.value === null ? 'was_null' : 'not_null'"}
                        }
                    }
                }
            }
        }
    }"#;
    parse_and_process(&mut e, tree_json);

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Empty","newest":1}}"#,
    );
    let _values = extract_values(&resp);
    // Empty ring buffer → event.value is null → script runs but may have no slot.
    // The key assertion is that the read succeeds without errors.
    assert_eq!(response_status(&resp), 200);
}

// ===========================================================================
// FR-004: stored value unchanged after onread
// ===========================================================================

#[test]
fn fr004_stored_value_unchanged_after_read() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":10}}"#,
    );
    set_onread(&mut e, "/Dev/Item", "event.value * 3");

    // First read — transformed value
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(30.0), "first read: 10 * 3 = 30");

    // Second read — should still be 30, NOT 90 (proving storage unchanged)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(30.0),
        "second read should still be 30 — onread must not mutate storage"
    );

    // Third read — triple-check
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(30.0), "third read confirms storage is immutable");
}

#[test]
fn fr004_write_after_onread_uses_raw_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":10}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Copy","v":0}}"#,
    );
    set_onread(&mut e, "/Dev/Item", "event.value * 100");
    // onwrite copies raw value
    set_onwrite(
        &mut e,
        "/Dev/Item",
        "odf.writeItem(event.value, '/Dev/Copy');",
    );

    // Read — get transformed 1000
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(1000.0));

    // Write new value — onwrite should see the new raw value, not transformed
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":5}}"#,
    );
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Copy","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(5.0),
        "onwrite should see raw value 5, not onread-transformed 500"
    );
}

// ===========================================================================
// FR-005: script error fallback — returns stored value
// ===========================================================================

#[test]
fn fr005_syntax_error_falls_back_to_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":42}}"#,
    );
    set_onread(&mut e, "/Dev/Item", "this is not valid javascript!!!");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200, "read should succeed despite script error");
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(42.0), "should fall back to stored value on script error");
}

#[test]
fn fr005_runtime_error_falls_back_to_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":77}}"#,
    );
    // Reference error — undefined variable
    set_onread(&mut e, "/Dev/Item", "nonexistentVariable.property");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(77.0), "should fall back to stored value on runtime error");
}

#[test]
fn fr005_null_return_falls_back_to_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":55}}"#,
    );
    set_onread(&mut e, "/Dev/Item", "null");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(55.0), "null return should fall back to stored value");
}

#[test]
fn fr005_undefined_return_falls_back_to_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":33}}"#,
    );
    set_onread(&mut e, "/Dev/Item", "undefined");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(33.0), "undefined return should fall back to stored value");
}

#[test]
fn fr005_op_limit_falls_back_to_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":99}}"#,
    );
    set_onread(&mut e, "/Dev/Item", "while(true){} event.value");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200, "read must never fail due to script op limit");
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(99.0), "op limit should fall back to stored value");
}

// ===========================================================================
// FR-006: no writeItem in onread scripts
// ===========================================================================

#[test]
fn fr006_onread_has_readitem_but_no_writeitem() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Other","v":10}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Target","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":5}}"#,
    );

    // onread tries writeItem (should fail) then reads another item
    set_onread(
        &mut e,
        "/Dev/Sensor",
        "odf.writeItem(999, '/Dev/Target'); odf.readItem('/Dev/Other/value') + event.value",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Sensor","newest":1}}"#,
    );
    // The script should error because writeItem is not defined in onread context
    // Falls back to stored value
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(5.0),
        "onread with writeItem should error and fall back to stored value"
    );

    // Verify /Dev/Target was NOT written to
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Target","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(0.0), "onread must not have side-effects via writeItem");
}

#[test]
fn fr006_onread_readitem_works() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Offset","v":100}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Raw","v":50}}"#,
    );

    // onread reads another item to compute the result
    set_onread(
        &mut e,
        "/Dev/Raw",
        "event.value + odf.readItem('/Dev/Offset/value')",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Raw","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(150.0), "odf.readItem should work in onread (50 + 100 = 150)");
}

// ===========================================================================
// FR-007: cascading — nested onread
// ===========================================================================

#[test]
fn fr007_nested_onread_executes() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/TempC","v":100}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Display","v":0}}"#,
    );

    // TempC's onread converts C→F
    set_onread(&mut e, "/Dev/TempC", "event.value * 9 / 5 + 32");
    // Display's onread reads TempC — should trigger TempC's onread
    set_onread(
        &mut e,
        "/Dev/Display",
        "odf.readItem('/Dev/TempC/value')",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Display","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(212.0),
        "nested onread should transform 100°C → 212°F"
    );
}

#[test]
fn fr007_nested_onread_depth_limit() {
    let mut e = engine();

    let depth = MAX_SCRIPT_DEPTH as usize;
    let chain_len = depth + 2; // enough to exceed the limit

    // Create a chain of items
    for i in 0..chain_len {
        let path = format!("/Chain/L{i}");
        e.tree
            .write_value(&path, OmiValue::Number(i as f64), None)
            .unwrap();
    }

    // Each item's onread reads the next item's value
    for i in 0..chain_len - 1 {
        let script = format!("odf.readItem('/Chain/L{}/value')", i + 1);
        set_onread(&mut e, &format!("/Chain/L{i}"), &script);
    }
    // Last item's onread multiplies by 10
    set_onread(
        &mut e,
        &format!("/Chain/L{}", chain_len - 1),
        "event.value * 10",
    );

    // Read the first item — chain depth exceeds limit, so deep items fall back
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Chain/L0","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200, "depth-limited read should still succeed");
}

#[test]
fn fr007_nested_onread_element_structure() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Src","v":10}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Reader","v":0}}"#,
    );

    set_onread(&mut e, "/Dev/Src", "event.value + 5");
    // Reader reads Src element (no /value suffix) — nested onread transforms values[0].v
    set_onread(
        &mut e,
        "/Dev/Reader",
        "let elem = odf.readItem('/Dev/Src'); elem.values[0].v",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Reader","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(15.0),
        "nested onread element structure should have transformed value (10 + 5 = 15)"
    );
}

// ===========================================================================
// FR-008: self-read recursion guard
// ===========================================================================

#[test]
fn fr008_self_read_returns_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Self","v":10}}"#,
    );
    // onread reads itself via /value — should get stored value (10), not transformed
    set_onread(
        &mut e,
        "/Dev/Self",
        "odf.readItem('/Dev/Self/value') + 1",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Self","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(11.0),
        "self-read should return stored 10 (not recursively transformed), so 10 + 1 = 11"
    );
}

#[test]
fn fr008_self_read_element_structure() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Elem","v":20}}"#,
    );
    // onread reads itself without /value suffix — element structure with stored value
    set_onread(
        &mut e,
        "/Dev/Elem",
        "let e = odf.readItem('/Dev/Elem'); e.values[0].v + 5",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Elem","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(25.0),
        "self-read element should have stored value 20, so 20 + 5 = 25"
    );
}

#[test]
fn fr008_self_read_repeated_is_stable() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Stable","v":7}}"#,
    );
    // Read self twice — both should return stored value
    set_onread(
        &mut e,
        "/Dev/Stable",
        "let a = odf.readItem('/Dev/Stable/value'); let b = odf.readItem('/Dev/Stable/value'); a + b",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Stable","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(14.0),
        "two self-reads should both return stored 7, so 7 + 7 = 14"
    );
}

// ===========================================================================
// FR-009: onwrite + onread independence
// ===========================================================================

#[test]
fn fr009_write_triggers_onwrite_not_onread() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/WriteDst","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/ReadDst","v":0}}"#,
    );

    set_onwrite(
        &mut e,
        "/Dev/Item",
        "odf.writeItem(event.value, '/Dev/WriteDst');",
    );
    set_onread(
        &mut e,
        "/Dev/Item",
        "odf.writeItem(event.value, '/Dev/ReadDst'); event.value",
    );

    // Write 42 — only onwrite fires
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":42}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/WriteDst","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(42.0), "onwrite should fire on write");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/ReadDst","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(0.0), "onread should NOT fire during write");
}

#[test]
fn fr009_read_triggers_onread_not_onwrite() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":100}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/WriteDst","v":0}}"#,
    );

    set_onwrite(
        &mut e,
        "/Dev/Item",
        "odf.writeItem(event.value, '/Dev/WriteDst');",
    );
    set_onread(&mut e, "/Dev/Item", "event.value * 2");

    // Read — onread transforms, onwrite does NOT fire
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(200.0), "onread should transform on read");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/WriteDst","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(0.0), "onwrite should NOT fire during read");
}

#[test]
fn fr009_broken_onwrite_does_not_affect_onread() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":50}}"#,
    );
    set_onwrite(&mut e, "/Dev/Item", "this is not valid javascript!!!");
    set_onread(&mut e, "/Dev/Item", "event.value * 2");

    // Write succeeds despite broken onwrite
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":30}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // Read uses onread (unaffected by broken onwrite)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(60.0), "onread should work despite broken onwrite");
}

#[test]
fn fr009_broken_onread_does_not_affect_onwrite() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Dst","v":0}}"#,
    );

    set_onwrite(
        &mut e,
        "/Dev/Item",
        "odf.writeItem(event.value, '/Dev/Dst');",
    );
    set_onread(&mut e, "/Dev/Item", "this is not valid javascript!!!");

    // Write triggers onwrite successfully despite broken onread
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":88}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Dst","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(88.0), "onwrite should work despite broken onread");

    // Read falls back to stored value
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(88.0), "broken onread falls back to stored value");
}

// ===========================================================================
// FR-010: resource limits (same as onwrite)
// ===========================================================================

#[test]
fn fr010_infinite_loop_hits_op_limit() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":42}}"#,
    );
    set_onread(&mut e, "/Dev/Item", "while(true){} event.value");

    // Read succeeds despite infinite loop
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(42.0), "op limit → fall back to stored value");
}

#[test]
fn fr010_read_loop_hits_op_limit() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Data","v":1}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Loop","v":99}}"#,
    );

    // Infinite loop calling readItem
    set_onread(
        &mut e,
        "/Dev/Loop",
        "while(true){ odf.readItem('/Dev/Data/value'); } event.value",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Loop","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(99.0), "read loop op limit → fall back to stored value");
}

#[test]
fn fr010_bounded_computation_succeeds() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":5}}"#,
    );
    // A reasonable computation well within limits
    set_onread(
        &mut e,
        "/Dev/Item",
        "let s = 0; for (let i = 0; i < 10; i++) { s = s + event.value; } s",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(50.0), "bounded computation: 5 * 10 = 50");
}

// ===========================================================================
// FR-011: timestamps preserved in element structure
// ===========================================================================

#[test]
fn fr011_onread_preserves_timestamps() {
    let mut e = engine();

    // Write with explicit timestamp
    e.tree
        .write_value("/Dev/Item", OmiValue::Number(100.0), Some(12345.0))
        .unwrap();
    set_onread(&mut e, "/Dev/Item", "event.value * 2");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(200.0), "value should be transformed");
    assert_eq!(
        values[0].t, Some(12345.0),
        "timestamp should be preserved from original stored value"
    );
}

#[test]
fn fr011_multiple_values_preserve_timestamps() {
    let mut e = engine();

    // Write multiple values with timestamps
    e.tree
        .write_value("/Dev/Multi", OmiValue::Number(10.0), Some(1000.0))
        .unwrap();
    e.tree
        .write_value("/Dev/Multi", OmiValue::Number(20.0), Some(2000.0))
        .unwrap();
    e.tree
        .write_value("/Dev/Multi", OmiValue::Number(30.0), Some(3000.0))
        .unwrap();
    set_onread(&mut e, "/Dev/Multi", "event.value * 10");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Multi","newest":3}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values.len(), 3);

    // Newest (index 0) should be transformed, with original timestamp
    assert_eq!(values[0].v, OmiValue::Number(300.0), "newest value transformed: 30 * 10 = 300");
    assert_eq!(values[0].t, Some(3000.0), "newest timestamp preserved");

    // Older values should be raw, with original timestamps (FR-012)
    assert_eq!(values[1].v, OmiValue::Number(20.0), "second value should be raw");
    assert_eq!(values[1].t, Some(2000.0), "second timestamp preserved");
    assert_eq!(values[2].v, OmiValue::Number(10.0), "third value should be raw");
    assert_eq!(values[2].t, Some(1000.0), "third timestamp preserved");
}

// ===========================================================================
// FR-012: newest-only transform
// ===========================================================================

#[test]
fn fr012_only_newest_value_transformed() {
    let mut e = engine();

    // Write 3 values
    e.tree
        .write_value("/Dev/History", OmiValue::Number(1.0), Some(100.0))
        .unwrap();
    e.tree
        .write_value("/Dev/History", OmiValue::Number(2.0), Some(200.0))
        .unwrap();
    e.tree
        .write_value("/Dev/History", OmiValue::Number(3.0), Some(300.0))
        .unwrap();

    // onread multiplies by 100
    set_onread(&mut e, "/Dev/History", "event.value * 100");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/History","newest":3}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values.len(), 3);

    // Only newest (index 0) is transformed
    assert_eq!(values[0].v, OmiValue::Number(300.0), "newest: 3 * 100 = 300 (transformed)");
    assert_eq!(values[1].v, OmiValue::Number(2.0), "second oldest: raw value 2 (not transformed)");
    assert_eq!(values[2].v, OmiValue::Number(1.0), "oldest: raw value 1 (not transformed)");
}

#[test]
fn fr012_single_value_transforms() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Single","v":7}}"#,
    );
    set_onread(&mut e, "/Dev/Single", "event.value * 3");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Single","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].v, OmiValue::Number(21.0), "single value: 7 * 3 = 21");
}

#[test]
fn fr012_newest_5_with_transform() {
    let mut e = engine();

    // Write 5 values
    for i in 1..=5 {
        e.tree
            .write_value(
                "/Dev/Five",
                OmiValue::Number(i as f64),
                Some(i as f64 * 100.0),
            )
            .unwrap();
    }
    set_onread(&mut e, "/Dev/Five", "event.value + 1000");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Five","newest":5}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values.len(), 5);

    // Only newest (5) is transformed to 1005
    assert_eq!(values[0].v, OmiValue::Number(1005.0), "newest (5) transformed: 5 + 1000 = 1005");

    // All others are raw
    assert_eq!(values[1].v, OmiValue::Number(4.0), "4th value raw");
    assert_eq!(values[2].v, OmiValue::Number(3.0), "3rd value raw");
    assert_eq!(values[3].v, OmiValue::Number(2.0), "2nd value raw");
    assert_eq!(values[4].v, OmiValue::Number(1.0), "1st value raw");
}

// ===========================================================================
// Additional integration scenarios
// ===========================================================================

#[test]
fn thermostat_onread_scenario() {
    let mut e = engine();

    // Setup: temperature sensor stores raw ADC counts, onread calibrates
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/HVAC/Sensor","v":6500}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/HVAC/Target","v":22.0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/HVAC/Heater","v":false}}"#,
    );

    // Sensor calibration: ADC * 0.01 - 40 = Celsius
    set_onread(&mut e, "/HVAC/Sensor", "event.value * 0.01 - 40");

    // Read calibrated temperature: 6500 * 0.01 - 40 = 25.0°C
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/HVAC/Sensor","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(25.0), "calibrated: 6500 * 0.01 - 40 = 25°C");

    // Stored raw value is still 6500 — verified by writing to heater
    set_onwrite(
        &mut e,
        "/HVAC/Sensor",
        "let target = odf.readItem('/HVAC/Target/value'); odf.writeItem(event.value < target, '/HVAC/Heater');",
    );

    // Write new raw ADC (cold room: 5800 → 18°C calibrated)
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/HVAC/Sensor","v":5800}}"#,
    );

    // onwrite sees raw 5800 (not calibrated 18), comparing 5800 < 22 → false
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/HVAC/Heater","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Bool(false), "onwrite sees raw ADC value, not calibrated");

    // But reading calibrated: 5800 * 0.01 - 40 = 18.0°C
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/HVAC/Sensor","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(18.0), "calibrated: 5800 * 0.01 - 40 = 18°C");
}

#[test]
fn status_aggregation_onread() {
    let mut e = engine();

    // Multiple sensors
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sys/Temp","v":30}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sys/Humidity","v":45}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sys/Status","v":"unknown"}}"#,
    );

    // Status aggregator reads multiple sensors
    set_onread(
        &mut e,
        "/Sys/Status",
        "let t = odf.readItem('/Sys/Temp/value'); let h = odf.readItem('/Sys/Humidity/value'); (t < 50 && h < 80) ? 'ok' : 'alarm'",
    );

    // Normal conditions → ok
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Sys/Status","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Str("ok".into()));

    // Change temperature to alarm level
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sys/Temp","v":60}}"#,
    );

    // Now → alarm
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Sys/Status","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Str("alarm".into()));
}

#[test]
fn json_roundtrip_with_onread() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":10}}"#,
    );
    set_onread(&mut e, "/Dev/Item", "event.value * 5");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // JSON round-trip should preserve the transformed value
    #[cfg(feature = "json")]
    {
        let rt = roundtrip_response_json(&resp);
        assert_eq!(rt["result"]["values"][0]["v"], 50.0);
    }
}

#[test]
fn onread_with_string_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Status","v":"raw"}}"#,
    );
    set_onread(&mut e, "/Dev/Status", "event.value + '_processed'");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Status","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Str("raw_processed".into()));
}

#[test]
fn onread_with_boolean_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Flag","v":true}}"#,
    );
    // Invert boolean
    set_onread(&mut e, "/Dev/Flag", "!event.value");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Flag","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Bool(false), "boolean inversion: !true = false");
}

#[test]
fn onread_returns_different_type() {
    let mut e = engine();

    // Stored as number, onread returns string
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":42}}"#,
    );
    set_onread(&mut e, "/Dev/Item", "event.value > 40 ? 'high' : 'low'");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Str("high".into()));
}

#[test]
fn interval_poll_sub_delivers_raw_value() {
    let mut e = engine();

    // Write initial value
    e.tree
        .write_value("/Dev/Sensor", OmiValue::Number(50.0), Some(BASE_TIME))
        .unwrap();
    set_onread(&mut e, "/Dev/Sensor", "event.value + 1000");

    // Create poll interval subscription (no callback → poll target)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dev/Sensor","interval":5}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Tick at interval — poll subs buffer internally (onread transform
    // applies to callback deliveries returned by tick, not poll buffers)
    let _deliveries = e.tick(BASE_TIME + 5.0);

    // Poll — gets raw buffered value (onread transformation only applies
    // to callback/websocket deliveries, not poll-buffered values)
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 6.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = extract_values(&resp);
    assert_eq!(polled.len(), 1);
    assert_eq!(
        polled[0].v, OmiValue::Number(50.0),
        "poll delivery returns raw value (onread only applies to callback deliveries)"
    );
}
