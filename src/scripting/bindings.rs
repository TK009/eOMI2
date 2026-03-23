//! Script callback implementations for `odf.writeItem()` and `odf.readItem()`.
//!
//! Write callbacks collect writes into a `Vec` rather than calling back into the
//! OMI Engine. The writes are processed after script execution completes,
//! eliminating re-entrant `&mut Engine` aliasing.
//!
//! Read callbacks resolve paths against the `ObjectTree` snapshot provided
//! in the context, returning raw values or element structures to the script.

use super::ffi;
use super::ffi::mjs_name;
use super::convert;
use crate::odf::{ObjectTree, PathTarget, InfoItem};

/// Maximum script cascading depth.
pub const MAX_SCRIPT_DEPTH: u8 = 4;

/// Encoding hint for protocol TX writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteEncoding {
    /// UTF-8 string (default).
    String,
    /// Hex-encoded binary data.
    Hex,
    /// Base64-encoded binary data.
    Base64,
}

impl WriteEncoding {
    /// Convert to the GPIO-layer [`DataEncoding`](crate::gpio::encoding::DataEncoding).
    pub(crate) fn to_data_encoding(self) -> crate::gpio::encoding::DataEncoding {
        match self {
            Self::String => crate::gpio::encoding::DataEncoding::String,
            Self::Hex => crate::gpio::encoding::DataEncoding::Hex,
            Self::Base64 => crate::gpio::encoding::DataEncoding::Base64,
        }
    }
}

/// A write collected during script execution, to be processed afterwards.
pub(crate) struct PendingWrite {
    pub path: String,
    pub value: crate::odf::OmiValue,
    pub encoding: Option<WriteEncoding>,
}

/// Context passed to the JS callback via a foreign pointer.
///
/// Valid only during script execution. Set before `exec()`, cleared after.
pub(crate) struct ScriptCallbackCtx {
    pub pending_writes: *mut Vec<PendingWrite>,
    pub depth: u8,
    /// Immutable reference to the object tree for `odf.readItem()`.
    /// Must remain valid for the duration of script execution.
    pub tree: *const ObjectTree,
    /// Path of the currently-executing onread script (FR-008).
    /// When `odf.readItem()` is called on this path, the stored value is
    /// returned directly without re-triggering the onread script, preventing
    /// infinite recursion. Null when not inside an onread script.
    pub onread_path_ptr: *const u8,
    pub onread_path_len: usize,
    /// Pre-compiled onread functions keyed by item path (FR-007).
    /// Compiled before script execution to avoid re-entrant mJS compilation.
    /// Null when no onread functions are available (e.g. onwrite context).
    pub onread_fns: *const std::collections::BTreeMap<String, ffi::mjs_val_t>,
}

