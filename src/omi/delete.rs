use super::error::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteOp {
    pub path: String,
}

impl DeleteOp {
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

}
