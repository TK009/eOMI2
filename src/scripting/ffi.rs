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

    // --- Function calls ---
    pub fn mjs_apply(
        mjs: *mut mjs,
        res: *mut mjs_val_t,
        func: mjs_val_t,
        this_val: mjs_val_t,
        nargs: c_int,
        args: *mut mjs_val_t,
    ) -> mjs_err_t;

    // --- Type checks (additional) ---
    pub fn mjs_is_function(v: mjs_val_t) -> c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mjs_name_macro_length() {
        let (_, len) = mjs_name!("hello");
        assert_eq!(len, 5);
    }

    #[test]
    fn mjs_name_macro_empty() {
        let (_, len) = mjs_name!("");
        assert_eq!(len, 0);
    }

    #[test]
    fn mjs_name_macro_nul_terminated() {
        let (ptr, len) = mjs_name!("test");
        assert_eq!(len, 4);
        unsafe {
            let byte = *(ptr as *const u8).add(len);
            assert_eq!(byte, 0);
        }
    }

    #[test]
    fn mjs_name_macro_content() {
        let (ptr, len) = mjs_name!("abc");
        unsafe {
            let bytes = std::slice::from_raw_parts(ptr as *const u8, len);
            assert_eq!(bytes, b"abc");
        }
    }

    #[test]
    fn mjs_create_destroy() {
        unsafe {
            let mjs = mjs_create();
            assert!(!mjs.is_null());
            mjs_destroy(mjs);
        }
    }

    #[test]
    fn mjs_null_is_null() {
        unsafe {
            let val = mjs_mk_null();
            assert_ne!(mjs_is_null(val), 0);
            assert_eq!(mjs_is_undefined(val), 0);
            assert_eq!(mjs_is_number(val), 0);
        }
    }

    #[test]
    fn mjs_undefined_is_undefined() {
        unsafe {
            let val = mjs_mk_undefined();
            assert_ne!(mjs_is_undefined(val), 0);
            assert_eq!(mjs_is_null(val), 0);
        }
    }

    #[test]
    fn mjs_number_type_checks() {
        unsafe {
            let mjs = mjs_create();
            let val = mjs_mk_number(mjs, 42.0);
            assert_ne!(mjs_is_number(val), 0);
            assert_eq!(mjs_is_string(val), 0);
            assert_eq!(mjs_is_boolean(val), 0);
            assert_eq!(mjs_is_null(val), 0);
            assert_eq!(mjs_is_object(val), 0);
            assert_eq!(mjs_get_double(mjs, val), 42.0);
            mjs_destroy(mjs);
        }
    }

    #[test]
    fn mjs_boolean_type_checks() {
        unsafe {
            let mjs = mjs_create();
            let val = mjs_mk_boolean(mjs, 1);
            assert_ne!(mjs_is_boolean(val), 0);
            assert_eq!(mjs_is_number(val), 0);
            assert_eq!(mjs_is_string(val), 0);
            assert_ne!(mjs_get_bool(mjs, val), 0);

            let val_f = mjs_mk_boolean(mjs, 0);
            assert_eq!(mjs_get_bool(mjs, val_f), 0);
            mjs_destroy(mjs);
        }
    }

    #[test]
    fn mjs_string_type_checks() {
        unsafe {
            let mjs = mjs_create();
            let s = "test";
            let val = mjs_mk_string(mjs, s.as_ptr() as *const _, s.len(), 1);
            assert_ne!(mjs_is_string(val), 0);
            assert_eq!(mjs_is_number(val), 0);
            assert_eq!(mjs_is_boolean(val), 0);

            let mut len: usize = 0;
            let mut v = val;
            let ptr = mjs_get_string(mjs, &mut v, &mut len);
            assert!(!ptr.is_null());
            assert_eq!(len, 4);
            mjs_destroy(mjs);
        }
    }

    #[test]
    fn mjs_object_type_checks() {
        unsafe {
            let mjs = mjs_create();
            let obj = mjs_mk_object(mjs);
            assert_ne!(mjs_is_object(obj), 0);
            assert_eq!(mjs_is_number(obj), 0);
            assert_eq!(mjs_is_string(obj), 0);
            mjs_destroy(mjs);
        }
    }

    #[test]
    fn mjs_foreign_type_checks() {
        unsafe {
            let mjs = mjs_create();
            let mut data: u32 = 123;
            let val = mjs_mk_foreign(mjs, &mut data as *mut u32 as *mut c_void);
            assert_ne!(mjs_is_foreign(val), 0);
            assert_eq!(mjs_is_object(val), 0);

            let ptr = mjs_get_ptr(mjs, val);
            assert_eq!(ptr as *mut u32, &mut data as *mut u32);
            mjs_destroy(mjs);
        }
    }

    #[test]
    fn mjs_object_set_get_property() {
        unsafe {
            let mjs = mjs_create();
            let obj = mjs_mk_object(mjs);
            let val = mjs_mk_number(mjs, 7.0);

            let (name, len) = mjs_name!("key");
            mjs_set(mjs, obj, name, len, val);

            let got = mjs_get(mjs, obj, name, len);
            assert_ne!(mjs_is_number(got), 0);
            assert_eq!(mjs_get_double(mjs, got), 7.0);
            mjs_destroy(mjs);
        }
    }

    #[test]
    fn mjs_array_push_creates_array() {
        unsafe {
            let mjs = mjs_create();
            let arr = mjs_mk_array(mjs);
            let val = mjs_mk_number(mjs, 1.0);
            mjs_array_push(mjs, arr, val);
            assert_ne!(mjs_is_object(arr), 0);
            mjs_destroy(mjs);
        }
    }

    #[test]
    fn mjs_exec_simple_expression() {
        unsafe {
            let mjs = mjs_create();
            let src = "1 + 2\0";
            let mut res: mjs_val_t = 0;
            let err = mjs_exec(mjs, src.as_ptr() as *const _, &mut res);
            assert_eq!(err, MJS_OK);
            assert_ne!(mjs_is_number(res), 0);
            assert_eq!(mjs_get_double(mjs, res), 3.0);
            mjs_destroy(mjs);
        }
    }

    #[test]
    fn mjs_global_object() {
        unsafe {
            let mjs = mjs_create();
            let global = mjs_get_global(mjs);
            assert_ne!(mjs_is_object(global), 0);
            mjs_destroy(mjs);
        }
    }

    #[test]
    fn mjs_foreign_func_creation() {
        unsafe extern "C" fn dummy(_mjs: *mut mjs) {}

        unsafe {
            let mjs_inst = mjs_create();
            let func = mjs_mk_foreign_func(mjs_inst, Some(dummy));
            assert_ne!(mjs_is_foreign(func), 0);
            mjs_destroy(mjs_inst);
        }
    }

    #[test]
    fn mjs_op_limit() {
        unsafe {
            let mjs = mjs_create();
            mjs_set_max_ops(mjs, 100);
            mjs_reset_ops_count(mjs);
            let src = "while(true){}\0";
            let mut res: mjs_val_t = 0;
            let err = mjs_exec(mjs, src.as_ptr() as *const _, &mut res);
            assert_eq!(err, MJS_OP_LIMIT_ERROR);
            mjs_destroy(mjs);
        }
    }
}
