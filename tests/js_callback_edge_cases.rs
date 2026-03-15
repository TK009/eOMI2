#![cfg(any(feature = "json", feature = "lite-json"))]
//! Edge-case and error-handling tests for javascript:// callback subscriptions.
//!
//! Covers SC-004/SC-007 from spec-008:
//! (1)  Missing script path → warning logged, delivery dropped, sub active
//! (2)  Empty script value → same
//! (3)  Non-MetaData target → same
//! (4)  Script error/timeout → warning logged, sub active, next tick fires
//! (5)  Script chaining via writeItem triggers onwrite and event subs
//! (6)  Depth limit prevents infinite recursion via callbacks
//! (7)  Multi-item delivery passes all values to callback
//! (8)  Self-monitoring pattern works
//! (9)  Malformed javascript:// URL handling
//! (10) Null values for empty ring buffer items

#![cfg(feature = "scripting")]

mod common;

use common::*;
use reconfigurable_device::odf::{OmiValue, Value};
use reconfigurable_device::omi::Engine;

const BASE_TIME: f64 = 1_000_000.0;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn engine() -> Engine {
    let e = Engine::new();
    assert!(
        e.has_script_engine(),
        "ScriptEngine failed to initialise — tests would be meaningless"
    );
    e
}

/// Store a script in a MetaData InfoItem so it can be resolved via javascript:// URL.
fn store_callback_script(e: &mut Engine, name: &str, script: &str) {
    let escaped = script.replace('\\', "\\\\").replace('"', "\\\"");
    let tree_json = [
        r#"{"omi":"1.0","ttl":0,"write":{"path":"/","objects":{"Callbacks":{"id":"Callbacks","objects":{"MetaData":{"id":"MetaData","items":{""#,
        name,
        r#"":{"values":[{"v":""#,
        &escaped,
        r#""}]}}}}}}}}"#,
    ].concat();
    let resp = parse_and_process(e, &tree_json);
    assert_eq!(response_status(&resp), 200, "failed to store callback script '{}'", name);
}

fn create_item(e: &mut Engine, path: &str, initial: f64) {
    let json = format!(
        r#"{{"omi":"1.0","ttl":10,"write":{{"path":"{}","v":{}}}}}"#,
        path, initial
    );
    let resp = parse_and_process(e, &json);
    let status = response_status(&resp);
    assert!(status == 200 || status == 201, "failed to create item {}", path);
}

fn read_newest(e: &mut Engine, path: &str) -> OmiValue {
    let json = format!(
        r#"{{"omi":"1.0","ttl":0,"read":{{"path":"{}","newest":1}}}}"#,
        path
    );
    let resp = parse_and_process(e, &json);
    assert_eq!(response_status(&resp), 200, "read {} failed", path);
    let values = extract_values(&resp);
    assert!(!values.is_empty(), "no values at {}", path);
    values[0].v.clone()
}

fn set_onwrite(e: &mut Engine, path: &str, script: &str) {
    use reconfigurable_device::odf::PathTargetMut;
    use std::collections::BTreeMap;
    if let Ok(PathTargetMut::InfoItem(item)) = e.tree.resolve_mut(path) {
        let meta = item.meta.get_or_insert_with(BTreeMap::new);
        meta.insert("onwrite".into(), OmiValue::Str(script.into()));
    } else {
        panic!("set_onwrite: path {} is not a writable InfoItem", path);
    }
}

// ===========================================================================
// (1) Missing script path → delivery dropped, subscription stays active
// ===========================================================================

#[test]
fn missing_script_path_drops_delivery_sub_stays_active() {
    let mut e = engine();
    create_item(&mut e, "/Sensor/Edge1", 0.0);

    // Create event subscription pointing to a non-existent script
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/Edge1","interval":-1,"callback":"javascript:///NonExistent/MetaData/missing"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Write triggers delivery
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Edge1","v":42}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    // Execute callback — script resolve fails, should return empty (delivery dropped)
    let cascaded = e.run_callback_script(
        "javascript:///NonExistent/MetaData/missing",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );
    assert!(cascaded.is_empty(), "missing script should produce no cascaded deliveries");

    // Subscription should still be alive — second write still fires
    let (_resp, deliveries2) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Edge1","v":99}}"#,
        BASE_TIME + 2.0,
        None,
    );
    assert_eq!(deliveries2.len(), 1, "subscription should still fire after missing script");
    assert_eq!(deliveries2[0].rid, rid);
}

// ===========================================================================
// (2) Empty script value → delivery dropped, subscription stays active
// ===========================================================================

