#![cfg(feature = "json")]
//! Integration tests for multi-step subscription lifecycles.
//!
//! These tests exercise the full JSON round-trip through the Engine for
//! subscription create → event/tick → poll/cancel flows.  Unlike the unit
//! tests in `src/omi/subscriptions.rs` they use the real sensor tree and
//! go through `OmiMessage::parse` / `Engine::process` / JSON serialization.

mod common;

use reconfigurable_device::odf::{OmiValue, Value};
use reconfigurable_device::omi::subscriptions::DeliveryTarget;

use common::*;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Arbitrary non-zero epoch used as a base for simulated time in these tests.
/// The exact value is unimportant; it just avoids edge-case behaviour at t=0.
const BASE_TIME: f64 = 1_000_000.0;

// ===========================================================================
// 2.1  Poll Subscription
// ===========================================================================

#[test]
fn poll_sub_create_write_poll() {
    let mut e = engine_with_sensor_tree();

    // Create poll subscription (interval=-1, no callback, no ws_session → Poll target)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Write a value directly (sensor items are read-only via JSON)
    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(23.5), Some(BASE_TIME + 1.0))
        .unwrap();

    // Simulate event notification (production code does this in main.rs)
    let values = vec![Value::new(OmiValue::Number(23.5), Some(BASE_TIME + 1.0))];
    e.subscriptions().notify_event("/System/FreeHeap", &values, BASE_TIME + 1.0);

    // Poll by rid
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 2.0, None);
    assert_eq!(response_status(&resp), 200);

    let polled_values = extract_values(&resp);
    assert_eq!(polled_values.len(), 1);
    assert_eq!(polled_values[0].v, OmiValue::Number(23.5));
}

#[test]
fn poll_sub_drain_clears_buffer() {
    let mut e = engine_with_sensor_tree();

    // Create poll subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    let rid = response_rid(&resp).to_string();

    // Write + notify
    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(20.0), Some(BASE_TIME + 1.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(20.0), Some(BASE_TIME + 1.0))];
    e.subscriptions().notify_event("/System/FreeHeap", &values, BASE_TIME + 1.0);

    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);

    // First poll: returns 1 value
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 2.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = extract_values(&resp);
    assert_eq!(polled.len(), 1);

    // Second poll: buffer drained, returns empty
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 3.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = extract_values(&resp);
    assert_eq!(polled.len(), 0);
}

#[test]
fn poll_sub_multiple_values() {
    let mut e = engine_with_sensor_tree();

    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    let rid = response_rid(&resp).to_string();

    // Write + notify 3 distinct values
    for i in 1..=3 {
        let temp = 20.0 + i as f64;
        let t = BASE_TIME + i as f64;
        e.tree
            .write_value("/System/FreeHeap", OmiValue::Number(temp), Some(t))
            .unwrap();
        let values = vec![Value::new(OmiValue::Number(temp), Some(t))];
        e.subscriptions().notify_event("/System/FreeHeap", &values, t);
    }

    // Single poll returns all 3
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 5.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = extract_values(&resp);
    assert_eq!(polled.len(), 3);
}

#[test]
fn poll_sub_ttl_expiry() {
    let mut e = engine_with_sensor_tree();

    // Create subscription with ttl=60
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    let rid = response_rid(&resp).to_string();

    // Poll after TTL has expired (BASE_TIME + 61 > BASE_TIME + 60)
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 61.0, None);
    assert_eq!(response_status(&resp), 404);
}

// ===========================================================================
// 2.2  Event Subscription
// ===========================================================================

#[test]
fn event_sub_triggers_on_write() {
    let mut e = engine_with_sensor_tree();

    // Create callback event subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Write + notify
    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(25.0), Some(BASE_TIME + 1.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(25.0), Some(BASE_TIME + 1.0))];
    let deliveries = e.subscriptions().notify_event("/System/FreeHeap", &values, BASE_TIME + 1.0);

    // Assert exactly 1 delivery with correct fields
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].rid, rid);
    assert_eq!(deliveries[0].path, "/System/FreeHeap");
    assert_eq!(deliveries[0].values.len(), 1);
    assert_eq!(deliveries[0].values[0].v, OmiValue::Number(25.0));
}

