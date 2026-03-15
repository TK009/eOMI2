use crate::odf::Value;
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
    /// Read result: path + values + optional metadata (for InfoItem reads and subscription events).
    ReadValues { path: String, values: Vec<Value>, meta: Option<std::collections::BTreeMap<String, crate::odf::OmiValue>> },
    /// Pre-rendered JSON string (only available with lite-json, for object subtree reads).
    #[cfg(feature = "lite-json")]
    JsonString(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResponseBody {
    pub status: u16,
    pub rid: Option<String>,
    pub desc: Option<String>,
    pub result: Option<ResponseResult>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResponseResult {
    Batch(Vec<ItemStatus>),
    Single(ResultPayload),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemStatus {
    pub path: String,
    pub status: u16,
    pub desc: Option<String>,
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
        Self::ok_read_result_with_meta(path, values, None)
    }

    pub fn ok_read_result_with_meta(path: String, values: Vec<Value>, meta: Option<std::collections::BTreeMap<String, crate::odf::OmiValue>>) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: None,
            desc: None,
            result: Some(ResponseResult::Single(ResultPayload::ReadValues { path, values, meta })),
        })
    }

    /// OK response with pre-rendered JSON string (requires `lite-json` feature).
    #[cfg(feature = "lite-json")]
    pub fn ok_json_string(result: String) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: None,
            desc: None,
            result: Some(ResponseResult::Single(ResultPayload::JsonString(result))),
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
                meta: None,
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
                    Some(ResponseResult::Single(ResultPayload::ReadValues { path, values, .. })) => {
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
                    Some(ResponseResult::Single(ResultPayload::ReadValues { path, values, .. })) => {
                        assert_eq!(path, "/Device/Sensor");
                        assert_eq!(values.len(), 2);
                    }
                    _ => panic!("expected ReadValues result"),
                }
            }
            _ => panic!("expected Response"),
        }
    }

}
