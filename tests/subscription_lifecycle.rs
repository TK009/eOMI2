//! Integration tests for multi-step subscription lifecycles.
//!
//! These tests exercise the full JSON round-trip through the Engine for
//! subscription create → event/tick → poll/cancel flows.  Unlike the unit
//! tests in `src/omi/subscriptions.rs` they use the real sensor tree and
//! go through `OmiMessage::parse` / `Engine::process` / JSON serialization.

mod common;

use reconfigurable_device::odf::{OmiValue, Value};
use reconfigurable_device::omi::{Engine, OmiMessage, Operation};

use common::{engine_with_sensor_tree, response_result, response_status};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Large base timestamp to avoid near-zero edge cases in expiry arithmetic.
const BASE_TIME: f64 = 1_000_000.0;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a JSON request, feed it to the engine at a given time/session, return response.
fn process_at(engine: &mut Engine, json: &str, now: f64, ws_session: Option<u64>) -> OmiMessage {
    let msg = OmiMessage::parse(json).expect("request JSON should parse");
    engine.process(msg, now, ws_session)
}

/// Extract the subscription rid from a response.
fn response_rid(resp: &OmiMessage) -> &str {
    match &resp.operation {
        Operation::Response(body) => body.rid.as_deref().expect("expected rid in response"),
        _ => panic!("expected Response"),
    }
}

// ===========================================================================
// 2.1  Poll Subscription
// ===========================================================================

#[test]
fn poll_sub_create_write_poll() {
    let mut e = engine_with_sensor_tree();

    // Create poll subscription (interval=-1, no callback, no ws_session → Poll target)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Write a value directly (sensor items are read-only via JSON)
    e.tree
        .write_value("/Dht11/Temperature", OmiValue::Number(23.5), Some(BASE_TIME + 1.0))
        .unwrap();

    // Simulate event notification (production code does this in main.rs)
    let values = vec![Value::new(OmiValue::Number(23.5), Some(BASE_TIME + 1.0))];
    e.subscriptions().notify_event("/Dht11/Temperature", &values, BASE_TIME + 1.0);

    // Poll by rid
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 2.0, None);
    assert_eq!(response_status(&resp), 200);

    let result = response_result(&resp);
    let polled_values = result["values"].as_array().expect("expected values array");
    assert_eq!(polled_values.len(), 1);
    assert_eq!(polled_values[0]["v"], 23.5);
}

#[test]
fn poll_sub_drain_clears_buffer() {
    let mut e = engine_with_sensor_tree();

    // Create poll subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    let rid = response_rid(&resp).to_string();

    // Write + notify
    e.tree
        .write_value("/Dht11/Temperature", OmiValue::Number(20.0), Some(BASE_TIME + 1.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(20.0), Some(BASE_TIME + 1.0))];
    e.subscriptions().notify_event("/Dht11/Temperature", &values, BASE_TIME + 1.0);

    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);

    // First poll: returns 1 value
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 2.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = response_result(&resp)["values"].as_array().unwrap();
    assert_eq!(polled.len(), 1);

    // Second poll: buffer drained, returns empty
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 3.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = response_result(&resp)["values"].as_array().unwrap();
    assert_eq!(polled.len(), 0);
}

#[test]
fn poll_sub_multiple_values() {
    let mut e = engine_with_sensor_tree();

    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":-1}}"#,
        BASE_TIME,
        None,
    );
    let rid = response_rid(&resp).to_string();

    // Write + notify 3 distinct values
    for i in 1..=3 {
        let temp = 20.0 + i as f64;
        let t = BASE_TIME + i as f64;
        e.tree
            .write_value("/Dht11/Temperature", OmiValue::Number(temp), Some(t))
            .unwrap();
        let values = vec![Value::new(OmiValue::Number(temp), Some(t))];
        e.subscriptions().notify_event("/Dht11/Temperature", &values, t);
    }

    // Single poll returns all 3
    let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
    let resp = process_at(&mut e, &poll_json, BASE_TIME + 5.0, None);
    assert_eq!(response_status(&resp), 200);
    let polled = response_result(&resp)["values"].as_array().unwrap();
    assert_eq!(polled.len(), 3);
}

#[test]
fn poll_sub_ttl_expiry() {
    let mut e = engine_with_sensor_tree();

    // Create subscription with ttl=60
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":-1}}"#,
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
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Write + notify
    e.tree
        .write_value("/Dht11/Temperature", OmiValue::Number(25.0), Some(BASE_TIME + 1.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(25.0), Some(BASE_TIME + 1.0))];
    let deliveries = e.subscriptions().notify_event("/Dht11/Temperature", &values, BASE_TIME + 1.0);

    // Assert exactly 1 delivery with correct fields
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].rid, rid);
    assert_eq!(deliveries[0].path, "/Dht11/Temperature");
    assert_eq!(deliveries[0].values.len(), 1);
    assert_eq!(deliveries[0].values[0].v, OmiValue::Number(25.0));
}

