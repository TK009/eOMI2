//! JSON parser for the lite-json parser.
//!
//! Provides [`JsonParser`] for parsing JSON bytes into O-DF types, and the
//! [`FromJson`] trait for convenient deserialization. Matches serde_json
//! behavior for all valid OMI messages (FR-012).

use std::collections::BTreeMap;

use crate::odf::{InfoItem, Object, OmiValue};
use crate::odf::value::{RingBuffer, Value};
use crate::omi::OmiMessage;
use crate::omi::Operation;
use crate::omi::cancel::CancelOp;
use crate::omi::delete::DeleteOp;
use crate::omi::error::ParseError;
use crate::omi::read::ReadOp;
use crate::omi::response::{ResponseBody, ResponseResult, ResultPayload, ItemStatus};
use crate::omi::write::{WriteItem, WriteOp, MAX_OBJECT_DEPTH, parsed_object_tree_depth};

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
/// Parses the OMI envelope (version, TTL, operation key) and delegates to
/// operation-specific sub-parsers. Unknown fields are silently ignored (FR-007).
/// Must produce identical results for all valid OMI messages (FR-012).
pub fn parse_omi_message(input: &str) -> Result<OmiMessage, ParseError> {
    let mut p = JsonParser::new(input.as_bytes());
    let msg = parse_envelope(&mut p)?;
    if !p.is_eof() {
        return Err(ParseError::InvalidJson(format!(
            "trailing content at byte {}",
            p.position()
        )));
    }
    Ok(msg)
}

// ---------------------------------------------------------------------------
// OMI envelope parsing
// ---------------------------------------------------------------------------

fn parse_envelope(p: &mut JsonParser) -> Result<OmiMessage, ParseError> {
    p.lexer().expect_token(&Token::ObjectStart).map_err(lpe)?;

    let mut version: Option<String> = None;
    let mut ttl: Option<i64> = None;
    let mut operation: Option<Operation> = None;
    let mut seen_ops: u8 = 0;

    const OP_READ: u8 = 1;
    const OP_WRITE: u8 = 2;
    const OP_DELETE: u8 = 4;
    const OP_CANCEL: u8 = 8;
    const OP_RESPONSE: u8 = 16;

    if !p.peek_is(&Token::ObjectEnd).map_err(lpe)? {
        loop {
            let key = p.expect_string().map_err(lpe)?;
            p.lexer().expect_token(&Token::Colon).map_err(lpe)?;

            match key.as_str() {
                "omi" => version = Some(p.expect_string().map_err(lpe)?),
                "ttl" => {
                    ttl = Some(expect_i64_from(p).map_err(lpe)?);
                }
                "read" => {
                    seen_ops |= OP_READ;
                    operation = Some(Operation::Read(parse_read_op(p)?));
                }
                "write" => {
                    seen_ops |= OP_WRITE;
                    operation = Some(Operation::Write(parse_write_op(p)?));
                }
                "delete" => {
                    seen_ops |= OP_DELETE;
                    operation = Some(Operation::Delete(parse_delete_op(p)?));
                }
                "cancel" => {
                    seen_ops |= OP_CANCEL;
                    operation = Some(Operation::Cancel(parse_cancel_op(p)?));
                }
                "response" => {
                    seen_ops |= OP_RESPONSE;
                    operation = Some(Operation::Response(parse_response_body(p)?));
                }
                _ => p.skip_value().map_err(lpe)?,
            }

            if p.peek_is(&Token::Comma).map_err(lpe)? {
                p.lexer().next_token().map_err(lpe)?;
            } else if p.peek_is(&Token::ObjectEnd).map_err(lpe)? {
                break;
            } else {
                return Err(lpe(LiteParseError::ExpectedToken {
                    expected: "',' or '}'",
                    pos: Pos::new(p.position()),
                }));
            }
        }
    }
    p.lexer().expect_token(&Token::ObjectEnd).map_err(lpe)?;

    let version = version.ok_or(ParseError::MissingField("omi"))?;
    if version != "1.0" {
        return Err(ParseError::UnsupportedVersion(version));
    }
    let ttl = ttl.ok_or(ParseError::MissingField("ttl"))?;
    let op_count = seen_ops.count_ones() as usize;
    if op_count != 1 {
        return Err(ParseError::InvalidOperationCount(op_count));
    }

    Ok(OmiMessage {
        version,
        ttl,
        operation: operation.unwrap(),
    })
}

