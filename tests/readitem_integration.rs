#![cfg(feature = "json")]
//! Integration tests for `odf.readItem()` — spec 003-odf-readitem.
//!
//! Covers all acceptance scenarios: /value suffix, element structure, null cases,
//! readability, resource limits, precedence, and read-after-write consistency.

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

fn set_meta(e: &mut Engine, path: &str, key: &str, val: OmiValue) {
    if let Ok(PathTargetMut::InfoItem(item)) = e.tree.resolve_mut(path) {
        let meta = item.meta.get_or_insert_with(BTreeMap::new);
        meta.insert(key.into(), val);
    } else {
        panic!("set_meta: path {path} is not a writable InfoItem");
    }
}

// ===========================================================================
// 1.  Read with /value suffix → raw primitive
// ===========================================================================

#[test]
fn read_value_suffix_returns_raw_number() {
    let mut e = engine();

    // Create /Dev/Target with a numeric value
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Target","v":22.5}}"#,
    );

    // Script reads /Dev/Target/value and writes to /Dev/Out
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "odf.writeItem(odf.readItem('/Dev/Target/value'), '/Dev/Result');",
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":0}}"#,
    );

    // Trigger
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // /Dev/Result should have raw 22.5
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(22.5));
}

#[test]
fn read_value_suffix_returns_raw_string() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Status","v":"OK"}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":""}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":""}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "odf.writeItem(odf.readItem('/Dev/Status/value'), '/Dev/Result');",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":"go"}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Str("OK".into()));
}

#[test]
fn read_value_suffix_returns_raw_boolean() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Flag","v":true}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":false}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "odf.writeItem(odf.readItem('/Dev/Flag/value'), '/Dev/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Bool(true));
}

// ===========================================================================
// 2.  Read without suffix → element structure
// ===========================================================================

#[test]
fn read_without_suffix_returns_element_structure() {
    let mut e = engine();

    // Create /Dev/Target with a value
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Target","v":22.5}}"#,
    );

    // Script reads /Dev/Target (no /value) — gets an object with .values array
    // We check by reading the .values[0].v field from the returned structure
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":0}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let elem = odf.readItem('/Dev/Target'); odf.writeItem(elem.values[0].v, '/Dev/Result');",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(22.5));
}

#[test]
fn element_structure_has_values_array() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Target","v":10}}"#,
    );
    // Write a second value
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Target","v":20}}"#,
    );

    // Script reads element and writes length of values array
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Len","v":0}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let elem = odf.readItem('/Dev/Target'); odf.writeItem(elem.values.length, '/Dev/Len');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Len","newest":1}}"#,
    );
    let values = extract_values(&resp);
    // Should have 2 values in the history
    assert_eq!(values[0].v, OmiValue::Number(2.0));
}

// ===========================================================================
// 3.  Read nonexistent path → null
// ===========================================================================

#[test]
fn read_nonexistent_path_returns_null() {
    let mut e = engine();

    // Script reads a path that doesn't exist, writes 1 if null, 0 otherwise
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":-1}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let r = odf.readItem('/nonexistent'); odf.writeItem(r === null ? 1 : 0, '/Dev/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(1.0), "readItem of nonexistent path should return null");
}

#[test]
fn read_nonexistent_path_with_value_suffix_returns_null() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":-1}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let r = odf.readItem('/nonexistent/value'); odf.writeItem(r === null ? 1 : 0, '/Dev/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(1.0));
}

// ===========================================================================
// 4.  Read non-readable item → null
// ===========================================================================

#[test]
fn read_non_readable_item_returns_null() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Secret","v":42}}"#,
    );
    set_meta(&mut e, "/Dev/Secret", "readable", OmiValue::Bool(false));

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":-1}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let r = odf.readItem('/Dev/Secret'); odf.writeItem(r === null ? 1 : 0, '/Dev/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(1.0), "non-readable item should return null");
}

#[test]
fn read_non_readable_item_with_value_suffix_returns_null() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Secret","v":42}}"#,
    );
    set_meta(&mut e, "/Dev/Secret", "readable", OmiValue::Bool(false));

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":-1}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let r = odf.readItem('/Dev/Secret/value'); odf.writeItem(r === null ? 1 : 0, '/Dev/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(1.0));
}