#[test]
fn empty_script_value_drops_delivery_sub_stays_active() {
    let mut e = engine();

    // Store an empty script
    let tree_json = r#"{"omi":"1.0","ttl":0,"write":{"path":"/","objects":{"Callbacks":{"id":"Callbacks","objects":{"MetaData":{"id":"MetaData","items":{"empty_script":{"values":[{"v":""}]}}}}}}}}"#;
    let resp = parse_and_process(&mut e, tree_json);
    assert_eq!(response_status(&resp), 200);

    create_item(&mut e, "/Sensor/Edge2", 0.0);

    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/Edge2","interval":-1,"callback":"javascript:///Callbacks/MetaData/empty_script"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Edge2","v":1}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    // Empty script resolve fails → delivery dropped
    let cascaded = e.run_callback_script(
        "javascript:///Callbacks/MetaData/empty_script",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );
    assert!(cascaded.is_empty(), "empty script should produce no cascaded deliveries");

    // Sub still active
    let (_resp, deliveries2) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Edge2","v":2}}"#,
        BASE_TIME + 2.0,
        None,
    );
    assert_eq!(deliveries2.len(), 1, "subscription should survive empty script");
    assert_eq!(deliveries2[0].rid, rid);
}

// ===========================================================================
// (3) Non-MetaData target → delivery dropped, subscription stays active
// ===========================================================================

#[test]
fn non_metadata_target_drops_delivery_sub_stays_active() {
    let mut e = engine();

    // Create a regular InfoItem (not under MetaData)
    create_item(&mut e, "/Regular/Item", 0.0);
    create_item(&mut e, "/Sensor/Edge3", 0.0);

    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/Edge3","interval":-1,"callback":"javascript:///Regular/Item"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Edge3","v":10}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    // Non-MetaData target → script resolve fails
    let cascaded = e.run_callback_script(
        "javascript:///Regular/Item",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );
    assert!(cascaded.is_empty(), "non-MetaData target should produce no cascaded deliveries");

    // Sub still active
    let (_resp, deliveries2) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Edge3","v":20}}"#,
        BASE_TIME + 2.0,
        None,
    );
    assert_eq!(deliveries2.len(), 1, "subscription should survive non-MetaData target");
    assert_eq!(deliveries2[0].rid, rid);
}

// ===========================================================================
// (4) Script error/timeout → warning logged, sub active, next tick fires
// ===========================================================================

#[test]
fn script_error_on_interval_sub_stays_active_next_tick_fires() {
    let mut e = engine();

    // Store a broken script and a working script
    store_callback_script(&mut e, "broken_tick", "this is not valid js!!!");
    create_item(&mut e, "/Sensor/Periodic", 10.0);

    // Create interval subscription with broken callback
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":120,"read":{"path":"/Sensor/Periodic","interval":5,"callback":"javascript:///Callbacks/MetaData/broken_tick"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // First tick — script errors but sub should stay alive
    let deliveries = e.tick(BASE_TIME + 5.0);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].rid, rid);

    let cascaded = e.run_callback_script(
        "javascript:///Callbacks/MetaData/broken_tick",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 5.0,
    );
    assert!(cascaded.is_empty(), "broken script produces no cascaded deliveries");

    // Second tick — subscription should still fire
    let deliveries2 = e.tick(BASE_TIME + 10.0);
    assert_eq!(deliveries2.len(), 1, "subscription should fire on next tick after script error");
    assert_eq!(deliveries2[0].rid, rid);
}

// ===========================================================================
// (5) Script chaining: writeItem triggers onwrite and event subs
// ===========================================================================

#[test]
fn callback_writeitem_triggers_onwrite_script() {
    let mut e = engine();

    // Setup: callback writes to /Chain/A, which has an onwrite that doubles to /Chain/B
    store_callback_script(
        &mut e,
        "chain_start",
        "odf.writeItem(event.values[0].value, '/Chain/A');",
    );
    create_item(&mut e, "/Chain/A", 0.0);
    create_item(&mut e, "/Chain/B", 0.0);
    set_onwrite(&mut e, "/Chain/A", "odf.writeItem(event.value * 2, '/Chain/B');");

    create_item(&mut e, "/Sensor/ChainSrc", 0.0);

    // Event subscription on source
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/ChainSrc","interval":-1,"callback":"javascript:///Callbacks/MetaData/chain_start"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Write to trigger the chain
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/ChainSrc","v":5}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    // Execute callback — writes 5 to /Chain/A → onwrite doubles to /Chain/B
    e.run_callback_script(
        "javascript:///Callbacks/MetaData/chain_start",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );

    assert_eq!(read_newest(&mut e, "/Chain/A"), OmiValue::Number(5.0));
    assert_eq!(read_newest(&mut e, "/Chain/B"), OmiValue::Number(10.0));
}

