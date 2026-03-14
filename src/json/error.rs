//! Error types for the lite-json parser.

use std::fmt;

/// Position information for error reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    /// Byte offset in the input.
    pub offset: usize,
}

impl Pos {
    pub fn new(offset: usize) -> Self {
        Self { offset }
    }
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "byte {}", self.offset)
    }
}

/// Error type for the lite-json lexer and parser.
#[derive(Debug, PartialEq)]
pub enum LiteParseError {
    /// Encountered an unexpected byte.
    UnexpectedChar { ch: u8, pos: Pos },
    /// Reached end of input unexpectedly.
    UnexpectedEof { pos: Pos },
    /// Invalid escape sequence in a string.
    InvalidEscape { pos: Pos },
    /// Invalid `\uXXXX` unicode escape.
    InvalidUnicodeEscape { pos: Pos },
    /// Invalid surrogate pair (lone surrogate or bad low surrogate).
    InvalidSurrogatePair { pos: Pos },
    /// Unterminated string literal.
    UnterminatedString { pos: Pos },
    /// Malformed number.
    InvalidNumber { pos: Pos },
    /// Invalid literal (expected `true`, `false`, or `null`).
    InvalidLiteral { pos: Pos },
    /// Expected a specific token but found something else.
    ExpectedToken { expected: &'static str, pos: Pos },
    /// A required field was missing from a JSON object.
    MissingField { field: &'static str, pos: Pos },
    /// Object/array nesting exceeds the configured limit.
    DepthExceeded { max: usize, pos: Pos },
    /// Valid JSON followed by unexpected trailing data.
    TrailingData { pos: Pos },
}

impl LiteParseError {
    /// Returns the byte-offset position of the error.
    pub fn pos(&self) -> Pos {
        match self {
            Self::UnexpectedChar { pos, .. }
            | Self::UnexpectedEof { pos }
            | Self::InvalidEscape { pos }
            | Self::InvalidUnicodeEscape { pos }
            | Self::InvalidSurrogatePair { pos }
            | Self::UnterminatedString { pos }
            | Self::InvalidNumber { pos }
            | Self::InvalidLiteral { pos }
            | Self::ExpectedToken { pos, .. }
            | Self::MissingField { pos, .. }
            | Self::DepthExceeded { pos, .. }
            | Self::TrailingData { pos } => *pos,
        }
    }
}

impl fmt::Display for LiteParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedChar { ch, pos } => {
                write!(f, "unexpected character '{}' at {}", *ch as char, pos)
            }
            Self::UnexpectedEof { pos } => write!(f, "unexpected end of input at {}", pos),
            Self::InvalidEscape { pos } => write!(f, "invalid escape sequence at {}", pos),
            Self::InvalidUnicodeEscape { pos } => {
                write!(f, "invalid unicode escape at {}", pos)
            }
            Self::InvalidSurrogatePair { pos } => {
                write!(f, "invalid surrogate pair at {}", pos)
            }
            Self::UnterminatedString { pos } => {
                write!(f, "unterminated string at {}", pos)
            }
            Self::InvalidNumber { pos } => write!(f, "invalid number at {}", pos),
            Self::InvalidLiteral { pos } => write!(f, "invalid literal at {}", pos),
            Self::ExpectedToken { expected, pos } => {
                write!(f, "expected {} at {}", expected, pos)
            }
            Self::MissingField { field, pos } => {
                write!(f, "missing required field '{}' at {}", field, pos)
            }
            Self::DepthExceeded { max, pos } => {
                write!(f, "nesting depth exceeds maximum of {} at {}", max, pos)
            }
            Self::TrailingData { pos } => {
                write!(f, "trailing data at {}", pos)
            }
        }
    }
}

impl std::error::Error for LiteParseError {}

