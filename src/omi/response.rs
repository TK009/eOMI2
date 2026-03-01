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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseBody {
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ResponseResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseResult {
    Batch(Vec<ItemStatus>),
    Single(serde_json::Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemStatus {
    pub path: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
}

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

    pub fn ok(result: serde_json::Value) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: None,
            desc: None,
            result: Some(ResponseResult::Single(result)),
        })
    }

    pub fn ok_with_rid(rid: String, result: serde_json::Value) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: Some(rid),
            desc: None,
            result: Some(ResponseResult::Single(result)),
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
    #[cfg(feature = "json")]
    pub fn subscription_event(rid: &str, path: &str, values: &[Value]) -> OmiMessage {
        Self::wrap(ResponseBody {
            status: StatusCode::Ok.as_u16(),
            rid: Some(rid.to_string()),
            desc: None,
            result: Some(ResponseResult::Single(serde_json::json!({
                "path": path,
                "values": values,
            }))),
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
        fn serialize_payload_too_large() {
            let msg = OmiResponse::payload_too_large("too big");
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["omi"], "1.0");
            assert_eq!(json["ttl"], 0);
            assert_eq!(json["response"]["status"], 413);
            assert_eq!(json["response"]["desc"], "too big");
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
        fn serialize_ok_response() {
            let msg = OmiResponse::ok(serde_json::json!({"v": 42}));
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["omi"], "1.0");
            assert_eq!(json["ttl"], 0);
            assert_eq!(json["response"]["status"], 200);
            assert_eq!(json["response"]["result"]["v"], 42);
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
    }
}
