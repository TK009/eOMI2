#![cfg(feature = "lite-json")]
//! Tests for onread behavior with javascript:// subscription callbacks (spec-008).
//!
//! FR-010: Interval javascript:// subscriptions run onread scripts on subscribed
//!         values before passing to callback (via tick()).
//! FR-011: Event javascript:// subscriptions do NOT run onread — they deliver
//!         the written value as-is.
//!
//! These behaviors are inherited from the subscription infrastructure (tick runs
//! onread for all callback deliveries, notify_event bypasses onread entirely),
//! but the tests here verify it explicitly for javascript:// targets.

#![cfg(feature = "scripting")]

mod common;

use std::collections::BTreeMap;

use common::*;
use reconfigurable_device::odf::{OmiValue, PathTargetMut};
use reconfigurable_device::omi::Engine;
use reconfigurable_device::omi::subscriptions::DeliveryTarget;

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

fn set_onread(e: &mut Engine, path: &str, script: &str) {
    if let Ok(PathTargetMut::InfoItem(item)) = e.tree.resolve_mut(path) {
        let meta = item.meta.get_or_insert_with(BTreeMap::new);
        meta.insert("onread".into(), OmiValue::Str(script.into()));
    } else {
        panic!("set_onread: path {path} is not a writable InfoItem");
    }
}

const BASE_TIME: f64 = 1_000_000.0;

/// Write a javascript:// callback script into a MetaData InfoItem so it can
/// be referenced by a subscription callback URL.
///
/// Creates `/Dev/MetaData/<name>` containing the script text, and returns
/// the corresponding `javascript:///Dev/MetaData/<name>` URL.
fn install_callback_script(e: &mut Engine, name: &str, script: &str) -> String {
    // Use a separate /Scripts object so we don't overwrite /Dev items
    let tree_json = format!(
        r#"{{
            "omi":"1.0","ttl":0,
            "write":{{
                "path":"/",
                "objects":{{
                    "Scripts":{{
                        "id":"Scripts",
                        "objects":{{
                            "MetaData":{{
                                "id":"MetaData",
                                "items":{{
                                    "{name}":{{
                                        "values":[{{"v":"{script_escaped}"}}],
                                        "meta":{{"writable":true}}
                                    }}
                                }}
                            }}
                        }}
                    }}
                }}
            }}
        }}"#,
        name = name,
        script_escaped = script.replace('\\', "\\\\").replace('"', "\\\""),
    );
    let resp = parse_and_process(e, &tree_json);
    assert_eq!(response_status(&resp), 200, "failed to install callback script '{name}'");

    format!("javascript:///Scripts/MetaData/{name}")
}

// ===========================================================================
// FR-010: Interval javascript:// subscriptions run onread
// ===========================================================================

#[test]
fn interval_js_callback_runs_onread_on_subscribed_value() {
    let mut e = engine();

    // Create a sensor item with a stored raw value
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":100}}"#,
    );

    // Attach onread: doubles the value
    set_onread(&mut e, "/Dev/Sensor", "event.value * 2");

    // Install a callback script that writes received value to /Dev/Result
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":0}}"#,
    );
    let cb_url = install_callback_script(
        &mut e,
        "handler",
        "odf.writeItem(event.values[0].value, '/Dev/Result');",
    );

    // Create interval subscription with javascript:// callback
    let sub_json = format!(
        r#"{{"omi":"1.0","ttl":60,"read":{{"path":"/Dev/Sensor","interval":5,"callback":"{cb_url}"}}}}"#,
    );
    let resp = process_at(&mut e, &sub_json, BASE_TIME, None);
    assert_eq!(response_status(&resp), 200);

    // Tick at interval — onread should transform the value (100 * 2 = 200)
    let deliveries = e.tick(BASE_TIME + 5.0);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        deliveries[0].values[0].v,
        OmiValue::Number(200.0),
        "interval delivery to javascript:// callback should have onread-transformed value (100 * 2 = 200)"
    );
    assert!(
        matches!(&deliveries[0].target, DeliveryTarget::Callback(url) if url == &cb_url),
        "expected javascript:// Callback target"
    );
}

