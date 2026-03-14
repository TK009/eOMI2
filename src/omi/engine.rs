use std::collections::BTreeMap;

use log::warn;

use crate::log_util::RateLimiter;
use crate::odf::{Object, ObjectTree, PathTarget, PathTargetMut, TreeError, OmiValue};
use super::cancel::CancelOp;
use super::delete::DeleteOp;
use super::read::{ReadKind, ReadOp};
use super::response::{ItemStatus, OmiResponse, ResponseBody, StatusCode};
use super::subscriptions::{Delivery, DeliveryTarget, PollResult, SessionId, SubscriptionRegistry};
use super::write::{WriteItem, WriteOp};
use super::{OmiMessage, Operation};

/// Request processing engine. Takes parsed OMI messages, operates on the
/// object tree, and returns OMI response messages.
pub struct Engine {
    /// The O-DF object tree. Public so platform code can populate it on boot.
    pub tree: ObjectTree,
    subscriptions: SubscriptionRegistry,
    /// Deliveries accumulated during write processing (event notifications).
    /// Drained by the caller after `process()` via `take_deliveries()`.
    pending_deliveries: Vec<Delivery>,
    #[cfg(feature = "scripting")]
    script_engine: Option<crate::scripting::ScriptEngine>,
    script_rl: RateLimiter,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            tree: ObjectTree::new(),
            subscriptions: SubscriptionRegistry::new(),
            pending_deliveries: Vec::new(),
            #[cfg(feature = "scripting")]
            script_engine: crate::scripting::ScriptEngine::new()
                .map_err(|e| warn!("Script engine init failed: {}", e))
                .ok(),
            script_rl: RateLimiter::new(),
        }
    }

    /// Drain and return all deliveries accumulated during write processing.
    pub fn take_deliveries(&mut self) -> Vec<Delivery> {
        std::mem::take(&mut self.pending_deliveries)
    }

    /// Returns `true` if the embedded script engine initialised successfully.
    #[cfg(feature = "scripting")]
    pub fn has_script_engine(&self) -> bool {
        self.script_engine.is_some()
    }

    /// Process an OMI request message and return a response.
    ///
    /// `now` is the current time as seconds since UNIX epoch, used for
    /// subscription TTL and expiry calculations.
    ///
    /// `ws_session` is the monotonic WebSocket session ID when the request
    /// arrives over a WebSocket connection. Subscriptions created without a
    /// callback will use WebSocket delivery instead of poll when this is `Some`.
    pub fn process(&mut self, msg: OmiMessage, now: f64, ws_session: Option<SessionId>) -> (OmiMessage, Vec<Delivery>) {
        let ttl = msg.ttl;
        match msg.operation {
            Operation::Read(op) => (self.process_read(op, ttl, now, ws_session), Vec::new()),
            Operation::Write(op) => self.process_write(op, now),
            Operation::Delete(op) => (self.process_delete(op), Vec::new()),
            Operation::Cancel(op) => (self.process_cancel(op), Vec::new()),
            Operation::Response(_) => {
                (OmiResponse::bad_request("Engine processes requests, not responses"), Vec::new())
            }
        }
    }

    /// Tick interval subscriptions and return any callback deliveries.
    ///
    /// For each due subscription, reads the newest value from the tree.
    /// Poll subscriptions buffer internally; callback subscriptions produce
    /// `Delivery` entries in the returned vec.
    pub fn tick(&mut self, now: f64) -> Vec<super::subscriptions::Delivery> {
        let tree = &self.tree;
        // next_trigger_time intentionally discarded: the main loop ticks on a
        // fixed interval rather than scheduling the next wake-up dynamically.
        let (mut deliveries, _next_trigger) = self.subscriptions.tick_intervals(now, &|path| {
            match tree.resolve(path) {
                Ok(PathTarget::InfoItem(item)) => {
                    Some(item.query_values(Some(1), None, None, None))
                }
                _ => None,
            }
        });
        // FR-002: Run onread scripts on interval subscription values before delivery.
        // Event-based subscriptions (notify_write_event) deliver the written value as-is.
        #[cfg(feature = "scripting")]
        for delivery in &mut deliveries {
            if let Some(newest) = delivery.values.first() {
                let has_onread = matches!(
                    self.tree.resolve(&delivery.path),
                    Ok(PathTarget::InfoItem(item)) if item.get_onread_script().is_some()
                );
                if has_onread {
                    let event_value = newest.v.clone();
                    let event_ts = newest.t;
                    if let Some(transformed) = self.run_onread_script(&delivery.path, &event_value, event_ts) {
                        if let Some(newest_mut) = delivery.values.first_mut() {
                            newest_mut.v = transformed;
                        }
                    }
                }
            }
        }
        deliveries
    }

    /// Provide access to the subscription registry for event notification
    /// and interval ticking by platform code.
    pub fn subscriptions(&mut self) -> &mut SubscriptionRegistry {
        &mut self.subscriptions
    }

    fn error_response(status: StatusCode, desc: String) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Response(ResponseBody {
                status: status.as_u16(),
                rid: None,
                desc: Some(desc),
                result: None,
            }),
        }
    }

    fn tree_error_to_response(err: TreeError) -> OmiMessage {
        match err {
            TreeError::NotFound(msg) => Self::error_response(StatusCode::NotFound, msg),
            TreeError::Forbidden(msg) => Self::error_response(StatusCode::Forbidden, msg),
            TreeError::InvalidPath(msg) => Self::error_response(StatusCode::BadRequest, msg),
            #[cfg(feature = "json")]
            TreeError::SerializationError(msg) => {
                Self::error_response(StatusCode::InternalError, msg)
            }
        }
    }

    fn tree_error_to_item_status(path: &str, err: TreeError) -> ItemStatus {
        let (status, desc) = match err {
            TreeError::NotFound(msg) => (StatusCode::NotFound, msg),
            TreeError::Forbidden(msg) => (StatusCode::Forbidden, msg),
            TreeError::InvalidPath(msg) => (StatusCode::BadRequest, msg),
            #[cfg(feature = "json")]
            TreeError::SerializationError(msg) => (StatusCode::InternalError, msg),
        };
        ItemStatus {
            path: path.into(),
            status: status.as_u16(),
            desc: Some(desc),
        }
    }

    // --- Read ---

    fn process_read(&mut self, op: ReadOp, ttl: i64, now: f64, ws_session: Option<SessionId>) -> OmiMessage {
        match op.kind() {
            ReadKind::OneTime => self.process_read_one_time(&op),
            ReadKind::Subscription => self.process_read_subscription(op, ttl, now, ws_session),
            ReadKind::Poll => self.process_read_poll(&op, now),
        }
    }

    fn process_read_one_time(&mut self, op: &ReadOp) -> OmiMessage {
        let path = op.path.as_deref().unwrap_or("/");
        match self.tree.resolve(path) {
            Ok(PathTarget::InfoItem(item)) => {
                if !item.is_readable() {
                    return OmiResponse::forbidden(&format!(
                        "InfoItem at '{}' is not readable",
                        path
                    ));
                }
                let _has_onread = item.get_onread_script().is_some();
                let mut values = item.query_values(
                    op.newest.map(|n| n as usize),
                    op.oldest.map(|n| n as usize),
                    op.begin,
                    op.end,
                );
                // FR-004, FR-011, FR-012: If item has an onread script, run it
                // on the newest value only (index 0, since values are newest-first).
                // Replace only the delivered value; preserve original timestamp.
                #[cfg(feature = "scripting")]
                if _has_onread {
                    let (event_value, event_ts) = if let Some(newest) = values.first() {
                        (newest.v.clone(), newest.t)
                    } else {
                        (OmiValue::Null, None)
                    };
                    if let Some(transformed) = self.run_onread_script(path, &event_value, event_ts) {
                        if let Some(newest) = values.first_mut() {
                            newest.v = transformed;
                        }
                    }
                }
                match serde_json::to_value(&values) {
                    Ok(val) => OmiResponse::ok(serde_json::json!({
                        "path": path,
                        "values": val,
                    })),
                    Err(e) => OmiResponse::error(&e.to_string()),
                }
            }
            Ok(PathTarget::Object(_)) | Ok(PathTarget::Root(_)) => {
                match self.tree.read(path, op.depth.map(|d| d as usize)) {
                    Ok(val) => OmiResponse::ok(val),
                    Err(e) => Self::tree_error_to_response(e),
                }
            }
            Err(e) => Self::tree_error_to_response(e),
        }
    }

    fn process_read_subscription(&mut self, op: ReadOp, ttl: i64, now: f64, ws_session: Option<SessionId>) -> OmiMessage {
        if ttl <= 0 {
            return OmiResponse::bad_request("Subscription requires ttl > 0");
        }
        let path = op.path.as_deref().unwrap_or("/");
        let interval = op.interval.unwrap_or(-1.0);
        let target = if let Some(url) = op.callback {
            DeliveryTarget::Callback(url)
        } else if let Some(session) = ws_session {
            DeliveryTarget::WebSocket(session)
        } else {
            DeliveryTarget::Poll
        };
        match self.subscriptions.create(path, interval, target, ttl as f64, now) {
            Ok(rid) => OmiResponse::ok_with_rid(rid, serde_json::json!(null)),
            Err(e) => OmiResponse::bad_request(e),
        }
    }

    fn process_read_poll(&mut self, op: &ReadOp, now: f64) -> OmiMessage {
        let rid = op.rid.as_deref().unwrap_or("");
        match self.subscriptions.poll(rid, now) {
            PollResult::Ok { path, values } => {
                match serde_json::to_value(&values) {
                    Ok(val) => OmiResponse::ok(serde_json::json!({
                        "path": path,
                        "values": val,
                    })),
                    Err(e) => OmiResponse::error(&e.to_string()),
                }
            }
            PollResult::NotPollable => OmiResponse::bad_request(&format!(
                "Subscription '{}' is not pollable",
                rid
            )),
            PollResult::NotFound => Self::error_response(
                StatusCode::NotFound,
                format!("Subscription '{}' not found", rid),
            ),
        }
    }

    // --- Write ---

    /// Notify event subscribers after a successful write to `path`.
    fn notify_write_event(&mut self, path: &str, now: f64) -> Vec<Delivery> {
        match self.tree.resolve(path) {
            Ok(PathTarget::InfoItem(item)) => {
                let values = item.query_values(Some(1), None, None, None);
                self.subscriptions.notify_event(path, &values, now)
            }
            _ => Vec::new(),
        }
    }

    /// Collect all InfoItem paths from a set of objects to be written at `base`.
    fn collect_info_item_paths(base: &str, objects: &BTreeMap<String, Object>) -> Vec<String> {
        let mut paths = Vec::new();
        fn walk(prefix: &str, obj: &Object, out: &mut Vec<String>) {
            let obj_path = if prefix == "/" {
                format!("/{}", obj.id)
            } else {
                format!("{}/{}", prefix, obj.id)
            };
            if let Some(items) = &obj.items {
                for name in items.keys() {
                    out.push(format!("{}/{}", obj_path, name));
                }
            }
            if let Some(children) = &obj.objects {
                for child in children.values() {
                    walk(&obj_path, child, out);
                }
            }
        }
        for obj in objects.values() {
            walk(base, obj, &mut paths);
        }
        paths
    }

    fn process_write(&mut self, op: WriteOp, now: f64) -> (OmiMessage, Vec<Delivery>) {
        match op {
            WriteOp::Single { path, v, t } => self.write_single(&path, v, t, now),
            WriteOp::Batch { items } => self.process_write_batch(items, now),
            WriteOp::Tree { path, objects } => {
                let item_paths = Self::collect_info_item_paths(&path, &objects);
                match self.tree.write_tree(&path, objects) {
                    Ok(()) => {
                        let mut deliveries = Vec::new();
                        for p in &item_paths {
                            deliveries.extend(self.notify_write_event(p, now));
                        }
                        (OmiResponse::ok(serde_json::json!(null)), deliveries)
                    }
                    Err(e) => (Self::tree_error_to_response(e), Vec::new()),
                }
            }
        }
    }

    fn write_single(&mut self, path: &str, v: OmiValue, t: Option<f64>, now: f64) -> (OmiMessage, Vec<Delivery>) {
        self.write_single_inner(path, v, t, now, 0)
    }

    /// Inner write logic with depth tracking for script cascading.
    ///
    /// `depth` starts at 0 for user-initiated writes and increments for each
    /// cascading write triggered by an onwrite script.
    fn write_single_inner(
        &mut self,
        path: &str,
        v: OmiValue,
        t: Option<f64>,
        now: f64,
        #[cfg_attr(not(feature = "scripting"), allow(unused))]
        depth: u8,
    ) -> (OmiMessage, Vec<Delivery>) {
        match self.tree.resolve(path) {
            Ok(PathTarget::InfoItem(item)) => {
                if !item.is_writable() {
                    return (OmiResponse::forbidden(&format!(
                        "InfoItem at '{}' is not writable",
                        path
                    )), Vec::new());
                }
            }
            Ok(PathTarget::Object(_)) | Ok(PathTarget::Root(_)) => {
                return (OmiResponse::bad_request(&format!(
                    "Cannot write value to object path '{}'",
                    path
                )), Vec::new());
            }
            Err(TreeError::NotFound(_)) => {} // will auto-create
            Err(e) => return (Self::tree_error_to_response(e), Vec::new()),
        }

        let saved_value = v.clone();
        match self.tree.write_value(path, v, t) {
            Ok(created) => {
                if created {
                    self.mark_writable(path);
                }

                #[cfg(feature = "scripting")]
                let (mut deliveries, script_err) = self.run_onwrite_script(path, &saved_value, t, now, depth);
                #[cfg(not(feature = "scripting"))]
                let (mut deliveries, script_err): (Vec<Delivery>, Option<String>) = (Vec::new(), None);

                deliveries.extend(self.notify_write_event(path, now));

                let response = if let Some(warning) = script_err {
                    OmiResponse::write_ok_with_warning(&warning)
                } else if created {
                    OmiResponse::created()
                } else {
                    OmiResponse::ok(serde_json::json!(null))
                };
                (response, deliveries)
            }
            Err(e) => (Self::tree_error_to_response(e), Vec::new()),
        }
    }

    fn process_write_batch(&mut self, items: Vec<WriteItem>, now: f64) -> (OmiMessage, Vec<Delivery>) {
        let mut statuses = Vec::with_capacity(items.len());
        let mut all_deliveries = Vec::new();
        for item in items {
            let (status, deliveries) = self.write_batch_item(item, now);
            statuses.push(status);
            all_deliveries.extend(deliveries);
        }
        (OmiResponse::partial_batch(statuses), all_deliveries)
    }

    fn write_batch_item(&mut self, item: WriteItem, now: f64) -> (ItemStatus, Vec<Delivery>) {
        let WriteItem { path, v, t } = item;

        match self.tree.resolve(&path) {
            Ok(PathTarget::InfoItem(existing)) => {
                if !existing.is_writable() {
                    let desc = format!("InfoItem at '{}' is not writable", path);
                    return (ItemStatus {
                        path,
                        status: StatusCode::Forbidden.as_u16(),
                        desc: Some(desc),
                    }, Vec::new());
                }
            }
            Ok(PathTarget::Object(_)) | Ok(PathTarget::Root(_)) => {
                let desc = format!("Cannot write value to object path '{}'", path);
                return (ItemStatus {
                    path,
                    status: StatusCode::BadRequest.as_u16(),
                    desc: Some(desc),
                }, Vec::new());
            }
            Err(TreeError::NotFound(_)) => {} // will auto-create
            Err(e) => return (Self::tree_error_to_item_status(&path, e), Vec::new()),
        }

        let saved_value = v.clone();
        match self.tree.write_value(&path, v, t) {
            Ok(created) => {
                if created {
                    self.mark_writable(&path);
                }

                #[cfg(feature = "scripting")]
                let (script_deliveries, script_err) = self.run_onwrite_script(&path, &saved_value, t, now, 0);
                #[cfg(not(feature = "scripting"))]
                let (script_deliveries, script_err): (Vec<Delivery>, Option<String>) = (Vec::new(), None);

                let mut deliveries = script_deliveries;
                deliveries.extend(self.notify_write_event(&path, now));

                let status = if created {
                    ItemStatus {
                        path,
                        status: StatusCode::Created.as_u16(),
                        desc: script_err,
                    }
                } else {
                    ItemStatus {
                        path,
                        status: StatusCode::Ok.as_u16(),
                        desc: script_err,
                    }
                };
                (status, deliveries)
            }
            Err(e) => (Self::tree_error_to_item_status(&path, e), Vec::new()),
        }
    }

    pub fn mark_writable(&mut self, path: &str) {
        if let Ok(PathTargetMut::InfoItem(item)) = self.tree.resolve_mut(path) {
            let meta = item.meta.get_or_insert_with(BTreeMap::new);
            meta.insert("writable".into(), OmiValue::Bool(true));
        }
    }

    /// Execute an onwrite script if the InfoItem at `path` has one in its metadata.
    ///
    /// Script writes are collected into a local `Vec` during execution and
    /// processed afterwards, avoiding re-entrant `&mut self` aliasing.
    /// Errors are logged but never fail the write — the value is already written.
    #[cfg(feature = "scripting")]
    fn run_onwrite_script(
        &mut self,
        path: &str,
        value: &OmiValue,
        timestamp: Option<f64>,
        now: f64,
        depth: u8,
    ) -> (Vec<Delivery>, Option<String>) {
        use crate::scripting::bindings::{MAX_SCRIPT_DEPTH, PendingWrite, ScriptCallbackCtx,
                                          js_odf_write_item, js_odf_read_item};
        use crate::scripting::ffi;
        use crate::scripting::ffi::mjs_name;
        use crate::scripting::convert::omi_to_mjs;

        if depth >= MAX_SCRIPT_DEPTH {
            return (Vec::new(), None);
        }

        // Look up the onwrite script from metadata
        let script_src = match self.tree.resolve(path) {
            Ok(PathTarget::InfoItem(item)) => {
                match item.meta.as_ref().and_then(|m| m.get("onwrite")) {
                    Some(OmiValue::Str(src)) => src.clone(),
                    _ => return (Vec::new(), None),
                }
            }
            _ => return (Vec::new(), None),
        };

        // Temporarily take the script engine out of self so we can use it
        // without borrowing self, then put it back before processing writes.
        let mut script_engine = match self.script_engine.take() {
            Some(se) => se,
            None => return (Vec::new(), None),
        };
        let mjs = script_engine.raw();

        let mut pending_writes: Vec<PendingWrite> = Vec::new();
        let mut script_error: Option<String> = None;

        // Safety: mjs is valid for the duration of this block. The callback
        // only writes to `pending_writes` through the ctx pointer — it never
        // accesses the Engine, eliminating aliasing concerns.
        unsafe {
            // Set up `event` object with { value, path, timestamp }
            let event = ffi::mjs_mk_object(mjs);
            let js_val = omi_to_mjs(mjs, value);
            let (n, l) = mjs_name!("value");
            ffi::mjs_set(mjs, event, n, l, js_val);
            let js_path = ffi::mjs_mk_string(mjs, path.as_ptr() as *const _, path.len(), 1);
            let (n, l) = mjs_name!("path");
            ffi::mjs_set(mjs, event, n, l, js_path);
            let js_ts = match timestamp {
                Some(t) => ffi::mjs_mk_number(mjs, t),
                None => ffi::mjs_mk_null(),
            };
            let (n, l) = mjs_name!("timestamp");
            ffi::mjs_set(mjs, event, n, l, js_ts);

            let global = ffi::mjs_get_global(mjs);
            let (n, l) = mjs_name!("event");
            ffi::mjs_set(mjs, global, n, l, event);

            // Set up `odf.writeItem` and `odf.readItem` bindings
            let odf = ffi::mjs_mk_object(mjs);
            let write_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_write_item));
            let (n, l) = mjs_name!("writeItem");
            ffi::mjs_set(mjs, odf, n, l, write_fn);
            let read_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_read_item));
            let (n, l) = mjs_name!("readItem");
            ffi::mjs_set(mjs, odf, n, l, read_fn);
            let (n, l) = mjs_name!("odf");
            ffi::mjs_set(mjs, global, n, l, odf);

            // Set up callback context on the stack
            let mut ctx = ScriptCallbackCtx {
                pending_writes: &mut pending_writes,
                depth,
                tree: &self.tree as *const _,
                onread_path_ptr: std::ptr::null(),
                onread_path_len: 0,
                onread_fns: std::ptr::null(),
            };
            let ctx_foreign = ffi::mjs_mk_foreign(mjs, &mut ctx as *mut ScriptCallbackCtx as *mut std::os::raw::c_void);
            let (n, l) = mjs_name!("__ctx");
            ffi::mjs_set(mjs, global, n, l, ctx_foreign);

            // Execute the script
            let c_src = match std::ffi::CString::new(script_src.as_str()) {
                Ok(c) => c,
                Err(_) => {
                    warn!("onwrite script at '{}' contains NUL byte", path);
                    let null_val = ffi::mjs_mk_null();
                    ffi::mjs_set(mjs, global, n, l, null_val);
                    self.script_engine = Some(script_engine);
                    return (Vec::new(), None);
                }
            };
            let mut res: ffi::mjs_val_t = 0;
            ffi::mjs_reset_ops_count(mjs);
            let deadline = std::time::Instant::now();
            let err = ffi::mjs_exec(mjs, c_src.as_ptr(), &mut res);
            let elapsed = deadline.elapsed();

            // Detect resource-limit errors (FR-004)
            let time_limit = std::time::Duration::from_millis(
                crate::scripting::engine::MAX_SCRIPT_EXEC_MS,
            );
            if elapsed >= time_limit {
                let log_msg = format!(
                    "onwrite script at '{}' exceeded time limit ({}ms)",
                    path, elapsed.as_millis(),
                );
                if self.script_rl.should_emit(&log_msg) {
                    warn!("{}", log_msg);
                }
                script_error = Some(format!(
                    "script exceeded time limit after {}ms", elapsed.as_millis(),
                ));
            } else if err == ffi::MJS_OP_LIMIT_ERROR {
                let log_msg = format!(
                    "onwrite script at '{}' exceeded operation limit ({}ms elapsed)",
                    path, elapsed.as_millis(),
                );
                if self.script_rl.should_emit(&log_msg) {
                    warn!("{}", log_msg);
                }
                script_error = Some("script exceeded operation limit".into());
            } else if err != ffi::MJS_OK {
                let err_ptr = ffi::mjs_strerror(mjs, err);
                let msg = if err_ptr.is_null() {
                    "unknown error"
                } else {
                    std::ffi::CStr::from_ptr(err_ptr).to_str().unwrap_or("unknown error")
                };
                let log_msg = format!("onwrite script error at '{}': {}", path, msg);
                if self.script_rl.should_emit(&log_msg) {
                    warn!("{}", log_msg);
                }
            }

            // Clear all script globals to prevent stale pointer use
            let null_val = ffi::mjs_mk_null();
            let (n, l) = mjs_name!("__ctx");
            ffi::mjs_set(mjs, global, n, l, null_val);
            let (n, l) = mjs_name!("event");
            ffi::mjs_set(mjs, global, n, l, null_val);
            let (n, l) = mjs_name!("odf");
            ffi::mjs_set(mjs, global, n, l, null_val);
        }

        // Put the script engine back before processing writes (nested calls
        // will take() it again for their own scripts).
        self.script_engine = Some(script_engine);

        // Process collected writes — each may trigger further onwrite scripts
        // at depth + 1, using normal &mut self calls with no aliasing.
        let mut deliveries = Vec::new();
        for pw in pending_writes {
            // Apply encoding: decode hex/base64 string values into raw bytes (FR-009b).
            let value = match pw.encoding {
                Some(enc) => {
                    let data_enc = enc.to_data_encoding();
                    match &pw.value {
                        OmiValue::Str(s) => match data_enc.decode(s) {
                            Ok(bytes) => OmiValue::Str(
                                std::string::String::from_utf8(bytes)
                                    .unwrap_or_else(|e| {
                                        // Binary data that isn't valid UTF-8: use lossy conversion.
                                        std::string::String::from_utf8_lossy(e.as_bytes()).into_owned()
                                    }),
                            ),
                            Err(e) => {
                                log::warn!(
                                    "encoding decode error for '{}' ({:?}): {}",
                                    pw.path, enc, e
                                );
                                pw.value
                            }
                        },
                        _ => pw.value, // Non-string values: encoding not applicable
                    }
                }
                None => pw.value,
            };
            let (_resp, d) = self.write_single_inner(&pw.path, value, timestamp, now, depth + 1);
            deliveries.extend(d);

            // FR-009a: persist encoding hint in InfoItem metadata so peripheral
            // drivers apply the correct encoding when transmitting.
            if let Some(enc) = pw.encoding {
                if let Ok(PathTargetMut::InfoItem(item)) = self.tree.resolve_mut(&pw.path) {
                    let meta = item.meta.get_or_insert_with(BTreeMap::new);
                    meta.insert(
                        "tx_encoding".into(),
                        OmiValue::Str(enc.to_data_encoding().as_str().into()),
                    );
                }
            }
        }

        // Run GC at the top level after the entire cascade completes
        if depth == 0 {
            if let Some(se) = self.script_engine.as_mut() {
                se.gc();
            }
        }
        (deliveries, script_error)
    }

    /// Run the onread script attached to an InfoItem, if any.
    ///
    /// Returns `Some(value)` when the script produces a non-null return value,
    /// which replaces the stored value for this read. Returns `None` on error
    /// or when no script is attached (FR-005).
    ///
    /// Only `odf.readItem()` is available to the script — `odf.writeItem` is
    /// intentionally omitted (FR-006). Same resource limits as onwrite (FR-010).
    #[cfg(feature = "scripting")]
    fn run_onread_script(
        &mut self,
        path: &str,
        value: &OmiValue,
        timestamp: Option<f64>,
    ) -> Option<OmiValue> {
        use crate::scripting::bindings::{PendingWrite, ScriptCallbackCtx, js_odf_read_item};
        use crate::scripting::ffi;
        use crate::scripting::ffi::mjs_name;
        use crate::scripting::convert::{omi_to_mjs, mjs_to_omi};

        // Look up the onread script from metadata
        let script_src = match self.tree.resolve(path) {
            Ok(PathTarget::InfoItem(item)) => {
                match item.get_onread_script() {
                    Some(src) => src.to_string(),
                    None => return None,
                }
            }
            _ => return None,
        };

        // Temporarily take the script engine out of self
        let mut script_engine = match self.script_engine.take() {
            Some(se) => se,
            None => return None,
        };
        let mjs = script_engine.raw();

        let mut pending_writes: Vec<PendingWrite> = Vec::new();
        let mut result_value: Option<OmiValue> = None;

        // FR-007: Pre-compile all onread scripts as functions before execution.
        // This avoids re-entrant mjs_exec from within readItem callbacks,
        // which would corrupt the bytecode buffer.
        let mut onread_fns = std::collections::BTreeMap::new();
        unsafe {
            for (item_path, script) in self.tree.collect_onread_scripts() {
                if item_path == path {
                    continue; // Skip self — handled by self-read guard
                }
                let func_src = format!("(function() {{ return {}; }})", script);
                if let Ok(c_src) = std::ffi::CString::new(func_src.as_str()) {
                    let mut func_val: ffi::mjs_val_t = 0;
                    let err = ffi::mjs_exec(mjs, c_src.as_ptr(), &mut func_val);
                    if err == ffi::MJS_OK && ffi::mjs_is_function(func_val) != 0 {
                        onread_fns.insert(item_path, func_val);
                    }
                }
            }
        }

        // Safety: mjs is valid for the duration of this block. The read-only
        // callback only reads from the tree — no aliasing concerns.
        unsafe {
            // Set up `event` object with { value, path, timestamp }
            let event = ffi::mjs_mk_object(mjs);
            let js_val = omi_to_mjs(mjs, value);
            let (n, l) = mjs_name!("value");
            ffi::mjs_set(mjs, event, n, l, js_val);
            let js_path = ffi::mjs_mk_string(mjs, path.as_ptr() as *const _, path.len(), 1);
            let (n, l) = mjs_name!("path");
            ffi::mjs_set(mjs, event, n, l, js_path);
            let js_ts = match timestamp {
                Some(t) => ffi::mjs_mk_number(mjs, t),
                None => ffi::mjs_mk_null(),
            };
            let (n, l) = mjs_name!("timestamp");
            ffi::mjs_set(mjs, event, n, l, js_ts);

            let global = ffi::mjs_get_global(mjs);
            let (n, l) = mjs_name!("event");
            ffi::mjs_set(mjs, global, n, l, event);

            // Set up `odf` with readItem only — NO writeItem (FR-006)
            let odf = ffi::mjs_mk_object(mjs);
            let read_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_read_item));
            let (n, l) = mjs_name!("readItem");
            ffi::mjs_set(mjs, odf, n, l, read_fn);
            let (n, l) = mjs_name!("odf");
            ffi::mjs_set(mjs, global, n, l, odf);

            // Set up callback context — pending_writes is empty but needed
            // by js_odf_read_item for read-after-write consistency checks
            let mut ctx = ScriptCallbackCtx {
                pending_writes: &mut pending_writes,
                depth: 0,
                tree: &self.tree as *const _,
                onread_path_ptr: path.as_ptr(),
                onread_path_len: path.len(),
                onread_fns: &onread_fns,
            };
            let ctx_foreign = ffi::mjs_mk_foreign(mjs, &mut ctx as *mut ScriptCallbackCtx as *mut std::os::raw::c_void);
            let (n, l) = mjs_name!("__ctx");
            ffi::mjs_set(mjs, global, n, l, ctx_foreign);

            // Execute the script
            let c_src = match std::ffi::CString::new(script_src.as_str()) {
                Ok(c) => c,
                Err(_) => {
                    warn!("onread script at '{}' contains NUL byte", path);
                    let null_val = ffi::mjs_mk_null();
                    ffi::mjs_set(mjs, global, n, l, null_val);
                    self.script_engine = Some(script_engine);
                    return None;
                }
            };
            let mut res: ffi::mjs_val_t = 0;
            ffi::mjs_reset_ops_count(mjs);
            let deadline = std::time::Instant::now();
            let err = ffi::mjs_exec(mjs, c_src.as_ptr(), &mut res);
            let elapsed = deadline.elapsed();

            // Detect resource-limit errors (FR-010)
            let time_limit = std::time::Duration::from_millis(
                crate::scripting::engine::MAX_SCRIPT_EXEC_MS,
            );
            if elapsed >= time_limit {
                let log_msg = format!(
                    "onread script at '{}' exceeded time limit ({}ms)",
                    path, elapsed.as_millis(),
                );
                if self.script_rl.should_emit(&log_msg) {
                    warn!("{}", log_msg);
                }
            } else if err == ffi::MJS_OP_LIMIT_ERROR {
                let log_msg = format!(
                    "onread script at '{}' exceeded operation limit ({}ms elapsed)",
                    path, elapsed.as_millis(),
                );
                if self.script_rl.should_emit(&log_msg) {
                    warn!("{}", log_msg);
                }
            } else if err != ffi::MJS_OK {
                let err_ptr = ffi::mjs_strerror(mjs, err);
                let msg = if err_ptr.is_null() {
                    "unknown error"
                } else {
                    std::ffi::CStr::from_ptr(err_ptr).to_str().unwrap_or("unknown error")
                };
                let log_msg = format!("onread script error at '{}': {}", path, msg);
                if self.script_rl.should_emit(&log_msg) {
                    warn!("{}", log_msg);
                }
            } else {
                // Script succeeded — capture return value if non-null
                if ffi::mjs_is_null(res) == 0 && ffi::mjs_is_undefined(res) == 0 {
                    result_value = Some(mjs_to_omi(mjs, res));
                }
            }

            // Clear all script globals to prevent stale pointer use
            let null_val = ffi::mjs_mk_null();
            let (n, l) = mjs_name!("__ctx");
            ffi::mjs_set(mjs, global, n, l, null_val);
            let (n, l) = mjs_name!("event");
            ffi::mjs_set(mjs, global, n, l, null_val);
            let (n, l) = mjs_name!("odf");
            ffi::mjs_set(mjs, global, n, l, null_val);
        }

        // Put the script engine back and run GC
        self.script_engine = Some(script_engine);
        if let Some(se) = self.script_engine.as_mut() {
            se.gc();
        }

        result_value
    }

    // --- Delete ---

    fn process_delete(&mut self, op: DeleteOp) -> OmiMessage {
        match self.tree.delete(&op.path) {
            Ok(()) => OmiResponse::ok(serde_json::json!(null)),
            Err(e) => Self::tree_error_to_response(e),
        }
    }

    // --- Cancel ---

    fn process_cancel(&mut self, op: CancelOp) -> OmiMessage {
        self.subscriptions.cancel(&op.rid);
        OmiResponse::ok(serde_json::json!(null))
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odf::{InfoItem, Object, OmiValue};
    use super::super::response::ResponseResult;

    // --- Helpers ---

    fn read_msg(path: &str) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Read(ReadOp {
                path: Some(path.into()),
                rid: None,
                newest: None, oldest: None, begin: None, end: None,
                depth: None, interval: None, callback: None,
            }),
        }
    }

    fn read_with(path: &str, newest: Option<u64>, oldest: Option<u64>,
                 begin: Option<f64>, end: Option<f64>, depth: Option<u64>) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Read(ReadOp {
                path: Some(path.into()), rid: None,
                newest, oldest, begin, end, depth,
                interval: None, callback: None,
            }),
        }
    }

    fn sub_msg(path: &str, interval: f64, ttl: i64) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl,
            operation: Operation::Read(ReadOp {
                path: Some(path.into()), rid: None,
                newest: None, oldest: None, begin: None, end: None,
                depth: None,
                interval: Some(interval),
                callback: Some("http://example.com/cb".into()),
            }),
        }
    }

    fn poll_sub_msg(path: &str, ttl: i64) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl,
            operation: Operation::Read(ReadOp {
                path: Some(path.into()), rid: None,
                newest: None, oldest: None, begin: None, end: None,
                depth: None,
                interval: Some(-1.0),
                callback: None,
            }),
        }
    }

    fn interval_poll_sub_msg(path: &str, interval: f64, ttl: i64) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl,
            operation: Operation::Read(ReadOp {
                path: Some(path.into()), rid: None,
                newest: None, oldest: None, begin: None, end: None,
                depth: None,
                interval: Some(interval),
                callback: None,
            }),
        }
    }

    fn poll_msg(rid: &str) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Read(ReadOp {
                path: None, rid: Some(rid.into()),
                newest: None, oldest: None, begin: None, end: None,
                depth: None, interval: None, callback: None,
            }),
        }
    }

    fn write_msg(path: &str, v: OmiValue) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl: 10,
            operation: Operation::Write(WriteOp::Single {
                path: path.into(), v, t: None,
            }),
        }
    }

    fn batch_msg(items: Vec<WriteItem>) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl: 10,
            operation: Operation::Write(WriteOp::Batch { items }),
        }
    }

    fn tree_msg(path: &str, objects: BTreeMap<String, Object>) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl: 10,
            operation: Operation::Write(WriteOp::Tree { path: path.into(), objects }),
        }
    }

    fn delete_msg(path: &str) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Delete(DeleteOp { path: path.into() }),
        }
    }

    fn cancel_msg(rids: &[&str]) -> OmiMessage {
        OmiMessage {
            version: "1.0".into(),
            ttl: 0,
            operation: Operation::Cancel(CancelOp {
                rid: rids.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }

    fn status(resp: &(OmiMessage, Vec<Delivery>)) -> u16 {
        match &resp.0.operation {
            Operation::Response(body) => body.status,
            _ => panic!("expected Response"),
        }
    }

    fn body(resp: &(OmiMessage, Vec<Delivery>)) -> &ResponseBody {
        match &resp.0.operation {
            Operation::Response(b) => b,
            _ => panic!("expected Response"),
        }
    }

    /// Engine with /Sensor1/Temperature (3 values) and /Sensor1/Humidity (1 value).
    /// Items created via tree directly, so they are NOT writable.
    fn setup() -> Engine {
        let mut e = Engine::new();
        e.tree.write_value("/Sensor1/Temperature", OmiValue::Number(20.0), Some(100.0)).unwrap();
        e.tree.write_value("/Sensor1/Temperature", OmiValue::Number(21.0), Some(200.0)).unwrap();
        e.tree.write_value("/Sensor1/Temperature", OmiValue::Number(22.0), Some(300.0)).unwrap();
        e.tree.write_value("/Sensor1/Humidity", OmiValue::Number(45.0), Some(100.0)).unwrap();
        e
    }

    // --- Read: one-time ---

    #[test]
    fn read_info_item() {
        let mut e = setup();
        let resp = e.process(read_msg("/Sensor1/Temperature"), 0.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                assert_eq!(val["path"], "/Sensor1/Temperature");
                assert_eq!(val["values"].as_array().unwrap().len(), 3);
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn read_info_item_newest() {
        let mut e = setup();
        let resp = e.process(read_with("/Sensor1/Temperature", Some(1), None, None, None, None), 0.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                let values = val["values"].as_array().unwrap();
                assert_eq!(values.len(), 1);
                assert_eq!(values[0]["v"], 22.0);
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn read_info_item_oldest() {
        let mut e = setup();
        let resp = e.process(read_with("/Sensor1/Temperature", None, Some(1), None, None, None), 0.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                let values = val["values"].as_array().unwrap();
                assert_eq!(values.len(), 1);
                assert_eq!(values[0]["v"], 20.0);
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn read_info_item_time_range() {
        let mut e = setup();
        let resp = e.process(read_with(
            "/Sensor1/Temperature", None, None, Some(150.0), Some(250.0), None,
        ), 0.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                let values = val["values"].as_array().unwrap();
                assert_eq!(values.len(), 1);
                assert_eq!(values[0]["v"], 21.0);
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn read_object() {
        let mut e = setup();
        let resp = e.process(read_msg("/Sensor1"), 0.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                assert_eq!(val["id"], "Sensor1");
                assert!(val["items"]["Temperature"].is_object());
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn read_root() {
        let mut e = setup();
        let resp = e.process(read_msg("/"), 0.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                assert!(val["Sensor1"].is_object());
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn read_with_depth() {
        let mut e = setup();
        let resp = e.process(read_with("/Sensor1", None, None, None, None, Some(0)), 0.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                assert_eq!(val["id"], "Sensor1");
                assert!(val.get("items").is_none());
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn read_not_found() {
        let mut e = setup();
        let resp = e.process(read_msg("/Missing/Path"), 0.0, None);
        assert_eq!(status(&resp), 404);
    }

    #[test]
    fn read_not_readable() {
        let mut e = Engine::new();
        e.tree.write_value("/A/B", OmiValue::Number(1.0), None).unwrap();
        if let Ok(PathTargetMut::InfoItem(item)) = e.tree.resolve_mut("/A/B") {
            let meta = item.meta.get_or_insert_with(BTreeMap::new);
            meta.insert("readable".into(), OmiValue::Bool(false));
        }
        let resp = e.process(read_msg("/A/B"), 0.0, None);
        assert_eq!(status(&resp), 403);
    }

    // --- Read: subscription ---

    #[test]
    fn subscription_returns_rid() {
        let mut e = setup();
        let resp = e.process(sub_msg("/Sensor1/Temperature", 10.0, 60), 1000.0, None);
        assert_eq!(status(&resp), 200);
        let b = body(&resp);
        assert!(b.rid.is_some());
        assert!(b.rid.as_ref().unwrap().starts_with("sub-"));
    }

    #[test]
    fn subscription_requires_positive_ttl() {
        let mut e = setup();
        let resp = e.process(sub_msg("/Sensor1/Temperature", 10.0, 0), 1000.0, None);
        assert_eq!(status(&resp), 400);
    }

    #[test]
    fn poll_callback_sub_rejected() {
        let mut e = setup();
        // Create a callback subscription (not pollable)
        let sub = e.process(sub_msg("/Sensor1/Temperature", 10.0, 60), 1000.0, None);
        let rid = body(&sub).rid.as_ref().unwrap();
        // Polling a callback sub returns 400
        let resp = e.process(poll_msg(rid), 1001.0, None);
        assert_eq!(status(&resp), 400);
    }

    #[test]
    fn poll_returns_empty_buffer() {
        let mut e = setup();
        // Create a poll subscription (no callback)
        let sub = e.process(poll_sub_msg("/Sensor1/Temperature", 60), 1000.0, None);
        assert_eq!(status(&sub), 200);
        let rid = body(&sub).rid.as_ref().unwrap();
        // Poll returns 200 with empty values
        let resp = e.process(poll_msg(rid), 1001.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                assert_eq!(val["path"], "/Sensor1/Temperature");
                assert_eq!(val["values"].as_array().unwrap().len(), 0);
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn poll_unknown_rid_returns_404() {
        let mut e = setup();
        let resp = e.process(poll_msg("nonexistent"), 0.0, None);
        assert_eq!(status(&resp), 404);
    }

    // --- Write: single ---

    #[test]
    fn write_single_new_path() {
        let mut e = Engine::new();
        let resp = e.process(write_msg("/Dev/Temp", OmiValue::Number(22.5)), 0.0, None);
        assert_eq!(status(&resp), 201);
        // Verify it was created
        assert!(matches!(e.tree.resolve("/Dev/Temp"), Ok(PathTarget::InfoItem(_))));
    }

    #[test]
    fn write_single_existing_writable() {
        let mut e = Engine::new();
        // First write creates the item (marks writable)
        e.process(write_msg("/Dev/Temp", OmiValue::Number(22.0)), 0.0, None);
        // Second write updates
        let resp = e.process(write_msg("/Dev/Temp", OmiValue::Number(23.0)), 0.0, None);
        assert_eq!(status(&resp), 200);
    }

    #[test]
    fn write_not_writable() {
        let mut e = setup(); // items are not writable
        let resp = e.process(write_msg("/Sensor1/Temperature", OmiValue::Number(99.0)), 0.0, None);
        assert_eq!(status(&resp), 403);
    }

    #[test]
    fn auto_created_item_writable_on_second_write() {
        let mut e = Engine::new();
        // Engine creates the item → marks writable
        let r1 = e.process(write_msg("/X/Y", OmiValue::Number(1.0)), 0.0, None);
        assert_eq!(status(&r1), 201);
        // Second write succeeds because item is writable
        let r2 = e.process(write_msg("/X/Y", OmiValue::Number(2.0)), 0.0, None);
        assert_eq!(status(&r2), 200);
    }

    #[test]
    fn write_to_object_path_rejected() {
        let mut e = Engine::new();
        e.tree.insert_root(Object::new("Device"));
        let resp = e.process(write_msg("/Device", OmiValue::Number(1.0)), 0.0, None);
        assert_eq!(status(&resp), 400);
    }

    // --- Write: batch ---

    #[test]
    fn write_batch_mixed() {
        let mut e = setup(); // Temperature exists but not writable
        let items = vec![
            WriteItem { path: "/Sensor1/NewItem".into(), v: OmiValue::Number(1.0), t: None },
            WriteItem { path: "/Sensor1/Temperature".into(), v: OmiValue::Number(99.0), t: None },
        ];
        let resp = e.process(batch_msg(items), 0.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Batch(statuses) => {
                assert_eq!(statuses.len(), 2);
                assert_eq!(statuses[0].status, 201); // created
                assert_eq!(statuses[1].status, 403); // not writable
            }
            _ => panic!("expected Batch"),
        }
    }

    // --- Write: tree ---

    #[test]
    fn write_tree() {
        let mut e = Engine::new();
        let mut objects = BTreeMap::new();
        let mut dev = Object::new("Device");
        dev.add_item("Temp".into(), InfoItem::new(10));
        objects.insert("Device".into(), dev);
        let resp = e.process(tree_msg("/", objects), 0.0, None);
        assert_eq!(status(&resp), 200);
        assert!(matches!(e.tree.resolve("/Device"), Ok(PathTarget::Object(_))));
    }

    // --- Delete ---

    #[test]
    fn delete_object() {
        let mut e = setup();
        let resp = e.process(delete_msg("/Sensor1"), 0.0, None);
        assert_eq!(status(&resp), 200);
        assert!(e.tree.is_empty());
    }

    #[test]
    fn delete_item() {
        let mut e = setup();
        let resp = e.process(delete_msg("/Sensor1/Temperature"), 0.0, None);
        assert_eq!(status(&resp), 200);
        assert!(e.tree.resolve("/Sensor1/Temperature").is_err());
        // Humidity still exists
        assert!(e.tree.resolve("/Sensor1/Humidity").is_ok());
    }

    #[test]
    fn delete_not_found() {
        let mut e = setup();
        let resp = e.process(delete_msg("/Missing"), 0.0, None);
        assert_eq!(status(&resp), 404);
    }

    #[test]
    fn delete_root_forbidden() {
        let mut e = setup();
        // Bypass parse validation to test defense-in-depth
        let resp = e.process(delete_msg("/"), 0.0, None);
        assert_eq!(status(&resp), 403);
    }

    // --- Cancel ---

    #[test]
    fn cancel_existing_subscription() {
        let mut e = setup();
        let sub = e.process(sub_msg("/Sensor1/Temperature", 10.0, 60), 1000.0, None);
        let rid = body(&sub).rid.as_ref().unwrap().clone();
        // Cancel
        let resp = e.process(cancel_msg(&[&rid]), 1001.0, None);
        assert_eq!(status(&resp), 200);
        // Verify subscription is gone
        let poll = e.process(poll_msg(&rid), 1002.0, None);
        assert_eq!(status(&poll), 404);
    }

    #[test]
    fn cancel_unknown_rid_idempotent() {
        let mut e = Engine::new();
        let resp = e.process(cancel_msg(&["nonexistent"]), 0.0, None);
        assert_eq!(status(&resp), 200);
    }

    #[test]
    fn cancel_multiple() {
        let mut e = setup();
        let s1 = e.process(sub_msg("/Sensor1/Temperature", 5.0, 60), 1000.0, None);
        let s2 = e.process(sub_msg("/Sensor1/Humidity", 10.0, 60), 1000.0, None);
        let r1 = body(&s1).rid.as_ref().unwrap().clone();
        let r2 = body(&s2).rid.as_ref().unwrap().clone();
        let resp = e.process(cancel_msg(&[&r1, &r2]), 1001.0, None);
        assert_eq!(status(&resp), 200);
        assert_eq!(status(&e.process(poll_msg(&r1), 1002.0, None)), 404);
        assert_eq!(status(&e.process(poll_msg(&r2), 1002.0, None)), 404);
    }

    // --- Integration ---

    #[test]
    fn write_then_read_round_trip() {
        let mut e = Engine::new();
        e.process(write_msg("/Dev/Sensor", OmiValue::Number(42.0)), 0.0, None);
        let resp = e.process(read_with("/Dev/Sensor", Some(1), None, None, None, None), 0.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                let values = val["values"].as_array().unwrap();
                assert_eq!(values[0]["v"], 42.0);
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn write_read_delete_read_lifecycle() {
        let mut e = Engine::new();
        // Write
        assert_eq!(status(&e.process(write_msg("/A/B", OmiValue::Number(1.0)), 0.0, None)), 201);
        // Read — exists
        assert_eq!(status(&e.process(read_msg("/A/B"), 0.0, None)), 200);
        // Delete
        assert_eq!(status(&e.process(delete_msg("/A/B"), 0.0, None)), 200);
        // Read — gone
        assert_eq!(status(&e.process(read_msg("/A/B"), 0.0, None)), 404);
    }

    #[test]
    fn response_message_rejected() {
        let mut e = Engine::new();
        let msg = OmiResponse::ok(serde_json::json!(null));
        let resp = e.process(msg, 0.0, None);
        assert_eq!(status(&resp), 400);
    }

    #[test]
    fn ws_subscription_creates_websocket_target() {
        let mut e = setup();
        // Subscribe without callback, but with ws_session → WebSocket target
        let sub = e.process(poll_sub_msg("/Sensor1/Temperature", 60), 1000.0, Some(42));
        assert_eq!(status(&sub), 200);
        let rid = body(&sub).rid.as_ref().unwrap().clone();
        // Polling a WebSocket sub should return NotPollable
        let resp = e.process(poll_msg(&rid), 1001.0, None);
        assert_eq!(status(&resp), 400);
    }

    #[test]
    fn ws_subscription_callback_overrides_session() {
        let mut e = setup();
        // Subscribe with both callback and ws_session → callback wins
        let sub = e.process(sub_msg("/Sensor1/Temperature", 10.0, 60), 1000.0, Some(42));
        assert_eq!(status(&sub), 200);
        let rid = body(&sub).rid.as_ref().unwrap().clone();
        // Should be a callback sub, not a ws sub — polling returns NotPollable
        let resp = e.process(poll_msg(&rid), 1001.0, None);
        assert_eq!(status(&resp), 400);
    }

    #[test]
    fn interval_poll_sub_tick_and_poll() {
        let mut e = setup();
        // Create interval poll sub (interval=5, no callback)
        let sub = e.process(interval_poll_sub_msg("/Sensor1/Temperature", 5.0, 60), 1000.0, None);
        assert_eq!(status(&sub), 200);
        let rid = body(&sub).rid.as_ref().unwrap().clone();

        // Poll before tick — empty buffer
        let resp = e.process(poll_msg(&rid), 1001.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                assert_eq!(val["values"].as_array().unwrap().len(), 0);
            }
            _ => panic!("expected Single"),
        }

        // Tick at 1006 (due at 1005) — buffers current value
        let (deliveries, _) = e.subscriptions().tick_intervals(1006.0, &|_| {
            Some(vec![crate::odf::Value::new(OmiValue::Number(22.0), Some(1006.0))])
        });
        // Poll subs don't produce deliveries
        assert!(deliveries.is_empty());

        // Poll — should get the buffered value
        let resp = e.process(poll_msg(&rid), 1007.0, None);
        assert_eq!(status(&resp), 200);
        match body(&resp).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                assert_eq!(val["path"], "/Sensor1/Temperature");
                let values = val["values"].as_array().unwrap();
                assert_eq!(values.len(), 1);
                assert_eq!(values[0]["v"], 22.0);
            }
            _ => panic!("expected Single"),
        }
    }

    // --- Event notifications on write ---

    #[test]
    fn write_triggers_poll_event_sub() {
        let mut e = Engine::new();
        // Create the path first
        e.process(write_msg("/Dev/Temp", OmiValue::Number(0.0)), 1000.0, None);
        // Create event poll subscription (interval=-1, no callback)
        let sub = e.process(poll_sub_msg("/Dev/Temp", 60), 1001.0, None);
        assert_eq!(status(&sub), 200);
        let rid = body(&sub).rid.as_ref().unwrap().clone();

        // Write a value — should trigger event notification into poll buffer
        let resp = e.process(write_msg("/Dev/Temp", OmiValue::Number(42.0)), 1002.0, None);
        assert_eq!(status(&resp), 200);

        // Poll — should get the written value
        let poll = e.process(poll_msg(&rid), 1003.0, None);
        assert_eq!(status(&poll), 200);
        match body(&poll).result.as_ref().unwrap() {
            ResponseResult::Single(val) => {
                let values = val["values"].as_array().unwrap();
                assert_eq!(values.len(), 1);
                assert_eq!(values[0]["v"], 42.0);
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn write_triggers_callback_delivery() {
        let mut e = Engine::new();
        e.process(write_msg("/Dev/Temp", OmiValue::Number(0.0)), 1000.0, None);
        // Create event callback subscription
        let sub = e.process(sub_msg("/Dev/Temp", -1.0, 60), 1001.0, None);
        assert_eq!(status(&sub), 200);

        // Write triggers callback delivery
        let resp = e.process(write_msg("/Dev/Temp", OmiValue::Number(99.0)), 1002.0, None);
        assert_eq!(status(&resp), 200);
        assert_eq!(resp.1.len(), 1);
        assert_eq!(resp.1[0].path, "/Dev/Temp");
        assert!(matches!(&resp.1[0].target, DeliveryTarget::Callback(_)));
    }

    #[test]
    fn batch_write_accumulates_deliveries() {
        let mut e = Engine::new();
        e.process(write_msg("/Dev/A", OmiValue::Number(0.0)), 1000.0, None);
        e.process(write_msg("/Dev/B", OmiValue::Number(0.0)), 1000.0, None);
        // Subscribe to both paths with callbacks
        e.process(sub_msg("/Dev/A", -1.0, 60), 1001.0, None);
        e.process(sub_msg("/Dev/B", -1.0, 60), 1001.0, None);

        // Batch write to both
        let items = vec![
            WriteItem { path: "/Dev/A".into(), v: OmiValue::Number(1.0), t: None },
            WriteItem { path: "/Dev/B".into(), v: OmiValue::Number(2.0), t: None },
        ];
        let resp = e.process(batch_msg(items), 1002.0, None);
        assert_eq!(resp.1.len(), 2);
    }

    #[test]
    fn write_returns_deliveries_inline() {
        let mut e = Engine::new();
        e.process(write_msg("/Dev/X", OmiValue::Number(0.0)), 1000.0, None);
        e.process(sub_msg("/Dev/X", -1.0, 60), 1001.0, None);

        let resp = e.process(write_msg("/Dev/X", OmiValue::Number(5.0)), 1002.0, None);
        assert_eq!(resp.1.len(), 1);

        // Non-write operations return no deliveries
        let resp2 = e.process(write_msg("/Dev/X", OmiValue::Number(6.0)), 1003.0, None);
        assert_eq!(resp2.1.len(), 1); // each write gets its own deliveries
    }

    #[test]
    fn write_to_unsubscribed_path_no_deliveries() {
        let mut e = Engine::new();
        let resp = e.process(write_msg("/Dev/NoSub", OmiValue::Number(42.0)), 1000.0, None);
        assert!(resp.1.is_empty());
    }

    // --- Scripting integration ---

    #[cfg(feature = "scripting")]
    mod scripting {
        use super::*;

        /// Set an onwrite script on a writable InfoItem.
        fn set_onwrite(e: &mut Engine, path: &str, script: &str) {
            if let Ok(PathTargetMut::InfoItem(item)) = e.tree.resolve_mut(path) {
                let meta = item.meta.get_or_insert_with(BTreeMap::new);
                meta.insert("onwrite".into(), OmiValue::Str(script.into()));
            }
        }

        /// Read the newest value at a path.
        fn newest_value(e: &mut Engine, path: &str) -> OmiValue {
            match e.tree.resolve(path) {
                Ok(PathTarget::InfoItem(item)) => {
                    let vals = item.query_values(Some(1), None, None, None);
                    vals.into_iter().next().map(|v| v.v).unwrap_or(OmiValue::Null)
                }
                _ => OmiValue::Null,
            }
        }

        #[test]
        fn onwrite_script_cascades_to_another_path() {
            let mut e = Engine::new();
            // Create TempC (writable)
            e.process(write_msg("/Dev/TempC", OmiValue::Number(0.0)), 0.0, None);
            // Create TempF (writable)
            e.process(write_msg("/Dev/TempF", OmiValue::Number(0.0)), 0.0, None);
            // Set onwrite on TempC to convert C→F
            set_onwrite(&mut e, "/Dev/TempC",
                "odf.writeItem(event.value * 9 / 5 + 32, '/Dev/TempF');");
            // Write 25°C → should cascade to 77°F
            e.process(write_msg("/Dev/TempC", OmiValue::Number(25.0)), 0.0, None);
            let temp_f = newest_value(&mut e, "/Dev/TempF");
            assert_eq!(temp_f, OmiValue::Number(77.0));
        }

        #[test]
        fn onwrite_depth_limit_prevents_infinite_recursion() {
            let mut e = Engine::new();
            // Create item that writes back to itself
            e.process(write_msg("/Dev/Loop", OmiValue::Number(0.0)), 0.0, None);
            set_onwrite(&mut e, "/Dev/Loop",
                "odf.writeItem(event.value + 1, '/Dev/Loop');");
            // Write 1.0 at depth 0 → script writes 2.0 (depth 1) → 3.0 (depth 2)
            // → 4.0 (depth 3) → script tries 5.0 but depth 4 >= MAX(4), blocked.
            e.process(write_msg("/Dev/Loop", OmiValue::Number(1.0)), 0.0, None);
            assert_eq!(newest_value(&mut e, "/Dev/Loop"), OmiValue::Number(4.0));
        }

        #[test]
        fn onwrite_script_error_does_not_fail_write() {
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Temp", OmiValue::Number(0.0)), 0.0, None);
            // Set a script with a syntax error
            set_onwrite(&mut e, "/Dev/Temp", "this is not valid javascript!!!");
            // Write should still succeed (script error is logged, not propagated)
            let resp = e.process(write_msg("/Dev/Temp", OmiValue::Number(42.0)), 0.0, None);
            assert_eq!(status(&resp), 200);
            // Value should be written
            assert_eq!(newest_value(&mut e, "/Dev/Temp"), OmiValue::Number(42.0));
        }

        #[test]
        fn onwrite_chain_a_b_c() {
            let mut e = Engine::new();
            // Create three items
            e.process(write_msg("/Dev/A", OmiValue::Number(0.0)), 0.0, None);
            e.process(write_msg("/Dev/B", OmiValue::Number(0.0)), 0.0, None);
            e.process(write_msg("/Dev/C", OmiValue::Number(0.0)), 0.0, None);
            // A → B (double), B → C (add 10)
            set_onwrite(&mut e, "/Dev/A",
                "odf.writeItem(event.value * 2, '/Dev/B');");
            set_onwrite(&mut e, "/Dev/B",
                "odf.writeItem(event.value + 10, '/Dev/C');");
            // Write A=5 → B=10 → C=20
            e.process(write_msg("/Dev/A", OmiValue::Number(5.0)), 0.0, None);
            assert_eq!(newest_value(&mut e, "/Dev/A"), OmiValue::Number(5.0));
            assert_eq!(newest_value(&mut e, "/Dev/B"), OmiValue::Number(10.0));
            assert_eq!(newest_value(&mut e, "/Dev/C"), OmiValue::Number(20.0));
        }

        /// Set an onread script on an InfoItem.
        fn set_onread(e: &mut Engine, path: &str, script: &str) {
            if let Ok(PathTargetMut::InfoItem(item)) = e.tree.resolve_mut(path) {
                let meta = item.meta.get_or_insert_with(BTreeMap::new);
                meta.insert("onread".into(), OmiValue::Str(script.into()));
            }
        }

        #[test]
        fn onread_script_returns_transformed_value() {
            let mut e = Engine::new();
            // Create an item with a value
            e.process(write_msg("/Dev/TempC", OmiValue::Number(25.0)), 0.0, None);
            // Set onread script that converts C→F
            set_onread(&mut e, "/Dev/TempC", "event.value * 9 / 5 + 32");
            // Call run_onread_script directly
            let result = e.run_onread_script(
                "/Dev/TempC",
                &OmiValue::Number(25.0),
                Some(1000.0),
            );
            assert_eq!(result, Some(OmiValue::Number(77.0)));
        }

        #[test]
        fn onread_script_no_script_returns_none() {
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Plain", OmiValue::Number(42.0)), 0.0, None);
            let result = e.run_onread_script(
                "/Dev/Plain",
                &OmiValue::Number(42.0),
                None,
            );
            assert_eq!(result, None);
        }

        #[test]
        fn onread_script_error_returns_none() {
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Bad", OmiValue::Number(1.0)), 0.0, None);
            set_onread(&mut e, "/Dev/Bad", "this is not valid javascript!!!");
            let result = e.run_onread_script(
                "/Dev/Bad",
                &OmiValue::Number(1.0),
                None,
            );
            assert_eq!(result, None);
        }

        #[test]
        fn onread_script_null_return_yields_none() {
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Nul", OmiValue::Number(1.0)), 0.0, None);
            set_onread(&mut e, "/Dev/Nul", "null");
            let result = e.run_onread_script(
                "/Dev/Nul",
                &OmiValue::Number(1.0),
                None,
            );
            assert_eq!(result, None);
        }

        #[test]
        fn onread_script_can_read_other_items() {
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Offset", OmiValue::Number(10.0)), 0.0, None);
            e.process(write_msg("/Dev/Raw", OmiValue::Number(5.0)), 0.0, None);
            // onread reads the offset from another item
            set_onread(&mut e, "/Dev/Raw",
                "event.value + odf.readItem('/Dev/Offset/value')");
            let result = e.run_onread_script(
                "/Dev/Raw",
                &OmiValue::Number(5.0),
                Some(1000.0),
            );
            assert_eq!(result, Some(OmiValue::Number(15.0)));
        }

        #[test]
        fn onread_script_has_event_fields() {
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Ev", OmiValue::Number(99.0)), 0.0, None);
            // Script that uses all event fields to verify they exist
            set_onread(&mut e, "/Dev/Ev",
                "event.path === '/Dev/Ev' && event.timestamp === 42.0 ? event.value : -1");
            let result = e.run_onread_script(
                "/Dev/Ev",
                &OmiValue::Number(99.0),
                Some(42.0),
            );
            assert_eq!(result, Some(OmiValue::Number(99.0)));
        }

        #[test]
        fn onread_script_nonexistent_path_returns_none() {
            let mut e = Engine::new();
            let result = e.run_onread_script(
                "/Dev/Nonexistent",
                &OmiValue::Number(1.0),
                None,
            );
            assert_eq!(result, None);
        }

        #[test]
        fn onread_self_read_returns_stored_value() {
            // FR-008: Self-read recursion guard — reading own path from within
            // an onread script returns the stored value, not the transformed one.
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Self", OmiValue::Number(42.0)), 0.0, None);
            // Script reads its own path — should get stored value (42), not recurse
            set_onread(&mut e, "/Dev/Self",
                "odf.readItem('/Dev/Self/value') + 100");
            let result = e.run_onread_script(
                "/Dev/Self",
                &OmiValue::Number(42.0),
                Some(1000.0),
            );
            // 42 (stored, via guard) + 100 = 142
            assert_eq!(result, Some(OmiValue::Number(142.0)));
        }

        #[test]
        fn onread_self_read_element_structure() {
            // FR-008: Self-read without /value suffix returns element structure
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Elem", OmiValue::Number(7.0)), 0.0, None);
            set_onread(&mut e, "/Dev/Elem",
                "let item = odf.readItem('/Dev/Elem'); item.values[0].v * 2");
            let result = e.run_onread_script(
                "/Dev/Elem",
                &OmiValue::Number(7.0),
                Some(1000.0),
            );
            // 7 (stored, via guard) * 2 = 14
            assert_eq!(result, Some(OmiValue::Number(14.0)));
        }

        #[test]
        fn onread_cross_read_not_blocked() {
            // FR-007: Reading a different item is NOT blocked by the self-read guard.
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Other", OmiValue::Number(10.0)), 0.0, None);
            e.process(write_msg("/Dev/Reader", OmiValue::Number(5.0)), 0.0, None);
            set_onread(&mut e, "/Dev/Reader",
                "event.value + odf.readItem('/Dev/Other/value')");
            let result = e.run_onread_script(
                "/Dev/Reader",
                &OmiValue::Number(5.0),
                Some(1000.0),
            );
            // 5 + 10 = 15 — cross-read works normally
            assert_eq!(result, Some(OmiValue::Number(15.0)));
        }

        #[test]
        fn onread_nested_executes_target_onread() {
            // FR-007: Reading an item with its own onread script from within
            // another onread script triggers the nested script.
            let mut e = Engine::new();
            e.process(write_msg("/Dev/TempC", OmiValue::Number(100.0)), 0.0, None);
            e.process(write_msg("/Dev/Display", OmiValue::Number(0.0)), 0.0, None);
            // TempC onread converts C→F
            set_onread(&mut e, "/Dev/TempC", "event.value * 9 / 5 + 32");
            // Display onread reads TempC (which triggers TempC's onread)
            set_onread(&mut e, "/Dev/Display",
                "odf.readItem('/Dev/TempC/value')");
            let result = e.run_onread_script(
                "/Dev/Display",
                &OmiValue::Number(0.0),
                Some(1000.0),
            );
            // 100°C → 212°F via nested onread
            assert_eq!(result, Some(OmiValue::Number(212.0)));
        }

        #[test]
        fn onread_nested_depth_limit() {
            // FR-007: Nested onread respects MAX_SCRIPT_DEPTH.
            // Create a chain: A reads B reads C reads D reads E.
            // With MAX_SCRIPT_DEPTH=4, the chain should be capped.
            let mut e = Engine::new();
            e.process(write_msg("/Dev/A", OmiValue::Number(1.0)), 0.0, None);
            e.process(write_msg("/Dev/B", OmiValue::Number(2.0)), 0.0, None);
            e.process(write_msg("/Dev/C", OmiValue::Number(3.0)), 0.0, None);
            e.process(write_msg("/Dev/D", OmiValue::Number(4.0)), 0.0, None);
            e.process(write_msg("/Dev/E", OmiValue::Number(5.0)), 0.0, None);
            // Each item's onread reads the next item's value
            set_onread(&mut e, "/Dev/A", "odf.readItem('/Dev/B/value')");
            set_onread(&mut e, "/Dev/B", "odf.readItem('/Dev/C/value')");
            set_onread(&mut e, "/Dev/C", "odf.readItem('/Dev/D/value')");
            set_onread(&mut e, "/Dev/D", "odf.readItem('/Dev/E/value')");
            set_onread(&mut e, "/Dev/E", "event.value * 10");
            // A(depth=0) → B(depth=1) → C(depth=2) → D(depth=3) → E would be depth=4
            // which equals MAX_SCRIPT_DEPTH, so E's onread is NOT executed.
            // D falls through to stored value of E (5.0), then C returns that, etc.
            let result = e.run_onread_script(
                "/Dev/A",
                &OmiValue::Number(1.0),
                None,
            );
            // Depth limit prevents E's onread (5*10=50) from running,
            // so D gets stored value 5.0, which propagates up the chain.
            assert_eq!(result, Some(OmiValue::Number(5.0)));
        }

        #[test]
        fn onread_nested_element_structure() {
            // FR-007: Nested onread with element structure (no /value suffix)
            // returns transformed value in the values array.
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Src", OmiValue::Number(10.0)), 0.0, None);
            set_onread(&mut e, "/Dev/Src", "event.value + 5");
            e.process(write_msg("/Dev/Reader", OmiValue::Number(0.0)), 0.0, None);
            // Read element structure of Src — nested onread transforms v
            set_onread(&mut e, "/Dev/Reader",
                "let item = odf.readItem('/Dev/Src'); item.values[0].v");
            let result = e.run_onread_script(
                "/Dev/Reader",
                &OmiValue::Number(0.0),
                Some(1000.0),
            );
            // Src stored=10, onread returns 10+5=15, element.values[0].v=15
            assert_eq!(result, Some(OmiValue::Number(15.0)));
        }

        #[test]
        fn onread_nested_no_writeitem() {
            // FR-006: Nested onread scripts should not have odf.writeItem.
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Src", OmiValue::Number(10.0)), 0.0, None);
            e.process(write_msg("/Dev/Target", OmiValue::Number(0.0)), 0.0, None);
            // Src's onread tries to write — should fail gracefully
            set_onread(&mut e, "/Dev/Src",
                "odf.writeItem(99, '/Dev/Target'); event.value + 1");
            e.process(write_msg("/Dev/Reader", OmiValue::Number(0.0)), 0.0, None);
            set_onread(&mut e, "/Dev/Reader",
                "odf.readItem('/Dev/Src/value')");
            let result = e.run_onread_script(
                "/Dev/Reader",
                &OmiValue::Number(0.0),
                Some(1000.0),
            );
            // Src's onread tries writeItem which doesn't exist → script error
            // or writeItem is undefined → falls through to stored value (10)
            // The exact behavior depends on mJS — either the script errors
            // (returning None, falling through to stored 10), or writeItem is
            // undefined and the script continues to return 10+1=11.
            // Either way, Target should NOT be modified.
            let target_val = newest_value(&mut e, "/Dev/Target");
            assert_eq!(target_val, OmiValue::Number(0.0), "writeItem must not modify Target");
        }

        #[test]
        fn onwrite_global_state_persists() {
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Counter", OmiValue::Number(0.0)), 0.0, None);
            e.process(write_msg("/Dev/Total", OmiValue::Number(0.0)), 0.0, None);
            // Pre-initialize global accumulator via the script engine
            if let Some(se) = e.script_engine.as_mut() {
                se.exec("let total = 0;").unwrap();
            }
            // Script accumulates a running total in the global variable
            set_onwrite(&mut e, "/Dev/Counter",
                "total = total + event.value; odf.writeItem(total, '/Dev/Total');");
            // Write 10 → total=10
            e.process(write_msg("/Dev/Counter", OmiValue::Number(10.0)), 0.0, None);
            assert_eq!(newest_value(&mut e, "/Dev/Total"), OmiValue::Number(10.0));
            // Write 5 → total=15
            e.process(write_msg("/Dev/Counter", OmiValue::Number(5.0)), 0.0, None);
            assert_eq!(newest_value(&mut e, "/Dev/Total"), OmiValue::Number(15.0));
        }

        #[test]
        fn tick_runs_onread_for_interval_subscriptions() {
            // FR-002: interval subscription deliveries run onread before delivery
            let mut e = Engine::new();
            e.process(write_msg("/Dev/TempC", OmiValue::Number(100.0)), 0.0, None);
            // onread converts C→F
            set_onread(&mut e, "/Dev/TempC", "event.value * 9 / 5 + 32");
            // Create interval callback subscription (interval=10s, ttl=60s)
            e.process(sub_msg("/Dev/TempC", 10.0, 60), 1000.0, None);
            // Tick past the interval
            let deliveries = e.tick(1011.0);
            assert_eq!(deliveries.len(), 1);
            // Delivered value should be transformed: 100°C → 212°F
            assert_eq!(deliveries[0].values[0].v, OmiValue::Number(212.0));
        }

        #[test]
        fn tick_no_onread_delivers_raw_value() {
            // Items without onread deliver stored value as-is
            let mut e = Engine::new();
            e.process(write_msg("/Dev/Raw", OmiValue::Number(42.0)), 0.0, None);
            e.process(sub_msg("/Dev/Raw", 10.0, 60), 1000.0, None);
            let deliveries = e.tick(1011.0);
            assert_eq!(deliveries.len(), 1);
            assert_eq!(deliveries[0].values[0].v, OmiValue::Number(42.0));
        }

        #[test]
        fn notify_write_event_does_not_run_onread() {
            // FR-002: event-based subscriptions deliver written value as-is
            let mut e = Engine::new();
            e.process(write_msg("/Dev/TempC", OmiValue::Number(100.0)), 0.0, None);
            set_onread(&mut e, "/Dev/TempC", "event.value * 9 / 5 + 32");
            // Create event subscription (interval=-1 means event-based)
            e.process(sub_msg("/Dev/TempC", -1.0, 60), 1000.0, None);
            // Write a new value — triggers event notification
            let (_resp, deliveries) = e.process(write_msg("/Dev/TempC", OmiValue::Number(50.0)), 1002.0, None);
            // Event delivery should have the raw written value, NOT transformed
            assert_eq!(deliveries.len(), 1);
            assert_eq!(deliveries[0].values[0].v, OmiValue::Number(50.0));
        }
    }
}
