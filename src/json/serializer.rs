//! JSON serializer for the lite-json parser.
//!
//! Provides [`JsonWriter`] for building JSON output incrementally, and
//! [`ToJson`] trait implementations for OMI-Lite protocol types.

use std::collections::BTreeMap;

use crate::odf::{InfoItem, Object, OmiValue};
use crate::odf::value::{RingBuffer, Value};

// ----- JsonWriter -----

/// Lightweight JSON writer that builds output in a `Vec<u8>`.
///
/// Handles comma placement automatically via a nesting stack. The caller
/// is responsible for matching `begin_object`/`end_object` and
/// `begin_array`/`end_array` pairs.
pub struct JsonWriter {
    buf: Vec<u8>,
    /// Stack tracking whether the current container has emitted its first element.
    /// `true` = first element (no comma needed), `false` = subsequent (comma needed).
    first: Vec<bool>,
    /// Set after [`key()`] so the following value does not emit a comma.
    after_key: bool,
}

impl JsonWriter {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            first: Vec::new(),
            after_key: false,
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
            first: Vec::new(),
            after_key: false,
        }
    }

    /// Emit a comma separator if required before the next value or key.
    fn pre_value(&mut self) {
        if self.after_key {
            self.after_key = false;
            return;
        }
        if let Some(first) = self.first.last_mut() {
            if !*first {
                self.buf.push(b',');
            }
            *first = false;
        }
    }

    // -- Containers --

    pub fn begin_object(&mut self) {
        self.pre_value();
        self.buf.push(b'{');
        self.first.push(true);
    }

    pub fn end_object(&mut self) {
        self.first.pop();
        self.buf.push(b'}');
    }

    pub fn begin_array(&mut self) {
        self.pre_value();
        self.buf.push(b'[');
        self.first.push(true);
    }

    pub fn end_array(&mut self) {
        self.first.pop();
        self.buf.push(b']');
    }

    // -- Keys --

    /// Write a JSON object key. Must be followed by exactly one value call.
    pub fn key(&mut self, k: &str) {
        self.pre_value();
        write_escaped_str(&mut self.buf, k);
        self.buf.push(b':');
        self.after_key = true;
    }

    // -- Primitive values --

    pub fn null(&mut self) {
        self.pre_value();
        self.buf.extend_from_slice(b"null");
    }

    pub fn bool_val(&mut self, b: bool) {
        self.pre_value();
        self.buf
            .extend_from_slice(if b { b"true" } else { b"false" });
    }

    pub fn i64_val(&mut self, n: i64) {
        self.pre_value();
        write_i64(&mut self.buf, n);
    }

    pub fn u16_val(&mut self, n: u16) {
        self.pre_value();
        write_u32(&mut self.buf, n as u32);
    }

    pub fn u64_val(&mut self, n: u64) {
        self.pre_value();
        write_u64(&mut self.buf, n);
    }

    pub fn f64_val(&mut self, n: f64) {
        self.pre_value();
        if n.is_nan() || n.is_infinite() {
            self.buf.extend_from_slice(b"null");
            return;
        }
        write_f64(&mut self.buf, n);
    }

    pub fn string(&mut self, s: &str) {
        self.pre_value();
        write_escaped_str(&mut self.buf, s);
    }

    /// Write a pre-formatted JSON string directly into the output.
    /// The caller must ensure the string is valid JSON.
    pub fn raw_json(&mut self, json: &str) {
        self.pre_value();
        self.buf.extend_from_slice(json.as_bytes());
    }

    // -- Convenience: key + value in one call --

    pub fn field_str(&mut self, k: &str, v: &str) {
        self.key(k);
        self.string(v);
    }

    pub fn field_i64(&mut self, k: &str, v: i64) {
        self.key(k);
        self.i64_val(v);
    }

    pub fn field_u16(&mut self, k: &str, v: u16) {
        self.key(k);
        self.u16_val(v);
    }

    pub fn field_u64(&mut self, k: &str, v: u64) {
        self.key(k);
        self.u64_val(v);
    }

    pub fn field_f64(&mut self, k: &str, v: f64) {
        self.key(k);
        self.f64_val(v);
    }

    pub fn field_bool(&mut self, k: &str, v: bool) {
        self.key(k);
        self.bool_val(v);
    }

    // -- Optional field helpers (FR-009: skip None) --

    pub fn field_str_opt(&mut self, k: &str, v: Option<&str>) {
        if let Some(v) = v {
            self.field_str(k, v);
        }
    }

    pub fn field_f64_opt(&mut self, k: &str, v: Option<f64>) {
        if let Some(v) = v {
            self.field_f64(k, v);
        }
    }

    pub fn field_u64_opt(&mut self, k: &str, v: Option<u64>) {
        if let Some(v) = v {
            self.field_u64(k, v);
        }
    }

    // -- Output --

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn into_string(self) -> String {
        // We only write valid UTF-8 (ASCII JSON + passthrough UTF-8 in strings).
        unsafe { String::from_utf8_unchecked(self.buf) }
    }
}

// ----- Number formatting (no extra dependencies) -----

fn write_i64(buf: &mut Vec<u8>, n: i64) {
    if n == 0 {
        buf.push(b'0');
        return;
    }
    if n == i64::MIN {
        buf.extend_from_slice(b"-9223372036854775808");
        return;
    }
    let neg = n < 0;
    if neg {
        buf.push(b'-');
    }
    write_u64(buf, n.unsigned_abs());
}

fn write_u64(buf: &mut Vec<u8>, n: u64) {
    if n == 0 {
        buf.push(b'0');
        return;
    }
    let start = buf.len();
    let mut val = n;
    while val > 0 {
        buf.push(b'0' + (val % 10) as u8);
        val /= 10;
    }
    buf[start..].reverse();
}

fn write_u32(buf: &mut Vec<u8>, n: u32) {
    write_u64(buf, n as u64);
}