#[test]
fn event_sub_no_trigger_on_unrelated_write() {
    let mut e = engine_with_sensor_tree();

    // Subscribe to Temperature
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Notify on a different path (RelativeHumidity)
    e.tree
        .write_value("/Dht11/RelativeHumidity", OmiValue::Number(55.0), Some(BASE_TIME + 1.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(55.0), Some(BASE_TIME + 1.0))];
    let deliveries = e.subscriptions().notify_event("/Dht11/RelativeHumidity", &values, BASE_TIME + 1.0);

    assert!(deliveries.is_empty());
}

// ===========================================================================
// 2.3  Interval Subscription
// ===========================================================================

#[test]
fn interval_sub_fires_on_tick() {
    let mut e = engine_with_sensor_tree();

    // Write a value to tree first (tick reads current values)
    e.tree
        .write_value("/Dht11/Temperature", OmiValue::Number(22.0), Some(BASE_TIME))
        .unwrap();

    // Create interval poll subscription (interval=10, no callback → Poll target)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":10}}"#,
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
    let polled = response_result(&resp)["values"].as_array().unwrap();
    assert!(!polled.is_empty(), "tick should have buffered at least one value");
}

#[test]
fn interval_sub_skips_before_due() {
    let mut e = engine_with_sensor_tree();

    e.tree
        .write_value("/Dht11/Temperature", OmiValue::Number(22.0), Some(BASE_TIME))
        .unwrap();

    // Create interval=10 subscription
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":10}}"#,
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
    let polled = response_result(&resp)["values"].as_array().unwrap();
    assert!(polled.is_empty(), "interval not yet due, poll should be empty");
}

// ===========================================================================
// 2.4  WebSocket
// ===========================================================================

#[test]
fn ws_sub_cancelled_on_disconnect() {
    let mut e = engine_with_sensor_tree();

    // Create subscription with ws_session=42 (no callback → WebSocket delivery)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":-1}}"#,
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
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":-1,"callback":"http://example.com/omi"}}"#,
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
        .write_value("/Dht11/Temperature", OmiValue::Number(30.0), Some(BASE_TIME + 2.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(30.0), Some(BASE_TIME + 2.0))];
    let deliveries = e.subscriptions().notify_event("/Dht11/Temperature", &values, BASE_TIME + 2.0);

    assert!(deliveries.is_empty(), "cancelled subscription should produce no deliveries");
}

#[test]
fn cancel_batch() {
    let mut e = engine_with_sensor_tree();

    // Create 3 poll subscriptions
    let mut rids = Vec::new();
    for _ in 0..3 {
        let resp = process_at(
            &mut e,
            r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":-1}}"#,
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
// 2.6  TTL Expiry (callback path)
// ===========================================================================

#[test]
fn event_sub_ttl_expiry_no_delivery() {
    let mut e = engine_with_sensor_tree();

    // Create callback event subscription with ttl=60
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":-1,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);

    // Write + notify after TTL has expired
    e.tree
        .write_value("/Dht11/Temperature", OmiValue::Number(99.0), Some(BASE_TIME + 61.0))
        .unwrap();
    let values = vec![Value::new(OmiValue::Number(99.0), Some(BASE_TIME + 61.0))];
    let deliveries = e.subscriptions().notify_event("/Dht11/Temperature", &values, BASE_TIME + 61.0);

    assert!(deliveries.is_empty(), "expired callback subscription should produce no deliveries");
}

// ===========================================================================
// 2.7  Interval + Callback Delivery
// ===========================================================================

#[test]
fn interval_callback_sub_delivers_on_tick() {
    let mut e = engine_with_sensor_tree();

    // Write a value to tree first (tick reads current values)
    e.tree
        .write_value("/Dht11/Temperature", OmiValue::Number(22.0), Some(BASE_TIME))
        .unwrap();

    // Create interval subscription with callback (interval=10, callback → Callback target)
    let resp = process_at(
        &mut e,
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/Dht11/Temperature","interval":10,"callback":"http://example.com/omi"}}"#,
        BASE_TIME,
        None,
    );
    assert_eq!(response_status(&resp), 200);
    let rid = response_rid(&resp).to_string();

    // Tick at BASE_TIME+10 (exactly when the interval fires).
    // Callback-target subs produce deliveries rather than buffering.
    let deliveries = e.tick(BASE_TIME + 10.0);
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].rid, rid);
    assert_eq!(deliveries[0].path, "/Dht11/Temperature");
    assert!(!deliveries[0].values.is_empty(), "delivery should contain the current value");
}
