//! Bidirectional conversion between `OmiValue` and mJS values.

use crate::odf::{OmiValue, InfoItem};
use super::ffi;
use super::ffi::mjs_name;

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

/// Convert an `InfoItem` to an mJS element object: `{ type, desc, values: [{v, t}, ...] }`.
///
/// Properties `type` and `desc` are only set when present on the item.
/// The `values` array is always present (empty array if the ring buffer is empty).
/// Values are ordered newest-first, matching the spec serialization order.
///
/// # Safety
/// `mjs` must be a valid mJS instance pointer.
pub unsafe fn omi_to_mjs_element(mjs: *mut ffi::mjs, item: &InfoItem) -> ffi::mjs_val_t {
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

    // values array (newest first via query_values)
    let arr = ffi::mjs_mk_array(mjs);
    let values = item.query_values(None, None, None, None);
    for entry in &values {
        let entry_obj = ffi::mjs_mk_object(mjs);

        let (v_name, v_len) = mjs_name!("v");
        let v_val = omi_to_mjs(mjs, &entry.v);
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