#[test]
fn interval_js_callback_receives_onread_transformed_value_in_script() {
    let mut e = engine();

    // Create sensor with raw value and onread transform
    e.tree
        .write_value("/Dev/Sensor", OmiValue::Number(50.0), Some(BASE_TIME))
        .unwrap();
    set_onread(&mut e, "/Dev/Sensor", "event.value * 10");

    // Install callback script: writes the received value to /Dev/Captured
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Captured","v":0}}"#,
    );
    let cb_url = install_callback_script(
        &mut e,
        "capture",
        "odf.writeItem(event.values[0].value, '/Dev/Captured');",
    );

    // Create interval subscription
    let sub_json = format!(
        r#"{{"omi":"1.0","ttl":60,"read":{{"path":"/Dev/Sensor","interval":5,"callback":"{cb_url}"}}}}"#,
    );
    let resp = process_at(&mut e, &sub_json, BASE_TIME, None);
    assert_eq!(response_status(&resp), 200);

    // Tick at interval — delivery has onread-transformed value
    let deliveries = e.tick(BASE_TIME + 5.0);
    assert_eq!(deliveries.len(), 1);

    // Now execute the callback script with the delivery
    let cascaded = e.run_callback_script(
        &cb_url,
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 5.0,
    );
    // Cascaded deliveries from the callback's odf.writeItem are not the focus —
    // we just need to verify the value written to /Dev/Captured
    let _ = cascaded;

    // Read /Dev/Captured — should have the onread-transformed value (50 * 10 = 500)
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Captured","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v,
        OmiValue::Number(500.0),
        "callback script should receive onread-transformed value (50 * 10 = 500)"
    );
}

// ===========================================================================
// FR-011: Event javascript:// subscriptions do NOT run onread
// ===========================================================================

#[test]
fn event_js_callback_does_not_run_onread() {
    let mut e = engine();

    // Create sensor with onread
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":50}}"#,
    );
    set_onread(&mut e, "/Dev/Sensor", "event.value * 2");

    // Install callback script
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Result","v":0}}"#,
    );
    let cb_url = install_callback_script(
        &mut e,
        "handler",
        "odf.writeItem(event.values[0].value, '/Dev/Result');",
    );

    // Create event subscription with javascript:// callback
    let sub_json = format!(
        r#"{{"omi":"1.0","ttl":60,"read":{{"path":"/Dev/Sensor","interval":-1,"callback":"{cb_url}"}}}}"#,
    );
    let resp = process_at(&mut e, &sub_json, BASE_TIME, None);
    assert_eq!(response_status(&resp), 200);

    // Write a new value — event delivery should deliver raw value (NOT onread-transformed)
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
        "event delivery to javascript:// should return raw written value (100), NOT onread-transformed (200)"
    );
    assert!(
        matches!(&deliveries[0].target, DeliveryTarget::Callback(url) if url == &cb_url),
        "expected javascript:// Callback target"
    );
}

#[test]
fn event_js_callback_delivers_raw_value_to_script() {
    let mut e = engine();

    // Create sensor with onread that transforms aggressively (multiply by 1000)
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":7}}"#,
    );
    set_onread(&mut e, "/Dev/Sensor", "event.value * 1000");

    // Install callback and destination
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Captured","v":0}}"#,
    );
    let cb_url = install_callback_script(
        &mut e,
        "capture",
        "odf.writeItem(event.values[0].value, '/Dev/Captured');",
    );

    // Event subscription
    let sub_json = format!(
        r#"{{"omi":"1.0","ttl":60,"read":{{"path":"/Dev/Sensor","interval":-1,"callback":"{cb_url}"}}}}"#,
    );
    let resp = process_at(&mut e, &sub_json, BASE_TIME, None);
    assert_eq!(response_status(&resp), 200);

    // Write triggers event delivery
    let (_write_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":42}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    // Execute the callback with the raw delivery
    let _cascaded = e.run_callback_script(
        &cb_url,
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );

    // Captured value should be the raw written value (42), NOT 42000
    let resp = parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":0,"read":{"path":"/Dev/Captured","newest":1}}"#,
    );
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(
        values[0].v,
        OmiValue::Number(42.0),
        "event callback should receive raw value (42), NOT onread-transformed (42000)"
    );
}

// ===========================================================================
// Contrast test: same item, interval vs event, different onread behavior
// ===========================================================================