#[test]
fn callback_writeitem_triggers_event_subscription() {
    let mut e = engine();

    // Callback writes to /Chain/Mid
    store_callback_script(
        &mut e,
        "ev_chain",
        "odf.writeItem(event.values[0].value * 3, '/Chain/Mid');",
    );
    create_item(&mut e, "/Chain/Mid", 0.0);
    create_item(&mut e, "/Sensor/EvSrc", 0.0);

    // Event sub on source → javascript callback
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/EvSrc","interval":-1,"callback":"javascript:///Callbacks/MetaData/ev_chain"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Poll sub on /Chain/Mid to observe cascaded events
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Chain/Mid","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let poll_rid = response_rid(&resp).to_string();

    // Write triggers callback → callback writes to /Chain/Mid → triggers event sub
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/EvSrc","v":7}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    e.run_callback_script(
        "javascript:///Callbacks/MetaData/ev_chain",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );

    // Verify /Chain/Mid was written
    assert_eq!(read_newest(&mut e, "/Chain/Mid"), OmiValue::Number(21.0));

    // Poll the cascaded event sub
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, poll_rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 2.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = extract_values(&resp);
    assert_eq!(polled.len(), 1, "cascaded write should buffer in poll subscription");
    assert_eq!(polled[0].v, OmiValue::Number(21.0));
}

// ===========================================================================
// (6) Depth limit prevents infinite recursion via callbacks
// ===========================================================================

#[test]
fn callback_depth_limit_prevents_infinite_recursion() {
    let mut e = engine();

    // Callback writes to /Loop/Item, which has an onwrite that writes back to itself
    store_callback_script(
        &mut e,
        "loop_start",
        "odf.writeItem(1, '/Loop/Item');",
    );
    create_item(&mut e, "/Loop/Item", 0.0);
    // onwrite increments by 1 and writes back → would loop infinitely without depth limit
    set_onwrite(
        &mut e,
        "/Loop/Item",
        "odf.writeItem(event.value + 1, '/Loop/Item');",
    );

    create_item(&mut e, "/Sensor/LoopTrig", 0.0);

    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/LoopTrig","interval":-1,"callback":"javascript:///Callbacks/MetaData/loop_start"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/LoopTrig","v":1}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    // Execute callback — depth starts at 0 for callback, writes go at depth 1,
    // onwrite cascades increment depth further. MAX_SCRIPT_DEPTH=4 should stop it.
    let cascaded = e.run_callback_script(
        "javascript:///Callbacks/MetaData/loop_start",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );
    // Cascaded deliveries may exist from event subs, but the key test is that
    // we didn't stack-overflow or hang. The value should be capped.
    let _ = cascaded;

    let val = read_newest(&mut e, "/Loop/Item");
    // Callback writes 1 at depth 1 → onwrite writes 2 at depth 2 → 3 at depth 3
    // → tries 4 but depth 4 >= MAX(4), blocked. Final value: 3.
    assert_eq!(val, OmiValue::Number(3.0), "depth limit should cap recursion");
}

// ===========================================================================
// (7) Multi-item delivery passes all values to callback
// ===========================================================================

#[test]
fn callback_receives_multiple_values() {
    let mut e = engine();

    // Script sums all values and writes the total
    store_callback_script(
        &mut e,
        "sum_all",
        "let total = 0; let i = 0; while (i < event.values.length) { total = total + event.values[i].value; i = i + 1; } odf.writeItem(total, '/Target/Sum');",
    );
    create_item(&mut e, "/Target/Sum", 0.0);

    // Directly call run_callback_script with multiple values
    let values = vec![
        Value::new(OmiValue::Number(10.0), Some(BASE_TIME)),
        Value::new(OmiValue::Number(20.0), Some(BASE_TIME + 1.0)),
        Value::new(OmiValue::Number(30.0), Some(BASE_TIME + 2.0)),
    ];

    e.run_callback_script(
        "javascript:///Callbacks/MetaData/sum_all",
        "/Multi/Source",
        &values,
        BASE_TIME + 3.0,
    );

    let val = read_newest(&mut e, "/Target/Sum");
    assert_eq!(val, OmiValue::Number(60.0), "callback should receive and sum all values");
}