#[test]
fn event_sub_no_trigger_on_unrelated_write() {
    let mut e = engine_with_sensor_tree();

    // Subscribe to FreeHeap
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Notify on a different path (unrelated user item)
    e.tree
        .write_value("/Other/Sensor", OmiValue::Number(55.0), Some(BASE_TIME + 1.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(55.0), Some(BASE_TIME + 1.0))];
    let deliveries = e.subscriptions().notify_event("/Other/Sensor", &values, BASE_TIME + 1.0);

    assert!(deliveries.is_empty());
}

#[test]
fn event_sub_ttl_expiry_no_delivery() {
    let mut e = engine_with_sensor_tree();

    // Create callback event subscription with ttl=60
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Write + notify after TTL has expired
    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(99.0), Some(BASE_TIME + 61.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(99.0), Some(BASE_TIME + 61.0))];
    let deliveries = e.subscriptions().notify_event("/System/FreeHeap", &values, BASE_TIME + 61.0);

    assert!(deliveries.is_empty(), "expired callback subscription should produce no deliveries");
}

// ===========================================================================
// 2.3  Interval Subscription
// ===========================================================================

#[test]
fn interval_sub_fires_on_tick() {
    let mut e = engine_with_sensor_tree();

    // Write a value to tree first (tick reads current values)
    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(22.0), Some(BASE_TIME))
        .unwrap();

    // Create interval poll subscription (interval=10, no callback → Poll target)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":10}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Tick at BASE_TIME+10 (exactly when the interval fires).
    // Poll-target subs buffer internally, so tick returns no deliveries.
    let deliveries = e.tick(BASE_TIME + 10.0);
    assert!(deliveries.is_empty(), "poll-target tick should produce no deliveries");

    // Poll to retrieve the buffered value
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 11.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = extract_values(&resp);
    assert_eq!(polled.len(), 1);
    assert_eq!(polled[0].v, OmiValue::Number(22.0));
}

#[test]
fn interval_sub_skips_before_due() {
    let mut e = engine_with_sensor_tree();

    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(22.0), Some(BASE_TIME))
        .unwrap();

    // Create interval=10 subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":10}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Tick at +5s (before the 10s interval is due)
    let deliveries = e.tick(BASE_TIME + 5.0);
    assert!(deliveries.is_empty(), "tick before interval due should produce no deliveries");

    // Poll — should be empty since interval hasn't fired yet
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 6.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = extract_values(&resp);
    assert!(polled.is_empty(), "interval not yet due, poll should be empty");
}

#[test]
fn interval_sub_callback_delivery() {
    let mut e = engine_with_sensor_tree();

    // Write a value to tree first (tick reads current values)
    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(22.0), Some(BASE_TIME))
        .unwrap();

    // Create interval subscription with callback (→ Callback delivery, not Poll)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":10,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Tick at BASE_TIME+10 (interval fires)
    let deliveries = e.tick(BASE_TIME + 10.0);

    // Callback subscription should produce a Delivery (not buffer internally)
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].rid, rid);
    assert_eq!(deliveries[0].path, "/System/FreeHeap");
    assert!(!deliveries[0].values.is_empty());
    assert_eq!(deliveries[0].values[0].v, OmiValue::Number(22.0));
    assert!(
        matches!(&deliveries[0].target, DeliveryTarget::Callback(url) if url == "http://example.com/omi"),
        "expected Callback target"
    );
}

// ===========================================================================
// 2.4  WebSocket
// ===========================================================================

#[test]
fn ws_sub_delivers_before_disconnect() {
    let mut e = engine_with_sensor_tree();

    // Create event subscription with ws_session=42 (→ WebSocket delivery)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1}}"#,
        BASE_TIME,
        Some(42),
    );
    assert_eq!(response_status(&resp), 200);

    // Write + notify while WS is still connected
    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(25.0), Some(BASE_TIME + 1.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(25.0), Some(BASE_TIME + 1.0))];
    let deliveries = e.subscriptions().notify_event("/System/FreeHeap", &values, BASE_TIME + 1.0);

    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].values[0].v, OmiValue::Number(25.0));
    assert!(
        matches!(&deliveries[0].target, DeliveryTarget::WebSocket(42)),
        "expected WebSocket(42) target"
    );
}

#[test]
fn ws_sub_cancelled_on_disconnect() {
    let mut e = engine_with_sensor_tree();

    // Create subscription with ws_session=42 (no callback → WebSocket delivery)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1}}"#,
        BASE_TIME,
        Some(42),
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Simulate WebSocket disconnect
    e.subscriptions().cancel_by_ws_session(42);

    // Attempt to poll — should be 404 (subscription cancelled)
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 1.0, None);
    assert_eq!(response_status(&resp), 404);
}