/// C callback for `odf.writeItem(value, path)`.
///
/// Collects the write request into `pending_writes` for deferred processing.
///
/// # Safety
/// Called from mJS during script execution. The `__ctx` foreign pointer on the
/// global object must point to a valid `ScriptCallbackCtx`.
pub(crate) unsafe extern "C" fn js_odf_write_item(mjs: *mut ffi::mjs) {
    let nargs = ffi::mjs_nargs(mjs);
    if nargs < 2 {
        ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, 0));
        return;
    }

    let js_value = ffi::mjs_arg(mjs, 0);
    let js_path = ffi::mjs_arg(mjs, 1);

    // Extract path string
    if ffi::mjs_is_string(js_path) == 0 {
        ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, 0));
        return;
    }
    let mut path_len: usize = 0;
    let mut js_path_copy = js_path;
    let path_ptr = ffi::mjs_get_string(mjs, &mut js_path_copy, &mut path_len);
    if path_ptr.is_null() {
        ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, 0));
        return;
    }
    let path_bytes = std::slice::from_raw_parts(path_ptr as *const u8, path_len);
    let path = match std::str::from_utf8(path_bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, 0));
            return;
        }
    };

    // Convert JS value to OmiValue
    let omi_value = super::convert::mjs_to_omi(mjs, js_value);

    // Retrieve context
    let global = ffi::mjs_get_global(mjs);
    let (ctx_name, ctx_len) = mjs_name!("__ctx");
    let ctx_val = ffi::mjs_get(mjs, global, ctx_name, ctx_len);
    if ffi::mjs_is_foreign(ctx_val) == 0 {
        ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, 0));
        return;
    }
    let ctx_ptr = ffi::mjs_get_ptr(mjs, ctx_val) as *mut ScriptCallbackCtx;
    if ctx_ptr.is_null() {
        ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, 0));
        return;
    }
    let ctx = &*ctx_ptr;

    // Check depth limit — block writes that would exceed max cascading depth
    let new_depth = ctx.depth + 1;
    if new_depth >= MAX_SCRIPT_DEPTH {
        log::warn!("Script depth limit reached at path '{}'", path);
        ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, 0));
        return;
    }

    // Parse optional 3rd arg: {type: 'hex'|'base64'|'string'}
    let encoding = if nargs >= 3 {
        let js_opts = ffi::mjs_arg(mjs, 2);
        if ffi::mjs_is_object(js_opts) != 0 {
            let (type_name, type_len) = mjs_name!("type");
            let js_type = ffi::mjs_get(mjs, js_opts, type_name, type_len);
            if ffi::mjs_is_string(js_type) != 0 {
                let mut type_str_len: usize = 0;
                let mut js_type_copy = js_type;
                let type_ptr = ffi::mjs_get_string(mjs, &mut js_type_copy, &mut type_str_len);
                if !type_ptr.is_null() {
                    let type_bytes = std::slice::from_raw_parts(type_ptr as *const u8, type_str_len);
                    match type_bytes {
                        b"hex" => Some(WriteEncoding::Hex),
                        b"base64" => Some(WriteEncoding::Base64),
                        b"string" => Some(WriteEncoding::String),
                        _ => None, // unknown type — ignore silently
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Collect the write for deferred processing
    let writes = &mut *ctx.pending_writes;
    writes.push(PendingWrite { path, value: omi_value, encoding });
    ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, 1));
}

/// Detect and strip `/value` suffix from a path.
///
/// Returns `(effective_path, wants_raw_value)`. The `/value` suffix is only
/// stripped if it does NOT correspond to an actual InfoItem named "value" at
/// that path — InfoItem names take precedence per FR-010.
fn strip_value_suffix<'a>(path: &'a str, tree: &ObjectTree) -> (&'a str, bool) {
    if !path.ends_with("/value") || path == "/value" {
        return (path, false);
    }

    let prefix = &path[..path.len() - "/value".len()];

    // If the full path resolves to an InfoItem named "value", it's a real item,
    // not the /value suffix accessor.
    if let Ok(PathTarget::InfoItem(_)) = tree.resolve(path) {
        return (path, false);
    }

    (prefix, true)
}

/// Build an mJS object representing an InfoItem element structure:
/// `{ type, desc, values: [{v, t}, ...] }`
///
/// # Safety
/// `mjs` must be a valid mJS instance pointer. `item` must be a valid reference.
unsafe fn info_item_to_mjs(mjs: *mut ffi::mjs, item: &InfoItem) -> ffi::mjs_val_t {
    let obj = ffi::mjs_mk_object(mjs);

    // type (optional)
    if let Some(ref type_uri) = item.type_uri {
        let (name, len) = mjs_name!("type");
        let val = ffi::mjs_mk_string(mjs, type_uri.as_ptr() as *const _, type_uri.len(), 1);
        ffi::mjs_set(mjs, obj, name, len, val);
    }

    // desc (optional)
    if let Some(ref desc) = item.desc {
        let (name, len) = mjs_name!("desc");
        let val = ffi::mjs_mk_string(mjs, desc.as_ptr() as *const _, desc.len(), 1);
        ffi::mjs_set(mjs, obj, name, len, val);
    }

    // values array
    let arr = ffi::mjs_mk_array(mjs);
    let values = item.query_values(None, None, None, None);
    for entry in &values {
        let entry_obj = ffi::mjs_mk_object(mjs);
        let (v_name, v_len) = mjs_name!("v");
        let v_val = convert::omi_to_mjs(mjs, &entry.v);
        ffi::mjs_set(mjs, entry_obj, v_name, v_len, v_val);

        if let Some(t) = entry.t {
            let (t_name, t_len) = mjs_name!("t");
            let t_val = ffi::mjs_mk_number(mjs, t);
            ffi::mjs_set(mjs, entry_obj, t_name, t_len, t_val);
        }

        ffi::mjs_array_push(mjs, arr, entry_obj);
    }

    let (vals_name, vals_len) = mjs_name!("values");
    ffi::mjs_set(mjs, obj, vals_name, vals_len, arr);

    // Top-level "value" shortcut — newest value accessible without array
    // indexing, which works around the mJS array indexing bug on xtensa.
    if !values.is_empty() {
        let (vname, vlen) = mjs_name!("value");
        let vval = convert::omi_to_mjs(mjs, &values[0].v);
        ffi::mjs_set(mjs, obj, vname, vlen, vval);
    }

    obj
}

/// Build an mJS object representing an InfoItem element structure with
/// the newest value overridden by a pre-transformed mJS value.
///
/// # Safety
/// `mjs` must be a valid mJS instance pointer. `item` must be a valid reference.
unsafe fn info_item_to_mjs_with_override(
    mjs: *mut ffi::mjs,
    item: &InfoItem,
    newest_override: ffi::mjs_val_t,
) -> ffi::mjs_val_t {
    let obj = ffi::mjs_mk_object(mjs);

    if let Some(ref type_uri) = item.type_uri {
        let (name, len) = mjs_name!("type");
        let val = ffi::mjs_mk_string(mjs, type_uri.as_ptr() as *const _, type_uri.len(), 1);
        ffi::mjs_set(mjs, obj, name, len, val);
    }

    if let Some(ref desc) = item.desc {
        let (name, len) = mjs_name!("desc");
        let val = ffi::mjs_mk_string(mjs, desc.as_ptr() as *const _, desc.len(), 1);
        ffi::mjs_set(mjs, obj, name, len, val);
    }

    let arr = ffi::mjs_mk_array(mjs);
    let values = item.query_values(None, None, None, None);
    for (i, entry) in values.iter().enumerate() {
        let entry_obj = ffi::mjs_mk_object(mjs);
        let (v_name, v_len) = mjs_name!("v");
        let v_val = if i == 0 {
            newest_override
        } else {
            convert::omi_to_mjs(mjs, &entry.v)
        };
        ffi::mjs_set(mjs, entry_obj, v_name, v_len, v_val);

        if let Some(t) = entry.t {
            let (t_name, t_len) = mjs_name!("t");
            let t_val = ffi::mjs_mk_number(mjs, t);
            ffi::mjs_set(mjs, entry_obj, t_name, t_len, t_val);
        }

        ffi::mjs_array_push(mjs, arr, entry_obj);
    }

    let (vals_name, vals_len) = mjs_name!("values");
    ffi::mjs_set(mjs, obj, vals_name, vals_len, arr);

    obj
}

/// Execute a nested onread script from within a `js_odf_read_item` callback.
///
/// Uses a pre-compiled function from the `onread_fns` cache to avoid
/// re-entrant mJS compilation (which would corrupt the bytecode buffer).
/// The function is called via `mjs_apply`. The nested script gets its own
/// `event` object and a read-only `odf` binding (no `writeItem` per FR-006).
/// The outer script's globals are saved and restored after execution.
///
/// Returns `Some(mjs_val_t)` if the script produced a non-null result.
/// Returns `None` if depth exceeded, no pre-compiled function, or script failed.
///
/// # Safety
/// `mjs` must be a valid mJS instance. `ctx` must point to a valid
/// `ScriptCallbackCtx` with a valid `onread_fns` map.
unsafe fn execute_nested_onread(
    mjs: *mut ffi::mjs,
    item: &crate::odf::InfoItem,
    effective_path: &str,
    ctx: &ScriptCallbackCtx,
) -> Option<ffi::mjs_val_t> {
    // Must have an onread script
    let _ = item.get_onread_script()?;

    let new_depth = ctx.depth + 1;
    if new_depth >= MAX_SCRIPT_DEPTH {
        log::warn!(
            "Nested onread depth limit reached at path '{}'",
            effective_path
        );
        return None;
    }

    // Look up pre-compiled function
    if ctx.onread_fns.is_null() {
        return None;
    }
    let onread_fns = &*ctx.onread_fns;
    let func_val = *onread_fns.get(effective_path)?;
    if ffi::mjs_is_function(func_val) == 0 {
        return None;
    }

    // Get stored value for event.value
    let newest = item.query_values(Some(1), None, None, None);
    let (stored_omi, stored_ts) = if !newest.is_empty() {
        (newest[0].v.clone(), newest[0].t)
    } else {
        (crate::odf::OmiValue::Null, None)
    };

    let global = ffi::mjs_get_global(mjs);

    // Save current globals: event, __ctx, odf
    let (ev_name, ev_len) = mjs_name!("event");
    let old_event = ffi::mjs_get(mjs, global, ev_name, ev_len);
    let (ctx_nm, ctx_ln) = mjs_name!("__ctx");
    let old_ctx_val = ffi::mjs_get(mjs, global, ctx_nm, ctx_ln);
    let (odf_nm, odf_ln) = mjs_name!("odf");
    let old_odf = ffi::mjs_get(mjs, global, odf_nm, odf_ln);

    // Set up new event: { value, path, timestamp }
    let new_event = ffi::mjs_mk_object(mjs);
    let js_val = convert::omi_to_mjs(mjs, &stored_omi);
    let (n, l) = mjs_name!("value");
    ffi::mjs_set(mjs, new_event, n, l, js_val);
    let js_path = ffi::mjs_mk_string(
        mjs,
        effective_path.as_ptr() as *const _,
        effective_path.len(),
        1,
    );
    let (n, l) = mjs_name!("path");
    ffi::mjs_set(mjs, new_event, n, l, js_path);
    let js_ts = match stored_ts {
        Some(t) => ffi::mjs_mk_number(mjs, t),
        None => ffi::mjs_mk_null(),
    };
    let (n, l) = mjs_name!("timestamp");
    ffi::mjs_set(mjs, new_event, n, l, js_ts);
    ffi::mjs_set(mjs, global, ev_name, ev_len, new_event);

    // Set up read-only odf (FR-006: no writeItem in onread scripts)
    let nested_odf = ffi::mjs_mk_object(mjs);
    let read_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_read_item));
    let (n, l) = mjs_name!("readItem");
    ffi::mjs_set(mjs, nested_odf, n, l, read_fn);
    ffi::mjs_set(mjs, global, odf_nm, odf_ln, nested_odf);

    // Set up nested context with incremented depth and self-read guard
    let mut nested_ctx = ScriptCallbackCtx {
        pending_writes: ctx.pending_writes,
        depth: new_depth,
        tree: ctx.tree,
        onread_path_ptr: effective_path.as_ptr(),
        onread_path_len: effective_path.len(),
        onread_fns: ctx.onread_fns,
    };
    let nested_foreign = ffi::mjs_mk_foreign(
        mjs,
        &mut nested_ctx as *mut ScriptCallbackCtx as *mut std::os::raw::c_void,
    );
    ffi::mjs_set(mjs, global, ctx_nm, ctx_ln, nested_foreign);

    // Call the pre-compiled function via mjs_apply (no compilation needed)
    let mut res: ffi::mjs_val_t = 0;
    let err = ffi::mjs_apply(
        mjs,
        &mut res,
        func_val,
        ffi::mjs_mk_undefined(),
        0,
        std::ptr::null_mut(),
    );

    let result = if err == ffi::MJS_OK
        && ffi::mjs_is_null(res) == 0
        && ffi::mjs_is_undefined(res) == 0
    {
        Some(res)
    } else {
        if err != ffi::MJS_OK {
            let err_ptr = ffi::mjs_strerror(mjs, err);
            if !err_ptr.is_null() {
                let msg = std::ffi::CStr::from_ptr(err_ptr)
                    .to_str()
                    .unwrap_or("unknown error");
                log::warn!(
                    "nested onread script error at '{}': {}",
                    effective_path,
                    msg
                );
            }
        }
        None
    };

    // Restore old globals
    ffi::mjs_set(mjs, global, ev_name, ev_len, old_event);
    ffi::mjs_set(mjs, global, ctx_nm, ctx_ln, old_ctx_val);
    ffi::mjs_set(mjs, global, odf_nm, odf_ln, old_odf);

    result
}

