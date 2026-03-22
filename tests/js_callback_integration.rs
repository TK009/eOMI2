#![cfg(feature = "lite-json")]
//! Integration tests for core javascript:// callback subscription flows.
//!
//! These exercise the full JSON round-trip through the Engine for
//! subscriptions whose callback URL uses the `javascript://` scheme,
//! routing delivery to the embedded script engine instead of HTTP POST.
//!
//! Covers SC-001 through SC-006 from spec-008:
//! - SC-001: Interval subscription fires and executes target script
//! - SC-002: Event subscription fires on write and executes script
//! - SC-003: TTL expiry stops script execution
//! - SC-004: No HTTP traffic generated (javascript:// stays local)
//! - SC-006: MAX_SUBSCRIPTIONS counting includes javascript:// subs

#![cfg(feature = "scripting")]

mod common;

use common::*;
use reconfigurable_device::odf::OmiValue;
use reconfigurable_device::omi::Engine;
use reconfigurable_device::omi::subscriptions::{DeliveryTarget, MAX_SUBSCRIPTIONS};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BASE_TIME: f64 = 1_000_000.0;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an engine with the script engine initialised.
fn engine() -> Engine {
    let e = Engine::new();
    assert!(
        e.has_script_engine(),
        "ScriptEngine failed to initialise — tests would be meaningless"
    );
    e
}

/// Store a script in a MetaData InfoItem so it can be resolved via javascript:// URL.
///
/// Creates the path `/Callbacks/MetaData/{name}` with the script text as its value.
/// The corresponding javascript:// URL is `javascript:///Callbacks/MetaData/{name}`.
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

/// Create an item that the callback script can write to (side-effect target).
fn create_item(e: &mut Engine, path: &str, initial: f64) {
    let json = format!(
        r#"{{"omi":"1.0","ttl":10,"write":{{"path":"{}","v":{}}}}}"#,
        path, initial
    );
    let resp = parse_and_process(e, &json);
    let status = response_status(&resp);
    assert!(status == 200 || status == 201, "failed to create item {}", path);
}

/// Read the newest value from a path.
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

// ===========================================================================
// SC-002: Event subscription with javascript:// callback fires on write
// ===========================================================================

#[test]
fn event_sub_js_callback_fires_on_write() {
    let mut e = engine();

    // Script copies the first delivered value to /Target/EventDst
    // Callback event format: event.values[0].value
    store_callback_script(&mut e, "on_event", "odf.writeItem(event.values[0].value, '/Target/EventDst');");
    create_item(&mut e, "/Target/EventDst", 0.0);
    create_item(&mut e, "/Sensor/Temp", 0.0);

    // Create event subscription with javascript:// callback
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/Temp","interval":-1,"callback":"javascript:///Callbacks/MetaData/on_event"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Write to the subscribed path — should trigger the callback script
    let (write_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Temp","v":42}}"#,
        BASE_TIME + 1.0,
        None,
    );
    let ws = response_status(&write_resp);
    assert!(ws == 200 || ws == 201, "write should succeed");

    // The delivery should target the javascript:// callback
    assert_eq!(deliveries.len(), 1);
    assert!(
        matches!(&deliveries[0].target, DeliveryTarget::Callback(url) if url.contains("javascript://")),
        "expected javascript:// callback target"
    );

    // Execute the callback script via the engine
    e.run_callback_script(
        "javascript:///Callbacks/MetaData/on_event",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );

    // Verify the script wrote to the destination
    let dst_val = read_newest(&mut e, "/Target/EventDst");
    assert_eq!(dst_val, OmiValue::Number(42.0));
}

#[test]
fn event_sub_js_callback_receives_correct_values() {
    let mut e = engine();

    // Script writes the value multiplied by 2 to verify it receives the actual value
    store_callback_script(&mut e, "double", "odf.writeItem(event.values[0].value * 2, '/Target/Doubled');");
    create_item(&mut e, "/Target/Doubled", 0.0);
    create_item(&mut e, "/Sensor/Input", 0.0);

    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/Input","interval":-1,"callback":"javascript:///Callbacks/MetaData/double"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Input","v":21}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    e.run_callback_script(
        "javascript:///Callbacks/MetaData/double",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );

    let val = read_newest(&mut e, "/Target/Doubled");
    assert_eq!(val, OmiValue::Number(42.0));
}

// ===========================================================================
// SC-001: Interval subscription fires and executes target script
// ===========================================================================