// Convert lite-json parse errors into OMI-level ParseError.
// Available when the omi module is compiled (std + json or lite-json).
#[cfg(all(feature = "std", any(feature = "json", feature = "lite-json")))]
impl From<LiteParseError> for crate::omi::error::ParseError {
    fn from(e: LiteParseError) -> Self {
        use crate::omi::error::ParseError;
        match e {
            LiteParseError::MissingField { field, .. } => ParseError::MissingField(field),
            other => ParseError::InvalidJson(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unexpected_char_display() {
        let e = LiteParseError::UnexpectedChar { ch: b'x', pos: Pos::new(5) };
        assert_eq!(e.to_string(), "unexpected character 'x' at byte 5");
    }

    #[test]
    fn unexpected_eof_display() {
        let e = LiteParseError::UnexpectedEof { pos: Pos::new(10) };
        assert_eq!(e.to_string(), "unexpected end of input at byte 10");
    }

    #[test]
    fn invalid_escape_display() {
        let e = LiteParseError::InvalidEscape { pos: Pos::new(3) };
        assert_eq!(e.to_string(), "invalid escape sequence at byte 3");
    }

    #[test]
    fn invalid_unicode_escape_display() {
        let e = LiteParseError::InvalidUnicodeEscape { pos: Pos::new(7) };
        assert_eq!(e.to_string(), "invalid unicode escape at byte 7");
    }

    #[test]
    fn invalid_surrogate_pair_display() {
        let e = LiteParseError::InvalidSurrogatePair { pos: Pos::new(2) };
        assert_eq!(e.to_string(), "invalid surrogate pair at byte 2");
    }

    #[test]
    fn unterminated_string_display() {
        let e = LiteParseError::UnterminatedString { pos: Pos::new(0) };
        assert_eq!(e.to_string(), "unterminated string at byte 0");
    }

    #[test]
    fn invalid_number_display() {
        let e = LiteParseError::InvalidNumber { pos: Pos::new(15) };
        assert_eq!(e.to_string(), "invalid number at byte 15");
    }

    #[test]
    fn invalid_literal_display() {
        let e = LiteParseError::InvalidLiteral { pos: Pos::new(8) };
        assert_eq!(e.to_string(), "invalid literal at byte 8");
    }

    #[test]
    fn expected_token_display() {
        let e = LiteParseError::ExpectedToken { expected: "':'", pos: Pos::new(4) };
        assert_eq!(e.to_string(), "expected ':' at byte 4");
    }

    #[test]
    fn pos_accessor() {
        let e = LiteParseError::UnexpectedChar { ch: b'!', pos: Pos::new(42) };
        assert_eq!(e.pos(), Pos::new(42));
    }

    #[test]
    fn display_formats_are_parseable() {
        // Verify all variants produce non-empty display strings
        let variants: Vec<LiteParseError> = vec![
            LiteParseError::UnexpectedChar { ch: b'?', pos: Pos::new(0) },
            LiteParseError::UnexpectedEof { pos: Pos::new(0) },
            LiteParseError::InvalidEscape { pos: Pos::new(0) },
            LiteParseError::InvalidUnicodeEscape { pos: Pos::new(0) },
            LiteParseError::InvalidSurrogatePair { pos: Pos::new(0) },
            LiteParseError::UnterminatedString { pos: Pos::new(0) },
            LiteParseError::InvalidNumber { pos: Pos::new(0) },
            LiteParseError::InvalidLiteral { pos: Pos::new(0) },
            LiteParseError::ExpectedToken { expected: "test", pos: Pos::new(0) },
            LiteParseError::MissingField { field: "id", pos: Pos::new(0) },
            LiteParseError::DepthExceeded { max: 32, pos: Pos::new(0) },
            LiteParseError::TrailingData { pos: Pos::new(0) },
        ];
        for v in &variants {
            let msg = v.to_string();
            assert!(!msg.is_empty());
            assert!(msg.contains("byte 0"));
        }
    }

    #[test]
    fn equality() {
        let a = LiteParseError::UnexpectedEof { pos: Pos::new(5) };
        let b = LiteParseError::UnexpectedEof { pos: Pos::new(5) };
        let c = LiteParseError::UnexpectedEof { pos: Pos::new(6) };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    // -- From<LiteParseError> for ParseError conversion tests --

    #[cfg(all(feature = "std", any(feature = "json", feature = "lite-json")))]
    mod from_parse_error {
        use super::*;
        use crate::omi::error::ParseError;

        #[test]
        fn missing_field_maps_directly() {
            let e = LiteParseError::MissingField { field: "version", pos: Pos::new(0) };
            let pe: ParseError = e.into();
            assert_eq!(pe, ParseError::MissingField("version"));
        }

        #[test]
        fn unexpected_char_maps_to_invalid_json() {
            let e = LiteParseError::UnexpectedChar { ch: b'{', pos: Pos::new(3) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(ref s) if s.contains("unexpected character")));
        }

        #[test]
        fn unexpected_eof_maps_to_invalid_json() {
            let e = LiteParseError::UnexpectedEof { pos: Pos::new(10) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(_)));
        }

        #[test]
        fn invalid_escape_maps_to_invalid_json() {
            let e = LiteParseError::InvalidEscape { pos: Pos::new(1) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(_)));
        }

        #[test]
        fn invalid_unicode_escape_maps_to_invalid_json() {
            let e = LiteParseError::InvalidUnicodeEscape { pos: Pos::new(4) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(_)));
        }

        #[test]
        fn invalid_surrogate_pair_maps_to_invalid_json() {
            let e = LiteParseError::InvalidSurrogatePair { pos: Pos::new(6) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(_)));
        }

        #[test]
        fn unterminated_string_maps_to_invalid_json() {
            let e = LiteParseError::UnterminatedString { pos: Pos::new(0) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(_)));
        }

        #[test]
        fn invalid_number_maps_to_invalid_json() {
            let e = LiteParseError::InvalidNumber { pos: Pos::new(2) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(_)));
        }

        #[test]
        fn invalid_literal_maps_to_invalid_json() {
            let e = LiteParseError::InvalidLiteral { pos: Pos::new(0) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(_)));
        }

        #[test]
        fn expected_token_maps_to_invalid_json() {
            let e = LiteParseError::ExpectedToken { expected: "':'", pos: Pos::new(5) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(_)));
        }

        #[test]
        fn depth_exceeded_maps_to_invalid_json() {
            let e = LiteParseError::DepthExceeded { max: 32, pos: Pos::new(0) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(_)));
        }

        #[test]
        fn trailing_data_maps_to_invalid_json() {
            let e = LiteParseError::TrailingData { pos: Pos::new(100) };
            let pe: ParseError = e.into();
            assert!(matches!(pe, ParseError::InvalidJson(_)));
        }
    }
}
