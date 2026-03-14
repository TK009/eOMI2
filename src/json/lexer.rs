//! JSON lexer / tokenizer for the lite-json parser.
//!
//! Byte-level tokenizer that handles all JSON token types, string escapes
//! (FR-005), whitespace, and position tracking for errors.

use super::error::{LiteParseError, Pos};

/// A JSON token produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// A JSON string value (after escape processing).
    String(std::string::String),
    /// An integer number that fits in i64.
    Integer(i64),
    /// A floating-point number (or integer that overflowed i64).
    Number(f64),
    /// A boolean value.
    Bool(bool),
    /// The `null` literal.
    Null,
    /// `{`
    ObjectStart,
    /// `}`
    ObjectEnd,
    /// `[`
    ArrayStart,
    /// `]`
    ArrayEnd,
    /// `:`
    Colon,
    /// `,`
    Comma,
}

/// Byte-level JSON lexer with position tracking.
pub struct Lexer<'a> {
    input: &'a [u8],
    /// Current byte offset into `input`.
    pos: usize,
    /// Peeked token, if any.
    peeked: Option<Token>,
}

impl<'a> Lexer<'a> {
    /// Create a new lexer over the given input bytes.
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            pos: 0,
            peeked: None,
        }
    }

    /// Returns the current byte offset in the input.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Returns true if all input has been consumed (ignoring trailing whitespace).
    pub fn is_eof(&mut self) -> bool {
        if self.peeked.is_some() {
            return false;
        }
        self.skip_whitespace();
        self.pos >= self.input.len()
    }

    /// Consume and return the next token, or `None` if at end of input.
    pub fn next_token(&mut self) -> Result<Option<Token>, LiteParseError> {
        if let Some(tok) = self.peeked.take() {
            return Ok(Some(tok));
        }
        self.scan_token()
    }

    /// Peek at the next token without consuming it.
    pub fn peek_token(&mut self) -> Result<Option<&Token>, LiteParseError> {
        if self.peeked.is_none() {
            self.peeked = self.scan_token()?;
        }
        Ok(self.peeked.as_ref())
    }

    /// Consume the next token and verify it matches `expected`. Returns an error
    /// if the token doesn't match or if input is exhausted.
    pub fn expect_token(&mut self, expected: &Token) -> Result<Token, LiteParseError> {
        let tok = self.next_token()?.ok_or(LiteParseError::UnexpectedEof {
            pos: Pos::new(self.pos),
        })?;
        if std::mem::discriminant(&tok) != std::mem::discriminant(expected) {
            return Err(LiteParseError::ExpectedToken {
                expected: token_name(expected),
                pos: Pos::new(self.pos),
            });
        }
        Ok(tok)
    }

    // ---- internal scanning ----

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() {
            match self.input[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn scan_token(&mut self) -> Result<Option<Token>, LiteParseError> {
        self.skip_whitespace();
        if self.pos >= self.input.len() {
            return Ok(None);
        }
        let b = self.input[self.pos];
        match b {
            b'{' => {
                self.pos += 1;
                Ok(Some(Token::ObjectStart))
            }
            b'}' => {
                self.pos += 1;
                Ok(Some(Token::ObjectEnd))
            }
            b'[' => {
                self.pos += 1;
                Ok(Some(Token::ArrayStart))
            }
            b']' => {
                self.pos += 1;
                Ok(Some(Token::ArrayEnd))
            }
            b':' => {
                self.pos += 1;
                Ok(Some(Token::Colon))
            }
            b',' => {
                self.pos += 1;
                Ok(Some(Token::Comma))
            }
            b'"' => self.scan_string().map(|s| Some(Token::String(s))),
            b't' | b'f' => self.scan_bool().map(|b| Some(Token::Bool(b))),
            b'n' => self.scan_null().map(|_| Some(Token::Null)),
            b'-' | b'0'..=b'9' => self.scan_number().map(Some),
            _ => Err(LiteParseError::UnexpectedChar {
                ch: b,
                pos: Pos::new(self.pos),
            }),
        }
    }

    fn scan_string(&mut self) -> Result<std::string::String, LiteParseError> {
        debug_assert_eq!(self.input[self.pos], b'"');
        let start = self.pos;
        self.pos += 1; // skip opening quote

        let mut s = std::string::String::new();
        loop {
            if self.pos >= self.input.len() {
                return Err(LiteParseError::UnterminatedString {
                    pos: Pos::new(start),
                });
            }
            let b = self.input[self.pos];
            match b {
                b'"' => {
                    self.pos += 1;
                    return Ok(s);
                }
                b'\\' => {
                    self.pos += 1;
                    self.scan_escape(&mut s, start)?;
                }
                // Control characters (0x00-0x1F) are invalid unescaped in JSON strings
                b if b < 0x20 => {
                    return Err(LiteParseError::UnexpectedChar {
                        ch: b,
                        pos: Pos::new(self.pos),
                    });
                }
                _ => {
                    // Pass through UTF-8 bytes as-is
                    s.push(b as char);
                    self.pos += 1;
                    // For multi-byte UTF-8, we need to handle continuation bytes
                    if b >= 0x80 {
                        // Undo the incorrect push and redo with proper UTF-8 decoding
                        s.pop();
                        self.pos -= 1;
                        self.scan_utf8_char(&mut s, start)?;
                    }
                }
            }
        }
    }

    /// Decode a single UTF-8 character from the input and push it onto `s`.
    fn scan_utf8_char(
        &mut self,
        s: &mut std::string::String,
        string_start: usize,
    ) -> Result<(), LiteParseError> {
        let b0 = self.input[self.pos];
        let len = utf8_char_len(b0);
        if len == 0 || self.pos + len > self.input.len() {
            return Err(LiteParseError::UnterminatedString {
                pos: Pos::new(string_start),
            });
        }
        let slice = &self.input[self.pos..self.pos + len];
        match std::str::from_utf8(slice) {
            Ok(ch) => {
                s.push_str(ch);
                self.pos += len;
                Ok(())
            }
            Err(_) => Err(LiteParseError::UnexpectedChar {
                ch: b0,
                pos: Pos::new(self.pos),
            }),
        }
    }

    fn scan_escape(
        &mut self,
        s: &mut std::string::String,
        string_start: usize,
    ) -> Result<(), LiteParseError> {
        if self.pos >= self.input.len() {
            return Err(LiteParseError::UnterminatedString {
                pos: Pos::new(string_start),
            });
        }
        let esc_pos = self.pos;
        let b = self.input[self.pos];
        self.pos += 1;
        match b {
            b'"' => s.push('"'),
            b'\\' => s.push('\\'),
            b'/' => s.push('/'),
            b'b' => s.push('\x08'),
            b'f' => s.push('\x0C'),
            b'n' => s.push('\n'),
            b'r' => s.push('\r'),
            b't' => s.push('\t'),
            b'u' => {
                let cp = self.scan_hex4(esc_pos)?;
                // Check for surrogate pairs (astral codepoints)
                if (0xD800..=0xDBFF).contains(&cp) {
                    // High surrogate — must be followed by \uDCxx low surrogate
                    if self.pos + 1 < self.input.len()
                        && self.input[self.pos] == b'\\'
                        && self.input[self.pos + 1] == b'u'
                    {
                        self.pos += 2; // skip \u
                        let low = self.scan_hex4(esc_pos)?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return Err(LiteParseError::InvalidSurrogatePair {
                                pos: Pos::new(esc_pos),
                            });
                        }
                        let combined =
                            0x10000 + ((cp as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);
                        let ch = char::from_u32(combined).ok_or(
                            LiteParseError::InvalidSurrogatePair {
                                pos: Pos::new(esc_pos),
                            },
                        )?;
                        s.push(ch);
                    } else {
                        return Err(LiteParseError::InvalidSurrogatePair {
                            pos: Pos::new(esc_pos),
                        });
                    }
                } else if (0xDC00..=0xDFFF).contains(&cp) {
                    // Lone low surrogate
                    return Err(LiteParseError::InvalidSurrogatePair {
                        pos: Pos::new(esc_pos),
                    });
                } else {
                    let ch = char::from_u32(cp as u32).ok_or(
                        LiteParseError::InvalidUnicodeEscape {
                            pos: Pos::new(esc_pos),
                        },
                    )?;
                    s.push(ch);
                }
            }
            _ => {
                return Err(LiteParseError::InvalidEscape {
                    pos: Pos::new(esc_pos),
                });
            }
        }
        Ok(())
    }

    /// Parse exactly 4 hex digits and return the u16 value.
    fn scan_hex4(&mut self, esc_pos: usize) -> Result<u16, LiteParseError> {
        if self.pos + 4 > self.input.len() {
            return Err(LiteParseError::InvalidUnicodeEscape {
                pos: Pos::new(esc_pos),
            });
        }
        let mut val: u16 = 0;
        for _ in 0..4 {
            let b = self.input[self.pos];
            let digit = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => 10 + b - b'a',
                b'A'..=b'F' => 10 + b - b'A',
                _ => {
                    return Err(LiteParseError::InvalidUnicodeEscape {
                        pos: Pos::new(esc_pos),
                    });
                }
            };
            val = val * 16 + digit as u16;
            self.pos += 1;
        }
        Ok(val)
    }

    fn scan_bool(&mut self) -> Result<bool, LiteParseError> {
        if self.input[self.pos] == b't' {
            self.expect_literal(b"true")?;
            Ok(true)
        } else {
            self.expect_literal(b"false")?;
            Ok(false)
        }
    }

    fn scan_null(&mut self) -> Result<(), LiteParseError> {
        self.expect_literal(b"null")
    }

    fn expect_literal(&mut self, literal: &[u8]) -> Result<(), LiteParseError> {
        let start = self.pos;
        if self.pos + literal.len() > self.input.len()
            || &self.input[self.pos..self.pos + literal.len()] != literal
        {
            return Err(LiteParseError::InvalidLiteral {
                pos: Pos::new(start),
            });
        }
        self.pos += literal.len();
        Ok(())
    }

    fn scan_number(&mut self) -> Result<Token, LiteParseError> {
        let start = self.pos;
        let mut is_float = false;

        // Optional leading minus
        if self.pos < self.input.len() && self.input[self.pos] == b'-' {
            self.pos += 1;
        }

        // Integer part
        if self.pos >= self.input.len() {
            return Err(LiteParseError::InvalidNumber {
                pos: Pos::new(start),
            });
        }

        if self.input[self.pos] == b'0' {
            self.pos += 1;
            // After leading 0, must not be followed by another digit
            if self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                return Err(LiteParseError::InvalidNumber {
                    pos: Pos::new(start),
                });
            }
        } else if self.input[self.pos].is_ascii_digit() {
            while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        } else {
            return Err(LiteParseError::InvalidNumber {
                pos: Pos::new(start),
            });
        }

        // Fractional part
        if self.pos < self.input.len() && self.input[self.pos] == b'.' {
            is_float = true;
            self.pos += 1;
            if self.pos >= self.input.len() || !self.input[self.pos].is_ascii_digit() {
                return Err(LiteParseError::InvalidNumber {
                    pos: Pos::new(start),
                });
            }
            while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }

        // Exponent part
        if self.pos < self.input.len() && (self.input[self.pos] == b'e' || self.input[self.pos] == b'E')
        {
            is_float = true;
            self.pos += 1;
            if self.pos < self.input.len()
                && (self.input[self.pos] == b'+' || self.input[self.pos] == b'-')
            {
                self.pos += 1;
            }
            if self.pos >= self.input.len() || !self.input[self.pos].is_ascii_digit() {
                return Err(LiteParseError::InvalidNumber {
                    pos: Pos::new(start),
                });
            }
            while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }

        let num_str =
            std::str::from_utf8(&self.input[start..self.pos]).expect("number bytes are ASCII");

        if is_float {
            let val: f64 = num_str.parse().map_err(|_| LiteParseError::InvalidNumber {
                pos: Pos::new(start),
            })?;
            Ok(Token::Number(val))
        } else {
            // Try i64 first, fall back to f64 on overflow
            match num_str.parse::<i64>() {
                Ok(val) => Ok(Token::Integer(val)),
                Err(_) => {
                    let val: f64 =
                        num_str.parse().map_err(|_| LiteParseError::InvalidNumber {
                            pos: Pos::new(start),
                        })?;
                    Ok(Token::Number(val))
                }
            }
        }
    }
}