#[test]
fn interval_sub_js_callback_fires_on_tick() {
    let mut e = engine();

    // Script copies the first delivered value to a destination
    store_callback_script(&mut e, "on_tick", "odf.writeItem(event.values[0].value, '/Target/TickDst');");
    create_item(&mut e, "/Target/TickDst", 0.0);
    create_item(&mut e, "/Sensor/Periodic", 55.0);

    // Create interval subscription with javascript:// callback (interval=10s)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/Periodic","interval":10,"callback":"javascript:///Callbacks/MetaData/on_tick"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Tick at BASE_TIME+10 — interval fires
    let (deliveries, _) = e.tick(BASE_TIME + 10.0);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].rid, rid);
    assert!(
        matches!(&deliveries[0].target, DeliveryTarget::Callback(url) if url.contains("javascript://")),
        "expected javascript:// callback target"
    );

    // Execute the callback
    e.run_callback_script(
        "javascript:///Callbacks/MetaData/on_tick",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 10.0,
    );

    let val = read_newest(&mut e, "/Target/TickDst");
    assert_eq!(val, OmiValue::Number(55.0));
}

#[test]
fn interval_sub_js_callback_fires_multiple_ticks() {
    let mut e = engine();

    // Script reads current counter (via /value suffix for raw number) and increments by 1
    store_callback_script(&mut e, "counter", "let c = odf.readItem('/Target/Counter/value'); odf.writeItem(c + 1, '/Target/Counter');");
    create_item(&mut e, "/Target/Counter", 0.0);
    create_item(&mut e, "/Sensor/Heartbeat", 1.0);

    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":120,"read":{"path":"/Sensor/Heartbeat","interval":5,"callback":"javascript:///Callbacks/MetaData/counter"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Tick 3 times
    for i in 1..=3 {
        let tick_time = BASE_TIME + (i as f64 * 5.0);
        let (deliveries, _) = e.tick(tick_time);
        assert_eq!(deliveries.len(), 1, "tick {} should produce 1 delivery", i);

        e.run_callback_script(
            "javascript:///Callbacks/MetaData/counter",
            &deliveries[0].path,
            &deliveries[0].values,
            tick_time,
        );
    }

    let val = read_newest(&mut e, "/Target/Counter");
    assert_eq!(val, OmiValue::Number(3.0));
}

// ===========================================================================
// SC-003: TTL expiry stops script execution
// ===========================================================================

#[test]
fn event_sub_js_callback_ttl_expiry_no_delivery() {
    let mut e = engine();

    store_callback_script(&mut e, "expired_cb", "odf.writeItem(999, '/Target/ShouldNotChange');");
    create_item(&mut e, "/Target/ShouldNotChange", -1.0);
    create_item(&mut e, "/Sensor/Expiring", 0.0);

    // Create event subscription with ttl=30
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":30,"read":{"path":"/Sensor/Expiring","interval":-1,"callback":"javascript:///Callbacks/MetaData/expired_cb"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Write after TTL has expired (BASE_TIME + 31 > BASE_TIME + 30)
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Expiring","v":999}}"#,
        BASE_TIME + 31.0,
        None,
    );

    // No deliveries — subscription expired
    assert!(deliveries.is_empty(), "expired javascript:// subscription should produce no deliveries");

    // Target should not have been updated
    let val = read_newest(&mut e, "/Target/ShouldNotChange");
    assert_eq!(val, OmiValue::Number(-1.0));
}

#[test]
fn interval_sub_js_callback_ttl_expiry_stops_ticks() {
    let mut e = engine();

    // Script writes the delivered value to target
    store_callback_script(&mut e, "tick_expired", "odf.writeItem(event.values[0].value, '/Target/TickExpired');");
    create_item(&mut e, "/Target/TickExpired", 0.0);
    create_item(&mut e, "/Sensor/ShortLived", 77.0);

    // Create interval subscription with ttl=15, interval=10
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":15,"read":{"path":"/Sensor/ShortLived","interval":10,"callback":"javascript:///Callbacks/MetaData/tick_expired"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // First tick at +10s — within TTL, should fire
    let (deliveries, _) = e.tick(BASE_TIME + 10.0);
    assert_eq!(deliveries.len(), 1, "first tick within TTL should fire");

    e.run_callback_script(
        "javascript:///Callbacks/MetaData/tick_expired",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 10.0,
    );
    assert_eq!(read_newest(&mut e, "/Target/TickExpired"), OmiValue::Number(77.0));

    // Update source value
    create_item(&mut e, "/Sensor/ShortLived", 88.0);

    // Second tick at +20s — beyond TTL (BASE_TIME + 15), should NOT fire
    let (deliveries, _) = e.tick(BASE_TIME + 20.0);
    assert!(deliveries.is_empty(), "tick after TTL expiry should produce no deliveries");

    // Target should still have old value
    assert_eq!(read_newest(&mut e, "/Target/TickExpired"), OmiValue::Number(77.0));
}

// ===========================================================================
// SC-004: No HTTP traffic generated (javascript:// stays local)
// ===========================================================================