// ---------------------------------------------------------------------------
// Operation sub-parsers
// ---------------------------------------------------------------------------

fn parse_read_op(p: &mut JsonParser) -> Result<ReadOp, ParseError> {
    p.lexer().expect_token(&Token::ObjectStart).map_err(lpe)?;

    let mut path: Option<String> = None;
    let mut rid: Option<String> = None;
    let mut newest: Option<u64> = None;
    let mut oldest: Option<u64> = None;
    let mut begin: Option<f64> = None;
    let mut end: Option<f64> = None;
    let mut depth: Option<u64> = None;
    let mut interval: Option<f64> = None;
    let mut callback: Option<String> = None;

    parse_op_fields(p, |p, key| {
        match key {
            "path" => path = Some(p.expect_string()?),
            "rid" => rid = Some(p.expect_string()?),
            "newest" => newest = Some(expect_u64_from(p)?),
            "oldest" => oldest = Some(expect_u64_from(p)?),
            "begin" => begin = Some(p.expect_f64()?),
            "end" => end = Some(p.expect_f64()?),
            "depth" => depth = Some(expect_u64_from(p)?),
            "interval" => interval = Some(p.expect_f64()?),
            "callback" => callback = Some(p.expect_string()?),
            _ => p.skip_value()?,
        }
        Ok(())
    })?;

    let op = ReadOp {
        path, rid, newest, oldest, begin, end, depth, interval, callback,
    };
    op.validate()?;
    Ok(op)
}

fn parse_write_op(p: &mut JsonParser) -> Result<WriteOp, ParseError> {
    p.lexer().expect_token(&Token::ObjectStart).map_err(lpe)?;

    let mut path: Option<String> = None;
    let mut v: Option<OmiValue> = None;
    let mut t: Option<f64> = None;
    let mut items: Option<Vec<WriteItem>> = None;
    let mut objects: Option<BTreeMap<String, Object>> = None;

    parse_op_fields(p, |p, key| {
        match key {
            "path" => path = Some(p.expect_string()?),
            "v" => v = Some(p.parse_omi_value()?),
            "t" => t = Some(p.expect_f64()?),
            "items" => items = Some(parse_write_items(p)?),
            "objects" => objects = Some(p.parse_objects_map()?),
            _ => p.skip_value()?,
        }
        Ok(())
    })?;

    let has_v = v.is_some();
    let has_items = items.is_some();
    let has_objects = objects.is_some();
    let form_count = has_v as u8 + has_items as u8 + has_objects as u8;

    if form_count == 0 {
        return Err(ParseError::MissingField("v, items, or objects"));
    }
    if has_v && has_items {
        return Err(ParseError::MutuallyExclusive("v", "items"));
    }
    if has_v && has_objects {
        return Err(ParseError::MutuallyExclusive("v", "objects"));
    }
    if has_items && has_objects {
        return Err(ParseError::MutuallyExclusive("items", "objects"));
    }

    if has_v {
        Ok(WriteOp::Single {
            path: path.ok_or(ParseError::MissingField("path"))?,
            v: v.unwrap(),
            t,
        })
    } else if has_items {
        let items = items.unwrap();
        if items.is_empty() {
            return Err(ParseError::InvalidField {
                field: "items",
                reason: "items array must not be empty".into(),
            });
        }
        Ok(WriteOp::Batch { items })
    } else {
        let objects = objects.unwrap();
        let depth = parsed_object_tree_depth(&objects);
        if depth > MAX_OBJECT_DEPTH {
            return Err(ParseError::InvalidField {
                field: "objects",
                reason: format!(
                    "nesting depth {} exceeds maximum of {}",
                    depth, MAX_OBJECT_DEPTH
                ),
            });
        }
        Ok(WriteOp::Tree {
            path: path.ok_or(ParseError::MissingField("path"))?,
            objects,
        })
    }
}

fn parse_write_items(p: &mut JsonParser) -> Result<Vec<WriteItem>, LiteParseError> {
    p.lexer().expect_token(&Token::ArrayStart)?;
    let mut items = Vec::new();
    if !p.peek_is(&Token::ArrayEnd)? {
        loop {
            items.push(parse_write_item(p)?);
            if p.peek_is(&Token::Comma)? {
                p.lexer().next_token()?;
            } else if p.peek_is(&Token::ArrayEnd)? {
                break;
            } else {
                return Err(LiteParseError::ExpectedToken {
                    expected: "',' or ']'",
                    pos: Pos::new(p.position()),
                });
            }
        }
    }
    p.lexer().expect_token(&Token::ArrayEnd)?;
    Ok(items)
}

