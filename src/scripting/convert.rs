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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odf::InfoItem;

    unsafe fn with_mjs<F: FnOnce(*mut ffi::mjs)>(f: F) {
        let mjs = ffi::mjs_create();
        assert!(!mjs.is_null());
        f(mjs);
        ffi::mjs_destroy(mjs);
    }

    // ── mjs_to_omi ──

    #[test]
    fn mjs_to_omi_number() {
        unsafe {
            with_mjs(|mjs| {
                let val = ffi::mjs_mk_number(mjs, 42.5);
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Number(42.5));
            });
        }
    }

    #[test]
    fn mjs_to_omi_negative_number() {
        unsafe {
            with_mjs(|mjs| {
                let val = ffi::mjs_mk_number(mjs, -3.14);
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Number(-3.14));
            });
        }
    }

    #[test]
    fn mjs_to_omi_zero() {
        unsafe {
            with_mjs(|mjs| {
                let val = ffi::mjs_mk_number(mjs, 0.0);
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Number(0.0));
            });
        }
    }

    #[test]
    fn mjs_to_omi_bool_true() {
        unsafe {
            with_mjs(|mjs| {
                let val = ffi::mjs_mk_boolean(mjs, 1);
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Bool(true));
            });
        }
    }

    #[test]
    fn mjs_to_omi_bool_false() {
        unsafe {
            with_mjs(|mjs| {
                let val = ffi::mjs_mk_boolean(mjs, 0);
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Bool(false));
            });
        }
    }

    #[test]
    fn mjs_to_omi_string() {
        unsafe {
            with_mjs(|mjs| {
                let s = "hello world";
                let val = ffi::mjs_mk_string(mjs, s.as_ptr() as *const _, s.len(), 1);
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Str("hello world".into()));
            });
        }
    }

    #[test]
    fn mjs_to_omi_empty_string() {
        unsafe {
            with_mjs(|mjs| {
                let s = "";
                let val = ffi::mjs_mk_string(mjs, s.as_ptr() as *const _, 0, 1);
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Str(String::new()));
            });
        }
    }

    #[test]
    fn mjs_to_omi_null() {
        unsafe {
            with_mjs(|mjs| {
                let val = ffi::mjs_mk_null();
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Null);
            });
        }
    }

    #[test]
    fn mjs_to_omi_undefined() {
        unsafe {
            with_mjs(|mjs| {
                let val = ffi::mjs_mk_undefined();
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Null);
            });
        }
    }

    #[test]
    fn mjs_to_omi_object_becomes_null() {
        unsafe {
            with_mjs(|mjs| {
                let val = ffi::mjs_mk_object(mjs);
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Null);
            });
        }
    }

    #[test]
    fn mjs_to_omi_foreign_becomes_null() {
        unsafe {
            with_mjs(|mjs| {
                let mut dummy: u32 = 0;
                let val = ffi::mjs_mk_foreign(mjs, &mut dummy as *mut u32 as *mut _);
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Null);
            });
        }
    }

    // ── omi_to_mjs ──

    #[test]
    fn omi_to_mjs_number() {
        unsafe {
            with_mjs(|mjs| {
                let val = omi_to_mjs(mjs, &OmiValue::Number(99.9));
                assert_ne!(ffi::mjs_is_number(val), 0);
                assert_eq!(ffi::mjs_get_double(mjs, val), 99.9);
            });
        }
    }

    #[test]
    fn omi_to_mjs_bool_true() {
        unsafe {
            with_mjs(|mjs| {
                let val = omi_to_mjs(mjs, &OmiValue::Bool(true));
                assert_ne!(ffi::mjs_is_boolean(val), 0);
                assert_ne!(ffi::mjs_get_bool(mjs, val), 0);
            });
        }
    }

    #[test]
    fn omi_to_mjs_bool_false() {
        unsafe {
            with_mjs(|mjs| {
                let val = omi_to_mjs(mjs, &OmiValue::Bool(false));
                assert_ne!(ffi::mjs_is_boolean(val), 0);
                assert_eq!(ffi::mjs_get_bool(mjs, val), 0);
            });
        }
    }

    #[test]
    fn omi_to_mjs_string() {
        unsafe {
            with_mjs(|mjs| {
                let val = omi_to_mjs(mjs, &OmiValue::Str("test".into()));
                assert_ne!(ffi::mjs_is_string(val), 0);
                let mut len: usize = 0;
                let mut v = val;
                let ptr = ffi::mjs_get_string(mjs, &mut v, &mut len);
                assert!(!ptr.is_null());
                let bytes = std::slice::from_raw_parts(ptr as *const u8, len);
                assert_eq!(bytes, b"test");
            });
        }
    }

    #[test]
    fn omi_to_mjs_null() {
        unsafe {
            with_mjs(|mjs| {
                let val = omi_to_mjs(mjs, &OmiValue::Null);
                assert_ne!(ffi::mjs_is_null(val), 0);
            });
        }
    }

    // ── round-trip ──

    #[test]
    fn round_trip_all_types() {
        unsafe {
            with_mjs(|mjs| {
                let cases = vec![
                    OmiValue::Number(3.14),
                    OmiValue::Number(0.0),
                    OmiValue::Number(-1.0),
                    OmiValue::Bool(true),
                    OmiValue::Bool(false),
                    OmiValue::Str("round trip".into()),
                    OmiValue::Null,
                ];
                for original in cases {
                    let mjs_val = omi_to_mjs(mjs, &original);
                    let back = mjs_to_omi(mjs, mjs_val);
                    assert_eq!(original, back, "round-trip failed for {:?}", original);
                }
            });
        }
    }

    // ── omi_to_mjs_element ──

    #[test]
    fn element_empty_item() {
        unsafe {
            with_mjs(|mjs| {
                let item = InfoItem::new(10);
                let obj = omi_to_mjs_element(mjs, &item);
                assert_ne!(ffi::mjs_is_object(obj), 0);

                let (n, l) = mjs_name!("type");
                let type_val = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_undefined(type_val), 0);

                let (n, l) = mjs_name!("desc");
                let desc_val = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_undefined(desc_val), 0);

                let (n, l) = mjs_name!("values");
                let arr = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_object(arr), 0);
            });
        }
    }

    #[test]
    fn element_with_type_desc_values() {
        unsafe {
            with_mjs(|mjs| {
                let mut item = InfoItem::new(10);
                item.type_uri = Some("omi:temperature".into());
                item.desc = Some("Room temp".into());
                item.add_value(OmiValue::Number(22.5), Some(1000.0));
                item.add_value(OmiValue::Number(23.0), Some(1001.0));

                let obj = omi_to_mjs_element(mjs, &item);
                assert_ne!(ffi::mjs_is_object(obj), 0);

                let (n, l) = mjs_name!("type");
                let type_val = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_string(type_val), 0);
                let omi_type = mjs_to_omi(mjs, type_val);
                assert_eq!(omi_type, OmiValue::Str("omi:temperature".into()));

                let (n, l) = mjs_name!("desc");
                let desc_val = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_string(desc_val), 0);
                let omi_desc = mjs_to_omi(mjs, desc_val);
                assert_eq!(omi_desc, OmiValue::Str("Room temp".into()));

                let (n, l) = mjs_name!("values");
                let arr = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_object(arr), 0);
            });
        }
    }

    #[test]
    fn element_value_without_timestamp() {
        unsafe {
            with_mjs(|mjs| {
                let mut item = InfoItem::new(10);
                item.add_value(OmiValue::Bool(true), None);
                let obj = omi_to_mjs_element(mjs, &item);
                let (n, l) = mjs_name!("values");
                let arr = ffi::mjs_get(mjs, obj, n, l);
                assert_ne!(ffi::mjs_is_object(arr), 0);
            });
        }
    }

    #[test]
    fn mjs_to_omi_string_with_utf8() {
        unsafe {
            with_mjs(|mjs| {
                let s = "héllo wörld";
                let val = ffi::mjs_mk_string(mjs, s.as_ptr() as *const _, s.len(), 1);
                assert_eq!(mjs_to_omi(mjs, val), OmiValue::Str("héllo wörld".into()));
            });
        }
    }
}
