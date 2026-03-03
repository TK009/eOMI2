//! Shared helpers for integration tests.

#![allow(dead_code)]

use reconfigurable_device::device;
use reconfigurable_device::omi::{Engine, ItemStatus, OmiMessage, Operation, ResponseResult, SessionId};

/// Build an engine pre-populated with the real DHT11 sensor tree.
pub fn engine_with_sensor_tree() -> Engine {
    let mut e = Engine::new();
    e.tree.write_tree("/", device::build_sensor_tree()).unwrap();
    e
}

/// Parse a JSON request, feed it to the engine at a given time/session, return response.
pub fn process_at(engine: &mut Engine, json: &str, now: f64, ws_session: Option<SessionId>) -> OmiMessage {
    let msg = OmiMessage::parse(json).expect("request JSON should parse");
    engine.process(msg, now, ws_session)
}

/// Parse a JSON request string, feed it to the engine, and return the response.
pub fn parse_and_process(engine: &mut Engine, json: &str) -> OmiMessage {
    process_at(engine, json, 0.0, None)
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

/// Extract the batch item-status list from a response.
pub fn response_batch(resp: &OmiMessage) -> &[ItemStatus] {
    match &resp.operation {
        Operation::Response(body) => match &body.result {
            Some(ResponseResult::Batch(items)) => items,
            other => panic!("expected Batch result, got {:?}", other),
        },
        _ => panic!("expected Response"),
    }
}

/// Extract the `rid` field from a response (returned by subscription creation).
pub fn response_rid(resp: &OmiMessage) -> &str {
    match &resp.operation {
        Operation::Response(body) => {
            body.rid.as_deref().expect("expected rid in response")
        }
        _ => panic!("expected Response"),
    }
}

/// Serialize a response to JSON, re-parse it, and return the `serde_json::Value`
/// for the `response` envelope field — proving the serialization round-trip works.
pub fn roundtrip_response_json(resp: &OmiMessage) -> serde_json::Value {
    let json_str = serde_json::to_string(resp).expect("response should serialize");
    let v: serde_json::Value = serde_json::from_str(&json_str).expect("should re-parse");
    v["response"].clone()
}