// ===========================================================================
// 5.  Read empty InfoItem (no values) → null
// ===========================================================================

#[test]
fn read_empty_infoitem_returns_null() {
    let mut e = engine();

    // Create an InfoItem with an empty values array via tree write
    let tree_json = r#"{
        "omi":"1.0","ttl":0,
        "write":{
            "path":"/",
            "objects":{
                "Dev":{
                    "id":"Dev",
                    "items":{
                        "Empty":{"values":[],"meta":{"writable":true}}
                    }
                }
            }
        }
    }"#;
    parse_and_process(&mut e, tree_json);

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":-1}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let r = odf.readItem('/Dev/Empty'); odf.writeItem(r === null ? 1 : 0, '/Dev/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(1.0), "empty InfoItem should return null");
}

// ===========================================================================
// 6.  Read Object path (not InfoItem) → null
// ===========================================================================

#[test]
fn read_object_path_returns_null() {
    let mut e = engine();

    // Create an item under /Dev so /Dev is an Object node
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Child","v":1}}"#,
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/Result","v":-1}}"#,
    );
    set_onwrite(
        &mut e,
        "/Test/Out",
        "let r = odf.readItem('/Dev'); odf.writeItem(r === null ? 1 : 0, '/Test/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Test/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(1.0), "Object path should return null");
}

// ===========================================================================
// 7.  InfoItem literally named "value" → precedence test
// ===========================================================================

#[test]
fn infoitem_named_value_takes_precedence() {
    let mut e = engine();

    // Create /Dev/value as an InfoItem with value 99
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/value","v":99}}"#,
    );

    // odf.readItem("/Dev/value") should return the element (not raw value of /Dev),
    // because "value" is a real InfoItem name.
    // We verify by checking it returns an object with a .values array.
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/Result","v":-1}}"#,
    );
    set_onwrite(
        &mut e,
        "/Test/Out",
        "let r = odf.readItem('/Dev/value'); odf.writeItem(r.values[0].v, '/Test/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Test/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(99.0), "InfoItem named 'value' should be returned as element");
}

#[test]
fn infoitem_named_value_raw_via_double_value_suffix() {
    let mut e = engine();

    // /Dev/value is an InfoItem with value 99
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/value","v":99}}"#,
    );

    // odf.readItem("/Dev/value/value") should return raw 99
    // (first "value" = InfoItem name, second "value" = raw accessor)
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/Result","v":-1}}"#,
    );
    set_onwrite(
        &mut e,
        "/Test/Out",
        "odf.writeItem(odf.readItem('/Dev/value/value'), '/Test/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Test/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(99.0), "/Dev/value/value should return raw 99");
}

// ===========================================================================
// 8.  Read-after-write consistency within same script
// ===========================================================================

#[test]
fn read_after_write_consistency() {
    let mut e = engine();

    // Create /Dev/Shared and /Dev/Out
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Shared","v":10}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Trigger","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":-1}}"#,
    );

    // Script: write 77 to /Dev/Shared, then read it back and write to /Dev/Result.
    // Read-after-write consistency (FR-007): the read should see the pending
    // write from the same script cycle, returning 77 (not the old value 10).
    set_onwrite(
        &mut e,
        "/Dev/Trigger",
        "odf.writeItem(77, '/Dev/Shared'); let r = odf.readItem('/Dev/Shared/value'); odf.writeItem(r, '/Dev/Result');",
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
    assert_eq!(values[0].v, OmiValue::Number(77.0), "read-after-write in same script sees the pending write");
}

#[test]
fn read_after_write_via_cascading_sees_updated_value() {
    let mut e = engine();

    // Create items
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Shared","v":10}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Trigger","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Reader","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":-1}}"#,
    );

    // Trigger writes 77 to Shared, then cascades to Reader
    set_onwrite(
        &mut e,
        "/Dev/Trigger",
        "odf.writeItem(77, '/Dev/Shared'); odf.writeItem(1, '/Dev/Reader');",
    );
    // Reader reads Shared/value — should see 77 because the deferred write from
    // Trigger has been applied before Reader's script runs
    set_onwrite(
        &mut e,
        "/Dev/Reader",
        "odf.writeItem(odf.readItem('/Dev/Shared/value'), '/Dev/Result');",
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
        values[0].v, OmiValue::Number(77.0),
        "cascaded script should see the updated value from prior deferred write"
    );
}

