use serde::{Deserialize, Serialize};

use super::error::ParseError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelOp {
    pub rid: Vec<String>,
}

impl CancelOp {
    pub fn from_value(value: serde_json::Value) -> Result<Self, ParseError> {
        let op: CancelOp = serde_json::from_value(value)
            .map_err(|e| ParseError::InvalidJson(e.to_string()))?;
        op.validate()?;
        Ok(op)
    }

    pub fn validate(&self) -> Result<(), ParseError> {
        if self.rid.is_empty() {
            return Err(ParseError::InvalidField {
                field: "rid",
                reason: "rid array must not be empty".into(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_rid() {
        let op = CancelOp {
            rid: vec!["req-1".into(), "req-2".into()],
        };
        assert!(op.validate().is_ok());
    }

    #[test]
    fn reject_empty_rid() {
        let op = CancelOp { rid: vec![] };
        let err = op.validate().unwrap_err();
        assert_eq!(
            err,
            ParseError::InvalidField {
                field: "rid",
                reason: "rid array must not be empty".into(),
            }
        );
    }

    #[cfg(feature = "json")]
    mod json {
        use super::*;

        #[test]
        fn from_value_valid() {
            let v = serde_json::json!({ "rid": ["req-1", "req-2"] });
            let op = CancelOp::from_value(v).unwrap();
            assert_eq!(op.rid, vec!["req-1", "req-2"]);
        }

        #[test]
        fn from_value_empty_array() {
            let v = serde_json::json!({ "rid": [] });
            let err = CancelOp::from_value(v).unwrap_err();
            assert!(matches!(err, ParseError::InvalidField { .. }));
        }

        #[test]
        fn from_value_missing_rid() {
            let v = serde_json::json!({});
            let err = CancelOp::from_value(v).unwrap_err();
            assert!(matches!(err, ParseError::InvalidJson(_)));
        }

        #[test]
        fn serialize_roundtrip() {
            let op = CancelOp {
                rid: vec!["abc".into()],
            };
            let json = serde_json::to_value(&op).unwrap();
            let op2 = CancelOp::from_value(json).unwrap();
            assert_eq!(op, op2);
        }
    }
}
