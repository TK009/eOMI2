//! JSON parser for the lite-json parser.
//!
//! Provides [`JsonParser`] for parsing JSON bytes into O-DF types, and the
//! [`FromJson`] trait for convenient deserialization. Matches serde_json
//! behavior for all valid OMI messages (FR-012).

use std::collections::BTreeMap;

use crate::odf::{InfoItem, Object, OmiValue};
use crate::odf::value::{RingBuffer, Value};
use crate::omi::OmiMessage;
use crate::omi::error::ParseError;

use super::error::{LiteParseError, Pos};
use super::lexer::{Lexer, Token};

/// Maximum nesting depth for JSON parsing. Each `{` or `[` counts as one level.
const MAX_DEPTH: usize = 32;

/// Trait for types that can be deserialized from JSON bytes.
pub trait FromJson: Sized {
    fn from_json_bytes(input: &[u8]) -> Result<Self, LiteParseError>;

    fn from_json_str(input: &str) -> Result<Self, LiteParseError> {
        Self::from_json_bytes(input.as_bytes())
    }
}

/// JSON parser with nesting depth tracking.
///
/// Wraps a [`Lexer`] and provides methods to parse JSON into O-DF types.
/// Also useful for higher-level parsers (e.g. OMI envelope parsing) that
/// need access to the lexer and depth tracking.
pub struct JsonParser<'a> {
    lex: Lexer<'a>,
    depth: usize,
}

