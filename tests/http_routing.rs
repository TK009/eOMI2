//! Integration tests for HTTP helpers + Engine wiring.
//!
//! These tests verify the cross-module chain:
//! **URI + query params → ODF path → read operation → Engine → response**.
//! Unit tests in `src/http.rs` cover each function in isolation; these
//! integration tests wire them together with a real Engine.

use reconfigurable_device::device;
use reconfigurable_device::http::{
    build_read_op, is_mutating_operation, omi_uri_to_odf_path, render_landing_page, uri_path,
    uri_query, OmiReadParams,
};
use reconfigurable_device::odf::OmiValue;
use reconfigurable_device::omi::{Engine, OmiMessage, Operation, ResponseResult};
use reconfigurable_device::pages::PageStore;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an engine pre-populated with the real DHT11 sensor tree.
fn engine_with_sensor_tree() -> Engine {
    let mut e = Engine::new();
    e.tree.write_tree("/", device::build_sensor_tree()).unwrap();
    e
}

/// Extract the HTTP-style status code from a response message.
fn response_status(resp: &OmiMessage) -> u16 {
    match &resp.operation {
        Operation::Response(body) => body.status,
        _ => panic!("expected Response"),
    }
}

/// Extract the `Single` result value from a 200 response.
fn response_result(resp: &OmiMessage) -> &serde_json::Value {
    match &resp.operation {
        Operation::Response(body) => match &body.result {
            Some(ResponseResult::Single(v)) => v,
            other => panic!("expected Single result, got {:?}", other),
        },
        _ => panic!("expected Response"),
    }
}

/// Chain the full REST GET flow: URI → parse → build read op → engine.process.
fn get_omi(engine: &mut Engine, uri: &str) -> OmiMessage {
    let path = uri_path(uri);
    let query = uri_query(uri);
    let (odf_path, _trailing) = omi_uri_to_odf_path(path);
    let params = match query {
        Some(q) => OmiReadParams::from_query(q),
        None => OmiReadParams::default(),
    };
    let msg = build_read_op(odf_path, &params);
    engine.process(msg, 0.0, None)
}

// ===========================================================================
// 3.1  REST Discovery
// ===========================================================================

#[test]
fn get_omi_root() {
    let mut e = engine_with_sensor_tree();
    let resp = get_omi(&mut e, "/omi/");
    assert_eq!(response_status(&resp), 200);
    let result = response_result(&resp);
    assert!(result["Dht11"].is_object(), "root should contain Dht11");
}

#[test]
fn get_omi_object() {
    let mut e = engine_with_sensor_tree();
    let resp = get_omi(&mut e, "/omi/Dht11/");
    assert_eq!(response_status(&resp), 200);
    let result = response_result(&resp);
    assert_eq!(result["id"], "Dht11");
    assert!(result["items"]["Temperature"].is_object());
    assert!(result["items"]["RelativeHumidity"].is_object());
}

#[test]
fn get_omi_infoitem() {
    let mut e = engine_with_sensor_tree();
    let resp = get_omi(&mut e, "/omi/Dht11/Temperature");
    assert_eq!(response_status(&resp), 200);
    let values = response_result(&resp)["values"].as_array().unwrap();
    assert!(values.is_empty());
}

#[test]
fn get_omi_with_query_params() {
    let mut e = engine_with_sensor_tree();

    // Write 5 values into the sensor item.
    for i in 1..=5 {
        e.tree
            .write_value(
                "/Dht11/Temperature",
                OmiValue::Number(20.0 + i as f64),
                Some(i as f64 * 100.0),
            )
            .unwrap();
    }

    let resp = get_omi(&mut e, "/omi/Dht11/Temperature?newest=3&depth=1");
    assert_eq!(response_status(&resp), 200);
    let values = response_result(&resp)["values"].as_array().unwrap();
    assert_eq!(values.len(), 3);
}

// ===========================================================================
// 3.2  Landing Page
// ===========================================================================

#[test]
fn landing_page_lists_pages() {
    let mut store = PageStore::new();
    store.store("/dashboard", "<h1>Dashboard</h1>").unwrap();
    store.store("/settings", "<h1>Settings</h1>").unwrap();

    let html = render_landing_page(&store);
    assert!(html.contains("<a href=\"/dashboard\">/dashboard</a>"));
    assert!(html.contains("<a href=\"/settings\">/settings</a>"));
    assert!(!html.contains("No pages stored yet."));
}

// ===========================================================================
// 3.3  Authentication Boundary
// ===========================================================================

#[test]
fn read_not_mutating() {
    // Default params (one-time read)
    let msg_default = build_read_op("/Sensor/Temp", &OmiReadParams::default());
    assert!(!is_mutating_operation(&msg_default.operation));

    // Full params (still one-time read)
    let msg_full = build_read_op(
        "/Sensor/Temp",
        &OmiReadParams {
            newest: Some(5),
            oldest: Some(1),
            begin: Some(100.0),
            end: Some(200.0),
            depth: Some(3),
        },
    );
    assert!(!is_mutating_operation(&msg_full.operation));
}

#[test]
fn write_is_mutating() {
    // Write
    let write = OmiMessage::parse(
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/A/B","v":42}}"#,
    )
    .unwrap();
    assert!(is_mutating_operation(&write.operation));

    // Delete
    let delete = OmiMessage::parse(
        r#"{"omi":"1.0","ttl":0,"delete":{"path":"/A"}}"#,
    )
    .unwrap();
    assert!(is_mutating_operation(&delete.operation));

    // Cancel
    let cancel = OmiMessage::parse(
        r#"{"omi":"1.0","ttl":0,"cancel":{"rid":["req-1"]}}"#,
    )
    .unwrap();
    assert!(is_mutating_operation(&cancel.operation));

    // Subscription (read with interval)
    let sub = OmiMessage::parse(
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/A/B","interval":10.0}}"#,
    )
    .unwrap();
    assert!(is_mutating_operation(&sub.operation));
}