#[test]
fn same_item_interval_applies_onread_event_does_not() {
    let mut e = engine();

    // Create sensor via JSON API (auto-marks writable) and set initial value
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":25}}"#,
    );
    set_onread(&mut e, "/Dev/Sensor", "event.value + 100");

    // Create interval subscription with callback
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dev/Sensor","interval":5,"callback":"http://interval.example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Create event subscription with callback
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dev/Sensor","interval":-1,"callback":"http://event.example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // --- Interval delivery (tick) should apply onread ---
    let interval_deliveries = e.tick(BASE_TIME + 5.0);
    assert_eq!(interval_deliveries.len(), 1);
    assert_eq!(
        interval_deliveries[0].values[0].v,
        OmiValue::Number(125.0),
        "interval delivery should have onread-transformed value (25 + 100 = 125)"
    );

    // --- Event delivery (write) should NOT apply onread ---
    let (_write_resp, event_deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Sensor","v":50}}"#,
        BASE_TIME + 6.0,
        None,
    );
    assert!(
        !event_deliveries.is_empty(),
        "write should trigger at least one event delivery"
    );
    let event_delivery = event_deliveries
        .iter()
        .find(|d| matches!(&d.target, DeliveryTarget::Callback(url) if url.contains("event")))
        .expect("should have event delivery for event callback");
    assert_eq!(
        event_delivery.values[0].v,
        OmiValue::Number(50.0),
        "event delivery should have raw written value (50), NOT onread-transformed (150)"
    );
}

// ===========================================================================
// Edge case: onread error on interval javascript:// falls back to stored value
// ===========================================================================

#[test]
fn interval_js_callback_onread_error_falls_back_to_stored() {
    let mut e = engine();

    e.tree
        .write_value("/Dev/Sensor", OmiValue::Number(77.0), Some(BASE_TIME))
        .unwrap();
    // Broken onread script
    set_onread(&mut e, "/Dev/Sensor", "this is not valid javascript!!!");

    let cb_url = install_callback_script(&mut e, "handler", "null;");

    let sub_json = format!(
        r#"{{"omi":"1.0","ttl":60,"read":{{"path":"/Dev/Sensor","interval":5,"callback":"{cb_url}"}}}}"#,
    );
    let resp = process_at(&mut e, &sub_json, BASE_TIME, None);
    assert_eq!(response_status(&resp), 200);

    // Tick — broken onread should fall back to stored value
    let deliveries = e.tick(BASE_TIME + 5.0);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        deliveries[0].values[0].v,
        OmiValue::Number(77.0),
        "broken onread should fall back to stored value (77)"
    );
}

// ===========================================================================
// Edge case: item without onread, interval javascript:// delivers raw value
// ===========================================================================

#[test]
fn interval_js_callback_without_onread_delivers_raw() {
    let mut e = engine();

    e.tree
        .write_value("/Dev/Sensor", OmiValue::Number(33.0), Some(BASE_TIME))
        .unwrap();
    // No onread attached

    let cb_url = install_callback_script(&mut e, "handler", "null;");

    let sub_json = format!(
        r#"{{"omi":"1.0","ttl":60,"read":{{"path":"/Dev/Sensor","interval":5,"callback":"{cb_url}"}}}}"#,
    );
    let resp = process_at(&mut e, &sub_json, BASE_TIME, None);
    assert_eq!(response_status(&resp), 200);

    let deliveries = e.tick(BASE_TIME + 5.0);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        deliveries[0].values[0].v,
        OmiValue::Number(33.0),
        "without onread, interval delivery should have stored value as-is"
    );
}

// ===========================================================================
// Onread with nested readItem in interval javascript:// callback
// ===========================================================================

#[test]
fn interval_js_callback_onread_with_nested_readitem() {
    let mut e = engine();

    // Create two items: Sensor (subscribed) and Offset (referenced by onread)
    parse_and_process(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Dev/Offset","v":10}}"#,
    );
    e.tree
        .write_value("/Dev/Sensor", OmiValue::Number(20.0), Some(BASE_TIME))
        .unwrap();

    // Sensor's onread reads Offset (via /value suffix) and adds it
    set_onread(
        &mut e,
        "/Dev/Sensor",
        "event.value + odf.readItem('/Dev/Offset/value')",
    );

    let cb_url = install_callback_script(&mut e, "handler", "null;");

    let sub_json = format!(
        r#"{{"omi":"1.0","ttl":60,"read":{{"path":"/Dev/Sensor","interval":5,"callback":"{cb_url}"}}}}"#,
    );
    let resp = process_at(&mut e, &sub_json, BASE_TIME, None);
    assert_eq!(response_status(&resp), 200);

    // Tick — onread should execute and add offset (20 + 10 = 30)
    let deliveries = e.tick(BASE_TIME + 5.0);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        deliveries[0].values[0].v,
        OmiValue::Number(30.0),
        "onread with readItem should work for interval javascript:// delivery (20 + 10 = 30)"
    );
}
