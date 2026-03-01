//! `odf.writeItem()` C callback implementation for cascading writes.
//!
//! The callback collects writes into a `Vec` rather than calling back into the
//! OMI Engine. The writes are processed after script execution completes,
//! eliminating re-entrant `&mut Engine` aliasing.

use super::ffi;
use super::ffi::mjs_name;

/// Maximum script cascading depth.
pub const MAX_SCRIPT_DEPTH: u8 = 4;

/// A write collected during script execution, to be processed afterwards.
pub(crate) struct PendingWrite {
    pub path: String,
    pub value: crate::odf::OmiValue,
}

/// Context passed to the JS callback via a foreign pointer.
///
/// Valid only during script execution. Set before `exec()`, cleared after.
pub(crate) struct ScriptCallbackCtx {
    pub pending_writes: *mut Vec<PendingWrite>,
    pub depth: u8,
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

    // Collect the write for deferred processing
    let writes = &mut *ctx.pending_writes;
    writes.push(PendingWrite { path, value: omi_value });
    ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, 1));
}
