use std::fmt;

#[derive(Debug, PartialEq)]
pub enum ParseError {
    InvalidJson(String),
    MissingField(&'static str),
    InvalidField { field: &'static str, reason: String },
    InvalidOperationCount(usize),
    UnsupportedVersion(String),
    MutuallyExclusive(&'static str, &'static str),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::InvalidJson(msg) => write!(f, "Invalid JSON: {}", msg),
            ParseError::MissingField(field) => write!(f, "Missing field: {}", field),
            ParseError::InvalidField { field, reason } => {
                write!(f, "Invalid field '{}': {}", field, reason)
            }
            ParseError::InvalidOperationCount(n) => {
                write!(f, "Expected exactly 1 operation, found {}", n)
            }
            ParseError::UnsupportedVersion(v) => {
                write!(f, "Unsupported OMI version: {}", v)
            }
            ParseError::MutuallyExclusive(a, b) => {
                write!(f, "Fields '{}' and '{}' are mutually exclusive", a, b)
            }
        }
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_invalid_json() {
        let e = ParseError::InvalidJson("unexpected EOF".into());
        assert_eq!(e.to_string(), "Invalid JSON: unexpected EOF");
    }

    #[test]
    fn display_missing_field() {
        let e = ParseError::MissingField("ttl");
        assert_eq!(e.to_string(), "Missing field: ttl");
    }

    #[test]
    fn display_invalid_field() {
        let e = ParseError::InvalidField {
            field: "path",
            reason: "must start with /".into(),
        };
        assert_eq!(e.to_string(), "Invalid field 'path': must start with /");
    }

    #[test]
    fn display_invalid_operation_count() {
        let e = ParseError::InvalidOperationCount(2);
        assert_eq!(e.to_string(), "Expected exactly 1 operation, found 2");
    }

    #[test]
    fn display_unsupported_version() {
        let e = ParseError::UnsupportedVersion("2.0".into());
        assert_eq!(e.to_string(), "Unsupported OMI version: 2.0");
    }

    #[test]
    fn display_mutually_exclusive() {
        let e = ParseError::MutuallyExclusive("path", "rid");
        assert_eq!(
            e.to_string(),
            "Fields 'path' and 'rid' are mutually exclusive"
        );
    }

    #[test]
    fn equality() {
        assert_eq!(ParseError::MissingField("a"), ParseError::MissingField("a"));
        assert_ne!(ParseError::MissingField("a"), ParseError::MissingField("b"));
    }
}