fn parse_write_item(p: &mut JsonParser) -> Result<WriteItem, LiteParseError> {
    p.lexer().expect_token(&Token::ObjectStart)?;

    let mut path: Option<String> = None;
    let mut v: Option<OmiValue> = None;
    let mut t: Option<f64> = None;
    let obj_pos = Pos::new(p.position());

    if !p.peek_is(&Token::ObjectEnd)? {
        loop {
            let key = p.expect_string()?;
            p.lexer().expect_token(&Token::Colon)?;
            match key.as_str() {
                "path" => path = Some(p.expect_string()?),
                "v" => v = Some(p.parse_omi_value()?),
                "t" => t = Some(p.expect_f64()?),
                _ => p.skip_value()?,
            }
            if p.peek_is(&Token::Comma)? {
                p.lexer().next_token()?;
            } else if p.peek_is(&Token::ObjectEnd)? {
                break;
            } else {
                return Err(LiteParseError::ExpectedToken {
                    expected: "',' or '}'",
                    pos: Pos::new(p.position()),
                });
            }
        }
    }
    p.lexer().expect_token(&Token::ObjectEnd)?;

    Ok(WriteItem {
        path: path.ok_or(LiteParseError::MissingField { field: "path", pos: obj_pos })?,
        v: v.ok_or(LiteParseError::MissingField { field: "v", pos: obj_pos })?,
        t,
    })
}

fn parse_delete_op(p: &mut JsonParser) -> Result<DeleteOp, ParseError> {
    p.lexer().expect_token(&Token::ObjectStart).map_err(lpe)?;

    let mut path: Option<String> = None;

    parse_op_fields(p, |p, key| {
        match key {
            "path" => path = Some(p.expect_string()?),
            _ => p.skip_value()?,
        }
        Ok(())
    })?;

    let op = DeleteOp {
        path: path.ok_or(ParseError::MissingField("path"))?,
    };
    op.validate()?;
    Ok(op)
}

fn parse_cancel_op(p: &mut JsonParser) -> Result<CancelOp, ParseError> {
    p.lexer().expect_token(&Token::ObjectStart).map_err(lpe)?;

    let mut rid: Option<Vec<String>> = None;

    parse_op_fields(p, |p, key| {
        match key {
            "rid" => rid = Some(parse_string_array(p)?),
            _ => p.skip_value()?,
        }
        Ok(())
    })?;

    let op = CancelOp {
        rid: rid.ok_or(ParseError::MissingField("rid"))?,
    };
    op.validate()?;
    Ok(op)
}

fn parse_response_body(p: &mut JsonParser) -> Result<ResponseBody, ParseError> {
    p.lexer().expect_token(&Token::ObjectStart).map_err(lpe)?;

    let mut status: Option<u16> = None;
    let mut rid: Option<String> = None;
    let mut desc: Option<String> = None;
    let mut result: Option<ResponseResult> = None;

    parse_op_fields(p, |p, key| {
        match key {
            "status" => {
                let n = expect_u64_from(p)?;
                status = Some(u16::try_from(n).map_err(|_| LiteParseError::ExpectedToken {
                    expected: "status code (0..65535)",
                    pos: Pos::new(p.position()),
                })?);
            }
            "rid" => rid = Some(p.expect_string()?),
            "desc" => desc = Some(p.expect_string()?),
            "result" => {
                result = Some(parse_response_result(p)?);
            }
            _ => p.skip_value()?,
        }
        Ok(())
    })?;

    Ok(ResponseBody {
        status: status.ok_or(ParseError::MissingField("status"))?,
        rid,
        desc,
        result,
    })
}

