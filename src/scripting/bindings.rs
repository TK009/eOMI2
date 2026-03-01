//! `odf.writeItem()` C callback implementation for cascading writes.

use super::ffi;

/// Maximum script cascading depth.
pub const MAX_SCRIPT_DEPTH: u8 = 4;

/// Context passed to the JS callback via a foreign pointer.
///
/// Valid only during script execution. Set before `exec()`, cleared after.
pub(crate) struct ScriptCallbackCtx {
    pub engine: *mut crate::omi::Engine,
    pub depth: u8,
    pub timestamp: Option<f64>,
}

/// C callback for `odf.writeItem(value, path)`.
///
/// # Safety
/// Called from mJS during script execution. The `__ctx` foreign pointer on the
/// global object must point to a valid `ScriptCallbackCtx`.
pub unsafe extern "C" fn js_odf_write_item(mjs: *mut ffi::mjs) {
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
    let ctx_val = ffi::mjs_get(mjs, global, "__ctx\0".as_ptr() as *const _, 5);
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

    // Check depth limit
    let new_depth = ctx.depth + 1;
    if new_depth >= MAX_SCRIPT_DEPTH {
        log::warn!("Script depth limit reached at path '{}'", path);
        ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, 0));
        return;
    }

    // Perform the cascading write
    let engine = &mut *ctx.engine;
    let result = engine.write_single_inner(&path, omi_value, ctx.timestamp, new_depth);
    let ok = result.is_ok();
    ffi::mjs_return(mjs, ffi::mjs_mk_boolean(mjs, ok as i32));
}
