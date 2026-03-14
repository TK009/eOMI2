#![cfg(any(feature = "json", feature = "lite-json"))]
//! Independence tests for onwrite + onread scripts on the same InfoItem.
//!
//! Verifies that write triggers onwrite only, read triggers onread only,
//! and neither interferes with the other. [FR-009]

#![cfg(feature = "scripting")]

mod common;

use std::collections::BTreeMap;

use common::*;
use reconfigurable_device::odf::{OmiValue, PathTargetMut};
use reconfigurable_device::omi::Engine;

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

// ===========================================================================
// 1.  Write triggers onwrite, not onread
// ===========================================================================

#[test]
fn write_triggers_onwrite_not_onread() {
    let mut e = engine();

    // Create items
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

    // onwrite copies value to /Dev/WriteDst
    set_onwrite(
        &mut e,
        "/Dev/Item",
        "odf.writeItem(event.value, '/Dev/WriteDst');",
    );
    // onread copies value to /Dev/ReadDst (should NOT fire on write)
    set_onread(
        &mut e,
        "/Dev/Item",
        "odf.writeItem(event.value, '/Dev/ReadDst'); event.value",
    );

    // Write 42 to /Dev/Item — only onwrite should fire
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":42}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // /Dev/WriteDst should have 42 (onwrite fired)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/WriteDst","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(42.0), "onwrite should have fired on write");

    // /Dev/ReadDst should still be 0 (onread should NOT have fired)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/ReadDst","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(0.0),
        "onread should NOT fire during a write operation"
    );
}

// ===========================================================================
// 2.  Read triggers onread, not onwrite
// ===========================================================================

#[test]
fn read_triggers_onread_not_onwrite() {
    let mut e = engine();

    // Create items
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":100}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/WriteDst","v":0}}"#,
    );

    // onwrite copies to /Dev/WriteDst (should NOT fire on read)
    set_onwrite(
        &mut e,
        "/Dev/Item",
        "odf.writeItem(event.value, '/Dev/WriteDst');",
    );
    // onread transforms value (doubles it)
    set_onread(&mut e, "/Dev/Item", "event.value * 2");

    // Read /Dev/Item — onread should transform, onwrite should NOT fire
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(200.0),
        "onread should transform the read value (100 * 2 = 200)"
    );

    // /Dev/WriteDst should still be 0 (onwrite should NOT have fired on read)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/WriteDst","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(0.0),
        "onwrite should NOT fire during a read operation"
    );
}

// ===========================================================================
// 3.  Both scripts on same item work independently
// ===========================================================================

#[test]
fn both_scripts_operate_independently() {
    let mut e = engine();

    // Create items
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Log","v":0}}"#,
    );

    // onwrite: log the raw value to /Dev/Log
    set_onwrite(
        &mut e,
        "/Dev/Sensor",
        "odf.writeItem(event.value, '/Dev/Log');",
    );
    // onread: convert C→F for display
    set_onread(&mut e, "/Dev/Sensor", "event.value * 9 / 5 + 32");

    // Write 25°C — onwrite logs raw value, onread does NOT fire
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":25}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // /Dev/Log should have raw 25 (onwrite fired)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Log","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(25.0), "onwrite should log the raw value");

    // Read /Dev/Sensor — onread transforms to Fahrenheit, onwrite does NOT fire
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Sensor","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(77.0),
        "onread should transform 25°C to 77°F"
    );

    // /Dev/Log should still be 25 (onwrite should NOT have fired again on read)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Log","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(25.0),
        "onwrite should not have fired again during read"
    );
}

// ===========================================================================
// 4.  Broken onwrite does not affect onread
// ===========================================================================

#[test]
fn broken_onwrite_does_not_affect_onread() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":50}}"#,
    );

    // onwrite is broken
    set_onwrite(&mut e, "/Dev/Item", "this is not valid javascript!!!");
    // onread is valid — doubles the value
    set_onread(&mut e, "/Dev/Item", "event.value * 2");

    // Write should succeed despite broken onwrite
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":30}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // Read should still use onread (unaffected by broken onwrite)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(60.0),
        "onread should work despite broken onwrite (30 * 2 = 60)"
    );
}