fn write_f64(buf: &mut Vec<u8>, n: f64) {
    use std::io::Write;
    let start = buf.len();
    write!(buf, "{}", n).unwrap();
    // Ensure whole-number floats include a decimal point so they round-trip as
    // floats (not integers) when re-parsed.  NaN/Infinity are handled upstream
    // (emitted as `null`), so only finite integral values need the suffix.
    if n.is_finite() && n.fract() == 0.0 && !buf[start..].contains(&b'.') {
        buf.extend_from_slice(b".0");
    }
}

// ----- String escaping (FR-005) -----

const HEX: [u8; 16] = *b"0123456789abcdef";

fn write_escaped_str(buf: &mut Vec<u8>, s: &str) {
    buf.push(b'"');
    for &b in s.as_bytes() {
        match b {
            b'"' => buf.extend_from_slice(b"\\\""),
            b'\\' => buf.extend_from_slice(b"\\\\"),
            b'\n' => buf.extend_from_slice(b"\\n"),
            b'\r' => buf.extend_from_slice(b"\\r"),
            b'\t' => buf.extend_from_slice(b"\\t"),
            0x08 => buf.extend_from_slice(b"\\b"),
            0x0C => buf.extend_from_slice(b"\\f"),
            b if b < 0x20 => {
                buf.extend_from_slice(b"\\u00");
                buf.push(HEX[(b >> 4) as usize]);
                buf.push(HEX[(b & 0xF) as usize]);
            }
            b => buf.push(b),
        }
    }
    buf.push(b'"');
}

// ----- ToJson trait -----

/// Trait for types that can serialize themselves to JSON via [`JsonWriter`].
pub trait ToJson {
    fn write_json(&self, w: &mut JsonWriter);

    fn to_json_bytes(&self) -> Vec<u8> {
        let mut w = JsonWriter::new();
        self.write_json(&mut w);
        w.into_bytes()
    }

    fn to_json_string(&self) -> String {
        let mut w = JsonWriter::new();
        self.write_json(&mut w);
        w.into_string()
    }
}

// ----- ToJson implementations for O-DF types -----

impl ToJson for OmiValue {
    fn write_json(&self, w: &mut JsonWriter) {
        match self {
            OmiValue::Null => w.null(),
            OmiValue::Bool(b) => w.bool_val(*b),
            OmiValue::Number(n) => w.f64_val(*n),
            OmiValue::Str(s) => w.string(s),
        }
    }
}

impl ToJson for Value {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.key("v");
        self.v.write_json(w);
        w.field_f64_opt("t", self.t);
        w.end_object();
    }
}

impl ToJson for RingBuffer {
    fn write_json(&self, w: &mut JsonWriter) {
        let values = self.newest(self.len());
        w.begin_array();
        for v in &values {
            v.write_json(w);
        }
        w.end_array();
    }
}

impl ToJson for InfoItem {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.field_str_opt("type", self.type_uri.as_deref());
        w.field_str_opt("desc", self.desc.as_deref());
        if let Some(ref meta) = self.meta {
            w.key("meta");
            w.begin_object();
            for (k, v) in meta {
                w.key(k);
                v.write_json(w);
            }
            w.end_object();
        }
        if !self.values.is_empty() {
            w.key("values");
            self.values.write_json(w);
        }
        w.end_object();
    }
}

impl ToJson for Object {
    fn write_json(&self, w: &mut JsonWriter) {
        self.write_json_with_depth(w, usize::MAX);
    }
}

impl Object {
    /// Serialize this object with a depth limit via [`JsonWriter`].
    ///
    /// Depth 0 = only id/type/desc, no items or child objects.
    /// Depth 1 = include direct items and child object shells (depth 0).
    /// Use `usize::MAX` for unlimited depth.
    pub fn write_json_with_depth(&self, w: &mut JsonWriter, depth: usize) {
        w.begin_object();
        w.field_str("id", &self.id);
        w.field_str_opt("type", self.type_uri.as_deref());
        w.field_str_opt("desc", self.desc.as_deref());
        if depth > 0 {
            if let Some(ref items) = self.items {
                w.key("items");
                w.begin_object();
                for (k, item) in items {
                    w.key(k);
                    item.write_json(w);
                }
                w.end_object();
            }
            if let Some(ref objects) = self.objects {
                w.key("objects");
                w.begin_object();
                for (k, obj) in objects {
                    w.key(k);
                    obj.write_json_with_depth(w, depth - 1);
                }
                w.end_object();
            }
        }
        w.end_object();
    }
}

// ----- ToJson for response types -----

use crate::omi::response::{ItemStatus, ResponseBody, ResponseResult, ResultPayload};

impl ToJson for ResultPayload {
    fn write_json(&self, w: &mut JsonWriter) {
        match self {
            ResultPayload::Null => w.null(),
            ResultPayload::ReadValues { path, values } => {
                w.begin_object();
                w.field_str("path", path);
                w.key("values");
                w.begin_array();
                for v in values {
                    v.write_json(w);
                }
                w.end_array();
                w.end_object();
            }
            #[cfg(feature = "json")]
            ResultPayload::Json(v) => {
                // For Json variant, fall back to serde_json serialization.
                let s = serde_json::to_string(v)
                    .expect("ResultPayload::Json serialization should not fail");
                w.raw_json(&s);
            }
        }
    }
}

impl ToJson for ItemStatus {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.field_str("path", &self.path);
        w.field_u16("status", self.status);
        w.field_str_opt("desc", self.desc.as_deref());
        w.end_object();
    }
}

impl ToJson for ResponseResult {
    fn write_json(&self, w: &mut JsonWriter) {
        match self {
            ResponseResult::Batch(items) => {
                w.begin_array();
                for item in items {
                    item.write_json(w);
                }
                w.end_array();
            }
            ResponseResult::Single(payload) => {
                payload.write_json(w);
            }
        }
    }
}

