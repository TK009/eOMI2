use super::error::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub enum ReadKind {
    OneTime,
    Subscription,
    Poll,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadOp {
    pub path: Option<String>,
    pub rid: Option<String>,
    pub newest: Option<u64>,
    pub oldest: Option<u64>,
    pub begin: Option<f64>,
    pub end: Option<f64>,
    pub depth: Option<u64>,
    pub interval: Option<f64>,
    pub callback: Option<String>,
}

impl ReadOp {
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

}
