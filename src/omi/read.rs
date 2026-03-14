#[cfg(feature = "json")]
use serde::{Deserialize, Serialize};

use super::error::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub enum ReadKind {
    OneTime,
    Subscription,
    Poll,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(Serialize, Deserialize))]
pub struct ReadOp {
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub path: Option<String>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub rid: Option<String>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub newest: Option<u64>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub oldest: Option<u64>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub begin: Option<f64>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub end: Option<f64>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub depth: Option<u64>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub interval: Option<f64>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub callback: Option<String>,
}

impl ReadOp {
    #[cfg(feature = "json")]
    pub fn from_value(value: serde_json::Value) -> Result<Self, ParseError> {
        let op: ReadOp = serde_json::from_value(value)
            .map_err(|e| ParseError::InvalidJson(e.to_string()))?;
        op.validate()?;
        Ok(op)
    }

    pub fn validate(&self) -> Result<(), ParseError> {
        match (&self.path, &self.rid) {
            (Some(_), Some(_)) => {
                return Err(ParseError::MutuallyExclusive("path", "rid"));
            }
            (None, None) => {
                return Err(ParseError::MissingField("path or rid"));
            }
            _ => {}
        }
        Ok(())
    }

    pub fn kind(&self) -> ReadKind {
        if self.rid.is_some() {
            ReadKind::Poll
        } else if self.interval.is_some() || self.callback.is_some() {
            ReadKind::Subscription
        } else {
            ReadKind::OneTime
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_time(path: &str) -> ReadOp {
        ReadOp {
            path: Some(path.into()),
            rid: None,
            newest: None,
            oldest: None,
            begin: None,
            end: None,
            depth: None,
            interval: None,
            callback: None,
        }
    }

    #[test]
    fn validate_path_only() {
        let op = one_time("/DeviceA/Temperature");
        assert!(op.validate().is_ok());
    }

    #[test]
    fn validate_rid_only() {
        let op = ReadOp {
            path: None,
            rid: Some("req-1".into()),
            newest: None,
            oldest: None,
            begin: None,
            end: None,
            depth: None,
            interval: None,
            callback: None,
        };
        assert!(op.validate().is_ok());
    }

    #[test]
    fn reject_both_path_and_rid() {
        let op = ReadOp {
            path: Some("/A".into()),
            rid: Some("req-1".into()),
            newest: None,
            oldest: None,
            begin: None,
            end: None,
            depth: None,
            interval: None,
            callback: None,
        };
        assert_eq!(
            op.validate().unwrap_err(),
            ParseError::MutuallyExclusive("path", "rid")
        );
    }

    #[test]
    fn reject_neither_path_nor_rid() {
        let op = ReadOp {
            path: None,
            rid: None,
            newest: None,
            oldest: None,
            begin: None,
            end: None,
            depth: None,
            interval: None,
            callback: None,
        };
        assert_eq!(
            op.validate().unwrap_err(),
            ParseError::MissingField("path or rid")
        );
    }

    #[test]
    fn kind_one_time() {
        let op = one_time("/A");
        assert_eq!(op.kind(), ReadKind::OneTime);
    }

    #[test]
    fn kind_subscription_interval() {
        let mut op = one_time("/A");
        op.interval = Some(5.0);
        assert_eq!(op.kind(), ReadKind::Subscription);
    }

    #[test]
    fn kind_subscription_callback() {
        let mut op = one_time("/A");
        op.callback = Some("http://example.com/cb".into());
        assert_eq!(op.kind(), ReadKind::Subscription);
    }

    #[test]
    fn kind_poll() {
        let op = ReadOp {
            path: None,
            rid: Some("req-1".into()),
            newest: None,
            oldest: None,
            begin: None,
            end: None,
            depth: None,
            interval: None,
            callback: None,
        };
        assert_eq!(op.kind(), ReadKind::Poll);
    }

    #[cfg(feature = "json")]
    mod json {
        use super::*;

        #[test]
        fn from_value_one_time() {
            let v = serde_json::json!({
                "path": "/DeviceA/Temperature",
                "newest": 5
            });
            let op = ReadOp::from_value(v).unwrap();
            assert_eq!(op.path.as_deref(), Some("/DeviceA/Temperature"));
            assert_eq!(op.newest, Some(5));
            assert_eq!(op.kind(), ReadKind::OneTime);
        }

        #[test]
        fn from_value_subscription() {
            let v = serde_json::json!({
                "path": "/DeviceA/Temperature",
                "interval": 10.0,
                "callback": "http://example.com/cb"
            });
            let op = ReadOp::from_value(v).unwrap();
            assert_eq!(op.kind(), ReadKind::Subscription);
            assert_eq!(op.interval, Some(10.0));
            assert_eq!(op.callback.as_deref(), Some("http://example.com/cb"));
        }

        #[test]
        fn from_value_poll() {
            let v = serde_json::json!({
                "rid": "req-123"
            });
            let op = ReadOp::from_value(v).unwrap();
            assert_eq!(op.kind(), ReadKind::Poll);
            assert_eq!(op.rid.as_deref(), Some("req-123"));
        }

        #[test]
        fn from_value_reject_both() {
            let v = serde_json::json!({
                "path": "/A",
                "rid": "req-1"
            });
            let err = ReadOp::from_value(v).unwrap_err();
            assert_eq!(err, ParseError::MutuallyExclusive("path", "rid"));
        }

        #[test]
        fn from_value_reject_neither() {
            let v = serde_json::json!({});
            let err = ReadOp::from_value(v).unwrap_err();
            assert_eq!(err, ParseError::MissingField("path or rid"));
        }

        #[test]
        fn serialize_omits_none_fields() {
            let op = one_time("/A");
            let json = serde_json::to_value(&op).unwrap();
            assert_eq!(json, serde_json::json!({ "path": "/A" }));
        }

        #[test]
        fn serialize_roundtrip() {
            let mut op = one_time("/DeviceA/Temp");
            op.newest = Some(10);
            op.depth = Some(2);
            let json = serde_json::to_value(&op).unwrap();
            let op2 = ReadOp::from_value(json).unwrap();
            assert_eq!(op, op2);
        }
    }
}