impl<'a> JsonParser<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            lex: Lexer::new(input),
            depth: 0,
        }
    }

    /// Access the underlying lexer (for higher-level parsers).
    pub fn lexer(&mut self) -> &mut Lexer<'a> {
        &mut self.lex
    }

    /// Check if all input has been consumed.
    pub fn is_eof(&mut self) -> bool {
        self.lex.is_eof()
    }

    /// Current byte position in the input.
    pub fn position(&self) -> usize {
        self.lex.position()
    }

    fn pos(&self) -> Pos {
        Pos::new(self.lex.position())
    }

    fn enter(&mut self) -> Result<(), LiteParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            Err(LiteParseError::DepthExceeded { max: MAX_DEPTH, pos: self.pos() })
        } else {
            Ok(())
        }
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Consume the next token and extract a String, or return an error.
    fn expect_string(&mut self) -> Result<String, LiteParseError> {
        match self.lex.next_token()? {
            Some(Token::String(s)) => Ok(s),
            Some(_) => Err(LiteParseError::ExpectedToken {
                expected: "string",
                pos: self.pos(),
            }),
            None => Err(LiteParseError::UnexpectedEof { pos: self.pos() }),
        }
    }

    /// Consume the next token and extract an f64, accepting both Integer and Number tokens.
    fn expect_f64(&mut self) -> Result<f64, LiteParseError> {
        match self.lex.next_token()? {
            Some(Token::Number(n)) => Ok(n),
            Some(Token::Integer(i)) => Ok(i as f64),
            Some(_) => Err(LiteParseError::ExpectedToken {
                expected: "number",
                pos: self.pos(),
            }),
            None => Err(LiteParseError::UnexpectedEof { pos: self.pos() }),
        }
    }

    /// Check if the next token matches a given variant (by discriminant) without consuming.
    fn peek_is(&mut self, expected: &Token) -> Result<bool, LiteParseError> {
        match self.lex.peek_token()? {
            Some(tok) => Ok(std::mem::discriminant(tok) == std::mem::discriminant(expected)),
            None => Ok(false),
        }
    }

    /// Skip any JSON value, tracking depth for nested structures.
    pub fn skip_value(&mut self) -> Result<(), LiteParseError> {
        match self.lex.next_token()? {
            None => Err(LiteParseError::UnexpectedEof { pos: self.pos() }),
            Some(Token::String(_) | Token::Integer(_) | Token::Number(_)
                | Token::Bool(_) | Token::Null) => Ok(()),
            Some(Token::ObjectStart) => {
                self.enter()?;
                if self.peek_is(&Token::ObjectEnd)? {
                    self.lex.next_token()?;
                    self.leave();
                    return Ok(());
                }
                loop {
                    // key
                    self.expect_string()?;
                    self.lex.expect_token(&Token::Colon)?;
                    self.skip_value()?;
                    if self.peek_is(&Token::Comma)? {
                        self.lex.next_token()?;
                    } else if self.peek_is(&Token::ObjectEnd)? {
                        self.lex.next_token()?;
                        self.leave();
                        return Ok(());
                    } else {
                        return Err(LiteParseError::ExpectedToken {
                            expected: "',' or '}'",
                            pos: self.pos(),
                        });
                    }
                }
            }
            Some(Token::ArrayStart) => {
                self.enter()?;
                if self.peek_is(&Token::ArrayEnd)? {
                    self.lex.next_token()?;
                    self.leave();
                    return Ok(());
                }
                loop {
                    self.skip_value()?;
                    if self.peek_is(&Token::Comma)? {
                        self.lex.next_token()?;
                    } else if self.peek_is(&Token::ArrayEnd)? {
                        self.lex.next_token()?;
                        self.leave();
                        return Ok(());
                    } else {
                        return Err(LiteParseError::ExpectedToken {
                            expected: "',' or ']'",
                            pos: self.pos(),
                        });
                    }
                }
            }
            Some(_) => Err(LiteParseError::ExpectedToken {
                expected: "value",
                pos: self.pos(),
            }),
        }
    }

    /// Parse an [`OmiValue`] (null, bool, number, or string).
    pub fn parse_omi_value(&mut self) -> Result<OmiValue, LiteParseError> {
        match self.lex.next_token()? {
            Some(Token::Null) => Ok(OmiValue::Null),
            Some(Token::Bool(b)) => Ok(OmiValue::Bool(b)),
            Some(Token::String(s)) => Ok(OmiValue::Str(s)),
            Some(Token::Number(n)) => Ok(OmiValue::Number(n)),
            Some(Token::Integer(i)) => Ok(OmiValue::Number(i as f64)),
            Some(_) => Err(LiteParseError::ExpectedToken {
                expected: "value (null, bool, number, or string)",
                pos: self.pos(),
            }),
            None => Err(LiteParseError::UnexpectedEof { pos: self.pos() }),
        }
    }

    /// Parse a [`Value`] object: `{"v": <omi_value>, "t": <opt_f64>}`.
    pub fn parse_value(&mut self) -> Result<Value, LiteParseError> {
        self.enter()?;
        self.lex.expect_token(&Token::ObjectStart)?;

        let mut v: Option<OmiValue> = None;
        let mut t: Option<f64> = None;
        let obj_pos = self.pos();

        if !self.peek_is(&Token::ObjectEnd)? {
            loop {
                let key = self.expect_string()?;
                self.lex.expect_token(&Token::Colon)?;
                match key.as_str() {
                    "v" => v = Some(self.parse_omi_value()?),
                    "t" => {
                        if self.peek_is(&Token::Null)? {
                            self.lex.next_token()?;
                        } else {
                            t = Some(self.expect_f64()?);
                        }
                    }
                    _ => self.skip_value()?,
                }
                if self.peek_is(&Token::Comma)? {
                    self.lex.next_token()?;
                } else if self.peek_is(&Token::ObjectEnd)? {
                    break;
                } else {
                    self.leave();
                    return Err(LiteParseError::ExpectedToken {
                        expected: "',' or '}'",
                        pos: self.pos(),
                    });
                }
            }
        }
        self.lex.expect_token(&Token::ObjectEnd)?;
        self.leave();

        let v = v.ok_or(LiteParseError::MissingField { field: "v", pos: obj_pos })?;
        Ok(Value::new(v, t))
    }

    /// Parse a [`RingBuffer`] from a JSON array of Value objects.
    ///
    /// Input array is newest-first (matching serde serialization).
    /// Values are pushed in reverse to maintain correct ring buffer order.
    pub fn parse_ring_buffer(&mut self) -> Result<RingBuffer, LiteParseError> {
        self.enter()?;
        self.lex.expect_token(&Token::ArrayStart)?;

        let mut values = Vec::new();
        if !self.peek_is(&Token::ArrayEnd)? {
            loop {
                values.push(self.parse_value()?);
                if self.peek_is(&Token::Comma)? {
                    self.lex.next_token()?;
                } else if self.peek_is(&Token::ArrayEnd)? {
                    break;
                } else {
                    self.leave();
                    return Err(LiteParseError::ExpectedToken {
                        expected: "',' or ']'",
                        pos: self.pos(),
                    });
                }
            }
        }
        self.lex.expect_token(&Token::ArrayEnd)?;
        self.leave();

        let capacity = values.len().max(1);
        let mut rb = RingBuffer::new(capacity);
        // Input is newest-first; push in reverse so oldest goes in first
        for v in values.into_iter().rev() {
            rb.push(v);
        }
        Ok(rb)
    }

    /// Parse a `BTreeMap<String, OmiValue>` (used for InfoItem metadata).
    fn parse_meta_map(&mut self) -> Result<BTreeMap<String, OmiValue>, LiteParseError> {
        self.enter()?;
        self.lex.expect_token(&Token::ObjectStart)?;

        let mut map = BTreeMap::new();
        if !self.peek_is(&Token::ObjectEnd)? {
            loop {
                let key = self.expect_string()?;
                self.lex.expect_token(&Token::Colon)?;
                let val = self.parse_omi_value()?;
                map.insert(key, val);
                if self.peek_is(&Token::Comma)? {
                    self.lex.next_token()?;
                } else if self.peek_is(&Token::ObjectEnd)? {
                    break;
                } else {
                    self.leave();
                    return Err(LiteParseError::ExpectedToken {
                        expected: "',' or '}'",
                        pos: self.pos(),
                    });
                }
            }
        }
        self.lex.expect_token(&Token::ObjectEnd)?;
        self.leave();
        Ok(map)
    }

    /// Parse an [`InfoItem`] from a JSON object.
    ///
    /// JSON fields: `type` (→ type_uri), `desc`, `meta`, `values` (required).
    /// Unknown fields are silently ignored (FR-007).
    pub fn parse_info_item(&mut self) -> Result<InfoItem, LiteParseError> {
        self.enter()?;
        self.lex.expect_token(&Token::ObjectStart)?;

        let mut type_uri: Option<String> = None;
        let mut desc: Option<String> = None;
        let mut meta: Option<BTreeMap<String, OmiValue>> = None;
        let mut values: Option<RingBuffer> = None;
        let obj_pos = self.pos();

        if !self.peek_is(&Token::ObjectEnd)? {
            loop {
                let key = self.expect_string()?;
                self.lex.expect_token(&Token::Colon)?;
                match key.as_str() {
                    "type" => {
                        if self.peek_is(&Token::Null)? {
                            self.lex.next_token()?;
                        } else {
                            type_uri = Some(self.expect_string()?);
                        }
                    }
                    "desc" => {
                        if self.peek_is(&Token::Null)? {
                            self.lex.next_token()?;
                        } else {
                            desc = Some(self.expect_string()?);
                        }
                    }
                    "meta" => {
                        if self.peek_is(&Token::Null)? {
                            self.lex.next_token()?;
                        } else {
                            meta = Some(self.parse_meta_map()?);
                        }
                    }
                    "values" => values = Some(self.parse_ring_buffer()?),
                    _ => self.skip_value()?,
                }
                if self.peek_is(&Token::Comma)? {
                    self.lex.next_token()?;
                } else if self.peek_is(&Token::ObjectEnd)? {
                    break;
                } else {
                    self.leave();
                    return Err(LiteParseError::ExpectedToken {
                        expected: "',' or '}'",
                        pos: self.pos(),
                    });
                }
            }
        }
        self.lex.expect_token(&Token::ObjectEnd)?;
        self.leave();

        let values = values.ok_or(LiteParseError::MissingField {
            field: "values",
            pos: obj_pos,
        })?;
        Ok(InfoItem { type_uri, desc, meta, values })
    }

    /// Parse a `BTreeMap<String, InfoItem>` (Object.items).
    fn parse_items_map(&mut self) -> Result<BTreeMap<String, InfoItem>, LiteParseError> {
        self.enter()?;
        self.lex.expect_token(&Token::ObjectStart)?;

        let mut map = BTreeMap::new();
        if !self.peek_is(&Token::ObjectEnd)? {
            loop {
                let key = self.expect_string()?;
                self.lex.expect_token(&Token::Colon)?;
                let item = self.parse_info_item()?;
                map.insert(key, item);
                if self.peek_is(&Token::Comma)? {
                    self.lex.next_token()?;
                } else if self.peek_is(&Token::ObjectEnd)? {
                    break;
                } else {
                    self.leave();
                    return Err(LiteParseError::ExpectedToken {
                        expected: "',' or '}'",
                        pos: self.pos(),
                    });
                }
            }
        }
        self.lex.expect_token(&Token::ObjectEnd)?;
        self.leave();
        Ok(map)
    }

    /// Parse an [`Object`] from a JSON object.
    ///
    /// JSON fields: `id` (required), `type` (→ type_uri), `desc`, `items`, `objects`.
    /// Unknown fields are silently ignored (FR-007).
    pub fn parse_object(&mut self) -> Result<Object, LiteParseError> {
        self.enter()?;
        self.lex.expect_token(&Token::ObjectStart)?;

        let mut id: Option<String> = None;
        let mut type_uri: Option<String> = None;
        let mut desc: Option<String> = None;
        let mut items: Option<BTreeMap<String, InfoItem>> = None;
        let mut objects: Option<BTreeMap<String, Object>> = None;
        let obj_pos = self.pos();

        if !self.peek_is(&Token::ObjectEnd)? {
            loop {
                let key = self.expect_string()?;
                self.lex.expect_token(&Token::Colon)?;
                match key.as_str() {
                    "id" => id = Some(self.expect_string()?),
                    "type" => {
                        if self.peek_is(&Token::Null)? {
                            self.lex.next_token()?;
                        } else {
                            type_uri = Some(self.expect_string()?);
                        }
                    }
                    "desc" => {
                        if self.peek_is(&Token::Null)? {
                            self.lex.next_token()?;
                        } else {
                            desc = Some(self.expect_string()?);
                        }
                    }
                    "items" => {
                        if self.peek_is(&Token::Null)? {
                            self.lex.next_token()?;
                        } else {
                            items = Some(self.parse_items_map()?);
                        }
                    }
                    "objects" => {
                        if self.peek_is(&Token::Null)? {
                            self.lex.next_token()?;
                        } else {
                            objects = Some(self.parse_objects_map()?);
                        }
                    }
                    _ => self.skip_value()?,
                }
                if self.peek_is(&Token::Comma)? {
                    self.lex.next_token()?;
                } else if self.peek_is(&Token::ObjectEnd)? {
                    break;
                } else {
                    self.leave();
                    return Err(LiteParseError::ExpectedToken {
                        expected: "',' or '}'",
                        pos: self.pos(),
                    });
                }
            }
        }
        self.lex.expect_token(&Token::ObjectEnd)?;
        self.leave();

        let id = id.ok_or(LiteParseError::MissingField { field: "id", pos: obj_pos })?;
        Ok(Object { id, type_uri, desc, items, objects })
    }

    /// Parse a `BTreeMap<String, Object>` (Object.objects or WriteOp::Tree objects).
    pub fn parse_objects_map(&mut self) -> Result<BTreeMap<String, Object>, LiteParseError> {
        self.enter()?;
        self.lex.expect_token(&Token::ObjectStart)?;

        let mut map = BTreeMap::new();
        if !self.peek_is(&Token::ObjectEnd)? {
            loop {
                let key = self.expect_string()?;
                self.lex.expect_token(&Token::Colon)?;
                let obj = self.parse_object()?;
                map.insert(key, obj);
                if self.peek_is(&Token::Comma)? {
                    self.lex.next_token()?;
                } else if self.peek_is(&Token::ObjectEnd)? {
                    break;
                } else {
                    self.leave();
                    return Err(LiteParseError::ExpectedToken {
                        expected: "',' or '}'",
                        pos: self.pos(),
                    });
                }
            }
        }
        self.lex.expect_token(&Token::ObjectEnd)?;
        self.leave();
        Ok(map)
    }
}

