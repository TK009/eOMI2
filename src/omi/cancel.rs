use super::error::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub struct CancelOp {
    pub rid: Vec<String>,
}

impl CancelOp {
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

}