impl ToJson for ResponseBody {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.field_u16("status", self.status);
        if let Some(ref rid) = self.rid {
            w.field_str("rid", rid);
        }
        if let Some(ref desc) = self.desc {
            w.field_str("desc", desc);
        }
        if let Some(ref result) = self.result {
            w.key("result");
            result.write_json(w);
        }
        w.end_object();
    }
}

// ----- ToJson for OmiMessage and Operation types -----

use crate::omi::{OmiMessage, Operation};
use crate::omi::read::ReadOp;
use crate::omi::write::{WriteOp, WriteItem};
use crate::omi::delete::DeleteOp;
use crate::omi::cancel::CancelOp;

impl ToJson for OmiMessage {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.field_str("omi", &self.version);
        w.field_i64("ttl", self.ttl);
        match &self.operation {
            Operation::Read(op) => {
                w.key("read");
                op.write_json(w);
            }
            Operation::Write(op) => {
                w.key("write");
                op.write_json(w);
            }
            Operation::Delete(op) => {
                w.key("delete");
                op.write_json(w);
            }
            Operation::Cancel(op) => {
                w.key("cancel");
                op.write_json(w);
            }
            Operation::Response(body) => {
                w.key("response");
                body.write_json(w);
            }
        }
        w.end_object();
    }
}

impl ToJson for ReadOp {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.field_str_opt("path", self.path.as_deref());
        w.field_str_opt("rid", self.rid.as_deref());
        w.field_u64_opt("newest", self.newest);
        w.field_u64_opt("oldest", self.oldest);
        w.field_f64_opt("begin", self.begin);
        w.field_f64_opt("end", self.end);
        w.field_u64_opt("depth", self.depth);
        w.field_f64_opt("interval", self.interval);
        w.field_str_opt("callback", self.callback.as_deref());
        w.end_object();
    }
}

impl ToJson for WriteOp {
    fn write_json(&self, w: &mut JsonWriter) {
        match self {
            WriteOp::Single { path, v, t } => {
                w.begin_object();
                w.field_str("path", path);
                w.key("v");
                v.write_json(w);
                w.field_f64_opt("t", *t);
                w.end_object();
            }
            WriteOp::Batch { items } => {
                w.begin_object();
                w.key("items");
                w.begin_array();
                for item in items {
                    item.write_json(w);
                }
                w.end_array();
                w.end_object();
            }
            WriteOp::Tree { path, objects } => {
                w.begin_object();
                w.field_str("path", path);
                w.key("objects");
                w.begin_object();
                for (k, obj) in objects {
                    w.key(k);
                    obj.write_json(w);
                }
                w.end_object();
                w.end_object();
            }
        }
    }
}

impl ToJson for WriteItem {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.field_str("path", &self.path);
        w.key("v");
        self.v.write_json(w);
        w.field_f64_opt("t", self.t);
        w.end_object();
    }
}

impl ToJson for DeleteOp {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.field_str("path", &self.path);
        w.end_object();
    }
}

impl ToJson for CancelOp {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.key("rid");
        w.begin_array();
        for rid in &self.rid {
            w.string(rid);
        }
        w.end_array();
        w.end_object();
    }
}

// ----- ToJson for captive portal types -----

#[cfg(feature = "std")]
use crate::captive_portal::{ScannedNetwork, ConnectionStatus, ConnectionState};

#[cfg(feature = "std")]
impl ToJson for ScannedNetwork {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.field_str("ssid", &self.ssid);
        w.key("rssi");
        w.i64_val(self.rssi as i64);
        w.field_str("auth", &self.auth);
        w.end_object();
    }
}

#[cfg(feature = "std")]
impl ToJson for ConnectionState {
    fn write_json(&self, w: &mut JsonWriter) {
        w.string(match self {
            ConnectionState::Idle => "idle",
            ConnectionState::Connecting => "connecting",
            ConnectionState::Connected => "connected",
            ConnectionState::Failed => "failed",
        });
    }
}

#[cfg(feature = "std")]
impl ToJson for ConnectionStatus {
    fn write_json(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.key("state");
        self.state.write_json(w);
        w.field_str_opt("message", self.message.as_deref());
        w.field_str_opt("ip", self.ip.as_deref());
        w.end_object();
    }
}

// ----- OMI message serialization helpers -----

/// Write the OMI message envelope. `write_op` writes the operation body.
pub fn write_omi_envelope(
    w: &mut JsonWriter,
    version: &str,
    ttl: i64,
    op_key: &str,
    write_op: impl FnOnce(&mut JsonWriter),
) {
    w.begin_object();
    w.field_str("omi", version);
    w.field_i64("ttl", ttl);
    w.key(op_key);
    write_op(w);
    w.end_object();
}

/// Write a read operation body.
pub fn write_read_op(
    w: &mut JsonWriter,
    path: Option<&str>,
    rid: Option<&str>,
    newest: Option<u64>,
    oldest: Option<u64>,
    begin: Option<f64>,
    end: Option<f64>,
    depth: Option<u64>,
    interval: Option<f64>,
    callback: Option<&str>,
) {
    w.begin_object();
    w.field_str_opt("path", path);
    w.field_str_opt("rid", rid);
    w.field_u64_opt("newest", newest);
    w.field_u64_opt("oldest", oldest);
    w.field_f64_opt("begin", begin);
    w.field_f64_opt("end", end);
    w.field_u64_opt("depth", depth);
    w.field_f64_opt("interval", interval);
    w.field_str_opt("callback", callback);
    w.end_object();
}

/// Write a single-value write operation body.
pub fn write_write_single(w: &mut JsonWriter, path: &str, v: &OmiValue, t: Option<f64>) {
    w.begin_object();
    w.field_str("path", path);
    w.key("v");
    v.write_json(w);
    w.field_f64_opt("t", t);
    w.end_object();
}