// ===========================================================================
// 2.5  Cancel
// ===========================================================================

#[test]
fn cancel_stops_delivery() {
    let mut e = engine_with_sensor_tree();

    // Create callback event subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Cancel via JSON
    let cancel_json = format!(
        r#"{{"omi":"1.0","ttl":0,"cancel":{{"rid":["{}"]}}}}"#,
        rid
    );
    let resp = process_at(&mut e, &cancel_json, BASE_TIME + 1.0, None);
    assert_eq!(response_status(&resp), 200);

    // Write + notify after cancel
    e.tree
        .write_value("/System/FreeHeap", OmiValue::Number(30.0), Some(BASE_TIME + 2.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(30.0), Some(BASE_TIME + 2.0))];
    let deliveries = e.subscriptions().notify_event("/System/FreeHeap", &values, BASE_TIME + 2.0);

    assert!(deliveries.is_empty(), "cancelled subscription should produce no deliveries");
}

#[test]
fn cancel_double_cancel_idempotent() {
    let mut e = engine_with_sensor_tree();

    // Create a poll subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    let cancel_json = format!(
        r#"{{"omi":"1.0","ttl":0,"cancel":{{"rid":["{}"]}}}}"#,
        rid
    );

    // First cancel — succeeds
    let resp = process_at(&mut e, &cancel_json, BASE_TIME + 1.0, None);
    assert_eq!(response_status(&resp), 200);

    // Second cancel of the same rid — still 200 (idempotent)
    let resp = process_at(&mut e, &cancel_json, BASE_TIME + 2.0, None);
    assert_eq!(response_status(&resp), 200);
}

#[test]
fn cancel_batch() {
    let mut e = engine_with_sensor_tree();

    // Create 3 poll subscriptions
    let mut rids = Vec::new();
    for _ in 0..3 {
        let resp = process_at(
            &mut e,
            r#"{"omi":"1.0","ttl":60,"read":{"path":"/System/FreeHeap","interval":-1}}"#,
            BASE_TIME,
            None,
        );
        assert_eq!(response_status(&resp), 200);
        rids.push(response_rid(&resp).to_string());
    }

    // Cancel first two via batch cancel
    let cancel_json = format!(
        r#"{{"omi":"1.0","ttl":0,"cancel":{{"rid":["{}","{}"]}}}}"#,
        rids[0], rids[1]
    );
    let resp = process_at(&mut e, &cancel_json, BASE_TIME + 1.0, None);
    assert_eq!(response_status(&resp), 200);

    // Cancelled subscriptions should 404
    for rid in &rids[..2] {
        let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
        let resp = process_at(&mut e, &poll_json, BASE_TIME + 2.0, None);
        assert_eq!(response_status(&resp), 404, "cancelled sub {} should be 404", rid);
    }

    // Remaining subscription should still be alive (200)
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rids[2]);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 2.0, None);
    assert_eq!(response_status(&resp), 200, "surviving sub should be 200");
}

// ===========================================================================
// 2.6  Write-triggered event delivery
// ===========================================================================

#[test]
fn subscribe_nonexistent_path_then_write_triggers_event() {
    let mut e = engine_with_sensor_tree();

    // Subscribe to a path that doesn't exist yet (event sub with callback)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Test/SubVal","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Write to that path via the engine — should trigger event notification
    let (write_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/SubVal","v":42}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(response_status(&write_resp), 201);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].rid, rid);
    assert_eq!(deliveries[0].path, "/Test/SubVal");
}

#[test]
fn write_triggers_poll_sub_event() {
    let mut e = engine_with_sensor_tree();

    // Create poll event subscription (interval=-1, no callback → Poll target)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Test/PollVal","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Write via engine — poll sub buffers internally (no deliveries returned)
    let (write_resp, deliveries) = process_at_with_deliveries(
        &mut e,
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/Test/PollVal","v":"hello"}}"#,
        BASE_TIME + 1.0,
        None,
    );
    assert_eq!(response_status(&write_resp), 201);
    assert!(deliveries.is_empty(), "poll sub should not produce deliveries");

    // Poll should now have the buffered value
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 2.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = extract_values(&resp);
    assert_eq!(polled.len(), 1);
    assert_eq!(polled[0].v, OmiValue::Str("hello".into()));
}