/// Return the expected UTF-8 byte length based on the leading byte.
fn utf8_char_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 0, // invalid leading byte
    }
}

/// Return a human-readable name for a token variant (for error messages).
fn token_name(t: &Token) -> &'static str {
    match t {
        Token::String(_) => "string",
        Token::Integer(_) => "integer",
        Token::Number(_) => "number",
        Token::Bool(_) => "boolean",
        Token::Null => "null",
        Token::ObjectStart => "'{'",
        Token::ObjectEnd => "'}'",
        Token::ArrayStart => "'['",
        Token::ArrayEnd => "']'",
        Token::Colon => "':'",
        Token::Comma => "','",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: lex all tokens from input.
    fn lex_all(input: &[u8]) -> Result<Vec<Token>, LiteParseError> {
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(tok) = lexer.next_token()? {
            tokens.push(tok);
        }
        Ok(tokens)
    }

    // ---- Structural tokens ----

    #[test]
    fn empty_input() {
        let tokens = lex_all(b"").unwrap();
        assert!(tokens.is_empty());
    }

    #[test]
    fn whitespace_only() {
        let tokens = lex_all(b"  \t\n\r  ").unwrap();
        assert!(tokens.is_empty());
    }

    #[test]
    fn structural_tokens() {
        let tokens = lex_all(b"{}[]:,").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::ObjectStart,
                Token::ObjectEnd,
                Token::ArrayStart,
                Token::ArrayEnd,
                Token::Colon,
                Token::Comma,
            ]
        );
    }

    #[test]
    fn structural_with_whitespace() {
        let tokens = lex_all(b" { } [ ] : , ").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::ObjectStart,
                Token::ObjectEnd,
                Token::ArrayStart,
                Token::ArrayEnd,
                Token::Colon,
                Token::Comma,
            ]
        );
    }

    // ---- Strings ----

    #[test]
    fn simple_string() {
        let tokens = lex_all(b"\"hello\"").unwrap();
        assert_eq!(tokens, vec![Token::String("hello".into())]);
    }

    #[test]
    fn empty_string() {
        let tokens = lex_all(b"\"\"").unwrap();
        assert_eq!(tokens, vec![Token::String(String::new())]);
    }

    #[test]
    fn string_with_basic_escapes() {
        let tokens = lex_all(br#""a\"b\\c\/d""#).unwrap();
        assert_eq!(tokens, vec![Token::String("a\"b\\c/d".into())]);
    }

    #[test]
    fn string_with_control_escapes() {
        let tokens = lex_all(br#""\b\f\n\r\t""#).unwrap();
        assert_eq!(
            tokens,
            vec![Token::String("\x08\x0C\n\r\t".into())]
        );
    }

    #[test]
    fn string_with_unicode_escape() {
        // \u0041 = 'A'
        let tokens = lex_all(br#""\u0041""#).unwrap();
        assert_eq!(tokens, vec![Token::String("A".into())]);
    }

    #[test]
    fn string_unicode_escape_lowercase() {
        let tokens = lex_all(br#""\u00e9""#).unwrap();
        assert_eq!(tokens, vec![Token::String("é".into())]);
    }

    #[test]
    fn string_unicode_escape_uppercase() {
        let tokens = lex_all(br#""\u00E9""#).unwrap();
        assert_eq!(tokens, vec![Token::String("é".into())]);
    }

    #[test]
    fn string_surrogate_pair() {
        // U+1F600 (grinning face) = \uD83D\uDE00
        let tokens = lex_all(br#""\uD83D\uDE00""#).unwrap();
        assert_eq!(tokens, vec![Token::String("😀".into())]);
    }

    #[test]
    fn string_surrogate_pair_musical_symbol() {
        // U+1D11E (musical symbol G clef) = \uD834\uDD1E
        let tokens = lex_all(br#""\uD834\uDD1E""#).unwrap();
        assert_eq!(tokens, vec![Token::String("𝄞".into())]);
    }

    #[test]
    fn string_utf8_passthrough() {
        let tokens = lex_all("\"héllo 日本語\"".as_bytes()).unwrap();
        assert_eq!(tokens, vec![Token::String("héllo 日本語".into())]);
    }

    #[test]
    fn string_unterminated() {
        let err = lex_all(b"\"hello").unwrap_err();
        assert!(matches!(err, LiteParseError::UnterminatedString { .. }));
    }

    #[test]
    fn string_invalid_escape() {
        let err = lex_all(br#""\x""#).unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidEscape { .. }));
    }

    #[test]
    fn string_invalid_unicode_short() {
        let err = lex_all(br#""\u00""#).unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidUnicodeEscape { .. }));
    }

    #[test]
    fn string_invalid_unicode_bad_hex() {
        let err = lex_all(br#""\u00GG""#).unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidUnicodeEscape { .. }));
    }

    #[test]
    fn string_lone_high_surrogate() {
        let err = lex_all(br#""\uD800""#).unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidSurrogatePair { .. }));
    }

    #[test]
    fn string_lone_low_surrogate() {
        let err = lex_all(br#""\uDC00""#).unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidSurrogatePair { .. }));
    }

    #[test]
    fn string_high_surrogate_without_low() {
        // High surrogate followed by non-surrogate \u escape
        let err = lex_all(br#""\uD800\u0041""#).unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidSurrogatePair { .. }));
    }

    #[test]
    fn string_unescaped_control_char() {
        let err = lex_all(b"\"\x01\"").unwrap_err();
        assert!(matches!(err, LiteParseError::UnexpectedChar { .. }));
    }

    // ---- Booleans ----

    #[test]
    fn bool_true() {
        let tokens = lex_all(b"true").unwrap();
        assert_eq!(tokens, vec![Token::Bool(true)]);
    }

    #[test]
    fn bool_false() {
        let tokens = lex_all(b"false").unwrap();
        assert_eq!(tokens, vec![Token::Bool(false)]);
    }

    #[test]
    fn bool_invalid_truncated() {
        let err = lex_all(b"tru").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidLiteral { .. }));
    }

    #[test]
    fn bool_invalid_misspelled() {
        let err = lex_all(b"falze").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidLiteral { .. }));
    }

    // ---- Null ----

    #[test]
    fn null_literal() {
        let tokens = lex_all(b"null").unwrap();
        assert_eq!(tokens, vec![Token::Null]);
    }

    #[test]
    fn null_truncated() {
        let err = lex_all(b"nul").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidLiteral { .. }));
    }

    // ---- Numbers ----

    #[test]
    fn integer_zero() {
        let tokens = lex_all(b"0").unwrap();
        assert_eq!(tokens, vec![Token::Integer(0)]);
    }

    #[test]
    fn integer_positive() {
        let tokens = lex_all(b"42").unwrap();
        assert_eq!(tokens, vec![Token::Integer(42)]);
    }

    #[test]
    fn integer_negative() {
        let tokens = lex_all(b"-1").unwrap();
        assert_eq!(tokens, vec![Token::Integer(-1)]);
    }

    #[test]
    fn integer_large() {
        let tokens = lex_all(b"9223372036854775807").unwrap(); // i64::MAX
        assert_eq!(tokens, vec![Token::Integer(i64::MAX)]);
    }

    #[test]
    fn integer_min() {
        let tokens = lex_all(b"-9223372036854775808").unwrap(); // i64::MIN
        assert_eq!(tokens, vec![Token::Integer(i64::MIN)]);
    }

    #[test]
    fn integer_overflow_to_f64() {
        // i64::MAX + 1 should become f64
        let tokens = lex_all(b"9223372036854775808").unwrap();
        assert!(matches!(tokens[0], Token::Number(_)));
    }

    #[test]
    fn float_simple() {
        let tokens = lex_all(b"3.14").unwrap();
        assert_eq!(tokens, vec![Token::Number(3.14)]);
    }

    #[test]
    fn float_negative() {
        let tokens = lex_all(b"-0.5").unwrap();
        assert_eq!(tokens, vec![Token::Number(-0.5)]);
    }

    #[test]
    fn float_with_exponent() {
        let tokens = lex_all(b"1e10").unwrap();
        assert_eq!(tokens, vec![Token::Number(1e10)]);
    }

    #[test]
    fn float_with_positive_exponent() {
        let tokens = lex_all(b"1E+3").unwrap();
        assert_eq!(tokens, vec![Token::Number(1e3)]);
    }

    #[test]
    fn float_with_negative_exponent() {
        let tokens = lex_all(b"1e-2").unwrap();
        assert_eq!(tokens, vec![Token::Number(0.01)]);
    }

    #[test]
    fn float_full_form() {
        let tokens = lex_all(b"-3.14e+2").unwrap();
        assert_eq!(tokens, vec![Token::Number(-314.0)]);
    }

    #[test]
    fn number_leading_zero_rejected() {
        let err = lex_all(b"01").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidNumber { .. }));
    }

    #[test]
    fn number_negative_zero() {
        let tokens = lex_all(b"-0").unwrap();
        assert_eq!(tokens, vec![Token::Integer(0)]);
    }

    #[test]
    fn number_trailing_dot_rejected() {
        let err = lex_all(b"1.").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidNumber { .. }));
    }

    #[test]
    fn number_trailing_e_rejected() {
        let err = lex_all(b"1e").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidNumber { .. }));
    }

    #[test]
    fn number_bare_minus_rejected() {
        let err = lex_all(b"-").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidNumber { .. }));
    }

    // ---- Complete JSON structures ----

    #[test]
    fn simple_object() {
        let tokens = lex_all(br#"{"key": "value"}"#).unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::ObjectStart,
                Token::String("key".into()),
                Token::Colon,
                Token::String("value".into()),
                Token::ObjectEnd,
            ]
        );
    }

    #[test]
    fn simple_array() {
        let tokens = lex_all(b"[1, 2, 3]").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::ArrayStart,
                Token::Integer(1),
                Token::Comma,
                Token::Integer(2),
                Token::Comma,
                Token::Integer(3),
                Token::ArrayEnd,
            ]
        );
    }

    #[test]
    fn nested_structure() {
        let tokens = lex_all(br#"{"a": [true, null, 1.5]}"#).unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::ObjectStart,
                Token::String("a".into()),
                Token::Colon,
                Token::ArrayStart,
                Token::Bool(true),
                Token::Comma,
                Token::Null,
                Token::Comma,
                Token::Number(1.5),
                Token::ArrayEnd,
                Token::ObjectEnd,
            ]
        );
    }

    #[test]
    fn omi_envelope() {
        let input = br#"{"omi":"1.0","ttl":0,"read":{"path":"/A/B"}}"#;
        let tokens = lex_all(input).unwrap();
        assert_eq!(tokens[0], Token::ObjectStart);
        assert_eq!(tokens[1], Token::String("omi".into()));
        assert_eq!(tokens[2], Token::Colon);
        assert_eq!(tokens[3], Token::String("1.0".into()));
        assert_eq!(tokens[4], Token::Comma);
        assert_eq!(tokens[5], Token::String("ttl".into()));
        assert_eq!(tokens[6], Token::Colon);
        assert_eq!(tokens[7], Token::Integer(0));
    }

    // ---- Peek and expect ----

    #[test]
    fn peek_does_not_consume() {
        let mut lexer = Lexer::new(b"42");
        let peek = lexer.peek_token().unwrap().cloned();
        assert_eq!(peek, Some(Token::Integer(42)));
        let next = lexer.next_token().unwrap();
        assert_eq!(next, Some(Token::Integer(42)));
        assert!(lexer.next_token().unwrap().is_none());
    }

    #[test]
    fn peek_multiple_times() {
        let mut lexer = Lexer::new(b"true");
        let p1 = lexer.peek_token().unwrap().cloned();
        let p2 = lexer.peek_token().unwrap().cloned();
        assert_eq!(p1, p2);
    }

    #[test]
    fn expect_token_success() {
        let mut lexer = Lexer::new(b"{}");
        let tok = lexer.expect_token(&Token::ObjectStart).unwrap();
        assert_eq!(tok, Token::ObjectStart);
        let tok = lexer.expect_token(&Token::ObjectEnd).unwrap();
        assert_eq!(tok, Token::ObjectEnd);
    }

    #[test]
    fn expect_token_wrong_type() {
        let mut lexer = Lexer::new(b"[");
        let err = lexer.expect_token(&Token::ObjectStart).unwrap_err();
        assert!(matches!(err, LiteParseError::ExpectedToken { .. }));
    }

    #[test]
    fn expect_token_eof() {
        let mut lexer = Lexer::new(b"");
        let err = lexer.expect_token(&Token::ObjectStart).unwrap_err();
        assert!(matches!(err, LiteParseError::UnexpectedEof { .. }));
    }

    // ---- Position tracking ----

    #[test]
    fn error_position_tracking() {
        let err = lex_all(b"  @").unwrap_err();
        match err {
            LiteParseError::UnexpectedChar { pos, ch } => {
                assert_eq!(pos, Pos::new(2));
                assert_eq!(ch, b'@');
            }
            _ => panic!("expected UnexpectedChar"),
        }
    }

    #[test]
    fn position_after_tokens() {
        let mut lexer = Lexer::new(b"[42]");
        lexer.next_token().unwrap(); // [
        assert_eq!(lexer.position(), 1);
        lexer.next_token().unwrap(); // 42
        assert_eq!(lexer.position(), 3);
        lexer.next_token().unwrap(); // ]
        assert_eq!(lexer.position(), 4);
    }

    // ---- is_eof ----

    #[test]
    fn is_eof_empty() {
        let mut lexer = Lexer::new(b"");
        assert!(lexer.is_eof());
    }

    #[test]
    fn is_eof_after_all_consumed() {
        let mut lexer = Lexer::new(b"42");
        assert!(!lexer.is_eof());
        lexer.next_token().unwrap();
        assert!(lexer.is_eof());
    }

    #[test]
    fn is_eof_trailing_whitespace() {
        let mut lexer = Lexer::new(b"42  ");
        lexer.next_token().unwrap();
        assert!(lexer.is_eof());
    }

    #[test]
    fn is_eof_with_peeked_token() {
        let mut lexer = Lexer::new(b"42");
        lexer.peek_token().unwrap();
        assert!(!lexer.is_eof());
    }

    // ---- Whitespace handling ----

    #[test]
    fn all_whitespace_types() {
        let tokens = lex_all(b" \t\n\r42 \t\n\r").unwrap();
        assert_eq!(tokens, vec![Token::Integer(42)]);
    }

    // ---- Unexpected characters ----

    #[test]
    fn unexpected_at_sign() {
        let err = lex_all(b"@").unwrap_err();
        assert!(matches!(
            err,
            LiteParseError::UnexpectedChar { ch: b'@', .. }
        ));
    }

    #[test]
    fn unexpected_after_valid() {
        let err = lex_all(b"42 @").unwrap_err();
        assert!(matches!(
            err,
            LiteParseError::UnexpectedChar { ch: b'@', .. }
        ));
    }
}