fn parse_response_result(p: &mut JsonParser) -> Result<ResponseResult, LiteParseError> {
    if p.peek_is(&Token::ArrayStart)? {
        // Batch: array of ItemStatus
        p.lexer().expect_token(&Token::ArrayStart)?;
        let mut items = Vec::new();
        if !p.peek_is(&Token::ArrayEnd)? {
            loop {
                items.push(parse_item_status(p)?);
                if p.peek_is(&Token::Comma)? {
                    p.lexer().next_token()?;
                } else if p.peek_is(&Token::ArrayEnd)? {
                    break;
                } else {
                    return Err(LiteParseError::ExpectedToken {
                        expected: "',' or ']'",
                        pos: Pos::new(p.position()),
                    });
                }
            }
        }
        p.lexer().expect_token(&Token::ArrayEnd)?;
        Ok(ResponseResult::Batch(items))
    } else {
        // Single result — skip the value, store as Null payload
        p.skip_value()?;
        Ok(ResponseResult::Single(ResultPayload::Null))
    }
}

fn parse_item_status(p: &mut JsonParser) -> Result<ItemStatus, LiteParseError> {
    p.lexer().expect_token(&Token::ObjectStart)?;

    let mut path: Option<String> = None;
    let mut status: Option<u16> = None;
    let mut desc: Option<String> = None;
    let obj_pos = Pos::new(p.position());

    if !p.peek_is(&Token::ObjectEnd)? {
        loop {
            let key = p.expect_string()?;
            p.lexer().expect_token(&Token::Colon)?;
            match key.as_str() {
                "path" => path = Some(p.expect_string()?),
                "status" => {
                    let n = expect_u64_from(p)?;
                    status = Some(u16::try_from(n).map_err(|_| LiteParseError::ExpectedToken {
                        expected: "status code",
                        pos: Pos::new(p.position()),
                    })?);
                }
                "desc" => desc = Some(p.expect_string()?),
                _ => p.skip_value()?,
            }
            if p.peek_is(&Token::Comma)? {
                p.lexer().next_token()?;
            } else if p.peek_is(&Token::ObjectEnd)? {
                break;
            } else {
                return Err(LiteParseError::ExpectedToken {
                    expected: "',' or '}'",
                    pos: Pos::new(p.position()),
                });
            }
        }
    }
    p.lexer().expect_token(&Token::ObjectEnd)?;

    Ok(ItemStatus {
        path: path.ok_or(LiteParseError::MissingField { field: "path", pos: obj_pos })?,
        status: status.ok_or(LiteParseError::MissingField { field: "status", pos: obj_pos })?,
        desc,
    })
}

// ---------------------------------------------------------------------------
// Helpers for envelope/operation parsing
// ---------------------------------------------------------------------------

/// Convert LiteParseError to ParseError.
fn lpe(e: LiteParseError) -> ParseError {
    ParseError::InvalidJson(e.to_string())
}

/// Extract i64 from the next token (Integer or Number).
fn expect_i64_from(p: &mut JsonParser) -> Result<i64, LiteParseError> {
    match p.lexer().next_token()? {
        Some(Token::Integer(i)) => Ok(i),
        Some(Token::Number(n)) => {
            let i = n as i64;
            if i as f64 == n {
                Ok(i)
            } else {
                Err(LiteParseError::ExpectedToken {
                    expected: "integer",
                    pos: Pos::new(p.position()),
                })
            }
        }
        _ => Err(LiteParseError::ExpectedToken {
            expected: "integer",
            pos: Pos::new(p.position()),
        }),
    }
}

/// Extract u64 from the next token.
fn expect_u64_from(p: &mut JsonParser) -> Result<u64, LiteParseError> {
    let i = expect_i64_from(p)?;
    if i < 0 {
        Err(LiteParseError::ExpectedToken {
            expected: "non-negative integer",
            pos: Pos::new(p.position()),
        })
    } else {
        Ok(i as u64)
    }
}

/// Parse key-value pairs of an already-opened object, calling `handler` for each.
/// Handles commas and closing `}`.
fn parse_op_fields<F>(p: &mut JsonParser, mut handler: F) -> Result<(), ParseError>
where
    F: FnMut(&mut JsonParser, &str) -> Result<(), LiteParseError>,
{
    if !p.peek_is(&Token::ObjectEnd).map_err(lpe)? {
        loop {
            let key = p.expect_string().map_err(lpe)?;
            p.lexer().expect_token(&Token::Colon).map_err(lpe)?;
            handler(p, &key).map_err(lpe)?;

            if p.peek_is(&Token::Comma).map_err(lpe)? {
                p.lexer().next_token().map_err(lpe)?;
            } else if p.peek_is(&Token::ObjectEnd).map_err(lpe)? {
                break;
            } else {
                return Err(lpe(LiteParseError::ExpectedToken {
                    expected: "',' or '}'",
                    pos: Pos::new(p.position()),
                }));
            }
        }
    }
    p.lexer().expect_token(&Token::ObjectEnd).map_err(lpe)?;
    Ok(())
}