// ===========================================================================
// 5.  Broken onread does not affect onwrite
// ===========================================================================

#[test]
fn broken_onread_does_not_affect_onwrite() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Dst","v":0}}"#,
    );

    // onwrite is valid — copies to /Dev/Dst
    set_onwrite(
        &mut e,
        "/Dev/Item",
        "odf.writeItem(event.value, '/Dev/Dst');",
    );
    // onread is broken
    set_onread(&mut e, "/Dev/Item", "this is not valid javascript!!!");

    // Write should trigger onwrite successfully
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":88}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // /Dev/Dst should have 88 (onwrite worked despite broken onread)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Dst","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(88.0),
        "onwrite should work despite broken onread"
    );

    // Read /Dev/Item — broken onread should not crash, returns raw value
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(88.0),
        "broken onread should fall back to raw value"
    );
}

// ===========================================================================
// 6.  Stored value is raw (onread does not mutate storage)
// ===========================================================================

#[test]
fn onread_does_not_mutate_stored_value() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":10}}"#,
    );

    // onread triples the value for display
    set_onread(&mut e, "/Dev/Item", "event.value * 3");

    // First read — should get transformed value (30)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(30.0), "first read: 10 * 3 = 30");

    // Second read — should still get 30 (not 90), proving storage is unchanged
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(30.0),
        "second read should still be 30 (not 90) — onread must not mutate storage"
    );
}

// ===========================================================================
// 7.  onwrite side-effects persist while onread is display-only
// ===========================================================================

#[test]
fn onwrite_persists_side_effects_onread_is_display_only() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Counter","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Accumulator","v":0}}"#,
    );

    // onwrite: add written value to accumulator
    set_onwrite(
        &mut e,
        "/Dev/Counter",
        "let acc = odf.readItem('/Dev/Accumulator/value'); odf.writeItem(acc + event.value, '/Dev/Accumulator');",
    );
    // onread: return value + 1000 (display offset, no side effects)
    set_onread(&mut e, "/Dev/Counter", "event.value + 1000");

    // Write 5 — accumulator becomes 0+5=5
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Counter","v":5}}"#,
    );
    // Write 3 — accumulator becomes 5+3=8
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Counter","v":3}}"#,
    );

    // Read accumulator — should be 8 (onwrite side-effects persisted)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Accumulator","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(8.0), "accumulator should be 5+3=8");

    // Read /Dev/Counter — onread adds display offset (3 + 1000 = 1003)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Counter","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(1003.0),
        "onread should add display offset (3 + 1000 = 1003)"
    );

    // Accumulator should still be 8 (reads didn't trigger onwrite)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Accumulator","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(8.0),
        "accumulator should still be 8 — reads must not trigger onwrite"
    );
}

// ===========================================================================
// 8.  Tree write with both onwrite and onread metadata
// ===========================================================================

#[test]
fn tree_write_with_both_scripts() {
    let mut e = engine();

    // Create item via tree write with both onwrite and onread in metadata
    let tree_json = r#"{
        "omi":"1.0","ttl":0,
        "write":{
            "path":"/",
            "objects":{
                "Dual":{
                    "id":"Dual",
                    "items":{
                        "Sensor":{
                            "values":[],
                            "meta":{
                                "writable":true,
                                "onwrite":"odf.writeItem(event.value, '/Dual/Mirror');",
                                "onread":"event.value * 10"
                            }
                        }
                    }
                }
            }
        }
    }"#;
    let resp = parse_and_process(&mut e, tree_json);
    assert_eq!(response_status(&resp), 200);

    // Create /Dual/Mirror for the onwrite cascade
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dual/Mirror","v":0}}"#,
    );

    // Write 7 to /Dual/Sensor — onwrite cascades to /Dual/Mirror
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dual/Sensor","v":7}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // /Dual/Mirror should have 7 (onwrite fired)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dual/Mirror","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(7.0), "onwrite should cascade value to mirror");

    // Read /Dual/Sensor — onread transforms (7 * 10 = 70)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dual/Sensor","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v, OmiValue::Number(70.0),
        "onread should transform read value (7 * 10 = 70)"
    );
}
