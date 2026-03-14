#[cfg(feature = "json")]
use serde::{Deserialize, Serialize};

use crate::odf::Value;
use super::error::ParseError;
use super::{OmiMessage, Operation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCode {
    Ok = 200,
    Created = 201,
    BadRequest = 400,
    Unauthorized = 401,
    Forbidden = 403,
    NotFound = 404,
    Timeout = 408,
    PayloadTooLarge = 413,
    InternalError = 500,
    NotImplemented = 501,
}

impl StatusCode {
    pub fn as_u16(self) -> u16 {
        self as u16
    }

    pub fn desc(self) -> &'static str {
        match self {
            StatusCode::Ok => "OK",
            StatusCode::Created => "Created",
            StatusCode::BadRequest => "Bad Request",
            StatusCode::Unauthorized => "Unauthorized",
            StatusCode::Forbidden => "Forbidden",
            StatusCode::NotFound => "Not Found",
            StatusCode::Timeout => "Timeout",
            StatusCode::PayloadTooLarge => "Payload Too Large",
            StatusCode::InternalError => "Internal Server Error",
            StatusCode::NotImplemented => "Not Implemented",
        }
    }
}

/// Protocol-specific result payload, replacing `serde_json::Value`.
#[derive(Debug, Clone, PartialEq)]
pub enum ResultPayload {
    /// Null/empty result (e.g., write acknowledgments).
    Null,
    /// Read result: path + values (for InfoItem reads and subscription events).
    ReadValues { path: String, values: Vec<Value> },
    /// Arbitrary JSON value (only available with serde_json).
    #[cfg(feature = "json")]
    Json(serde_json::Value),
}

#[cfg(feature = "json")]
impl Serialize for ResultPayload {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ResultPayload::Null => serializer.serialize_none(),
            ResultPayload::ReadValues { path, values } => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("path", path)?;
                map.serialize_entry("values", values)?;
                map.end()
            }
            ResultPayload::Json(v) => v.serialize(serializer),
        }
    }
}

#[cfg(feature = "json")]
impl<'de> Deserialize<'de> for ResultPayload {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(deserializer)?;
        Ok(ResultPayload::Json(v))
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(Serialize, Deserialize))]
pub struct ResponseBody {
    pub status: u16,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub rid: Option<String>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub desc: Option<String>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub result: Option<ResponseResult>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "json", serde(untagged))]
pub enum ResponseResult {
    Batch(Vec<ItemStatus>),
    Single(ResultPayload),
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(Serialize, Deserialize))]
pub struct ItemStatus {
    pub path: String,
    pub status: u16,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub desc: Option<String>,
}

#[cfg(feature = "json")]
impl ResponseBody {
    pub fn from_value(value: serde_json::Value) -> Result<Self, ParseError> {
        serde_json::from_value(value).map_err(|e| ParseError::InvalidJson(e.to_string()))
    }
}

/// Helper for building common OMI response messages.
pub struct OmiResponse;

