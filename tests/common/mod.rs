//! Shared helpers for OMI integration tests.

#![allow(dead_code)]

use reconfigurable_device::omi::{Engine, ItemStatus, OmiMessage, Operation, ResponseResult};

/// Parse a JSON request string, feed it to the engine, and return the response.
pub fn parse_and_process(engine: &mut Engine, json: &str) -> OmiMessage {
    let msg = OmiMessage::parse(json).expect("request JSON should parse");
    engine.process(msg, 0.0, None)
}

/// Extract the HTTP-style status code from a response message.
pub fn response_status(resp: &OmiMessage) -> u16 {
    match &resp.operation {
        Operation::Response(body) => body.status,
        _ => panic!("expected Response"),
    }
}

/// Extract the `Single` result value from a 200 response.
pub fn response_result(resp: &OmiMessage) -> &serde_json::Value {
    match &resp.operation {
        Operation::Response(body) => match &body.result {
            Some(ResponseResult::Single(v)) => v,
            other => panic!("expected Single result, got {:?}", other),
        },
        _ => panic!("expected Response"),
    }
}

/// Extract the batch item-status list from a response.
pub fn response_batch(resp: &OmiMessage) -> &Vec<ItemStatus> {
    match &resp.operation {
        Operation::Response(body) => match &body.result {
            Some(ResponseResult::Batch(items)) => items,
            other => panic!("expected Batch result, got {:?}", other),
        },
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