#[test]
fn js_callback_delivery_has_callback_target_not_http() {
    let mut e = engine();

    store_callback_script(&mut e, "local_only", "odf.writeItem(1, '/Target/Local');");
    create_item(&mut e, "/Target/Local", 0.0);
    create_item(&mut e, "/Sensor/NoHttp", 0.0);

    // Create event subscription with javascript:// callback
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/NoHttp","interval":-1,"callback":"javascript:///Callbacks/MetaData/local_only"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Write to trigger delivery
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/NoHttp","v":1}}"#,
        BASE_TIME + 1.0,
        None,
    );

    assert_eq!(deliveries.len(), 1);

    // The delivery target should be Callback with javascript:// URL (not http://)
    match &deliveries[0].target {
        DeliveryTarget::Callback(url) => {
            assert!(
                url.starts_with("javascript://"),
                "callback URL should use javascript:// scheme, got: {}",
                url
            );
            assert!(
                !url.starts_with("http://") && !url.starts_with("https://"),
                "javascript:// callback should NOT use HTTP scheme"
            );
        }
        other => panic!("expected Callback target, got {:?}", other),
    }
}

#[test]
fn js_and_http_callbacks_coexist_independently() {
    let mut e = engine();

    store_callback_script(&mut e, "js_side", "odf.writeItem(event.values[0].value, '/Target/JsSide');");
    create_item(&mut e, "/Target/JsSide", 0.0);
    create_item(&mut e, "/Sensor/Dual", 0.0);

    // Create javascript:// event subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/Dual","interval":-1,"callback":"javascript:///Callbacks/MetaData/js_side"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let js_rid = response_rid(&resp).to_string();

    // Create http:// event subscription on same path
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/Dual","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let http_rid = response_rid(&resp).to_string();

    // Write — should trigger both subscriptions
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Dual","v":99}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 2, "both subscriptions should fire");

    // Verify one is javascript:// and one is http://
    let js_delivery = deliveries.iter().find(|d| d.rid == js_rid);
    let http_delivery = deliveries.iter().find(|d| d.rid == http_rid);

    assert!(js_delivery.is_some(), "javascript:// delivery missing");
    assert!(http_delivery.is_some(), "http:// delivery missing");

    assert!(
        matches!(&js_delivery.unwrap().target, DeliveryTarget::Callback(url) if url.starts_with("javascript://")),
        "expected javascript:// scheme"
    );
    assert!(
        matches!(&http_delivery.unwrap().target, DeliveryTarget::Callback(url) if url.starts_with("http://")),
        "expected http:// scheme"
    );
}

// ===========================================================================
// SC-006: MAX_SUBSCRIPTIONS counting includes javascript:// subs
// ===========================================================================

#[test]
fn js_callback_subs_count_toward_max_subscriptions() {
    let mut e = engine();

    store_callback_script(&mut e, "noop", "1;");

    // Fill up to MAX_SUBSCRIPTIONS with javascript:// subscriptions
    for i in 0..MAX_SUBSCRIPTIONS {
        let path = format!("/Limit/Item{}", i);
        create_item(&mut e, &path, 0.0);

        let json = format!(
            r#"{{"omi":"1.0","ttl":60,"read":{{"path":"{}","interval":-1,"callback":"javascript:///Callbacks/MetaData/noop"}}}}"#,
            path
        );
        let resp = process_at(&mut e, &json, BASE_TIME, None);
        assert_eq!(
            response_status(&resp), 200,
            "subscription {} should succeed (limit is {})", i, MAX_SUBSCRIPTIONS
        );
    }

    // The next subscription (any type) should fail
    create_item(&mut e, "/Limit/Overflow", 0.0);
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Limit/Overflow","interval":-1,"callback":"javascript:///Callbacks/MetaData/noop"}}"#,
        BASE_TIME,
        None,
    );
    let status = response_status(&resp);
    assert_ne!(status, 200, "exceeding MAX_SUBSCRIPTIONS should fail, got 200");
}

#[test]
fn mixed_js_and_http_subs_share_max_limit() {
    let mut e = engine();

    store_callback_script(&mut e, "noop2", "1;");

    // Fill half with javascript:// and half with http://
    let half = MAX_SUBSCRIPTIONS / 2;

    for i in 0..half {
        let path = format!("/Mixed/JS{}", i);
        create_item(&mut e, &path, 0.0);
        let json = format!(
            r#"{{"omi":"1.0","ttl":60,"read":{{"path":"{}","interval":-1,"callback":"javascript:///Callbacks/MetaData/noop2"}}}}"#,
            path
        );
        let resp = process_at(&mut e, &json, BASE_TIME, None);
        assert_eq!(response_status(&resp), 200);
    }

    for i in 0..(MAX_SUBSCRIPTIONS - half) {
        let path = format!("/Mixed/HTTP{}", i);
        create_item(&mut e, &path, 0.0);
        let json = format!(
            r#"{{"omi":"1.0","ttl":60,"read":{{"path":"{}","interval":-1,"callback":"http://example.com/cb"}}}}"#,
            path
        );
        let resp = process_at(&mut e, &json, BASE_TIME, None);
        assert_eq!(response_status(&resp), 200);
    }

    // Now at MAX — one more should fail
    create_item(&mut e, "/Mixed/Overflow", 0.0);
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Mixed/Overflow","interval":-1,"callback":"http://example.com/overflow"}}"#,
        BASE_TIME,
        None,
    );
    assert_ne!(response_status(&resp), 200, "exceeding MAX_SUBSCRIPTIONS with mixed types should fail");
}