/// Parse a JSON array of strings.
fn parse_string_array(p: &mut JsonParser) -> Result<Vec<String>, LiteParseError> {
    p.lexer().expect_token(&Token::ArrayStart)?;
    let mut items = Vec::new();
    if !p.peek_is(&Token::ArrayEnd)? {
        loop {
            items.push(p.expect_string()?);
            if p.peek_is(&Token::Comma)? {
                p.lexer().next_token()?;
            } else if p.peek_is(&Token::ArrayEnd)? {
                break;
            } else {
                return Err(LiteParseError::ExpectedToken {
                    expected: "',' or ']'",
                    pos: Pos::new(p.position()),
                });
            }
        }
    }
    p.lexer().expect_token(&Token::ArrayEnd)?;
    Ok(items)
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

    // ----- OMI envelope parsing (T04) -----

    #[test]
    fn omi_parse_minimal_read() {
        let msg = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/DeviceA/Temperature"}}"#,
        ).unwrap();
        assert_eq!(msg.version, "1.0");
        assert_eq!(msg.ttl, 0);
        match &msg.operation {
            Operation::Read(op) => assert_eq!(op.path.as_deref(), Some("/DeviceA/Temperature")),
            _ => panic!("expected Read"),
        }
    }

    #[test]
    fn omi_parse_read_all_fields() {
        let msg = parse_omi_message(
            r#"{"omi":"1.0","ttl":5,"read":{"path":"/A/B","newest":10,"oldest":5,"begin":100.0,"end":200.0,"depth":3,"interval":10.0,"callback":"http://example.com/cb"}}"#,
        ).unwrap();
        match &msg.operation {
            Operation::Read(op) => {
                assert_eq!(op.newest, Some(10));
                assert_eq!(op.oldest, Some(5));
                assert_eq!(op.depth, Some(3));
                assert_eq!(op.interval, Some(10.0));
                assert_eq!(op.callback.as_deref(), Some("http://example.com/cb"));
            }
            _ => panic!("expected Read"),
        }
    }

    #[test]
    fn omi_parse_read_poll() {
        let msg = parse_omi_message(r#"{"omi":"1.0","ttl":0,"read":{"rid":"sub-123"}}"#).unwrap();
        match &msg.operation {
            Operation::Read(op) => assert_eq!(op.rid.as_deref(), Some("sub-123")),
            _ => panic!("expected Read"),
        }
    }

    #[test]
    fn omi_parse_write_single() {
        let msg = parse_omi_message(
            r#"{"omi":"1.0","ttl":10,"write":{"path":"/A/B","v":22.5}}"#,
        ).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Single { path, v, t }) => {
                assert_eq!(path, "/A/B");
                assert_eq!(*v, OmiValue::Number(22.5));
                assert!(t.is_none());
            }
            _ => panic!("expected Write Single"),
        }
    }

    #[test]
    fn omi_parse_write_batch() {
        let msg = parse_omi_message(
            r#"{"omi":"1.0","ttl":10,"write":{"items":[{"path":"/A","v":1},{"path":"/B","v":"hi"}]}}"#,
        ).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Batch { items }) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].path, "/A");
            }
            _ => panic!("expected Write Batch"),
        }
    }

    #[test]
    fn omi_parse_write_tree() {
        let msg = parse_omi_message(
            r#"{"omi":"1.0","ttl":10,"write":{"path":"/","objects":{"Dev":{"id":"Dev"}}}}"#,
        ).unwrap();
        match &msg.operation {
            Operation::Write(WriteOp::Tree { path, objects }) => {
                assert_eq!(path, "/");
                assert!(objects.contains_key("Dev"));
            }
            _ => panic!("expected Write Tree"),
        }
    }

    #[test]
    fn omi_parse_delete() {
        let msg = parse_omi_message(r#"{"omi":"1.0","ttl":0,"delete":{"path":"/DeviceA"}}"#).unwrap();
        match &msg.operation {
            Operation::Delete(op) => assert_eq!(op.path, "/DeviceA"),
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn omi_parse_cancel() {
        let msg = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"cancel":{"rid":["req-1","req-2"]}}"#,
        ).unwrap();
        match &msg.operation {
            Operation::Cancel(op) => assert_eq!(op.rid, vec!["req-1", "req-2"]),
            _ => panic!("expected Cancel"),
        }
    }

    #[test]
    fn omi_parse_response() {
        let msg = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"response":{"status":200,"desc":"OK"}}"#,
        ).unwrap();
        match &msg.operation {
            Operation::Response(body) => {
                assert_eq!(body.status, 200);
                assert_eq!(body.desc.as_deref(), Some("OK"));
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn omi_unknown_fields_ignored() {
        let msg = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"extra":{"nested":true},"read":{"path":"/A","unknown":[1,2]}}"#,
        ).unwrap();
        assert!(matches!(msg.operation, Operation::Read(_)));
    }

    #[test]
    fn omi_reject_missing_omi() {
        assert_eq!(
            parse_omi_message(r#"{"ttl":0,"read":{"path":"/A"}}"#).unwrap_err(),
            ParseError::MissingField("omi")
        );
    }

    #[test]
    fn omi_reject_wrong_version() {
        assert_eq!(
            parse_omi_message(r#"{"omi":"2.0","ttl":0,"read":{"path":"/A"}}"#).unwrap_err(),
            ParseError::UnsupportedVersion("2.0".into())
        );
    }

    #[test]
    fn omi_reject_missing_ttl() {
        assert_eq!(
            parse_omi_message(r#"{"omi":"1.0","read":{"path":"/A"}}"#).unwrap_err(),
            ParseError::MissingField("ttl")
        );
    }

    #[test]
    fn omi_reject_zero_operations() {
        assert_eq!(
            parse_omi_message(r#"{"omi":"1.0","ttl":0}"#).unwrap_err(),
            ParseError::InvalidOperationCount(0)
        );
    }

    #[test]
    fn omi_reject_multiple_operations() {
        assert_eq!(
            parse_omi_message(r#"{"omi":"1.0","ttl":0,"read":{"path":"/A"},"delete":{"path":"/B"}}"#)
                .unwrap_err(),
            ParseError::InvalidOperationCount(2)
        );
    }

    #[test]
    fn omi_reject_invalid_json() {
        assert!(matches!(parse_omi_message("not json").unwrap_err(), ParseError::InvalidJson(_)));
    }

    #[test]
    fn omi_negative_ttl_allowed() {
        let msg = parse_omi_message(r#"{"omi":"1.0","ttl":-1,"read":{"path":"/A"}}"#).unwrap();
        assert_eq!(msg.ttl, -1);
    }

    #[test]
    fn omi_duplicate_key_last_wins() {
        let msg = parse_omi_message(r#"{"omi":"2.0","omi":"1.0","ttl":0,"read":{"path":"/A"}}"#).unwrap();
        assert_eq!(msg.version, "1.0");
    }

    #[test]
    fn omi_reject_read_both_path_and_rid() {
        assert_eq!(
            parse_omi_message(r#"{"omi":"1.0","ttl":0,"read":{"path":"/A","rid":"r1"}}"#).unwrap_err(),
            ParseError::MutuallyExclusive("path", "rid")
        );
    }

    #[test]
    fn omi_reject_write_no_form() {
        assert_eq!(
            parse_omi_message(r#"{"omi":"1.0","ttl":0,"write":{"path":"/A"}}"#).unwrap_err(),
            ParseError::MissingField("v, items, or objects")
        );
    }

    #[test]
    fn omi_reject_write_empty_items() {
        assert_eq!(
            parse_omi_message(r#"{"omi":"1.0","ttl":0,"write":{"items":[]}}"#).unwrap_err(),
            ParseError::InvalidField {
                field: "items",
                reason: "items array must not be empty".into(),
            }
        );
    }

    #[test]
    fn omi_reject_cancel_empty_rid() {
        assert!(matches!(
            parse_omi_message(r#"{"omi":"1.0","ttl":0,"cancel":{"rid":[]}}"#).unwrap_err(),
            ParseError::InvalidField { .. }
        ));
    }

    #[test]
    fn omi_reject_delete_root() {
        assert!(matches!(
            parse_omi_message(r#"{"omi":"1.0","ttl":0,"delete":{"path":"/"}}"#).unwrap_err(),
            ParseError::InvalidField { .. }
        ));
    }

}
