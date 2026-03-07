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

    obj
}

/// C callback for `odf.readItem(path)`.
///
/// Resolves the path against the ObjectTree and returns:
/// - With `/value` suffix: the raw primitive value of the most recent entry
/// - Without suffix: the full element structure `{ type, desc, values }`
/// - `null` for: nonexistent path, Object (not InfoItem), non-readable item,
///   empty ring buffer, no args, invalid path
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