// ===========================================================================
// Script error resilience — callback script errors don't cancel subscription
// ===========================================================================

#[test]
fn js_callback_script_error_does_not_cancel_subscription() {
    let mut e = engine();

    // Store a broken script
    store_callback_script(&mut e, "broken", "this is not valid js!!!");
    create_item(&mut e, "/Sensor/Resilient", 0.0);

    // Create event subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/Resilient","interval":-1,"callback":"javascript:///Callbacks/MetaData/broken"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // First write — triggers broken script
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Resilient","v":1}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    // Execute the broken script — should not panic
    let cascaded = e.run_callback_script(
        "javascript:///Callbacks/MetaData/broken",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );
    assert!(cascaded.is_empty(), "broken script should produce no cascaded deliveries");

    // Second write — subscription should still be alive
    let (_resp, deliveries2) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Resilient","v":2}}"#,
        BASE_TIME + 2.0,
        None,
    );
    assert_eq!(deliveries2.len(), 1, "subscription should still fire after script error");
    assert_eq!(deliveries2[0].rid, rid);
}

// ===========================================================================
// Cascading: javascript:// callback triggers further deliveries
// ===========================================================================

#[test]
fn js_callback_cascade_triggers_event_subscription() {
    let mut e = engine();

    // Script writes value * 10 to /Cascade/Mid
    store_callback_script(&mut e, "cascade_a", "odf.writeItem(event.values[0].value * 10, '/Cascade/Mid');");
    create_item(&mut e, "/Cascade/Src", 0.0);
    create_item(&mut e, "/Cascade/Mid", 0.0);

    // Subscribe to /Cascade/Src with javascript:// callback
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Cascade/Src","interval":-1,"callback":"javascript:///Callbacks/MetaData/cascade_a"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Subscribe to /Cascade/Mid with a poll subscription (to verify cascading)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Cascade/Mid","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let poll_rid = response_rid(&resp).to_string();

    // Write to /Cascade/Src
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Cascade/Src","v":5}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);

    // Execute the callback — it writes to /Cascade/Mid, which should trigger cascade
    e.run_callback_script(
        "javascript:///Callbacks/MetaData/cascade_a",
        &deliveries[0].path,
        &deliveries[0].values,
        BASE_TIME + 1.0,
    );

    // Verify /Cascade/Mid was written by the script
    let val = read_newest(&mut e, "/Cascade/Mid");
    assert_eq!(val, OmiValue::Number(50.0));

    // Poll the mid subscription to verify the cascaded event was buffered
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, poll_rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 2.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = extract_values(&resp);
    assert_eq!(polled.len(), 1, "cascaded write should buffer in poll subscription");
    assert_eq!(polled[0].v, OmiValue::Number(50.0));
}

// ===========================================================================
// Existing subscription types still work unchanged
// ===========================================================================

#[test]
fn http_callback_sub_still_works_alongside_js() {
    let mut e = engine();

    store_callback_script(&mut e, "coexist", "1;");
    create_item(&mut e, "/Sensor/Coexist", 0.0);

    // Create an HTTP callback subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/Coexist","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let http_rid = response_rid(&resp).to_string();

    // Write — should still produce HTTP callback delivery
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/Coexist","v":7}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].rid, http_rid);
    assert!(
        matches!(&deliveries[0].target, DeliveryTarget::Callback(url) if url == "http://example.com/omi"),
        "HTTP callback should still work"
    );
}

#[test]
fn poll_sub_still_works_alongside_js() {
    let mut e = engine();

    store_callback_script(&mut e, "poll_coexist", "1;");
    create_item(&mut e, "/Sensor/PollCoexist", 0.0);

    // Create a poll subscription (no callback)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Sensor/PollCoexist","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let poll_rid = response_rid(&resp).to_string();

    // Write
    let (_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Sensor/PollCoexist","v":33}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert!(deliveries.is_empty(), "poll sub should not produce deliveries");

    // Poll should have the value
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, poll_rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 2.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = extract_values(&resp);
    assert_eq!(polled.len(), 1);
    assert_eq!(polled[0].v, OmiValue::Number(33.0));
}