/// C callback for `odf.readItem(path)`.
///
/// Resolves the path against the ObjectTree and returns:
/// - With `/value` suffix: the raw primitive value of the most recent entry
/// - Without suffix: the full element structure `{ type, desc, values }`
/// - `null` for: nonexistent path, Object (not InfoItem), non-readable item,
///   empty ring buffer, no args, invalid path
///
/// If the target item has an onread script and the current depth allows it
/// (FR-007), the script is executed inline to transform the value.
///
/// # Safety
/// Called from mJS during script execution. The `__ctx` foreign pointer on the
/// global object must point to a valid `ScriptCallbackCtx` with a valid `tree`.
pub(crate) unsafe extern "C" fn js_odf_read_item(mjs: *mut ffi::mjs) {
    // No args → null
    let nargs = ffi::mjs_nargs(mjs);
    if nargs < 1 {
        ffi::mjs_return(mjs, ffi::mjs_mk_null());
        return;
    }

    // Extract path string
    let js_path = ffi::mjs_arg(mjs, 0);
    if ffi::mjs_is_string(js_path) == 0 {
        ffi::mjs_return(mjs, ffi::mjs_mk_null());
        return;
    }
    let mut path_len: usize = 0;
    let mut js_path_copy = js_path;
    let path_ptr = ffi::mjs_get_string(mjs, &mut js_path_copy, &mut path_len);
    if path_ptr.is_null() {
        ffi::mjs_return(mjs, ffi::mjs_mk_null());
        return;
    }
    let path_bytes = std::slice::from_raw_parts(path_ptr as *const u8, path_len);
    let path = match std::str::from_utf8(path_bytes) {
        Ok(s) => s,
        Err(_) => {
            ffi::mjs_return(mjs, ffi::mjs_mk_null());
            return;
        }
    };

    // Retrieve context
    let global = ffi::mjs_get_global(mjs);
    let (ctx_name, ctx_len) = mjs_name!("__ctx");
    let ctx_val = ffi::mjs_get(mjs, global, ctx_name, ctx_len);
    if ffi::mjs_is_foreign(ctx_val) == 0 {
        ffi::mjs_return(mjs, ffi::mjs_mk_null());
        return;
    }
    let ctx_ptr = ffi::mjs_get_ptr(mjs, ctx_val) as *mut ScriptCallbackCtx;
    if ctx_ptr.is_null() || (*ctx_ptr).tree.is_null() {
        ffi::mjs_return(mjs, ffi::mjs_mk_null());
        return;
    }
    let ctx = &*ctx_ptr;
    let tree = &*ctx.tree;
    let pending_writes = &*ctx.pending_writes;

    // Detect /value suffix (InfoItem named "value" takes precedence)
    let (effective_path, wants_raw) = strip_value_suffix(path, tree);

    // FR-008: Self-read recursion guard. If the requested path matches the
    // currently-executing onread script path, return the stored value directly
    // without re-triggering the onread script (prevents infinite recursion).
    let is_self_read = if !ctx.onread_path_ptr.is_null() {
        let onread_path = std::str::from_utf8_unchecked(
            std::slice::from_raw_parts(ctx.onread_path_ptr, ctx.onread_path_len),
        );
        effective_path == onread_path
    } else {
        false
    };
    if is_self_read {
        // Resolve and return stored value directly — no onread trigger
        let target = match tree.resolve(effective_path) {
            Ok(t) => t,
            Err(_) => {
                ffi::mjs_return(mjs, ffi::mjs_mk_null());
                return;
            }
        };
        let item = match target {
            PathTarget::InfoItem(item) => item,
            _ => {
                ffi::mjs_return(mjs, ffi::mjs_mk_null());
                return;
            }
        };
        if !item.is_readable() || item.values.is_empty() {
            ffi::mjs_return(mjs, ffi::mjs_mk_null());
            return;
        }
        if wants_raw {
            let newest = item.query_values(Some(1), None, None, None);
            if newest.is_empty() {
                ffi::mjs_return(mjs, ffi::mjs_mk_null());
            } else {
                ffi::mjs_return(mjs, convert::omi_to_mjs(mjs, &newest[0].v));
            }
        } else {
            ffi::mjs_return(mjs, info_item_to_mjs(mjs, item));
        }
        return;
    }

    // Read-after-write consistency: check pending writes from the same
    // script cycle first. The last write to this path wins.
    if wants_raw {
        if let Some(pw) = pending_writes.iter().rev().find(|pw| pw.path == effective_path) {
            ffi::mjs_return(mjs, convert::omi_to_mjs(mjs, &pw.value));
            return;
        }
    }

    // Resolve path via ObjectTree
    let target = match tree.resolve(effective_path) {
        Ok(t) => t,
        Err(_) => {
            // Nonexistent or invalid path → null
            ffi::mjs_return(mjs, ffi::mjs_mk_null());
            return;
        }
    };

    // Only InfoItems can be read; Root and Object → null
    let item = match target {
        PathTarget::InfoItem(item) => item,
        _ => {
            ffi::mjs_return(mjs, ffi::mjs_mk_null());
            return;
        }
    };

    // Check readability
    if !item.is_readable() {
        ffi::mjs_return(mjs, ffi::mjs_mk_null());
        return;
    }

    // FR-007: Nested onread — if target item has an onread script,
    // execute it to transform the read value (subject to depth limit).
    if item.get_onread_script().is_some() {
        if let Some(transformed) = execute_nested_onread(mjs, item, effective_path, ctx) {
            if wants_raw {
                ffi::mjs_return(mjs, transformed);
            } else {
                ffi::mjs_return(mjs, info_item_to_mjs_with_override(mjs, item, transformed));
            }
            return;
        }
        // Script failed or depth exceeded — fall through to stored value
    }

    // Empty ring buffer → null (unless pending writes exist for non-raw)
    if item.values.is_empty() && !pending_writes.iter().any(|pw| pw.path == effective_path) {
        ffi::mjs_return(mjs, ffi::mjs_mk_null());
        return;
    }

    if wants_raw {
        // /value suffix: return raw primitive of most recent value
        // (pending writes already checked above)
        let newest = item.query_values(Some(1), None, None, None);
        if newest.is_empty() {
            ffi::mjs_return(mjs, ffi::mjs_mk_null());
        } else {
            ffi::mjs_return(mjs, convert::omi_to_mjs(mjs, &newest[0].v));
        }
    } else {
        // No suffix: return full element structure
        ffi::mjs_return(mjs, info_item_to_mjs(mjs, item));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odf::{OmiValue, ObjectTree, Object, InfoItem};

    unsafe fn with_mjs<F: FnOnce(*mut ffi::mjs)>(f: F) {
        let mjs = ffi::mjs_create();
        assert!(!mjs.is_null());
        f(mjs);
        ffi::mjs_destroy(mjs);
    }

    /// Set up mJS with odf.writeItem and odf.readItem bound to a context,
    /// execute `script`, and return the result as an OmiValue.
    unsafe fn exec_with_bindings(
        tree: &ObjectTree,
        pending: &mut Vec<PendingWrite>,
        depth: u8,
        script: &str,
    ) -> OmiValue {
        let mjs = ffi::mjs_create();
        assert!(!mjs.is_null());
        ffi::mjs_set_max_ops(mjs, 50_000);

        let mut ctx = ScriptCallbackCtx {
            pending_writes: pending as *mut Vec<PendingWrite>,
            depth,
            tree: tree as *const ObjectTree,
            onread_path_ptr: std::ptr::null(),
            onread_path_len: 0,
            onread_fns: std::ptr::null(),
        };

        let global = ffi::mjs_get_global(mjs);

        let ctx_foreign = ffi::mjs_mk_foreign(
            mjs,
            &mut ctx as *mut ScriptCallbackCtx as *mut std::os::raw::c_void,
        );
        let (n, l) = ffi::mjs_name!("__ctx");
        ffi::mjs_set(mjs, global, n, l, ctx_foreign);

        let odf = ffi::mjs_mk_object(mjs);
        let write_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_write_item));
        let (n, l) = ffi::mjs_name!("writeItem");
        ffi::mjs_set(mjs, odf, n, l, write_fn);
        let read_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_read_item));
        let (n, l) = ffi::mjs_name!("readItem");
        ffi::mjs_set(mjs, odf, n, l, read_fn);
        let (n, l) = ffi::mjs_name!("odf");
        ffi::mjs_set(mjs, global, n, l, odf);

        let c_src = std::ffi::CString::new(script).unwrap();
        let mut res: ffi::mjs_val_t = 0;
        ffi::mjs_reset_ops_count(mjs);
        let err = ffi::mjs_exec(mjs, c_src.as_ptr(), &mut res);
        let result = if err == ffi::MJS_OK {
            convert::mjs_to_omi(mjs, res)
        } else {
            let err_ptr = ffi::mjs_strerror(mjs, err);
            let msg = if err_ptr.is_null() {
                "unknown".to_string()
            } else {
                std::ffi::CStr::from_ptr(err_ptr).to_string_lossy().into_owned()
            };
            panic!("mjs_exec failed: {}", msg);
        };

        ffi::mjs_destroy(mjs);
        result
    }

    // ── WriteEncoding ──

    #[test]
    fn write_encoding_equality() {
        assert_eq!(WriteEncoding::String, WriteEncoding::String);
        assert_eq!(WriteEncoding::Hex, WriteEncoding::Hex);
        assert_eq!(WriteEncoding::Base64, WriteEncoding::Base64);
        assert_ne!(WriteEncoding::String, WriteEncoding::Hex);
    }

    #[test]
    fn write_encoding_debug() {
        let s = format!("{:?}", WriteEncoding::Hex);
        assert!(s.contains("Hex"));
    }

    #[test]
    fn write_encoding_clone_copy() {
        let enc = WriteEncoding::Base64;
        let copied = enc;
        assert_eq!(enc, copied);
    }

    // ── strip_value_suffix ──

    #[test]
    fn strip_value_suffix_no_suffix() {
        let tree = ObjectTree::new();
        let (path, wants_raw) = strip_value_suffix("/Dev/Temp", &tree);
        assert_eq!(path, "/Dev/Temp");
        assert!(!wants_raw);
    }

    #[test]
    fn strip_value_suffix_with_suffix() {
        let tree = ObjectTree::new();
        let (path, wants_raw) = strip_value_suffix("/Dev/Temp/value", &tree);
        assert_eq!(path, "/Dev/Temp");
        assert!(wants_raw);
    }

    #[test]
    fn strip_value_suffix_just_value() {
        let tree = ObjectTree::new();
        let (path, wants_raw) = strip_value_suffix("/value", &tree);
        assert_eq!(path, "/value");
        assert!(!wants_raw);
    }

    #[test]
    fn strip_value_suffix_real_item_named_value() {
        let mut tree = ObjectTree::new();
        let mut obj = Object::new("Dev");
        let mut child = Object::new("Temp");
        let item = InfoItem::new(10);
        child.add_item("value".into(), item);
        obj.add_child(child);
        tree.insert_root(obj);

        let (path, wants_raw) = strip_value_suffix("/Dev/Temp/value", &tree);
        assert_eq!(path, "/Dev/Temp/value");
        assert!(!wants_raw);
    }

    #[test]
    fn strip_value_suffix_no_trailing_value() {
        let tree = ObjectTree::new();
        let (path, wants_raw) = strip_value_suffix("/Dev/values", &tree);
        assert_eq!(path, "/Dev/values");
        assert!(!wants_raw);
    }

    // ── ScriptCallbackCtx ──

    #[test]
    fn callback_ctx_layout() {
        let mut writes = Vec::new();
        let tree = ObjectTree::new();
        let ctx = ScriptCallbackCtx {
            pending_writes: &mut writes as *mut _,
            depth: 0,
            tree: &tree as *const _,
            onread_path_ptr: std::ptr::null(),
            onread_path_len: 0,
            onread_fns: std::ptr::null(),
        };
        assert_eq!(ctx.depth, 0);
        assert!(ctx.onread_path_ptr.is_null());
        assert_eq!(ctx.onread_path_len, 0);
        assert!(ctx.onread_fns.is_null());
    }

    // ── js_odf_write_item ──

    #[test]
    fn write_item_collects_pending_write() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            exec_with_bindings(&tree, &mut pending, 0, "odf.writeItem(42, '/Dev/Temp')");
        }
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].path, "/Dev/Temp");
        assert_eq!(pending[0].value, OmiValue::Number(42.0));
        assert!(pending[0].encoding.is_none());
    }

    #[test]
    fn write_item_string_value() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            exec_with_bindings(&tree, &mut pending, 0, "odf.writeItem('hello', '/Dev/Msg')");
        }
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].value, OmiValue::Str("hello".into()));
        assert_eq!(pending[0].path, "/Dev/Msg");
    }

    #[test]
    fn write_item_bool_value() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            exec_with_bindings(&tree, &mut pending, 0, "odf.writeItem(true, '/Dev/Flag')");
        }
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].value, OmiValue::Bool(true));
    }

    #[test]
    fn write_item_null_value() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            exec_with_bindings(&tree, &mut pending, 0, "odf.writeItem(null, '/Dev/X')");
        }
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].value, OmiValue::Null);
    }

    #[test]
    fn write_item_returns_true_on_success() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(&tree, &mut pending, 0, "odf.writeItem(1, '/x')");
            assert_eq!(result, OmiValue::Bool(true));
        }
    }

    #[test]
    fn write_item_too_few_args_returns_false() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(&tree, &mut pending, 0, "odf.writeItem(42)");
            assert_eq!(result, OmiValue::Bool(false));
        }
        assert!(pending.is_empty());
    }

    #[test]
    fn write_item_no_args_returns_false() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(&tree, &mut pending, 0, "odf.writeItem()");
            assert_eq!(result, OmiValue::Bool(false));
        }
        assert!(pending.is_empty());
    }

    #[test]
    fn write_item_non_string_path_returns_false() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(&tree, &mut pending, 0, "odf.writeItem(42, 123)");
            assert_eq!(result, OmiValue::Bool(false));
        }
        assert!(pending.is_empty());
    }

    #[test]
    fn write_item_depth_limit_returns_false() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                MAX_SCRIPT_DEPTH - 1,
                "odf.writeItem(1, '/x')",
            );
            assert_eq!(result, OmiValue::Bool(false));
        }
        assert!(pending.is_empty());
    }

    #[test]
    fn write_item_at_penultimate_depth_succeeds() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                MAX_SCRIPT_DEPTH - 2,
                "odf.writeItem(1, '/x')",
            );
            assert_eq!(result, OmiValue::Bool(true));
        }
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn write_item_with_hex_encoding() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.writeItem('deadbeef', '/Dev/Data', {type: 'hex'})",
            );
        }
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].encoding, Some(WriteEncoding::Hex));
    }

    #[test]
    fn write_item_with_base64_encoding() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.writeItem('data', '/Dev/D', {type: 'base64'})",
            );
        }
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].encoding, Some(WriteEncoding::Base64));
    }

    #[test]
    fn write_item_with_string_encoding() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.writeItem('txt', '/Dev/D', {type: 'string'})",
            );
        }
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].encoding, Some(WriteEncoding::String));
    }

    #[test]
    fn write_item_with_unknown_encoding() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.writeItem('x', '/Dev/D', {type: 'unknown'})",
            );
        }
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].encoding, None);
    }

    #[test]
    fn write_item_opts_without_type() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.writeItem('x', '/Dev/D', {foo: 'bar'})",
            );
        }
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].encoding, None);
    }

    #[test]
    fn write_item_multiple_writes() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.writeItem(1, '/a'); odf.writeItem(2, '/b'); odf.writeItem(3, '/c')",
            );
        }
        assert_eq!(pending.len(), 3);
        assert_eq!(pending[0].path, "/a");
        assert_eq!(pending[1].path, "/b");
        assert_eq!(pending[2].path, "/c");
        assert_eq!(pending[0].value, OmiValue::Number(1.0));
        assert_eq!(pending[1].value, OmiValue::Number(2.0));
        assert_eq!(pending[2].value, OmiValue::Number(3.0));
    }

    // ── js_odf_read_item ──

    fn make_tree_with_item(path: &str, value: OmiValue, ts: Option<f64>) -> ObjectTree {
        let mut tree = ObjectTree::new();
        tree.write_value(path, value, ts).unwrap();
        tree
    }

    #[test]
    fn read_item_with_value_suffix() {
        let tree = make_tree_with_item("/Dev/Temp", OmiValue::Number(22.5), Some(1000.0));
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.readItem('/Dev/Temp/value')",
            );
            assert_eq!(result, OmiValue::Number(22.5));
        }
    }

    #[test]
    fn read_item_without_suffix_returns_object() {
        let tree = make_tree_with_item("/Dev/Temp", OmiValue::Number(22.5), Some(1000.0));
        let mut pending = Vec::new();
        unsafe {
            // Without /value suffix, returns element object (mjs_to_omi → Null for objects)
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.readItem('/Dev/Temp')",
            );
            assert_eq!(result, OmiValue::Null);
        }
    }

    #[test]
    fn read_item_nonexistent_returns_null() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.readItem('/nonexistent/path/value')",
            );
            assert_eq!(result, OmiValue::Null);
        }
    }

    #[test]
    fn read_item_no_args_returns_null() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(&tree, &mut pending, 0, "odf.readItem()");
            assert_eq!(result, OmiValue::Null);
        }
    }

    #[test]
    fn read_item_non_string_arg_returns_null() {
        let tree = ObjectTree::new();
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(&tree, &mut pending, 0, "odf.readItem(123)");
            assert_eq!(result, OmiValue::Null);
        }
    }

    #[test]
    fn read_item_object_path_returns_null() {
        let mut tree = ObjectTree::new();
        tree.insert_root(Object::new("Dev"));
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.readItem('/Dev')",
            );
            assert_eq!(result, OmiValue::Null);
        }
    }

    #[test]
    fn read_item_non_readable_returns_null() {
        let mut tree = ObjectTree::new();
        let mut obj = Object::new("Dev");
        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Number(42.0), None);
        let mut meta = std::collections::BTreeMap::new();
        meta.insert("readable".into(), OmiValue::Bool(false));
        item.meta = Some(meta);
        obj.add_item("Temp".into(), item);
        tree.insert_root(obj);

        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.readItem('/Dev/Temp/value')",
            );
            assert_eq!(result, OmiValue::Null);
        }
    }

    #[test]
    fn read_item_empty_values_returns_null() {
        let mut tree = ObjectTree::new();
        let mut obj = Object::new("Dev");
        let item = InfoItem::new(10);
        obj.add_item("Temp".into(), item);
        tree.insert_root(obj);

        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.readItem('/Dev/Temp/value')",
            );
            assert_eq!(result, OmiValue::Null);
        }
    }

    #[test]
    fn read_item_string_value() {
        let tree = make_tree_with_item("/Dev/Msg", OmiValue::Str("hello".into()), None);
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.readItem('/Dev/Msg/value')",
            );
            assert_eq!(result, OmiValue::Str("hello".into()));
        }
    }

    #[test]
    fn read_item_bool_value() {
        let tree = make_tree_with_item("/Dev/Flag", OmiValue::Bool(true), None);
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.readItem('/Dev/Flag/value')",
            );
            assert_eq!(result, OmiValue::Bool(true));
        }
    }

    // ── Read-after-write consistency ──

    #[test]
    fn read_after_write_returns_pending_value() {
        let tree = make_tree_with_item("/Dev/Temp", OmiValue::Number(10.0), None);
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.writeItem(99, '/Dev/Temp'); odf.readItem('/Dev/Temp/value')",
            );
            assert_eq!(result, OmiValue::Number(99.0));
        }
    }

    #[test]
    fn read_after_multiple_writes_returns_last() {
        let tree = make_tree_with_item("/Dev/Temp", OmiValue::Number(10.0), None);
        let mut pending = Vec::new();
        unsafe {
            let result = exec_with_bindings(
                &tree,
                &mut pending,
                0,
                "odf.writeItem(1, '/Dev/Temp'); odf.writeItem(2, '/Dev/Temp'); odf.readItem('/Dev/Temp/value')",
            );
            assert_eq!(result, OmiValue::Number(2.0));
        }
    }

    // ── info_item_to_mjs ──

    #[test]
    fn info_item_to_mjs_with_type_and_desc() {
        unsafe {
            with_mjs(|mjs| {
                let mut item = InfoItem::new(10);
                item.type_uri = Some("omi:temp".into());
                item.desc = Some("Temperature".into());
                item.add_value(OmiValue::Number(22.5), Some(1000.0));

                let obj = info_item_to_mjs(mjs, &item);
                assert_ne!(ffi::mjs_is_object(obj), 0);

                let (n, l) = ffi::mjs_name!("type");
                let type_val = ffi::mjs_get(mjs, obj, n, l);
                let omi_type = convert::mjs_to_omi(mjs, type_val);
                assert_eq!(omi_type, OmiValue::Str("omi:temp".into()));

                let (n, l) = ffi::mjs_name!("desc");
                let desc_val = ffi::mjs_get(mjs, obj, n, l);
                let omi_desc = convert::mjs_to_omi(mjs, desc_val);
                assert_eq!(omi_desc, OmiValue::Str("Temperature".into()));

                let (n, l) = ffi::mjs_name!("values");
                let arr = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_object(arr), 0);
            });
        }
    }

    #[test]
    fn info_item_to_mjs_no_optional_fields() {
        unsafe {
            with_mjs(|mjs| {
                let item = InfoItem::new(10);
                let obj = info_item_to_mjs(mjs, &item);
                assert_ne!(ffi::mjs_is_object(obj), 0);

                let (n, l) = ffi::mjs_name!("type");
                let type_val = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_undefined(type_val), 0);

                let (n, l) = ffi::mjs_name!("desc");
                let desc_val = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_undefined(desc_val), 0);
            });
        }
    }

    // ── info_item_to_mjs_with_override ──

    #[test]
    fn info_item_override_replaces_newest() {
        unsafe {
            with_mjs(|mjs| {
                let mut item = InfoItem::new(10);
                item.add_value(OmiValue::Number(10.0), Some(1.0));
                item.add_value(OmiValue::Number(20.0), Some(2.0));

                let override_val = ffi::mjs_mk_number(mjs, 999.0);
                let obj = info_item_to_mjs_with_override(mjs, &item, override_val);
                assert_ne!(ffi::mjs_is_object(obj), 0);

                let (n, l) = ffi::mjs_name!("values");
                let arr = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_object(arr), 0);
            });
        }
    }

    // ── PendingWrite ──

    #[test]
    fn pending_write_fields() {
        let pw = PendingWrite {
            path: "/Dev/Temp".into(),
            value: OmiValue::Number(42.0),
            encoding: Some(WriteEncoding::Hex),
        };
        assert_eq!(pw.path, "/Dev/Temp");
        assert_eq!(pw.value, OmiValue::Number(42.0));
        assert_eq!(pw.encoding, Some(WriteEncoding::Hex));
    }

    #[test]
    fn pending_write_no_encoding() {
        let pw = PendingWrite {
            path: "/x".into(),
            value: OmiValue::Null,
            encoding: None,
        };
        assert!(pw.encoding.is_none());
    }

    // ── Self-read recursion guard (FR-008) ──

    #[test]
    fn read_item_self_read_guard_returns_stored_value() {
        let tree = make_tree_with_item("/Dev/Temp", OmiValue::Number(55.0), Some(100.0));
        let mut pending = Vec::new();
        let onread_path = "/Dev/Temp";

        unsafe {
            let mjs = ffi::mjs_create();
            assert!(!mjs.is_null());
            ffi::mjs_set_max_ops(mjs, 50_000);

            let mut ctx = ScriptCallbackCtx {
                pending_writes: &mut pending as *mut Vec<PendingWrite>,
                depth: 1,
                tree: &tree as *const ObjectTree,
                onread_path_ptr: onread_path.as_ptr(),
                onread_path_len: onread_path.len(),
                onread_fns: std::ptr::null(),
            };

            let global = ffi::mjs_get_global(mjs);
            let ctx_foreign = ffi::mjs_mk_foreign(
                mjs,
                &mut ctx as *mut ScriptCallbackCtx as *mut std::os::raw::c_void,
            );
            let (n, l) = ffi::mjs_name!("__ctx");
            ffi::mjs_set(mjs, global, n, l, ctx_foreign);

            let odf = ffi::mjs_mk_object(mjs);
            let read_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_read_item));
            let (n, l) = ffi::mjs_name!("readItem");
            ffi::mjs_set(mjs, odf, n, l, read_fn);
            let (n, l) = ffi::mjs_name!("odf");
            ffi::mjs_set(mjs, global, n, l, odf);

            let c_src = std::ffi::CString::new("odf.readItem('/Dev/Temp/value')").unwrap();
            let mut res: ffi::mjs_val_t = 0;
            ffi::mjs_reset_ops_count(mjs);
            let err = ffi::mjs_exec(mjs, c_src.as_ptr(), &mut res);
            assert_eq!(err, ffi::MJS_OK);
            let result = convert::mjs_to_omi(mjs, res);
            assert_eq!(result, OmiValue::Number(55.0));

            ffi::mjs_destroy(mjs);
        }
    }

    // ── Context safety: null/missing __ctx ──

    #[test]
    fn write_item_without_ctx_returns_false() {
        unsafe {
            let mjs = ffi::mjs_create();
            assert!(!mjs.is_null());
            ffi::mjs_set_max_ops(mjs, 50_000);

            let global = ffi::mjs_get_global(mjs);

            let odf = ffi::mjs_mk_object(mjs);
            let write_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_write_item));
            let (n, l) = ffi::mjs_name!("writeItem");
            ffi::mjs_set(mjs, odf, n, l, write_fn);
            let (n, l) = ffi::mjs_name!("odf");
            ffi::mjs_set(mjs, global, n, l, odf);

            let c_src = std::ffi::CString::new("odf.writeItem(1, '/x')").unwrap();
            let mut res: ffi::mjs_val_t = 0;
            ffi::mjs_reset_ops_count(mjs);
            let err = ffi::mjs_exec(mjs, c_src.as_ptr(), &mut res);
            assert_eq!(err, ffi::MJS_OK);
            let result = convert::mjs_to_omi(mjs, res);
            assert_eq!(result, OmiValue::Bool(false));

            ffi::mjs_destroy(mjs);
        }
    }

    #[test]
    fn read_item_without_ctx_returns_null() {
        unsafe {
            let mjs = ffi::mjs_create();
            assert!(!mjs.is_null());
            ffi::mjs_set_max_ops(mjs, 50_000);

            let global = ffi::mjs_get_global(mjs);

            let odf = ffi::mjs_mk_object(mjs);
            let read_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_read_item));
            let (n, l) = ffi::mjs_name!("readItem");
            ffi::mjs_set(mjs, odf, n, l, read_fn);
            let (n, l) = ffi::mjs_name!("odf");
            ffi::mjs_set(mjs, global, n, l, odf);

            let c_src = std::ffi::CString::new("odf.readItem('/Dev/Temp/value')").unwrap();
            let mut res: ffi::mjs_val_t = 0;
            ffi::mjs_reset_ops_count(mjs);
            let err = ffi::mjs_exec(mjs, c_src.as_ptr(), &mut res);
            assert_eq!(err, ffi::MJS_OK);
            let result = convert::mjs_to_omi(mjs, res);
            assert_eq!(result, OmiValue::Null);

            ffi::mjs_destroy(mjs);
        }
    }

    // ── execute_nested_onread ──

    #[test]
    fn nested_onread_no_script_returns_none() {
        unsafe {
            with_mjs(|mjs| {
                // Item WITHOUT onread script
                let mut item = InfoItem::new(10);
                item.add_value(OmiValue::Number(1.0), None);

                let mut writes = Vec::new();
                let tree = ObjectTree::new();
                let onread_fns: std::collections::BTreeMap<String, ffi::mjs_val_t> =
                    std::collections::BTreeMap::new();
                let ctx = ScriptCallbackCtx {
                    pending_writes: &mut writes,
                    depth: 0,
                    tree: &tree,
                    onread_path_ptr: std::ptr::null(),
                    onread_path_len: 0,
                    onread_fns: &onread_fns,
                };

                let result = execute_nested_onread(mjs, &item, "/Dev/Temp", &ctx);
                assert!(result.is_none());
            });
        }
    }

    #[test]
    fn nested_onread_depth_exceeded_returns_none() {
        unsafe {
            with_mjs(|mjs| {
                let mut item = InfoItem::new(10);
                item.add_value(OmiValue::Number(1.0), None);
                let mut meta = std::collections::BTreeMap::new();
                meta.insert("onread".into(), OmiValue::Str("event.value".into()));
                item.meta = Some(meta);

                let mut writes = Vec::new();
                let tree = ObjectTree::new();
                let onread_fns: std::collections::BTreeMap<String, ffi::mjs_val_t> =
                    std::collections::BTreeMap::new();
                let ctx = ScriptCallbackCtx {
                    pending_writes: &mut writes,
                    depth: MAX_SCRIPT_DEPTH - 1, // new_depth == MAX_SCRIPT_DEPTH → blocked
                    tree: &tree,
                    onread_path_ptr: std::ptr::null(),
                    onread_path_len: 0,
                    onread_fns: &onread_fns,
                };

                let result = execute_nested_onread(mjs, &item, "/Dev/Temp", &ctx);
                assert!(result.is_none());
            });
        }
    }

    #[test]
    fn nested_onread_null_fns_returns_none() {
        unsafe {
            with_mjs(|mjs| {
                let mut item = InfoItem::new(10);
                item.add_value(OmiValue::Number(1.0), None);
                let mut meta = std::collections::BTreeMap::new();
                meta.insert("onread".into(), OmiValue::Str("event.value".into()));
                item.meta = Some(meta);

                let mut writes = Vec::new();
                let tree = ObjectTree::new();
                let ctx = ScriptCallbackCtx {
                    pending_writes: &mut writes,
                    depth: 0,
                    tree: &tree,
                    onread_path_ptr: std::ptr::null(),
                    onread_path_len: 0,
                    onread_fns: std::ptr::null(), // null onread_fns
                };

                let result = execute_nested_onread(mjs, &item, "/Dev/Temp", &ctx);
                assert!(result.is_none());
            });
        }
    }

    #[test]
    fn nested_onread_missing_func_in_map_returns_none() {
        unsafe {
            with_mjs(|mjs| {
                let mut item = InfoItem::new(10);
                item.add_value(OmiValue::Number(1.0), None);
                let mut meta = std::collections::BTreeMap::new();
                meta.insert("onread".into(), OmiValue::Str("event.value".into()));
                item.meta = Some(meta);

                let mut writes = Vec::new();
                let tree = ObjectTree::new();
                // Map exists but doesn't contain the path
                let onread_fns: std::collections::BTreeMap<String, ffi::mjs_val_t> =
                    std::collections::BTreeMap::new();
                let ctx = ScriptCallbackCtx {
                    pending_writes: &mut writes,
                    depth: 0,
                    tree: &tree,
                    onread_path_ptr: std::ptr::null(),
                    onread_path_len: 0,
                    onread_fns: &onread_fns,
                };

                let result = execute_nested_onread(mjs, &item, "/Dev/Temp", &ctx);
                assert!(result.is_none());
            });
        }
    }

    #[test]
    fn nested_onread_non_function_value_returns_none() {
        unsafe {
            with_mjs(|mjs| {
                let mut item = InfoItem::new(10);
                item.add_value(OmiValue::Number(1.0), None);
                let mut meta = std::collections::BTreeMap::new();
                meta.insert("onread".into(), OmiValue::Str("event.value".into()));
                item.meta = Some(meta);

                let mut writes = Vec::new();
                let tree = ObjectTree::new();
                // Map contains the path but value is not a function
                let mut onread_fns: std::collections::BTreeMap<String, ffi::mjs_val_t> =
                    std::collections::BTreeMap::new();
                let not_a_func = ffi::mjs_mk_number(mjs, 42.0);
                onread_fns.insert("/Dev/Temp".into(), not_a_func);
                let ctx = ScriptCallbackCtx {
                    pending_writes: &mut writes,
                    depth: 0,
                    tree: &tree,
                    onread_path_ptr: std::ptr::null(),
                    onread_path_len: 0,
                    onread_fns: &onread_fns,
                };

                let result = execute_nested_onread(mjs, &item, "/Dev/Temp", &ctx);
                assert!(result.is_none());
            });
        }
    }

    #[test]
    fn nested_onread_executes_precompiled_function() {
        unsafe {
            with_mjs(|mjs| {
                // Build a tree and item with onread script
                let mut tree = ObjectTree::new();
                let mut obj = Object::new("Dev");
                let mut item = InfoItem::new(10);
                item.add_value(OmiValue::Number(5.0), Some(100.0));
                let mut meta = std::collections::BTreeMap::new();
                meta.insert("onread".into(), OmiValue::Str("event.value * 10".into()));
                item.meta = Some(meta);
                obj.add_item("Temp".into(), item);
                tree.insert_root(obj);

                // Pre-compile the function
                let script_src = "(function(){ return event.value * 10; })\0";
                let mut func_val: ffi::mjs_val_t = 0;
                let err = ffi::mjs_exec(mjs, script_src.as_ptr() as *const _, &mut func_val);
                assert_eq!(err, ffi::MJS_OK);
                assert_ne!(ffi::mjs_is_function(func_val), 0);

                let mut onread_fns: std::collections::BTreeMap<String, ffi::mjs_val_t> =
                    std::collections::BTreeMap::new();
                onread_fns.insert("/Dev/Temp".into(), func_val);

                let mut writes = Vec::new();
                let mut ctx = ScriptCallbackCtx {
                    pending_writes: &mut writes,
                    depth: 0,
                    tree: &tree,
                    onread_path_ptr: std::ptr::null(),
                    onread_path_len: 0,
                    onread_fns: &onread_fns,
                };

                // Set up globals needed by nested execution
                let global = ffi::mjs_get_global(mjs);
                let ctx_foreign = ffi::mjs_mk_foreign(
                    mjs,
                    &mut ctx as *mut ScriptCallbackCtx as *mut std::os::raw::c_void,
                );
                let (n, l) = ffi::mjs_name!("__ctx");
                ffi::mjs_set(mjs, global, n, l, ctx_foreign);
                let odf = ffi::mjs_mk_object(mjs);
                let read_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_read_item));
                let (n, l) = ffi::mjs_name!("readItem");
                ffi::mjs_set(mjs, odf, n, l, read_fn);
                let (n, l) = ffi::mjs_name!("odf");
                ffi::mjs_set(mjs, global, n, l, odf);
                let event = ffi::mjs_mk_object(mjs);
                let (n, l) = ffi::mjs_name!("event");
                ffi::mjs_set(mjs, global, n, l, event);

                if let Ok(PathTarget::InfoItem(item)) = tree.resolve("/Dev/Temp") {
                    let result = execute_nested_onread(mjs, item, "/Dev/Temp", &ctx);
                    assert!(result.is_some(), "expected Some from nested onread");
                    let val = result.unwrap();
                    assert_ne!(ffi::mjs_is_number(val), 0);
                    assert_eq!(ffi::mjs_get_double(mjs, val), 50.0); // 5.0 * 10
                } else {
                    panic!("expected InfoItem at /Dev/Temp");
                }
            });
        }
    }

    #[test]
    fn nested_onread_restores_globals() {
        unsafe {
            with_mjs(|mjs| {
                let mut tree = ObjectTree::new();
                let mut obj = Object::new("Dev");
                let mut item = InfoItem::new(10);
                item.add_value(OmiValue::Number(3.0), None);
                let mut meta = std::collections::BTreeMap::new();
                meta.insert("onread".into(), OmiValue::Str("event.value + 1".into()));
                item.meta = Some(meta);
                obj.add_item("Temp".into(), item);
                tree.insert_root(obj);

                let script_src = "(function(){ return event.value + 1; })\0";
                let mut func_val: ffi::mjs_val_t = 0;
                ffi::mjs_exec(mjs, script_src.as_ptr() as *const _, &mut func_val);
                let mut onread_fns: std::collections::BTreeMap<String, ffi::mjs_val_t> =
                    std::collections::BTreeMap::new();
                onread_fns.insert("/Dev/Temp".into(), func_val);

                let mut writes = Vec::new();
                let mut ctx = ScriptCallbackCtx {
                    pending_writes: &mut writes,
                    depth: 0,
                    tree: &tree,
                    onread_path_ptr: std::ptr::null(),
                    onread_path_len: 0,
                    onread_fns: &onread_fns,
                };

                // Set up initial globals — save the raw event val for comparison
                let global = ffi::mjs_get_global(mjs);
                let original_event = ffi::mjs_mk_object(mjs);
                let (n, l) = ffi::mjs_name!("event");
                ffi::mjs_set(mjs, global, n, l, original_event);

                let ctx_foreign = ffi::mjs_mk_foreign(
                    mjs,
                    &mut ctx as *mut ScriptCallbackCtx as *mut std::os::raw::c_void,
                );
                let (n, l) = ffi::mjs_name!("__ctx");
                ffi::mjs_set(mjs, global, n, l, ctx_foreign);

                let original_odf = ffi::mjs_mk_object(mjs);
                let read_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_read_item));
                let (n, l) = ffi::mjs_name!("readItem");
                ffi::mjs_set(mjs, original_odf, n, l, read_fn);
                let (n, l) = ffi::mjs_name!("odf");
                ffi::mjs_set(mjs, global, n, l, original_odf);

                // Execute nested onread
                if let Ok(PathTarget::InfoItem(item)) = tree.resolve("/Dev/Temp") {
                    execute_nested_onread(mjs, item, "/Dev/Temp", &ctx);
                }

                // Verify event global was restored to the original object (same mjs_val_t)
                let (n, l) = ffi::mjs_name!("event");
                let restored_event = ffi::mjs_get(mjs, global, n, l);
                assert_eq!(restored_event, original_event, "event global should be restored");

                // Verify odf global was restored
                let (n, l) = ffi::mjs_name!("odf");
                let restored_odf = ffi::mjs_get(mjs, global, n, l);
                assert_eq!(restored_odf, original_odf, "odf global should be restored");
            });
        }
    }

    #[test]
    fn nested_onread_null_result_returns_none() {
        unsafe {
            with_mjs(|mjs| {
                let mut tree = ObjectTree::new();
                let mut obj = Object::new("Dev");
                let mut item = InfoItem::new(10);
                item.add_value(OmiValue::Number(1.0), None);
                let mut meta = std::collections::BTreeMap::new();
                meta.insert("onread".into(), OmiValue::Str("null".into()));
                item.meta = Some(meta);
                obj.add_item("Temp".into(), item);
                tree.insert_root(obj);

                // Function returns null
                let script_src = "(function(){ return null; })\0";
                let mut func_val: ffi::mjs_val_t = 0;
                ffi::mjs_exec(mjs, script_src.as_ptr() as *const _, &mut func_val);
                let mut onread_fns: std::collections::BTreeMap<String, ffi::mjs_val_t> =
                    std::collections::BTreeMap::new();
                onread_fns.insert("/Dev/Temp".into(), func_val);

                let mut writes = Vec::new();
                let mut ctx = ScriptCallbackCtx {
                    pending_writes: &mut writes,
                    depth: 0,
                    tree: &tree,
                    onread_path_ptr: std::ptr::null(),
                    onread_path_len: 0,
                    onread_fns: &onread_fns,
                };

                let global = ffi::mjs_get_global(mjs);
                let ctx_foreign = ffi::mjs_mk_foreign(
                    mjs,
                    &mut ctx as *mut ScriptCallbackCtx as *mut std::os::raw::c_void,
                );
                let (n, l) = ffi::mjs_name!("__ctx");
                ffi::mjs_set(mjs, global, n, l, ctx_foreign);
                let odf = ffi::mjs_mk_object(mjs);
                let (n, l) = ffi::mjs_name!("odf");
                ffi::mjs_set(mjs, global, n, l, odf);
                let event = ffi::mjs_mk_object(mjs);
                let (n, l) = ffi::mjs_name!("event");
                ffi::mjs_set(mjs, global, n, l, event);

                if let Ok(PathTarget::InfoItem(item)) = tree.resolve("/Dev/Temp") {
                    let result = execute_nested_onread(mjs, item, "/Dev/Temp", &ctx);
                    assert!(result.is_none(), "null return should yield None");
                }
            });
        }
    }

    // ── readItem with onread via js_odf_read_item callback ──

    #[test]
    fn read_item_triggers_onread_script() {
        unsafe {
            let mjs = ffi::mjs_create();
            assert!(!mjs.is_null());
            ffi::mjs_set_max_ops(mjs, 50_000);

            // Build tree: /Dev/Sensor with onread script and value
            let mut tree = ObjectTree::new();
            let mut obj = Object::new("Dev");
            let mut item = InfoItem::new(10);
            item.add_value(OmiValue::Number(7.0), Some(100.0));
            let mut meta = std::collections::BTreeMap::new();
            meta.insert("onread".into(), OmiValue::Str("event.value * 2".into()));
            item.meta = Some(meta);
            obj.add_item("Sensor".into(), item);
            tree.insert_root(obj);

            // Pre-compile onread function
            let script_src = "(function(){ return event.value * 2; })\0";
            let mut func_val: ffi::mjs_val_t = 0;
            let err = ffi::mjs_exec(mjs, script_src.as_ptr() as *const _, &mut func_val);
            assert_eq!(err, ffi::MJS_OK);

            let mut onread_fns: std::collections::BTreeMap<String, ffi::mjs_val_t> =
                std::collections::BTreeMap::new();
            onread_fns.insert("/Dev/Sensor".into(), func_val);

            let mut writes = Vec::new();
            let mut ctx = ScriptCallbackCtx {
                pending_writes: &mut writes,
                depth: 0,
                tree: &tree,
                onread_path_ptr: std::ptr::null(),
                onread_path_len: 0,
                onread_fns: &onread_fns,
            };

            let global = ffi::mjs_get_global(mjs);
            let ctx_foreign = ffi::mjs_mk_foreign(
                mjs,
                &mut ctx as *mut ScriptCallbackCtx as *mut std::os::raw::c_void,
            );
            let (n, l) = ffi::mjs_name!("__ctx");
            ffi::mjs_set(mjs, global, n, l, ctx_foreign);

            let odf = ffi::mjs_mk_object(mjs);
            let read_fn = ffi::mjs_mk_foreign_func(mjs, Some(js_odf_read_item));
            let (n, l) = ffi::mjs_name!("readItem");
            ffi::mjs_set(mjs, odf, n, l, read_fn);
            let (n, l) = ffi::mjs_name!("odf");
            ffi::mjs_set(mjs, global, n, l, odf);

            // Read with /value suffix — should trigger onread and return transformed value
            let c_src = std::ffi::CString::new("odf.readItem('/Dev/Sensor/value')").unwrap();
            let mut res: ffi::mjs_val_t = 0;
            ffi::mjs_reset_ops_count(mjs);
            let err = ffi::mjs_exec(mjs, c_src.as_ptr(), &mut res);
            assert_eq!(err, ffi::MJS_OK);
            assert_ne!(ffi::mjs_is_number(res), 0);
            assert_eq!(ffi::mjs_get_double(mjs, res), 14.0); // 7.0 * 2

            ffi::mjs_destroy(mjs);
        }
    }

    // ── MAX_SCRIPT_DEPTH constant ──

    #[test]
    fn max_script_depth_is_4() {
        assert_eq!(MAX_SCRIPT_DEPTH, 4);
    }
}