#[test]
fn callback_receives_values_with_correct_paths() {
    let mut e = engine();

    // Script writes the path of the first value entry to verify it's correct
    store_callback_script(
        &mut e,
        "check_path",
        "odf.writeItem(event.values[0].path, '/Target/Path');",
    );
    create_item(&mut e, "/Target/Path", 0.0);

    let values = vec![
        Value::new(OmiValue::Number(42.0), Some(BASE_TIME)),
    ];

    e.run_callback_script(
        "javascript:///Callbacks/MetaData/check_path",
        "/Source/Sensor",
        &values,
        BASE_TIME,
    );

    let val = read_newest(&mut e, "/Target/Path");
    assert_eq!(val, OmiValue::Str("/Source/Sensor".into()), "event.values[0].path should match delivery path");
}

// ===========================================================================
// (8) Self-monitoring pattern works
// ===========================================================================

#[test]
fn self_monitoring_pattern_reads_own_value() {
    let mut e = engine();

    // A script that reads the current value and writes an average with the new value
    // pattern: self-monitoring by reading own path via odf.readItem
    store_callback_script(
        &mut e,
        "self_mon",
        "let cur = odf.readItem('/Monitor/Sensor/value'); let avg = (cur + event.values[0].value) / 2; odf.writeItem(avg, '/Monitor/Avg');",
    );
    create_item(&mut e, "/Monitor/Sensor", 100.0);
    create_item(&mut e, "/Monitor/Avg", 0.0);

    // Event subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Monitor/Sensor","interval":-1,"callback":"javascript:///Callbacks/MetaData/self_mon"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Write new value — callback reads current (100) and averages with new (200)
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Monitor/Sensor","v":200}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    e.run_callback_script(
        "javascript:///Callbacks/MetaData/self_mon",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );

    // odf.readItem reads the tree (which now has 200 written), so avg = (200 + 200) / 2 = 200
    // Note: event delivery carries the raw written value (200) which is also the new stored value
    let val = read_newest(&mut e, "/Monitor/Avg");
    assert_eq!(val, OmiValue::Number(200.0));
}

#[test]
fn self_monitoring_readitem_sees_latest_written_value() {
    let mut e = engine();

    // Script reads own path to verify readItem sees latest written value
    store_callback_script(
        &mut e,
        "self_read",
        "let v = odf.readItem('/Monitor/Self/value'); odf.writeItem(v * 2, '/Monitor/Result');",
    );
    create_item(&mut e, "/Monitor/Self", 50.0);
    create_item(&mut e, "/Monitor/Result", 0.0);

    // Create interval subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Monitor/Self","interval":5,"callback":"javascript:///Callbacks/MetaData/self_read"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Tick fires — readItem returns stored value (50), script writes 50*2=100
    let deliveries = e.tick(BASE_TIME + 5.0);
    assert_eq!(deliveries.len(), 1);

    e.run_callback_script(
        "javascript:///Callbacks/MetaData/self_read",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 5.0,
    );

    let val = read_newest(&mut e, "/Monitor/Result");
    assert_eq!(val, OmiValue::Number(100.0));
}

// ===========================================================================
// (9) Malformed javascript:// URL handling
// ===========================================================================

#[test]
fn malformed_url_empty_path_drops_delivery() {
    let mut e = engine();

    // javascript:// with no path at all
    let cascaded = e.run_callback_script(
        "javascript://",
        "/Sensor/X",
        &[Value::new(OmiValue::Number(1.0), Some(BASE_TIME))],
        BASE_TIME,
    );
    assert!(cascaded.is_empty(), "empty javascript:// URL should produce no deliveries");
}

#[test]
fn malformed_url_no_leading_slash_drops_delivery() {
    let mut e = engine();

    // javascript:// without leading slash on path
    let cascaded = e.run_callback_script(
        "javascript://NoSlash/MetaData/script",
        "/Sensor/X",
        &[Value::new(OmiValue::Number(1.0), Some(BASE_TIME))],
        BASE_TIME,
    );
    assert!(cascaded.is_empty(), "javascript:// without leading slash should fail resolve");
}

#[test]
fn completely_wrong_url_scheme_drops_delivery() {
    let mut e = engine();

    // Not a javascript:// URL at all
    let cascaded = e.run_callback_script(
        "http://example.com/script",
        "/Sensor/X",
        &[Value::new(OmiValue::Number(1.0), Some(BASE_TIME))],
        BASE_TIME,
    );
    assert!(cascaded.is_empty(), "non-javascript:// URL should produce no deliveries");
}