// ===========================================================================
// 9.  Script resource limits with reads in a loop
// ===========================================================================

#[test]
fn reads_in_loop_hit_op_limit() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Data","v":1}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Loop","v":0}}"#,
    );

    // Infinite loop calling readItem — should be terminated by op limit
    set_onwrite(
        &mut e,
        "/Dev/Loop",
        "while(true){ odf.readItem('/Dev/Data/value'); }",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Loop","v":1}}"#,
    );
    // Write should succeed despite script hitting op limit
    assert_eq!(response_status(&resp), 200);
    let desc = response_desc(&resp);
    assert!(
        desc.is_some(),
        "should have warning desc for op-limit script with reads"
    );
    assert!(
        desc.unwrap().contains("operation limit"),
        "desc should mention operation limit, got: {:?}",
        desc,
    );
}

#[test]
fn bounded_reads_succeed_normally() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Data","v":5}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sum","v":0}}"#,
    );

    // Read 10 times in a loop (well within limits), sum the results
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let s = 0; for (let i = 0; i < 10; i++) { s = s + odf.readItem('/Dev/Data/value'); } odf.writeItem(s, '/Dev/Sum');",
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let desc = response_desc(&resp);
    assert!(desc.is_none(), "bounded reads should not produce a warning");

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Sum","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(50.0), "10 reads of value 5 should sum to 50");
}

// ===========================================================================
// 10. Thermostat scenario: read target, compare with sensor, write output
// ===========================================================================

#[test]
fn thermostat_scenario() {
    let mut e = engine();

    // Set target temperature
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/HVAC/Target","v":22.0}}"#,
    );
    // Create sensor and heater output
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/HVAC/Sensor","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/HVAC/Heater","v":false}}"#,
    );

    // Thermostat script: if sensor < target, turn on heater
    set_onwrite(
        &mut e,
        "/HVAC/Sensor",
        "let target = odf.readItem('/HVAC/Target/value'); odf.writeItem(event.value < target, '/HVAC/Heater');",
    );

    // Sensor reads 18°C (below target 22°C) → heater ON
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/HVAC/Sensor","v":18}}"#,
    );
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/HVAC/Heater","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Bool(true), "heater should be ON when sensor < target");

    // Sensor reads 25°C (above target 22°C) → heater OFF
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/HVAC/Sensor","v":25}}"#,
    );
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/HVAC/Heater","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Bool(false), "heater should be OFF when sensor > target");
}

// ===========================================================================
// 11. No args → null
// ===========================================================================

#[test]
fn read_no_args_returns_null() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":-1}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let r = odf.readItem(); odf.writeItem(r === null ? 1 : 0, '/Dev/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(1.0), "readItem() with no args should return null");
}

// ===========================================================================
// 12. Empty string path → null
// ===========================================================================

#[test]
fn read_empty_string_returns_null() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":-1}}"#,
    );
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let r = odf.readItem(''); odf.writeItem(r === null ? 1 : 0, '/Dev/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(1.0), "readItem('') should return null");
}

// ===========================================================================
// 13. Invalid path → null
// ===========================================================================

#[test]
fn read_invalid_path_returns_null() {
    let mut e = engine();

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":-1}}"#,
    );
    // Root "/value" should return null (root has no value)
    set_onwrite(
        &mut e,
        "/Dev/Out",
        "let r = odf.readItem('/value'); odf.writeItem(r === null ? 1 : 0, '/Dev/Result');",
    );

    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Out","v":1}}"#,
    );

    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Result","newest":1}}"#,
    );
    let values = extract_values(&resp);
    assert_eq!(values[0].v, OmiValue::Number(1.0), "readItem('/value') should return null");
}
