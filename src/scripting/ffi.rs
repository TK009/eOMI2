//! Minimal hand-written FFI bindings to mJS (Cesanta).
//!
//! Only the functions actually used by the scripting module are declared here.

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_void};

/// Compile-time name+length pair for mJS property access.
/// Returns `(*const c_char, usize)` — pointer to a NUL-terminated static
/// string and the byte length excluding the NUL.
macro_rules! mjs_name {
    ($name:literal) => {
        (concat!($name, "\0").as_ptr() as *const std::os::raw::c_char, $name.len())
    };
}
pub(crate) use mjs_name;

/// mJS NaN-boxed value type.
pub type mjs_val_t = u64;

/// mJS error code.
pub type mjs_err_t = c_int;

/// mJS error enum values.
pub const MJS_OK: mjs_err_t = 0;
pub const MJS_OP_LIMIT_ERROR: mjs_err_t = 9;

/// Function pointer type for foreign functions callable from mJS.
///
/// mJS stores these as `void (*)(void)` but always calls them as
/// `void (*)(struct mjs *)`. We declare the true calling convention.
pub type mjs_ffi_cb_t = Option<unsafe extern "C" fn(*mut mjs)>;

/// Opaque mJS engine handle.
#[repr(C)]
pub struct mjs {
    _opaque: [u8; 0],
}

extern "C" {
    // --- Lifecycle ---
    pub fn mjs_create() -> *mut mjs;
    pub fn mjs_destroy(mjs: *mut mjs);

    // --- Execution ---
    pub fn mjs_exec(
        mjs: *mut mjs,
        src: *const c_char,
        res: *mut mjs_val_t,
    ) -> mjs_err_t;

    // --- Value constructors ---
    pub fn mjs_mk_number(mjs: *mut mjs, num: f64) -> mjs_val_t;
    pub fn mjs_mk_boolean(mjs: *mut mjs, v: c_int) -> mjs_val_t;
    pub fn mjs_mk_string(
        mjs: *mut mjs,
        str: *const c_char,
        len: usize,
        copy: c_int,
    ) -> mjs_val_t;
    pub fn mjs_mk_null() -> mjs_val_t;
    pub fn mjs_mk_undefined() -> mjs_val_t;
    pub fn mjs_mk_object(mjs: *mut mjs) -> mjs_val_t;
    pub fn mjs_mk_foreign(mjs: *mut mjs, ptr: *mut c_void) -> mjs_val_t;
    pub fn mjs_mk_foreign_func(mjs: *mut mjs, f: mjs_ffi_cb_t) -> mjs_val_t;

    // --- Object access ---
    pub fn mjs_set(
        mjs: *mut mjs,
        obj: mjs_val_t,
        name: *const c_char,
        len: usize,
        val: mjs_val_t,
    ) -> mjs_err_t;
    pub fn mjs_get(
        mjs: *mut mjs,
        obj: mjs_val_t,
        name: *const c_char,
        len: usize,
    ) -> mjs_val_t;
    pub fn mjs_get_global(mjs: *mut mjs) -> mjs_val_t;

    // --- Type checks ---
    pub fn mjs_is_number(v: mjs_val_t) -> c_int;
    pub fn mjs_is_boolean(v: mjs_val_t) -> c_int;
    pub fn mjs_is_string(v: mjs_val_t) -> c_int;
    pub fn mjs_is_null(v: mjs_val_t) -> c_int;
    pub fn mjs_is_undefined(v: mjs_val_t) -> c_int;
    pub fn mjs_is_foreign(v: mjs_val_t) -> c_int;
    pub fn mjs_is_object(v: mjs_val_t) -> c_int;

    // --- Value extraction ---
    pub fn mjs_get_double(mjs: *mut mjs, v: mjs_val_t) -> f64;
    pub fn mjs_get_bool(mjs: *mut mjs, v: mjs_val_t) -> c_int;
    pub fn mjs_get_string(
        mjs: *mut mjs,
        v: *mut mjs_val_t,
        len: *mut usize,
    ) -> *const c_char;
    pub fn mjs_get_ptr(mjs: *mut mjs, v: mjs_val_t) -> *mut c_void;

    // --- Callbacks ---
    pub fn mjs_nargs(mjs: *mut mjs) -> c_int;
    pub fn mjs_arg(mjs: *mut mjs, n: c_int) -> mjs_val_t;
    pub fn mjs_return(mjs: *mut mjs, v: mjs_val_t);

    // --- Arrays ---
    pub fn mjs_mk_array(mjs: *mut mjs) -> mjs_val_t;
    pub fn mjs_array_push(mjs: *mut mjs, arr: mjs_val_t, val: mjs_val_t) -> mjs_err_t;

    // --- GC ---
    pub fn mjs_gc(mjs: *mut mjs, full: c_int);

    // --- Errors ---
    pub fn mjs_strerror(mjs: *mut mjs, err: mjs_err_t) -> *const c_char;

    // --- Operation limit ---
    pub fn mjs_set_max_ops(mjs: *mut mjs, max_ops: std::os::raw::c_ulong);
    pub fn mjs_reset_ops_count(mjs: *mut mjs);
}
