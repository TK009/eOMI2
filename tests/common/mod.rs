//! Shared helpers for integration tests.

#![allow(dead_code)]

use reconfigurable_device::device;
use reconfigurable_device::omi::{Engine, OmiMessage, Operation, ResponseResult};

/// Build an engine pre-populated with the real DHT11 sensor tree.
pub fn engine_with_sensor_tree() -> Engine {
    let mut e = Engine::new();
    e.tree.write_tree("/", device::build_sensor_tree()).unwrap();
    e
}

/// Parse a JSON request, feed it to the engine at a given time/session, return response.
pub fn process_at(engine: &mut Engine, json: &str, now: f64, ws_session: Option<u64>) -> OmiMessage {
    let msg = OmiMessage::parse(json).expect("request JSON should parse");
    engine.process(msg, now, ws_session)
}

/// Extract the HTTP-style status code from a response message.
pub fn response_status(resp: &OmiMessage) -> u16 {
    match &resp.operation {
        Operation::Response(body) => body.status,
        _ => panic!("expected Response"),
    }
}

/// Extract the `Single` result value from a 200 response.
pub fn extract_single_result(resp: &OmiMessage) -> &serde_json::Value {
    match &resp.operation {
        Operation::Response(body) => match &body.result {
            Some(ResponseResult::Single(v)) => v,
            other => panic!("expected Single result, got {:?}", other),
        },
        _ => panic!("expected Response"),
    }
}

/// Extract the subscription rid from a response.
pub fn response_rid(resp: &OmiMessage) -> &str {
    match &resp.operation {
        Operation::Response(body) => body.rid.as_deref().expect("expected rid in response"),
        _ => panic!("expected Response"),
    }
}