/// Write one item in a batch write: `{"path": "...", "v": ..., "t": ...}`.
pub fn write_batch_item(w: &mut JsonWriter, path: &str, v: &OmiValue, t: Option<f64>) {
    w.begin_object();
    w.field_str("path", path);
    w.key("v");
    v.write_json(w);
    w.field_f64_opt("t", t);
    w.end_object();
}

/// Write a tree write operation body.
pub fn write_write_tree(w: &mut JsonWriter, path: &str, objects: &BTreeMap<String, Object>) {
    w.begin_object();
    w.field_str("path", path);
    w.key("objects");
    w.begin_object();
    for (k, obj) in objects {
        w.key(k);
        obj.write_json(w);
    }
    w.end_object();
    w.end_object();
}

/// Write a delete operation body.
pub fn write_delete_op(w: &mut JsonWriter, path: &str) {
    w.begin_object();
    w.field_str("path", path);
    w.end_object();
}

/// Write a cancel operation body.
pub fn write_cancel_op(w: &mut JsonWriter, rids: &[&str]) {
    w.begin_object();
    w.key("rid");
    w.begin_array();
    for rid in rids {
        w.string(rid);
    }
    w.end_array();
    w.end_object();
}

/// Write a response body. Use `write_result` to emit the result value if present.
pub fn write_response_body(
    w: &mut JsonWriter,
    status: u16,
    rid: Option<&str>,
    desc: Option<&str>,
    write_result: Option<&dyn Fn(&mut JsonWriter)>,
) {
    w.begin_object();
    w.field_u16("status", status);
    w.field_str_opt("rid", rid);
    w.field_str_opt("desc", desc);
    if let Some(write_fn) = write_result {
        w.key("result");
        write_fn(w);
    }
    w.end_object();
}

/// Write a single item-status entry: `{"path": "...", "status": N, "desc": "..."}`.
pub fn write_item_status(w: &mut JsonWriter, path: &str, status: u16, desc: Option<&str>) {
    w.begin_object();
    w.field_str("path", path);
    w.field_u16("status", status);
    w.field_str_opt("desc", desc);
    w.end_object();
}

// ----- Tests -----

#[cfg(all(test, feature = "json"))]
mod tests {
    use super::*;

    /// Parse JSON string with serde_json for cross-checking.
    fn parse(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap_or_else(|e| panic!("invalid JSON: {e}\n{json}"))
    }

    // -- JsonWriter basics --

    #[test]
    fn empty_object() {
        let mut w = JsonWriter::new();
        w.begin_object();
        w.end_object();
        assert_eq!(w.into_string(), "{}");
    }

    #[test]
    fn empty_array() {
        let mut w = JsonWriter::new();
        w.begin_array();
        w.end_array();
        assert_eq!(w.into_string(), "[]");
    }

    #[test]
    fn object_with_fields() {
        let mut w = JsonWriter::new();
        w.begin_object();
        w.field_str("name", "test");
        w.field_i64("count", 42);
        w.field_bool("ok", true);
        w.end_object();
        let v = parse(&w.into_string());
        assert_eq!(v["name"], "test");
        assert_eq!(v["count"], 42);
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn array_of_values() {
        let mut w = JsonWriter::new();
        w.begin_array();
        w.string("a");
        w.i64_val(1);
        w.bool_val(false);
        w.null();
        w.end_array();
        let v = parse(&w.into_string());
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[0], "a");
        assert_eq!(arr[1], 1);
        assert_eq!(arr[2], false);
        assert!(arr[3].is_null());
    }