#[test]
fn javascript_url_pointing_to_object_drops_delivery() {
    let mut e = engine();

    // Create an Object (not an InfoItem) at this path
    store_callback_script(&mut e, "dummy", "1;");
    // /Callbacks/MetaData is an Object, not an InfoItem
    let cascaded = e.run_callback_script(
        "javascript:///Callbacks/MetaData",
        "/Sensor/X",
        &[Value::new(OmiValue::Number(1.0), Some(BASE_TIME))],
        BASE_TIME,
    );
    assert!(cascaded.is_empty(), "URL pointing to Object should fail resolve");
}

// ===========================================================================
// (10) Null values for empty ring buffer items
// ===========================================================================

#[test]
fn callback_handles_null_value_in_delivery() {
    let mut e = engine();

    // Script should handle null gracefully and write a sentinel
    store_callback_script(
        &mut e,
        "null_handler",
        "let v = event.values[0].value; if (v === null) { odf.writeItem(-1, '/Target/NullResult'); } else { odf.writeItem(v, '/Target/NullResult'); }",
    );
    create_item(&mut e, "/Target/NullResult", 0.0);

    // Deliver a null value (simulating empty ring buffer)
    let values = vec![Value::new(OmiValue::Null, Some(BASE_TIME))];
    e.run_callback_script(
        "javascript:///Callbacks/MetaData/null_handler",
        "/Empty/Buffer",
        &values,
        BASE_TIME,
    );

    let val = read_newest(&mut e, "/Target/NullResult");
    assert_eq!(val, OmiValue::Number(-1.0), "callback should detect null and write sentinel");
}

#[test]
fn callback_handles_empty_values_array() {
    let mut e = engine();

    // Script guards against empty values array
    store_callback_script(
        &mut e,
        "empty_vals",
        "if (event.values.length === 0) { odf.writeItem(0, '/Target/EmptyResult'); } else { odf.writeItem(1, '/Target/EmptyResult'); }",
    );
    create_item(&mut e, "/Target/EmptyResult", -1.0);

    // Empty values array
    let values: Vec<Value> = vec![];
    e.run_callback_script(
        "javascript:///Callbacks/MetaData/empty_vals",
        "/Empty/Source",
        &values,
        BASE_TIME,
    );

    let val = read_newest(&mut e, "/Target/EmptyResult");
    assert_eq!(val, OmiValue::Number(0.0), "callback should handle empty values array");
}

#[test]
fn callback_handles_mixed_null_and_real_values() {
    let mut e = engine();

    // Script counts non-null values
    store_callback_script(
        &mut e,
        "count_real",
        "let count = 0; let i = 0; while (i < event.values.length) { if (event.values[i].value !== null) { count = count + 1; } i = i + 1; } odf.writeItem(count, '/Target/RealCount');",
    );
    create_item(&mut e, "/Target/RealCount", 0.0);

    let values = vec![
        Value::new(OmiValue::Number(10.0), Some(BASE_TIME)),
        Value::new(OmiValue::Null, Some(BASE_TIME + 1.0)),
        Value::new(OmiValue::Number(30.0), Some(BASE_TIME + 2.0)),
        Value::new(OmiValue::Null, None),
    ];

    e.run_callback_script(
        "javascript:///Callbacks/MetaData/count_real",
        "/Mixed/Source",
        &values,
        BASE_TIME + 3.0,
    );

    let val = read_newest(&mut e, "/Target/RealCount");
    assert_eq!(val, OmiValue::Number(2.0), "should count exactly 2 non-null values");
}

// ===========================================================================
// Additional edge case: interval subscription with missing script still ticks
// ===========================================================================

#[test]
fn interval_sub_missing_script_still_produces_deliveries() {
    let mut e = engine();
    create_item(&mut e, "/Sensor/MissTick", 77.0);

    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":120,"read":{"path":"/Sensor/MissTick","interval":5,"callback":"javascript:///Missing/MetaData/nope"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Tick 1 — delivery produced even though script will fail
    let deliveries = e.tick(BASE_TIME + 5.0);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].rid, rid);

    // Execute (will fail silently)
    let cascaded = e.run_callback_script(
        "javascript:///Missing/MetaData/nope",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 5.0,
    );
    assert!(cascaded.is_empty());

    // Tick 2 — should still fire
    let deliveries2 = e.tick(BASE_TIME + 10.0);
    assert_eq!(deliveries2.len(), 1, "interval sub should keep ticking despite missing script");
    assert_eq!(deliveries2[0].rid, rid);
}
