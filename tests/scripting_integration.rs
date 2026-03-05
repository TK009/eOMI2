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
    let values = extract_single_result(&resp)["values"].as_array().unwrap();
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
    let values = extract_single_result(&resp)["values"].as_array().unwrap();
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
    let values = extract_single_result(&resp)["values"].as_array().unwrap();
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
        let values = extract_single_result(&resp)["values"].as_array().unwrap();
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
        let values = extract_single_result(&resp)["values"].as_array().unwrap();
        assert_eq!(
            values[0]["v"], -1.0,
            "/Chain/L{i} should NOT have been updated (beyond depth limit)"
        );
    }
}

// ===========================================================================
// 5.  Tree write with items carrying onwrite metadata (e2e payload shape)
// ===========================================================================

#[test]
fn tree_write_with_onwrite_meta() {
    let mut e = engine();

    // Exact JSON shape the e2e test sends
    let tree_json = r#"{
        "omi":"1.0","ttl":0,
        "write":{
            "path":"/",
            "objects":{
                "Script":{
                    "id":"Script",
                    "items":{
                        "Src":{
                            "values":[],
                            "meta":{
                                "writable":true,
                                "onwrite":"odf.writeItem(event.value, '/Script/Dst');"
                            }
                        }
                    }
                }
            }
        }
    }"#;
    let resp = parse_and_process(&mut e, tree_json);
    assert_eq!(response_status(&resp), 200);

    // Write to Src → should trigger onwrite → cascade to /Script/Dst
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Script/Src","v":42}}"#,
    );
    assert_eq!(response_status(&resp), 200);

    // Read /Script/Dst — should have the cascaded value
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Script/Dst","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_single_result(&resp)["values"].as_array().unwrap();
    assert_eq!(values[0]["v"], 42.0);
}

// ===========================================================================
// 6.  Write with onwrite script hitting op limit returns warning in response
// ===========================================================================

#[test]
fn op_limit_script_returns_warning_in_response() {
    let mut e = engine();

    // Create /Dev/Item via write
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":0}}"#,
    );

    // Attach infinite-loop script that hits op limit
    set_onwrite(&mut e, "/Dev/Item", "while(true){}");

    // Write should succeed (value written) but response includes warning
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Item","v":42}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let desc = response_desc(&resp);
    assert!(
        desc.is_some(),
        "response should have a warning desc for op-limit script"
    );
    assert!(
        desc.unwrap().contains("operation limit"),
        "desc should mention operation limit, got: {:?}",
        desc,
    );

    // Value should still be written despite script failure
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Item","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_single_result(&resp)["values"].as_array().unwrap();
    assert_eq!(values[0]["v"], 42.0);
}

// ===========================================================================
// 7.  Write with valid onwrite script gets normal 200 (no warning)
// ===========================================================================

#[test]
fn valid_script_returns_normal_200() {
    let mut e = engine();

    // Create items
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Src","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Dst","v":0}}"#,
    );

    // Attach a valid script
    set_onwrite(
        &mut e,
        "/Dev/Src",
        "odf.writeItem(event.value * 2, '/Dev/Dst');",
    );

    // Write should get a clean 200 with no warning desc
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Src","v":5}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let desc = response_desc(&resp);
    assert!(desc.is_none(), "valid script should not produce a warning desc, got: {:?}", desc);
}

// ===========================================================================
// 8.  Cascading timeout preserves earlier writes
// ===========================================================================

#[test]
fn cascading_timeout_preserves_earlier_writes() {
    let mut e = engine();

    // Create chain: /Chain/A → /Chain/B → /Chain/C
    // /Chain/B has an infinite loop script (hits op limit)
    for p in &["/Chain/A", "/Chain/B", "/Chain/C"] {
        parse_and_process(
            &mut e,
            &format!(r#"{{"omi":"1.0","ttl":10,"write":{{"path":"{}","v":-1}}}}"#, p),
        );
    }

    set_onwrite(
        &mut e,
        "/Chain/A",
        "odf.writeItem(event.value, '/Chain/B');",
    );
    set_onwrite(&mut e, "/Chain/B", "while(true){}");

    // Write to /Chain/A — cascades to B, B's script times out
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Chain/A","v":77}}"#,
    );
    // The top-level write still succeeds
    assert_eq!(response_status(&resp), 200);

    // /Chain/A should have the new value
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Chain/A","newest":1}}"#,
    );
    let values = extract_single_result(&resp)["values"].as_array().unwrap();
    assert_eq!(values[0]["v"], 77.0, "/Chain/A should be updated");

    // /Chain/B should also have the value (write happened before script ran)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Chain/B","newest":1}}"#,
    );
    let values = extract_single_result(&resp)["values"].as_array().unwrap();
    assert_eq!(values[0]["v"], 77.0, "/Chain/B should be updated (write before script)");

    // /Chain/C should NOT have been updated (B's script failed before writing to C)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Chain/C","newest":1}}"#,
    );
    let values = extract_single_result(&resp)["values"].as_array().unwrap();
    assert_eq!(values[0]["v"], -1.0, "/Chain/C should retain initial value");
}

// ===========================================================================
// 9.  Batch write: op-limit script produces per-item warning
// ===========================================================================

#[test]
fn batch_write_op_limit_produces_per_item_warning() {
    let mut e = engine();

    // Create two items
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Batch/Good","v":0}}"#,
    );
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Batch/Bad","v":0}}"#,
    );

    // Attach infinite loop to /Batch/Bad only
    set_onwrite(&mut e, "/Batch/Bad", "while(true){}");

    // Batch write both items
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{
            "items":[
                {"path":"/Batch/Good","v":1},
                {"path":"/Batch/Bad","v":2}
            ]
        }}"#,
    );
    assert_eq!(response_status(&resp), 200);

    let items = response_batch(&resp);
    assert_eq!(items.len(), 2);

    // /Batch/Good should have no warning
    assert_eq!(items[0].status, 200);
    assert!(items[0].desc.is_none(), "good item should have no warning");

    // /Batch/Bad should have a warning about op limit
    assert_eq!(items[1].status, 200);
    assert!(
        items[1].desc.as_ref().map_or(false, |d| d.contains("operation limit")),
        "bad item should have op-limit warning, got: {:?}",
        items[1].desc,
    );
}