    #[test]
    fn nested_objects_in_array() {
        let mut w = JsonWriter::new();
        w.begin_array();
        w.begin_object();
        w.field_str("x", "1");
        w.end_object();
        w.begin_object();
        w.field_str("x", "2");
        w.end_object();
        w.end_array();
        let v = parse(&w.into_string());
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["x"], "1");
        assert_eq!(arr[1]["x"], "2");
    }

    #[test]
    fn nested_object_in_object() {
        let mut w = JsonWriter::new();
        w.begin_object();
        w.field_str("a", "1");
        w.key("nested");
        w.begin_object();
        w.field_i64("b", 2);
        w.end_object();
        w.field_str("c", "3");
        w.end_object();
        let v = parse(&w.into_string());
        assert_eq!(v["a"], "1");
        assert_eq!(v["nested"]["b"], 2);
        assert_eq!(v["c"], "3");
    }

    #[test]
    fn optional_fields_skip_none() {
        let mut w = JsonWriter::new();
        w.begin_object();
        w.field_str("present", "yes");
        w.field_str_opt("missing", None);
        w.field_f64_opt("also_missing", None);
        w.field_u64_opt("gone", None);
        w.field_str_opt("here", Some("ok"));
        w.end_object();
        let v = parse(&w.into_string());
        assert_eq!(v["present"], "yes");
        assert!(v.get("missing").is_none());
        assert!(v.get("also_missing").is_none());
        assert!(v.get("gone").is_none());
        assert_eq!(v["here"], "ok");
    }

    // -- String escaping --

    #[test]
    fn escape_quotes_and_backslash() {
        let mut w = JsonWriter::new();
        w.string(r#"say "hello" \world"#);
        let s = w.into_string();
        assert_eq!(s, r#""say \"hello\" \\world""#);
        // Verify it round-trips through serde_json
        let v: String = serde_json::from_str(&s).unwrap();
        assert_eq!(v, r#"say "hello" \world"#);
    }

    #[test]
    fn escape_control_characters() {
        let mut w = JsonWriter::new();
        w.string("line1\nline2\ttab\r\n");
        let s = w.into_string();
        let v: String = serde_json::from_str(&s).unwrap();
        assert_eq!(v, "line1\nline2\ttab\r\n");
    }

    #[test]
    fn escape_backspace_and_formfeed() {
        let mut w = JsonWriter::new();
        w.string("\x08\x0C");
        let s = w.into_string();
        assert_eq!(s, r#""\b\f""#);
        let v: String = serde_json::from_str(&s).unwrap();
        assert_eq!(v, "\x08\x0C");
    }

    #[test]
    fn escape_other_control_chars() {
        let mut w = JsonWriter::new();
        w.string("\x01\x1F");
        let s = w.into_string();
        assert!(s.contains("\\u0001"));
        assert!(s.contains("\\u001f"));
        let v: String = serde_json::from_str(&s).unwrap();
        assert_eq!(v, "\x01\x1F");
    }

    #[test]
    fn utf8_passthrough() {
        let mut w = JsonWriter::new();
        w.string("héllo wörld 日本語");
        let s = w.into_string();
        let v: String = serde_json::from_str(&s).unwrap();
        assert_eq!(v, "héllo wörld 日本語");
    }

    // -- Number formatting --

    #[test]
    fn format_integers() {
        let mut w = JsonWriter::new();
        w.begin_array();
        w.i64_val(0);
        w.i64_val(42);
        w.i64_val(-1);
        w.i64_val(i64::MAX);
        w.i64_val(i64::MIN);
        w.end_array();
        let v = parse(&w.into_string());
        let arr = v.as_array().unwrap();
        assert_eq!(arr[0], 0);
        assert_eq!(arr[1], 42);
        assert_eq!(arr[2], -1);
        assert_eq!(arr[3], i64::MAX);
        assert_eq!(arr[4], i64::MIN);
    }

    #[test]
    fn format_u16() {
        let mut w = JsonWriter::new();
        w.begin_array();
        w.u16_val(0);
        w.u16_val(200);
        w.u16_val(404);
        w.u16_val(u16::MAX);
        w.end_array();
        let v = parse(&w.into_string());
        let arr = v.as_array().unwrap();
        assert_eq!(arr[0], 0);
        assert_eq!(arr[1], 200);
        assert_eq!(arr[2], 404);
        assert_eq!(arr[3], u16::MAX);
    }

    #[test]
    fn format_f64() {
        let mut w = JsonWriter::new();
        w.begin_array();
        w.f64_val(0.0);
        w.f64_val(3.14);
        w.f64_val(-1.5);
        w.f64_val(42.0);
        w.f64_val(1700000000.0);
        w.end_array();
        let v = parse(&w.into_string());
        let arr = v.as_array().unwrap();
        assert_eq!(arr[0], 0.0);
        assert_eq!(arr[1], 3.14);
        assert_eq!(arr[2], -1.5);
        assert_eq!(arr[3], 42.0);
        assert_eq!(arr[4], 1700000000.0);
    }

    #[test]
    fn f64_nan_becomes_null() {
        let mut w = JsonWriter::new();
        w.f64_val(f64::NAN);
        assert_eq!(w.into_string(), "null");
    }

    #[test]
    fn f64_infinity_becomes_null() {
        let mut w = JsonWriter::new();
        w.f64_val(f64::INFINITY);
        assert_eq!(w.into_string(), "null");
    }

    // -- OmiValue --

    #[test]
    fn omi_value_null() {
        assert_eq!(OmiValue::Null.to_json_string(), "null");
    }

    #[test]
    fn omi_value_bool() {
        assert_eq!(OmiValue::Bool(true).to_json_string(), "true");
        assert_eq!(OmiValue::Bool(false).to_json_string(), "false");
    }

    #[test]
    fn omi_value_number() {
        let s = OmiValue::Number(22.5).to_json_string();
        let v = parse(&s);
        assert_eq!(v, 22.5);
    }

    #[test]
    fn omi_value_string() {
        let s = OmiValue::Str("hello".into()).to_json_string();
        let v: String = serde_json::from_str(&s).unwrap();
        assert_eq!(v, "hello");
    }

    #[test]
    fn omi_value_string_with_escapes() {
        let s = OmiValue::Str("line1\nline2".into()).to_json_string();
        let v: String = serde_json::from_str(&s).unwrap();
        assert_eq!(v, "line1\nline2");
    }

    // -- Value --

    #[test]
    fn value_with_timestamp() {
        let val = Value::new(OmiValue::Number(22.5), Some(1000.0));
        let s = val.to_json_string();
        let v = parse(&s);
        assert_eq!(v["v"], 22.5);
        assert_eq!(v["t"], 1000.0);
    }

    #[test]
    fn value_without_timestamp() {
        let val = Value::new(OmiValue::Bool(true), None);
        let s = val.to_json_string();
        let v = parse(&s);
        assert_eq!(v["v"], true);
        assert!(v.get("t").is_none());
    }

    // -- RingBuffer --

    #[test]
    fn ringbuffer_newest_first() {
        let mut rb = RingBuffer::new(5);
        rb.push(Value::new(OmiValue::Number(1.0), Some(100.0)));
        rb.push(Value::new(OmiValue::Number(2.0), Some(200.0)));
        rb.push(Value::new(OmiValue::Number(3.0), Some(300.0)));
        let s = rb.to_json_string();
        let v = parse(&s);
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["v"], 3.0); // newest first
        assert_eq!(arr[1]["v"], 2.0);
        assert_eq!(arr[2]["v"], 1.0);
    }

    #[test]
    fn ringbuffer_empty() {
        let rb = RingBuffer::new(5);
        assert_eq!(rb.to_json_string(), "[]");
    }

    // -- InfoItem --

    #[test]
    fn infoitem_empty() {
        let item = InfoItem::new(10);
        let s = item.to_json_string();
        let v = parse(&s);
        assert!(v.get("type").is_none());
        assert!(v.get("desc").is_none());
        assert!(v.get("meta").is_none());
        assert!(v.get("values").is_none());
    }

    #[test]
    fn infoitem_with_all_fields() {
        let mut item = InfoItem::new(10);
        item.type_uri = Some("omi:temperature".into());
        item.desc = Some("Room temp".into());
        let mut meta = BTreeMap::new();
        meta.insert("writable".into(), OmiValue::Bool(true));
        item.meta = Some(meta);
        item.add_value(OmiValue::Number(22.5), Some(1000.0));

        let s = item.to_json_string();
        let v = parse(&s);
        assert_eq!(v["type"], "omi:temperature");
        assert_eq!(v["desc"], "Room temp");
        assert_eq!(v["meta"]["writable"], true);
        assert_eq!(v["values"][0]["v"], 22.5);
    }

    // -- Object --

    #[test]
    fn object_minimal() {
        let obj = Object::new("DeviceA");
        let s = obj.to_json_string();
        let v = parse(&s);
        assert_eq!(v["id"], "DeviceA");
        assert!(v.get("type").is_none());
        assert!(v.get("items").is_none());
        assert!(v.get("objects").is_none());
    }

    #[test]
    fn object_with_children() {
        let mut root = Object::new("Root");
        root.type_uri = Some("omi:building".into());
        let mut child = Object::new("Floor1");
        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Number(22.5), None);
        child.add_item("Temp".into(), item);
        root.add_child(child);

        let s = root.to_json_string();
        let v = parse(&s);
        assert_eq!(v["id"], "Root");
        assert_eq!(v["type"], "omi:building");
        assert_eq!(v["objects"]["Floor1"]["id"], "Floor1");
        assert_eq!(v["objects"]["Floor1"]["items"]["Temp"]["values"][0]["v"], 22.5);
    }

    // -- OMI message serialization --

    #[test]
    fn serialize_read_one_time() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "read", |w| {
            write_read_op(w, Some("/DeviceA/Temperature"), None, Some(1), None, None, None, None, None, None);
        });
        let v = parse(&w.into_string());
        assert_eq!(v["omi"], "1.0");
        assert_eq!(v["ttl"], 0);
        assert_eq!(v["read"]["path"], "/DeviceA/Temperature");
        assert_eq!(v["read"]["newest"], 1);
        assert!(v["read"].get("rid").is_none());
    }

    #[test]
    fn serialize_read_subscription() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 60, "read", |w| {
            write_read_op(
                w,
                Some("/DeviceA/Temperature"),
                None, None, None, None, None, None,
                Some(10.0),
                Some("http://client.example.com/omi"),
            );
        });
        let v = parse(&w.into_string());
        assert_eq!(v["ttl"], 60);
        assert_eq!(v["read"]["interval"], 10.0);
        assert_eq!(v["read"]["callback"], "http://client.example.com/omi");
    }

    #[test]
    fn serialize_read_poll() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "read", |w| {
            write_read_op(w, None, Some("sub-abc-123"), None, None, None, None, None, None, None);
        });
        let v = parse(&w.into_string());
        assert_eq!(v["read"]["rid"], "sub-abc-123");
        assert!(v["read"].get("path").is_none());
    }

    #[test]
    fn serialize_read_with_all_params() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 5, "read", |w| {
            write_read_op(
                w,
                Some("/A/B"),
                None,
                Some(10),
                Some(5),
                Some(100.0),
                Some(2000.0),
                Some(3),
                None,
                None,
            );
        });
        let v = parse(&w.into_string());
        assert_eq!(v["read"]["path"], "/A/B");
        assert_eq!(v["read"]["newest"], 10);
        assert_eq!(v["read"]["oldest"], 5);
        assert_eq!(v["read"]["begin"], 100.0);
        assert_eq!(v["read"]["end"], 2000.0);
        assert_eq!(v["read"]["depth"], 3);
    }

    #[test]
    fn serialize_write_single() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 10, "write", |w| {
            write_write_single(w, "/A/B", &OmiValue::Number(22.5), Some(1700000000.0));
        });
        let v = parse(&w.into_string());
        assert_eq!(v["omi"], "1.0");
        assert_eq!(v["ttl"], 10);
        assert_eq!(v["write"]["path"], "/A/B");
        assert_eq!(v["write"]["v"], 22.5);
        assert_eq!(v["write"]["t"], 1700000000.0);
    }

    #[test]
    fn serialize_write_single_no_timestamp() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 10, "write", |w| {
            write_write_single(w, "/A/B", &OmiValue::Str("hello".into()), None);
        });
        let v = parse(&w.into_string());
        assert_eq!(v["write"]["v"], "hello");
        assert!(v["write"].get("t").is_none());
    }

    #[test]
    fn serialize_write_batch() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 10, "write", |w| {
            w.begin_object();
            w.key("items");
            w.begin_array();
            write_batch_item(w, "/House/Room1/Temp", &OmiValue::Number(22.5), None);
            write_batch_item(w, "/House/Room1/Humidity", &OmiValue::Number(45.0), Some(1000.0));
            w.end_array();
            w.end_object();
        });
        let v = parse(&w.into_string());
        let items = v["write"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["path"], "/House/Room1/Temp");
        assert_eq!(items[0]["v"], 22.5);
        assert!(items[0].get("t").is_none());
        assert_eq!(items[1]["path"], "/House/Room1/Humidity");
        assert_eq!(items[1]["t"], 1000.0);
    }

    #[test]
    fn serialize_write_tree() {
        let mut objects = BTreeMap::new();
        let mut obj = Object::new("SmartHouse");
        obj.type_uri = Some("omi:building".into());
        objects.insert("SmartHouse".into(), obj);

        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 10, "write", |w| {
            write_write_tree(w, "/", &objects);
        });
        let v = parse(&w.into_string());
        assert_eq!(v["write"]["path"], "/");
        assert_eq!(v["write"]["objects"]["SmartHouse"]["id"], "SmartHouse");
        assert_eq!(v["write"]["objects"]["SmartHouse"]["type"], "omi:building");
    }

    #[test]
    fn serialize_delete() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "delete", |w| {
            write_delete_op(w, "/DeviceA/Temperature");
        });
        let v = parse(&w.into_string());
        assert_eq!(v["delete"]["path"], "/DeviceA/Temperature");
    }

    #[test]
    fn serialize_cancel() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "cancel", |w| {
            write_cancel_op(w, &["sub-abc-123", "sub-def-456"]);
        });
        let v = parse(&w.into_string());
        let rids = v["cancel"]["rid"].as_array().unwrap();
        assert_eq!(rids.len(), 2);
        assert_eq!(rids[0], "sub-abc-123");
        assert_eq!(rids[1], "sub-def-456");
    }

    #[test]
    fn serialize_response_ok() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "response", |w| {
            write_response_body(w, 200, None, None, Some(&|w| {
                w.begin_object();
                w.field_f64("temperature", 22.5);
                w.end_object();
            }));
        });
        let v = parse(&w.into_string());
        assert_eq!(v["response"]["status"], 200);
        assert_eq!(v["response"]["result"]["temperature"], 22.5);
        assert!(v["response"].get("rid").is_none());
        assert!(v["response"].get("desc").is_none());
    }

    #[test]
    fn serialize_response_not_found() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "response", |w| {
            write_response_body(w, 404, None, Some("Path not found: /Missing/Path"), None);
        });
        let v = parse(&w.into_string());
        assert_eq!(v["response"]["status"], 404);
        assert_eq!(v["response"]["desc"], "Path not found: /Missing/Path");
        assert!(v["response"].get("result").is_none());
    }

    #[test]
    fn serialize_response_with_rid() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "response", |w| {
            write_response_body(w, 200, Some("sub-1"), None, Some(&|w| {
                w.begin_object();
                w.field_str("path", "/Device/Sensor");
                w.end_object();
            }));
        });
        let v = parse(&w.into_string());
        assert_eq!(v["response"]["rid"], "sub-1");
    }

    #[test]
    fn serialize_response_batch() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", 0, "response", |w| {
            write_response_body(w, 200, None, None, Some(&|w| {
                w.begin_array();
                write_item_status(w, "/A/B", 200, None);
                write_item_status(w, "/A/C", 404, Some("not found"));
                w.end_array();
            }));
        });
        let v = parse(&w.into_string());
        assert_eq!(v["response"]["status"], 200);
        let result = v["response"]["result"].as_array().unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["path"], "/A/B");
        assert_eq!(result[0]["status"], 200);
        assert!(result[0].get("desc").is_none());
        assert_eq!(result[1]["path"], "/A/C");
        assert_eq!(result[1]["status"], 404);
        assert_eq!(result[1]["desc"], "not found");
    }

    #[test]
    fn serialize_negative_ttl() {
        let mut w = JsonWriter::new();
        write_omi_envelope(&mut w, "1.0", -1, "read", |w| {
            write_read_op(w, Some("/A"), None, None, None, None, None, None, None, None);
        });
        let v = parse(&w.into_string());
        assert_eq!(v["ttl"], -1);
    }

    #[test]
    fn serialize_write_value_types() {
        // Test each OmiValue variant in a write operation
        for (val, expected_json) in [
            (OmiValue::Null, serde_json::Value::Null),
            (OmiValue::Bool(true), serde_json::json!(true)),
            (OmiValue::Number(3.14), serde_json::json!(3.14)),
            (OmiValue::Str("hi".into()), serde_json::json!("hi")),
        ] {
            let mut w = JsonWriter::new();
            write_omi_envelope(&mut w, "1.0", 0, "write", |w| {
                write_write_single(w, "/A/B", &val, None);
            });
            let v = parse(&w.into_string());
            assert_eq!(v["write"]["v"], expected_json);
        }
    }

    #[test]
    fn with_capacity_works() {
        let mut w = JsonWriter::with_capacity(256);
        w.begin_object();
        w.field_str("test", "value");
        w.end_object();
        let v = parse(&w.into_string());
        assert_eq!(v["test"], "value");
    }

    // -- Depth-limited Object serialization --

    #[test]
    fn object_depth_zero_only_scalars() {
        let mut root = Object::new("Root");
        root.type_uri = Some("omi:building".into());
        root.desc = Some("HQ".into());
        let mut child = Object::new("Floor1");
        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Number(22.5), Some(1000.0));
        child.add_item("Temp".into(), item);
        root.add_child(child);

        let mut w = JsonWriter::new();
        root.write_json_with_depth(&mut w, 0);
        let v = parse(&w.into_string());
        assert_eq!(v["id"], "Root");
        assert_eq!(v["type"], "omi:building");
        assert_eq!(v["desc"], "HQ");
        assert!(v.get("items").is_none());
        assert!(v.get("objects").is_none());
    }

    #[test]
    fn object_depth_one_child_shells() {
        let mut root = Object::new("Root");
        let mut child = Object::new("Child");
        child.type_uri = Some("omi:floor".into());
        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Number(22.5), None);
        child.add_item("Temp".into(), item);
        root.add_child(child);

        let mut w = JsonWriter::new();
        root.write_json_with_depth(&mut w, 1);
        let v = parse(&w.into_string());
        // Root has no items, so "items" should be absent
        assert!(v.get("items").is_none());
        // Child exists at depth 0 — no items inside
        assert_eq!(v["objects"]["Child"]["id"], "Child");
        assert_eq!(v["objects"]["Child"]["type"], "omi:floor");
        assert!(v["objects"]["Child"].get("items").is_none());
    }

    #[test]
    fn object_depth_two_full_tree() {
        let mut root = Object::new("Root");
        let mut child = Object::new("Child");
        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Number(22.5), None);
        child.add_item("Temp".into(), item);
        root.add_child(child);

        let mut w = JsonWriter::new();
        root.write_json_with_depth(&mut w, 2);
        let v = parse(&w.into_string());
        assert_eq!(v["objects"]["Child"]["items"]["Temp"]["values"][0]["v"], 22.5);
    }

    #[test]
    fn object_unlimited_depth_matches_write_json() {
        let mut root = Object::new("Root");
        let mut child = Object::new("Child");
        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Number(22.5), Some(1000.0));
        child.add_item("Temp".into(), item);
        root.add_child(child);

        let full = root.to_json_string();
        let mut w = JsonWriter::new();
        root.write_json_with_depth(&mut w, usize::MAX);
        assert_eq!(full, w.into_string());
    }

    // -- Serde parity tests --
    // Verify ToJson output matches serde_json output for the same structures.
    // These require serde Serialize impls, so they only run under the json feature.

    #[cfg(feature = "json")]
    #[test]
    fn parity_omi_value() {
        for val in &[
            OmiValue::Null,
            OmiValue::Bool(true),
            OmiValue::Bool(false),
            OmiValue::Number(42.5),
            OmiValue::Str("hello \"world\"".into()),
        ] {
            let serde_out = serde_json::to_string(val).unwrap();
            let lite_out = val.to_json_string();
            assert_eq!(
                parse(&serde_out), parse(&lite_out),
                "OmiValue parity failed for {:?}", val
            );
        }
    }

    #[cfg(feature = "json")]
    #[test]
    fn parity_value_with_timestamp() {
        let v = Value::new(OmiValue::Number(22.5), Some(1000.0));
        let serde_out = serde_json::to_string(&v).unwrap();
        let lite_out = v.to_json_string();
        assert_eq!(parse(&serde_out), parse(&lite_out));
    }

    #[cfg(feature = "json")]
    #[test]
    fn parity_value_without_timestamp() {
        let v = Value::new(OmiValue::Str("test".into()), None);
        let serde_out = serde_json::to_string(&v).unwrap();
        let lite_out = v.to_json_string();
        assert_eq!(parse(&serde_out), parse(&lite_out));
    }

    #[cfg(feature = "json")]
    #[test]
    fn parity_ring_buffer() {
        let mut rb = RingBuffer::new(5);
        rb.push(Value::new(OmiValue::Number(1.0), Some(100.0)));
        rb.push(Value::new(OmiValue::Number(2.0), Some(200.0)));
        rb.push(Value::new(OmiValue::Number(3.0), Some(300.0)));

        let serde_out = serde_json::to_string(&rb).unwrap();
        let lite_out = rb.to_json_string();
        assert_eq!(parse(&serde_out), parse(&lite_out));
    }

    #[cfg(feature = "json")]
    #[test]
    fn parity_ring_buffer_after_overflow() {
        let mut rb = RingBuffer::new(3);
        for i in 1..=5 {
            rb.push(Value::new(OmiValue::Number(i as f64), Some(i as f64 * 100.0)));
        }

        let serde_out = serde_json::to_string(&rb).unwrap();
        let lite_out = rb.to_json_string();
        assert_eq!(parse(&serde_out), parse(&lite_out));
    }

    #[cfg(feature = "json")]
    #[test]
    fn parity_info_item_empty() {
        let item = InfoItem::new(10);
        let serde_out = serde_json::to_string(&item).unwrap();
        let lite_out = item.to_json_string();
        assert_eq!(parse(&serde_out), parse(&lite_out));
    }

    #[cfg(feature = "json")]
    #[test]
    fn parity_info_item_full() {
        let mut item = InfoItem::new(10);
        item.type_uri = Some("omi:temperature".into());
        item.desc = Some("Room temp".into());
        let mut meta = BTreeMap::new();
        meta.insert("writable".into(), OmiValue::Bool(true));
        meta.insert("unit".into(), OmiValue::Str("celsius".into()));
        item.meta = Some(meta);
        item.add_value(OmiValue::Number(22.5), Some(1000.0));
        item.add_value(OmiValue::Number(23.0), Some(1001.0));

        let serde_out = serde_json::to_string(&item).unwrap();
        let lite_out = item.to_json_string();
        assert_eq!(parse(&serde_out), parse(&lite_out));
    }

    #[cfg(feature = "json")]
    #[test]
    fn parity_object_minimal() {
        let obj = Object::new("DeviceA");
        let serde_out = serde_json::to_string(&obj).unwrap();
        let lite_out = obj.to_json_string();
        assert_eq!(parse(&serde_out), parse(&lite_out));
    }

    #[cfg(feature = "json")]
    #[test]
    fn parity_object_nested() {
        let mut root = Object::new("Root");
        root.type_uri = Some("omi:building".into());
        root.desc = Some("Headquarters".into());

        let mut child = Object::new("Floor1");
        child.type_uri = Some("omi:floor".into());
        let mut item = InfoItem::new(10);
        item.type_uri = Some("omi:temperature".into());
        item.add_value(OmiValue::Number(22.5), Some(1000.0));
        item.add_value(OmiValue::Number(23.0), Some(1001.0));
        child.add_item("Temp".into(), item);

        let mut item2 = InfoItem::new(10);
        item2.add_value(OmiValue::Number(45.0), None);
        child.add_item("Humidity".into(), item2);

        root.add_child(child);
        root.add_child(Object::new("Floor2"));

        let serde_out = serde_json::to_string(&root).unwrap();
        let lite_out = root.to_json_string();
        assert_eq!(parse(&serde_out), parse(&lite_out));
    }

    #[cfg(feature = "json")]
    #[test]
    fn parity_depth_limited() {
        let mut root = Object::new("Root");
        let mut child = Object::new("Child");
        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Number(22.5), Some(1000.0));
        child.add_item("Temp".into(), item);
        root.add_child(child);

        for depth in 0..=3 {
            let serde_val = root.serialize_with_depth(depth).unwrap();
            let mut w = JsonWriter::new();
            root.write_json_with_depth(&mut w, depth);
            let lite_val = parse(&w.into_string());
            assert_eq!(
                serde_val, lite_val,
                "Depth-limited parity failed at depth {}", depth
            );
        }
    }
}