// -- FromJson implementations --

impl FromJson for OmiValue {
    fn from_json_bytes(input: &[u8]) -> Result<Self, LiteParseError> {
        let mut parser = JsonParser::new(input);
        let val = parser.parse_omi_value()?;
        if !parser.is_eof() {
            return Err(LiteParseError::TrailingData { pos: parser.pos() });
        }
        Ok(val)
    }
}

impl FromJson for Value {
    fn from_json_bytes(input: &[u8]) -> Result<Self, LiteParseError> {
        let mut parser = JsonParser::new(input);
        let val = parser.parse_value()?;
        if !parser.is_eof() {
            return Err(LiteParseError::TrailingData { pos: parser.pos() });
        }
        Ok(val)
    }
}

impl FromJson for RingBuffer {
    fn from_json_bytes(input: &[u8]) -> Result<Self, LiteParseError> {
        let mut parser = JsonParser::new(input);
        let rb = parser.parse_ring_buffer()?;
        if !parser.is_eof() {
            return Err(LiteParseError::TrailingData { pos: parser.pos() });
        }
        Ok(rb)
    }
}

impl FromJson for InfoItem {
    fn from_json_bytes(input: &[u8]) -> Result<Self, LiteParseError> {
        let mut parser = JsonParser::new(input);
        let item = parser.parse_info_item()?;
        if !parser.is_eof() {
            return Err(LiteParseError::TrailingData { pos: parser.pos() });
        }
        Ok(item)
    }
}

