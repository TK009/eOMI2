#![cfg(any(feature = "json", feature = "lite-json"))]
//! Integration tests for HTTP helpers + Engine wiring.
//!
//! These tests verify the cross-module chain:
//! **URI + query params → ODF path → read operation → Engine → response**.
//! Unit tests in `src/http.rs` cover each function in isolation; these
//! integration tests wire them together with a real Engine.

mod common;

use reconfigurable_device::http::{
    build_read_op, is_mutating_operation, omi_uri_to_odf_path, render_landing_page, uri_path,
    uri_query, OmiReadParams,
};
use reconfigurable_device::odf::OmiValue;
use reconfigurable_device::omi::{Engine, OmiMessage};
use reconfigurable_device::pages::PageStore;

#[cfg(feature = "json")]
use common::extract_json_result;
use common::{engine_with_sensor_tree, extract_values, response_status};

/// Chain the full REST GET flow: URI → parse → build read op → engine.process.
fn get_omi(engine: &mut Engine, uri: &str) -> OmiMessage {
    let path = uri_path(uri);
    let query = uri_query(uri);
    // Trailing-slash flag is only used for Content-Type selection in the
    // HTTP layer; the Engine treats the path identically either way.
    let (odf_path, _trailing) = omi_uri_to_odf_path(path);
    let params = match query {
        Some(q) => OmiReadParams::from_query(q),
        None => OmiReadParams::default(),
    };
    let msg = build_read_op(odf_path, &params);
    let (resp, _) = engine.process(msg, 0.0, None);
    resp
}

// ===========================================================================
// 3.1  REST Discovery
// ===========================================================================

#[cfg(feature = "json")]
#[test]
fn get_omi_root() {
    let mut e = engine_with_sensor_tree();
    let resp = get_omi(&mut e, "/omi/");
    assert_eq!(response_status(&resp), 200);
    let result = extract_json_result(&resp);
    assert!(result["System"].is_object(), "root should contain System");
}

#[cfg(feature = "lite-json")]
#[test]
fn get_omi_root_lite() {
    use reconfigurable_device::omi::response::ResultPayload;
    let mut e = engine_with_sensor_tree();
    let resp = get_omi(&mut e, "/omi/");
    assert_eq!(response_status(&resp), 200);
    match extract_single_result(&resp) {
        ResultPayload::JsonString(s) => assert!(s.contains("System")),
        _ => panic!("expected JsonString"),
    }
}

#[cfg(feature = "json")]
#[test]
fn get_omi_object() {
    let mut e = engine_with_sensor_tree();
    let resp = get_omi(&mut e, "/omi/System/");
    assert_eq!(response_status(&resp), 200);
    let result = extract_json_result(&resp);
    assert_eq!(result["id"], "System");
    assert!(result["items"]["FreeHeap"].is_object());
}

#[cfg(feature = "lite-json")]
#[test]
fn get_omi_object_lite() {
    use reconfigurable_device::omi::response::ResultPayload;
    let mut e = engine_with_sensor_tree();
    let resp = get_omi(&mut e, "/omi/System/");
    assert_eq!(response_status(&resp), 200);
    match extract_single_result(&resp) {
        ResultPayload::JsonString(s) => {
            assert!(s.contains("\"id\""));
            assert!(s.contains("System"));
            assert!(s.contains("FreeHeap"));
        }
        _ => panic!("expected JsonString"),
    }
}

#[test]
fn get_omi_infoitem() {
    let mut e = engine_with_sensor_tree();
    let resp = get_omi(&mut e, "/omi/System/FreeHeap");
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert!(values.is_empty());
}

#[test]
fn get_omi_with_query_params() {
    let mut e = engine_with_sensor_tree();

    // Write 5 values into the sensor item.
    for i in 1..=5 {
        e.tree
            .write_value(
                "/System/FreeHeap",
                OmiValue::Number(20.0 + i as f64),
                Some(i as f64 * 100.0),
            )
            .unwrap();
    }

    let resp = get_omi(&mut e, "/omi/System/FreeHeap?newest=3&depth=1");
    assert_eq!(response_status(&resp), 200);
    let values = extract_values(&resp);
    assert_eq!(values.len(), 3);
    // Verify the 3 newest values were returned (newest-first order).
    assert_eq!(values[0].v, OmiValue::Number(25.0));
    assert_eq!(values[1].v, OmiValue::Number(24.0));
    assert_eq!(values[2].v, OmiValue::Number(23.0));
}

#[test]
fn get_omi_nonexistent_path() {
    let mut e = engine_with_sensor_tree();
    let resp = get_omi(&mut e, "/omi/NoSuchSensor/Item");
    assert_eq!(response_status(&resp), 404);
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

#[test]
fn landing_page_escapes_html_in_paths() {
    let mut store = PageStore::new();
    store.store("/x<script>alert(1)</script>", "<h1>XSS</h1>").unwrap();

    let html = render_landing_page(&store);
    assert!(!html.contains("<script>"), "path must be HTML-escaped");
    assert!(html.contains("&lt;script&gt;"));
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
    let msg = OmiMessage::parse(
        r#"{"omi":"1.0","ttl":10,"write":{"path":"/A/B","v":42}}"#,
    )
    .unwrap();
    assert!(is_mutating_operation(&msg.operation));
}

#[test]
fn delete_is_mutating() {
    let msg = OmiMessage::parse(
        r#"{"omi":"1.0","ttl":0,"delete":{"path":"/A"}}"#,
    )
    .unwrap();
    assert!(is_mutating_operation(&msg.operation));
}

#[test]
fn cancel_is_mutating() {
    let msg = OmiMessage::parse(
        r#"{"omi":"1.0","ttl":0,"cancel":{"rid":["req-1"]}}"#,
    )
    .unwrap();
    assert!(is_mutating_operation(&msg.operation));
}

#[test]
fn subscription_is_mutating() {
    let msg = OmiMessage::parse(
        r#"{"omi":"1.0","ttl":60,"read":{"path":"/A/B","interval":10.0}}"#,
    )
    .unwrap();
    assert!(is_mutating_operation(&msg.operation));
}