impl OmiResponse {
    fn wrap(body: ResponseBody) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Response(body),
        }
    }

    /// OK response with null result (acknowledgment).
    pub fn ok_null() -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: None,
            desc: None,
            result: Some(ResponseResult::Single(ResultPayload::Null)),
        })
    }

    /// OK response with a read result (path + values).
    pub fn ok_read_result(path: String, values: Vec<Value>) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: None,
            desc: None,
            result: Some(ResponseResult::Single(ResultPayload::ReadValues { path, values })),
        })
    }

    /// OK response with arbitrary JSON (requires `json` feature).
    #[cfg(feature = "json")]
    pub fn ok(result: serde_json::Value) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: None,
            desc: None,
            result: Some(ResponseResult::Single(ResultPayload::Json(result))),
        })
    }

    /// OK response with rid and null result.
    pub fn ok_with_rid_null(rid: String) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: Some(rid),
            desc: None,
            result: Some(ResponseResult::Single(ResultPayload::Null)),
        })
    }

    /// OK response with rid and arbitrary JSON (requires `json` feature).
    #[cfg(feature = "json")]
    pub fn ok_with_rid(rid: String, result: serde_json::Value) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: Some(rid),
            desc: None,
            result: Some(ResponseResult::Single(ResultPayload::Json(result))),
        })
    }

    /// Write succeeded but the onwrite script failed (FR-005).
    /// Returns status 200 with a warning description.
    pub fn write_ok_with_warning(desc: &str) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: None,
            desc: Some(desc.into()),
            result: None,
        })
    }

    pub fn created() -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Created.as_u16(),
            rid: None,
            desc: Some(StatusCode::Created.desc().into()),
            result: None,
        })
    }

    pub fn not_found(path: &str) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::NotFound.as_u16(),
            rid: None,
            desc: Some(format!("Path not found: {}", path)),
            result: None,
        })
    }

    pub fn forbidden(desc: &str) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Forbidden.as_u16(),
            rid: None,
            desc: Some(desc.into()),
            result: None,
        })
    }

    pub fn bad_request(desc: &str) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::BadRequest.as_u16(),
            rid: None,
            desc: Some(desc.into()),
            result: None,
        })
    }

    pub fn unauthorized(desc: &str) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Unauthorized.as_u16(),
            rid: None,
            desc: Some(desc.into()),
            result: None,
        })
    }

    pub fn payload_too_large(desc: &str) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::PayloadTooLarge.as_u16(),
            rid: None,
            desc: Some(desc.into()),
            result: None,
        })
    }

    pub fn error(desc: &str) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::InternalError.as_u16(),
            rid: None,
            desc: Some(desc.into()),
            result: None,
        })
    }

    pub fn not_implemented(desc: &str) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::NotImplemented.as_u16(),
            rid: None,
            desc: Some(desc.into()),
            result: None,
        })
    }

    pub fn timeout() -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Timeout.as_u16(),
            rid: None,
            desc: Some(StatusCode::Timeout.desc().into()),
            result: None,
        })
    }

    pub fn partial_batch(items: Vec<ItemStatus>) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: None,
            desc: None,
            result: Some(ResponseResult::Batch(items)),
        })
    }

    /// Build a subscription event delivery message (for WebSocket push).
    pub fn subscription_event(rid: &str, path: &str, values: &[Value]) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: Some(rid.to_string()),
            desc: None,
            result: Some(ResponseResult::Single(ResultPayload::ReadValues {
                path: path.to_string(),
                values: values.to_vec(),
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_code_values() {
        assert_eq!(StatusCode::Ok.as_u16(), 200);
        assert_eq!(StatusCode::Created.as_u16(), 201);
        assert_eq!(StatusCode::BadRequest.as_u16(), 400);
        assert_eq!(StatusCode::Unauthorized.as_u16(), 401);
        assert_eq!(StatusCode::Forbidden.as_u16(), 403);
        assert_eq!(StatusCode::NotFound.as_u16(), 404);
        assert_eq!(StatusCode::Timeout.as_u16(), 408);
        assert_eq!(StatusCode::PayloadTooLarge.as_u16(), 413);
        assert_eq!(StatusCode::InternalError.as_u16(), 500);
        assert_eq!(StatusCode::NotImplemented.as_u16(), 501);
    }

    #[test]
    fn status_code_desc() {
        assert_eq!(StatusCode::Ok.desc(), "OK");
        assert_eq!(StatusCode::NotFound.desc(), "Not Found");
    }

    // -- Feature-independent builder tests --

    #[test]
    fn ok_null_response() {
        let msg = OmiResponse::ok_null();
        assert_eq!(msg.version, "1.0");
        assert_eq!(msg.ttl, 0);
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 200);
                match &body.result {
                    Some(ResponseResult::Single(ResultPayload::Null)) => {}
                    _ => panic!("expected Single(Null) result"),
                }
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn ok_read_result_response() {
        use crate::odf::OmiValue;
        let values = vec![
            Value::new(OmiValue::Number(22.5), Some(1000.0)),
        ];
        let msg = OmiResponse::ok_read_result("/Device/Temp".into(), values);
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 200);
                match &body.result {
                    Some(ResponseResult::Single(ResultPayload::ReadValues { path, values })) => {
                        assert_eq!(path, "/Device/Temp");
                        assert_eq!(values.len(), 1);
                    }
                    _ => panic!("expected ReadValues result"),
                }
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn ok_with_rid_null_response() {
        let msg = OmiResponse::ok_with_rid_null("req-1".into());
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.rid.as_deref(), Some("req-1"));
                match &body.result {
                    Some(ResponseResult::Single(ResultPayload::Null)) => {}
                    _ => panic!("expected Single(Null) result"),
                }
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn created_response() {
        let msg = OmiResponse::created();
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 201);
                assert_eq!(body.desc.as_deref(), Some("Created"));
                assert!(body.result.is_none());
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn not_found_response() {
        let msg = OmiResponse::not_found("/DeviceA/Missing");
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 404);
                assert_eq!(
                    body.desc.as_deref(),
                    Some("Path not found: /DeviceA/Missing")
                );
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn forbidden_response() {
        let msg = OmiResponse::forbidden("access denied");
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 403);
                assert_eq!(body.desc.as_deref(), Some("access denied"));
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn bad_request_response() {
        let msg = OmiResponse::bad_request("invalid path");
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 400);
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn unauthorized_response() {
        let msg = OmiResponse::unauthorized("missing token");
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 401);
                assert_eq!(body.desc.as_deref(), Some("missing token"));
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn payload_too_large_response() {
        let msg = OmiResponse::payload_too_large("Message too large");
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 413);
                assert_eq!(body.desc.as_deref(), Some("Message too large"));
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn error_response() {
        let msg = OmiResponse::error("something broke");
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 500);
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn not_implemented_response() {
        let msg = OmiResponse::not_implemented("subscriptions");
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 501);
                assert_eq!(body.desc.as_deref(), Some("subscriptions"));
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn write_ok_with_warning_response() {
        let msg = OmiResponse::write_ok_with_warning("script exceeded operation limit");
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 200);
                assert_eq!(
                    body.desc.as_deref(),
                    Some("script exceeded operation limit")
                );
                assert!(body.result.is_none());
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn timeout_response() {
        let msg = OmiResponse::timeout();
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 408);
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn partial_batch_response() {
        let items = vec![
            ItemStatus {
                path: "/A/B".into(),
                status: 200,
                desc: None,
            },
            ItemStatus {
                path: "/A/C".into(),
                status: 404,
                desc: Some("not found".into()),
            },
        ];
        let msg = OmiResponse::partial_batch(items);
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 200);
                match &body.result {
                    Some(ResponseResult::Batch(items)) => {
                        assert_eq!(items.len(), 2);
                        assert_eq!(items[0].status, 200);
                        assert_eq!(items[1].status, 404);
                    }
                    _ => panic!("expected Batch result"),
                }
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn subscription_event_format() {
        use crate::odf::OmiValue;
        let values = vec![
            Value::new(OmiValue::Number(22.5), None),
            Value::new(OmiValue::Number(60.0), Some(1000.0)),
        ];
        let msg = OmiResponse::subscription_event("sub-1", "/Device/Sensor", &values);
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 200);
                assert_eq!(body.rid.as_deref(), Some("sub-1"));
                assert!(body.desc.is_none());
                match &body.result {
                    Some(ResponseResult::Single(ResultPayload::ReadValues { path, values })) => {
                        assert_eq!(path, "/Device/Sensor");
                        assert_eq!(values.len(), 2);
                    }
                    _ => panic!("expected ReadValues result"),
                }
            }
            _ => panic!("expected Response"),
        }
    }

    #[cfg(feature = "json")]
    mod json {
        use super::*;

        #[test]
        fn ok_response() {
            let msg = OmiResponse::ok(serde_json::json!({"temperature": 22.5}));
            assert_eq!(msg.version, "1.0");
            assert_eq!(msg.ttl, 0);
            match &msg.operation {
                Operation::Response(body) => {
                    assert_eq!(body.status, 200);
                    assert!(body.result.is_some());
                }
                _ => panic!("expected Response"),
            }
        }

        #[test]
        fn ok_with_rid_response() {
            let msg = OmiResponse::ok_with_rid("req-1".into(), serde_json::json!(null));
            match &msg.operation {
                Operation::Response(body) => {
                    assert_eq!(body.rid.as_deref(), Some("req-1"));
                }
                _ => panic!("expected Response"),
            }
        }

        #[test]
        fn serialize_payload_too_large() {
            let msg = OmiResponse::payload_too_large("too big");
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["omi"], "1.0");
            assert_eq!(json["ttl"], 0);
            assert_eq!(json["response"]["status"], 413);
            assert_eq!(json["response"]["desc"], "too big");
        }

        #[test]
        fn serialize_ok_response() {
            let msg = OmiResponse::ok(serde_json::json!({"v": 42}));
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["omi"], "1.0");
            assert_eq!(json["ttl"], 0);
            assert_eq!(json["response"]["status"], 200);
            assert_eq!(json["response"]["result"]["v"], 42);
        }

        #[test]
        fn serialize_ok_null_response() {
            let msg = OmiResponse::ok_null();
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["response"]["status"], 200);
            assert!(json["response"]["result"].is_null());
        }

        #[test]
        fn serialize_ok_read_result() {
            use crate::odf::OmiValue;
            let values = vec![
                Value::new(OmiValue::Number(22.5), Some(1000.0)),
            ];
            let msg = OmiResponse::ok_read_result("/Device/Temp".into(), values);
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["response"]["status"], 200);
            assert_eq!(json["response"]["result"]["path"], "/Device/Temp");
            let vals = json["response"]["result"]["values"].as_array().unwrap();
            assert_eq!(vals.len(), 1);
            assert_eq!(vals[0]["v"], 22.5);
            assert_eq!(vals[0]["t"], 1000.0);
        }

        #[test]
        fn serialize_not_found_response() {
            let msg = OmiResponse::not_found("/X");
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["response"]["status"], 404);
            assert_eq!(json["response"]["desc"], "Path not found: /X");
        }

        #[test]
        fn serialize_batch_response() {
            let items = vec![
                ItemStatus { path: "/A".into(), status: 200, desc: None },
                ItemStatus { path: "/B".into(), status: 404, desc: Some("gone".into()) },
            ];
            let msg = OmiResponse::partial_batch(items);
            let json = serde_json::to_value(&msg).unwrap();
            let result = json["response"]["result"].as_array().unwrap();
            assert_eq!(result.len(), 2);
            assert_eq!(result[0]["path"], "/A");
            assert_eq!(result[1]["desc"], "gone");
        }

        #[test]
        fn serialize_subscription_event() {
            use crate::odf::OmiValue;
            let values = vec![
                Value::new(OmiValue::Number(22.5), None),
                Value::new(OmiValue::Number(60.0), Some(1000.0)),
            ];
            let msg = OmiResponse::subscription_event("sub-1", "/Device/Sensor", &values);
            let json = serde_json::to_value(&msg).unwrap();

            assert_eq!(json["omi"], "1.0");
            assert_eq!(json["ttl"], 0);
            assert_eq!(json["response"]["status"], 200);
            assert_eq!(json["response"]["rid"], "sub-1");
            assert!(json["response"]["desc"].is_null());

            let result = &json["response"]["result"];
            assert_eq!(result["path"], "/Device/Sensor");
            let vals = result["values"].as_array().unwrap();
            assert_eq!(vals.len(), 2);
            assert_eq!(vals[0]["v"], 22.5);
            assert!(vals[0]["t"].is_null());
            assert_eq!(vals[1]["v"], 60.0);
            assert_eq!(vals[1]["t"], 1000.0);
        }

        #[test]
        fn response_body_from_value() {
            let v = serde_json::json!({
                "status": 200,
                "desc": "OK",
                "result": { "temperature": 22.5 }
            });
            let body = ResponseBody::from_value(v).unwrap();
            assert_eq!(body.status, 200);
            assert!(body.result.is_some());
        }

        #[test]
        fn roundtrip_response_message() {
            let msg = OmiResponse::ok(serde_json::json!({"data": [1, 2, 3]}));
            let json = serde_json::to_string(&msg).unwrap();
            let msg2 = OmiMessage::parse(&json).unwrap();
            assert_eq!(msg2.version, "1.0");
            assert_eq!(msg2.ttl, 0);
            match msg2.operation {
                Operation::Response(body) => {
                    assert_eq!(body.status, 200);
                }
                _ => panic!("expected Response"),
            }
        }

        #[test]
        fn roundtrip_ok_null() {
            // Null result serializes as JSON null, which serde deserializes
            // as None for Option<ResponseResult>. This is expected behavior.
            let msg = OmiResponse::ok_null();
            let json = serde_json::to_string(&msg).unwrap();
            let msg2 = OmiMessage::parse(&json).unwrap();
            match msg2.operation {
                Operation::Response(body) => {
                    assert_eq!(body.status, 200);
                    assert!(body.result.is_none());
                }
                _ => panic!("expected Response"),
            }
        }

        #[test]
        fn roundtrip_read_result() {
            use crate::odf::OmiValue;
            let values = vec![
                Value::new(OmiValue::Number(22.5), Some(1000.0)),
            ];
            let msg = OmiResponse::ok_read_result("/Device/Temp".into(), values);
            let json = serde_json::to_string(&msg).unwrap();
            let msg2 = OmiMessage::parse(&json).unwrap();
            match msg2.operation {
                Operation::Response(body) => {
                    assert_eq!(body.status, 200);
                    assert!(body.result.is_some());
                }
                _ => panic!("expected Response"),
            }
        }
    }
}
