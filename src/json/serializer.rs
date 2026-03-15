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
#[derive(Default)]
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
        Self::default()
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
            #[cfg(feature = "lite-json")]
            ResultPayload::JsonString(s) => {
                w.raw_json(s);
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
#[allow(clippy::too_many_arguments)]
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

// Serde parity tests were here — removed with serde.
// Lite-json serializer tests live in json/parser.rs round-trip suite.
