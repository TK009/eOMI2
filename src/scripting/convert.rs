//! Bidirectional conversion between `OmiValue` and mJS values.

use crate::odf::OmiValue;
use super::ffi;

/// Convert an mJS value to an `OmiValue`.
///
/// # Safety
/// `mjs` must be a valid mJS instance pointer.
pub unsafe fn mjs_to_omi(mjs: *mut ffi::mjs, val: ffi::mjs_val_t) -> OmiValue {
    if ffi::mjs_is_number(val) != 0 {
        OmiValue::Number(ffi::mjs_get_double(mjs, val))
    } else if ffi::mjs_is_boolean(val) != 0 {
        OmiValue::Bool(ffi::mjs_get_bool(mjs, val) != 0)
    } else if ffi::mjs_is_string(val) != 0 {
        let mut len: usize = 0;
        let mut val_copy = val;
        let ptr = ffi::mjs_get_string(mjs, &mut val_copy, &mut len);
        if ptr.is_null() || len == 0 {
            OmiValue::Str(String::new())
        } else {
            let bytes = std::slice::from_raw_parts(ptr as *const u8, len);
            OmiValue::Str(String::from_utf8_lossy(bytes).into_owned())
        }
    } else if ffi::mjs_is_null(val) != 0 || ffi::mjs_is_undefined(val) != 0 {
        OmiValue::Null
    } else {
        // Objects, foreign pointers, functions → Null
        OmiValue::Null
    }
}

/// Convert an `OmiValue` to an mJS value.
///
/// # Safety
/// `mjs` must be a valid mJS instance pointer.
pub unsafe fn omi_to_mjs(mjs: *mut ffi::mjs, val: &OmiValue) -> ffi::mjs_val_t {
    match val {
        OmiValue::Number(n) => ffi::mjs_mk_number(mjs, *n),
        OmiValue::Bool(b) => ffi::mjs_mk_boolean(mjs, *b as i32),
        OmiValue::Str(s) => {
            ffi::mjs_mk_string(mjs, s.as_ptr() as *const _, s.len(), 1)
        }
        OmiValue::Null => ffi::mjs_mk_null(),
    }
}
