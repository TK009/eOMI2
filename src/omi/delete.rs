use serde::{Deserialize, Serialize};

use super::error::ParseError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteOp {
    pub path: String,
}

impl DeleteOp {
    pub fn from_value(value: serde_json::Value) -> Result<Self, ParseError> {
        let op: DeleteOp = serde_json::from_value(value)
            .map_err(|e| ParseError::InvalidJson(e.to_string()))?;
        op.validate()?;
        Ok(op)
    }

    pub fn validate(&self) -> Result<(), ParseError> {
        if !self.path.starts_with('/') {
            return Err(ParseError::InvalidField {
                field: "path",
                reason: "must start with '/'".into(),
            });
        }
        if self.path == "/" {
            return Err(ParseError::InvalidField {
                field: "path",
                reason: "cannot delete root '/'".into(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_path() {
        let op = DeleteOp {
            path: "/DeviceA/Temperature".into(),
        };
        assert!(op.validate().is_ok());
    }

    #[test]
    fn reject_root() {
        let op = DeleteOp {
            path: "/".into(),
        };
        let err = op.validate().unwrap_err();
        assert_eq!(
            err,
            ParseError::InvalidField {
                field: "path",
                reason: "cannot delete root '/'".into(),
            }
        );
    }

    #[test]
    fn reject_no_leading_slash() {
        let op = DeleteOp {
            path: "DeviceA".into(),
        };
        let err = op.validate().unwrap_err();
        assert_eq!(
            err,
            ParseError::InvalidField {
                field: "path",
                reason: "must start with '/'".into(),
            }
        );
    }

    #[cfg(feature = "json")]
    mod json {
        use super::*;

        #[test]
        fn from_value_valid() {
            let v = serde_json::json!({ "path": "/DeviceA/Temperature" });
            let op = DeleteOp::from_value(v).unwrap();
            assert_eq!(op.path, "/DeviceA/Temperature");
        }

        #[test]
        fn from_value_missing_path() {
            let v = serde_json::json!({});
            let err = DeleteOp::from_value(v).unwrap_err();
            assert!(matches!(err, ParseError::InvalidJson(_)));
        }

        #[test]
        fn from_value_reject_root() {
            let v = serde_json::json!({ "path": "/" });
            let err = DeleteOp::from_value(v).unwrap_err();
            assert!(matches!(err, ParseError::InvalidField { .. }));
        }

        #[test]
        fn serialize_roundtrip() {
            let op = DeleteOp {
                path: "/A/B".into(),
            };
            let json = serde_json::to_value(&op).unwrap();
            let op2 = DeleteOp::from_value(json).unwrap();
            assert_eq!(op, op2);
        }
    }
}