impl FromJson for Object {
    fn from_json_bytes(input: &[u8]) -> Result<Self, LiteParseError> {
        let mut parser = JsonParser::new(input);
        let obj = parser.parse_object()?;
        if !parser.is_eof() {
            return Err(LiteParseError::TrailingData { pos: parser.pos() });
        }
        Ok(obj)
    }
}

/// Parse an OMI message from a JSON string using the lite-json parser.
///
/// This is the lite-json equivalent of `OmiMessage::parse()` from the serde path.
/// It must produce identical results for all valid OMI messages (FR-012).
pub fn parse_omi_message(_input: &str) -> Result<OmiMessage, ParseError> {
    // Stub: will be implemented by T05 (operation sub-parsers) and T04 (envelope parser).
    Err(ParseError::InvalidJson("lite-json OMI envelope parser not yet implemented".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- OmiValue -----

    #[test]
    fn parse_omi_null() {
        assert_eq!(OmiValue::from_json_str("null").unwrap(), OmiValue::Null);
    }

    #[test]
    fn parse_omi_bool_true() {
        assert_eq!(OmiValue::from_json_str("true").unwrap(), OmiValue::Bool(true));
    }

    #[test]
    fn parse_omi_bool_false() {
        assert_eq!(OmiValue::from_json_str("false").unwrap(), OmiValue::Bool(false));
    }

    #[test]
    fn parse_omi_number_float() {
        assert_eq!(OmiValue::from_json_str("42.5").unwrap(), OmiValue::Number(42.5));
    }

    #[test]
    fn parse_omi_number_integer() {
        assert_eq!(OmiValue::from_json_str("42").unwrap(), OmiValue::Number(42.0));
    }

    #[test]
    fn parse_omi_string() {
        assert_eq!(
            OmiValue::from_json_str("\"hello\"").unwrap(),
            OmiValue::Str("hello".into())
        );
    }

    #[test]
    fn parse_omi_negative_number() {
        assert_eq!(OmiValue::from_json_str("-3.14").unwrap(), OmiValue::Number(-3.14));
    }

    // ----- Value -----

    #[test]
    fn parse_value_with_timestamp() {
        let v = Value::from_json_str(r#"{"v": 22.5, "t": 1000.0}"#).unwrap();
        assert_eq!(v.v, OmiValue::Number(22.5));
        assert_eq!(v.t, Some(1000.0));
    }

    #[test]
    fn parse_value_without_timestamp() {
        let v = Value::from_json_str(r#"{"v": true}"#).unwrap();
        assert_eq!(v.v, OmiValue::Bool(true));
        assert_eq!(v.t, None);
    }

    #[test]
    fn parse_value_null_timestamp() {
        let v = Value::from_json_str(r#"{"v": "hi", "t": null}"#).unwrap();
        assert_eq!(v.v, OmiValue::Str("hi".into()));
        assert_eq!(v.t, None);
    }

    #[test]
    fn parse_value_with_null_v() {
        let v = Value::from_json_str(r#"{"v": null}"#).unwrap();
        assert_eq!(v.v, OmiValue::Null);
    }

    #[test]
    fn parse_value_ignores_unknown_fields() {
        let v = Value::from_json_str(r#"{"v": 1.0, "extra": "ignored", "t": 2.0}"#).unwrap();
        assert_eq!(v.v, OmiValue::Number(1.0));
        assert_eq!(v.t, Some(2.0));
    }

    #[test]
    fn parse_value_missing_v() {
        let err = Value::from_json_str(r#"{"t": 1.0}"#).unwrap_err();
        assert!(matches!(err, LiteParseError::MissingField { field: "v", .. }));
    }

    #[test]
    fn parse_value_empty_object() {
        let err = Value::from_json_str(r#"{}"#).unwrap_err();
        assert!(matches!(err, LiteParseError::MissingField { field: "v", .. }));
    }

    #[test]
    fn parse_value_integer_timestamp() {
        let v = Value::from_json_str(r#"{"v": 1, "t": 1000}"#).unwrap();
        assert_eq!(v.v, OmiValue::Number(1.0));
        assert_eq!(v.t, Some(1000.0));
    }

    // ----- RingBuffer -----

    #[test]
    fn parse_ring_buffer_empty() {
        let rb = RingBuffer::from_json_str("[]").unwrap();
        assert_eq!(rb.len(), 0);
        assert_eq!(rb.capacity(), 1);
    }

    #[test]
    fn parse_ring_buffer_single() {
        let rb = RingBuffer::from_json_str(r#"[{"v": 42.0, "t": 100.0}]"#).unwrap();
        assert_eq!(rb.len(), 1);
        assert_eq!(rb.capacity(), 1);
        let newest = rb.newest(1);
        assert_eq!(newest[0].v, OmiValue::Number(42.0));
        assert_eq!(newest[0].t, Some(100.0));
    }

    #[test]
    fn parse_ring_buffer_preserves_order() {
        // Input is newest-first: 3, 2, 1
        let json = r#"[
            {"v": 3.0, "t": 300.0},
            {"v": 2.0, "t": 200.0},
            {"v": 1.0, "t": 100.0}
        ]"#;
        let rb = RingBuffer::from_json_str(json).unwrap();
        assert_eq!(rb.len(), 3);

        let newest = rb.newest(3);
        assert_eq!(newest[0].v, OmiValue::Number(3.0));
        assert_eq!(newest[1].v, OmiValue::Number(2.0));
        assert_eq!(newest[2].v, OmiValue::Number(1.0));
    }

    // ----- InfoItem -----

    #[test]
    fn parse_info_item_minimal() {
        let json = r#"{"values": []}"#;
        let item = InfoItem::from_json_str(json).unwrap();
        assert!(item.type_uri.is_none());
        assert!(item.desc.is_none());
        assert!(item.meta.is_none());
        assert!(item.values.is_empty());
    }

    #[test]
    fn parse_info_item_full() {
        let json = r#"{
            "type": "omi:temperature",
            "desc": "Room temperature",
            "meta": {"writable": true, "unit": "celsius"},
            "values": [
                {"v": 23.0, "t": 1001.0},
                {"v": 22.5, "t": 1000.0}
            ]
        }"#;
        let item = InfoItem::from_json_str(json).unwrap();
        assert_eq!(item.type_uri.as_deref(), Some("omi:temperature"));
        assert_eq!(item.desc.as_deref(), Some("Room temperature"));
        assert!(item.is_writable());
        assert_eq!(item.values.len(), 2);

        let newest = item.values.newest(1);
        assert_eq!(newest[0].v, OmiValue::Number(23.0));
    }

    #[test]
    fn parse_info_item_null_optional_fields() {
        let json = r#"{"type": null, "desc": null, "meta": null, "values": []}"#;
        let item = InfoItem::from_json_str(json).unwrap();
        assert!(item.type_uri.is_none());
        assert!(item.desc.is_none());
        assert!(item.meta.is_none());
    }

    #[test]
    fn parse_info_item_ignores_unknown() {
        let json = r#"{"unknown_field": [1,2,3], "values": [{"v": 1.0}]}"#;
        let item = InfoItem::from_json_str(json).unwrap();
        assert_eq!(item.values.len(), 1);
    }

    #[test]
    fn parse_info_item_missing_values() {
        let json = r#"{"type": "omi:temp"}"#;
        let err = InfoItem::from_json_str(json).unwrap_err();
        assert!(matches!(err, LiteParseError::MissingField { field: "values", .. }));
    }

    // ----- Object -----

    #[test]
    fn parse_object_minimal() {
        let json = r#"{"id": "DeviceA"}"#;
        let obj = Object::from_json_str(json).unwrap();
        assert_eq!(obj.id, "DeviceA");
        assert!(obj.type_uri.is_none());
        assert!(obj.desc.is_none());
        assert!(obj.items.is_none());
        assert!(obj.objects.is_none());
    }

    #[test]
    fn parse_object_with_type_and_desc() {
        let json = r#"{"id": "A", "type": "omi:device", "desc": "Main controller"}"#;
        let obj = Object::from_json_str(json).unwrap();
        assert_eq!(obj.id, "A");
        assert_eq!(obj.type_uri.as_deref(), Some("omi:device"));
        assert_eq!(obj.desc.as_deref(), Some("Main controller"));
    }

    #[test]
    fn parse_object_with_items() {
        let json = r#"{
            "id": "DeviceA",
            "items": {
                "Temperature": {
                    "type": "omi:temperature",
                    "values": [{"v": 22.5, "t": 1000.0}]
                }
            }
        }"#;
        let obj = Object::from_json_str(json).unwrap();
        assert_eq!(obj.id, "DeviceA");
        let item = obj.get_item("Temperature").unwrap();
        assert_eq!(item.type_uri.as_deref(), Some("omi:temperature"));
        assert_eq!(item.values.len(), 1);
    }

    #[test]
    fn parse_object_nested() {
        let json = r#"{
            "id": "Root",
            "objects": {
                "Child": {
                    "id": "Child",
                    "items": {
                        "Voltage": {
                            "values": [{"v": 3.3}]
                        }
                    },
                    "objects": {
                        "GrandChild": {
                            "id": "GrandChild"
                        }
                    }
                }
            }
        }"#;
        let obj = Object::from_json_str(json).unwrap();
        assert_eq!(obj.id, "Root");
        let child = obj.get_child("Child").unwrap();
        assert_eq!(child.id, "Child");
        assert!(child.get_item("Voltage").is_some());
        assert!(child.get_child("GrandChild").is_some());
    }

    #[test]
    fn parse_object_ignores_unknown_fields() {
        let json = r#"{"id": "A", "firmware": "v2.0", "serial": 12345}"#;
        let obj = Object::from_json_str(json).unwrap();
        assert_eq!(obj.id, "A");
    }

    #[test]
    fn parse_object_missing_id() {
        let json = r#"{"type": "omi:device"}"#;
        let err = Object::from_json_str(json).unwrap_err();
        assert!(matches!(err, LiteParseError::MissingField { field: "id", .. }));
    }

    #[test]
    fn parse_object_null_optional_fields() {
        let json = r#"{"id": "A", "type": null, "desc": null, "items": null, "objects": null}"#;
        let obj = Object::from_json_str(json).unwrap();
        assert_eq!(obj.id, "A");
        assert!(obj.type_uri.is_none());
        assert!(obj.items.is_none());
        assert!(obj.objects.is_none());
    }

    // ----- objects_map -----

    #[test]
    fn parse_objects_map() {
        let json = r#"{
            "DeviceA": {"id": "DeviceA"},
            "DeviceB": {"id": "DeviceB", "type": "omi:sensor"}
        }"#;
        let mut parser = JsonParser::new(json.as_bytes());
        let map = parser.parse_objects_map().unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map["DeviceA"].id, "DeviceA");
        assert_eq!(map["DeviceB"].type_uri.as_deref(), Some("omi:sensor"));
    }

    #[test]
    fn parse_objects_map_empty() {
        let mut parser = JsonParser::new(b"{}");
        let map = parser.parse_objects_map().unwrap();
        assert!(map.is_empty());
    }

    // ----- WriteOp::Tree acceptance test -----

    #[test]
    fn parse_write_tree_nested_objects_and_items() {
        let json = r#"{
            "DeviceA": {
                "id": "DeviceA",
                "type": "omi:device",
                "items": {
                    "Temperature": {
                        "type": "omi:temperature",
                        "desc": "Room temp",
                        "values": [
                            {"v": 23.5, "t": 1001.0},
                            {"v": 22.0, "t": 1000.0}
                        ]
                    },
                    "Humidity": {
                        "values": [{"v": 65.0}]
                    }
                },
                "objects": {
                    "SubUnit": {
                        "id": "SubUnit",
                        "items": {
                            "Status": {
                                "values": [{"v": "active"}]
                            }
                        }
                    }
                }
            }
        }"#;
        let mut parser = JsonParser::new(json.as_bytes());
        let map = parser.parse_objects_map().unwrap();

        let dev = &map["DeviceA"];
        assert_eq!(dev.id, "DeviceA");
        assert_eq!(dev.type_uri.as_deref(), Some("omi:device"));

        let temp = dev.get_item("Temperature").unwrap();
        assert_eq!(temp.desc.as_deref(), Some("Room temp"));
        assert_eq!(temp.values.len(), 2);
        let newest = temp.values.newest(1);
        assert_eq!(newest[0].v, OmiValue::Number(23.5));

        let humidity = dev.get_item("Humidity").unwrap();
        assert_eq!(humidity.values.len(), 1);

        let sub = dev.get_child("SubUnit").unwrap();
        let status = sub.get_item("Status").unwrap();
        assert_eq!(status.values.newest(1)[0].v, OmiValue::Str("active".into()));
    }

    // ----- Error cases -----

    #[test]
    fn parse_truncated_input() {
        assert!(Object::from_json_str(r#"{"id": "A""#).is_err());
    }

    #[test]
    fn parse_empty_input() {
        assert!(Object::from_json_str("").is_err());
    }

    #[test]
    fn parse_trailing_data() {
        let err = Object::from_json_str(r#"{"id": "A"} extra"#).unwrap_err();
        assert!(matches!(err, LiteParseError::TrailingData { .. }));
    }

    #[test]
    fn parse_whitespace_only() {
        assert!(Object::from_json_str("   ").is_err());
    }

}
