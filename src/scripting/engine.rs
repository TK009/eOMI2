//! Safe wrapper around the mJS scripting engine.

use crate::odf::OmiValue;
use super::ffi;
use super::convert;
use super::error::ScriptError;

/// Maximum script source length in bytes.
pub const MAX_SCRIPT_LEN: usize = 4096;

/// Maximum bytecode operations per script execution.
pub const MAX_SCRIPT_OPS: u64 = 50_000;

// Ensure MAX_SCRIPT_OPS fits in c_ulong (32-bit on Xtensa/ESP32).
const _: () = assert!(MAX_SCRIPT_OPS <= u32::MAX as u64);

/// Safe wrapper around an mJS engine instance.
///
/// The mJS engine is single-threaded. This struct takes `&mut self` for all
/// operations, and the `Engine` that owns it is behind a `Mutex`, so exclusive
/// access is guaranteed.
pub struct ScriptEngine {
    mjs: *mut ffi::mjs,
}

// Safety: mJS is single-threaded. Access is serialized through the Mutex
// that guards the OMI Engine containing this ScriptEngine.
unsafe impl Send for ScriptEngine {}

impl ScriptEngine {
    /// Create a new mJS engine instance.
    pub fn new() -> Result<Self, ScriptError> {
        let mjs = unsafe { ffi::mjs_create() };
        if mjs.is_null() {
            return Err(ScriptError::InitFailed);
        }
        unsafe { ffi::mjs_set_max_ops(mjs, MAX_SCRIPT_OPS as std::os::raw::c_ulong) };
        Ok(Self { mjs })
    }

    /// Execute a JavaScript source string and return the result as an `OmiValue`.
    pub fn exec(&mut self, src: &str) -> Result<OmiValue, ScriptError> {
        if src.len() > MAX_SCRIPT_LEN {
            return Err(ScriptError::ScriptTooLarge(src.len()));
        }
        let c_src = std::ffi::CString::new(src)
            .map_err(|_| ScriptError::Execution("script contains NUL byte".into()))?;
        let mut res: ffi::mjs_val_t = 0;
        unsafe { ffi::mjs_reset_ops_count(self.mjs) };
        let err = unsafe {
            ffi::mjs_exec(self.mjs, c_src.as_ptr(), &mut res)
        };
        if err == ffi::MJS_OP_LIMIT_ERROR {
            return Err(ScriptError::OpLimitExceeded);
        }
        if err != ffi::MJS_OK {
            let msg = unsafe {
                let ptr = ffi::mjs_strerror(self.mjs, err);
                if ptr.is_null() {
                    "unknown error".to_string()
                } else {
                    std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
                }
            };
            return Err(ScriptError::Execution(msg));
        }
        Ok(unsafe { convert::mjs_to_omi(self.mjs, res) })
    }

    /// Get the raw mJS pointer. For use by bindings module.
    pub(crate) fn raw(&mut self) -> *mut ffi::mjs {
        self.mjs
    }

    /// Create an mJS number value.
    pub fn mk_number(&mut self, n: f64) -> ffi::mjs_val_t {
        unsafe { ffi::mjs_mk_number(self.mjs, n) }
    }

    /// Create an mJS string value (copied).
    pub fn mk_string(&mut self, s: &str) -> ffi::mjs_val_t {
        unsafe { ffi::mjs_mk_string(self.mjs, s.as_ptr() as *const _, s.len(), 1) }
    }

    /// Create an mJS boolean value.
    pub fn mk_boolean(&mut self, b: bool) -> ffi::mjs_val_t {
        unsafe { ffi::mjs_mk_boolean(self.mjs, b as i32) }
    }

    /// Create an empty mJS object.
    pub fn mk_object(&mut self) -> ffi::mjs_val_t {
        unsafe { ffi::mjs_mk_object(self.mjs) }
    }

    /// Create an mJS foreign pointer value.
    pub fn mk_foreign(&mut self, ptr: *mut std::os::raw::c_void) -> ffi::mjs_val_t {
        unsafe { ffi::mjs_mk_foreign(self.mjs, ptr) }
    }

    /// Create an mJS foreign function value.
    pub fn mk_foreign_func(&mut self, f: ffi::mjs_ffi_cb_t) -> ffi::mjs_val_t {
        unsafe { ffi::mjs_mk_foreign_func(self.mjs, f) }
    }

    /// Set a property on an mJS object.
    pub fn set_property(
        &mut self,
        obj: ffi::mjs_val_t,
        name: &str,
        val: ffi::mjs_val_t,
    ) {
        unsafe {
            ffi::mjs_set(self.mjs, obj, name.as_ptr() as *const _, name.len(), val);
        }
    }

    /// Get a property from an mJS object.
    pub fn get_property(&mut self, obj: ffi::mjs_val_t, name: &str) -> ffi::mjs_val_t {
        unsafe { ffi::mjs_get(self.mjs, obj, name.as_ptr() as *const _, name.len()) }
    }

    /// Get the mJS global object.
    pub fn global(&mut self) -> ffi::mjs_val_t {
        unsafe { ffi::mjs_get_global(self.mjs) }
    }

    /// Convert an mJS value to an OmiValue.
    pub fn to_omi_value(&mut self, val: ffi::mjs_val_t) -> OmiValue {
        unsafe { convert::mjs_to_omi(self.mjs, val) }
    }

    /// Convert an OmiValue to an mJS value.
    pub fn from_omi_value(&mut self, val: &OmiValue) -> ffi::mjs_val_t {
        unsafe { convert::omi_to_mjs(self.mjs, val) }
    }

    /// Run garbage collection.
    pub fn gc(&mut self) {
        unsafe { ffi::mjs_gc(self.mjs, 1) }
    }
}

impl Drop for ScriptEngine {
    fn drop(&mut self) {
        if !self.mjs.is_null() {
            unsafe { ffi::mjs_destroy(self.mjs) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_destroy_lifecycle() {
        let engine = ScriptEngine::new().unwrap();
        drop(engine);
    }

    #[test]
    fn exec_number() {
        let mut engine = ScriptEngine::new().unwrap();
        let result = engine.exec("1 + 2").unwrap();
        assert_eq!(result, OmiValue::Number(3.0));
    }

    #[test]
    fn exec_string() {
        let mut engine = ScriptEngine::new().unwrap();
        let result = engine.exec("'hello'").unwrap();
        assert_eq!(result, OmiValue::Str("hello".into()));
    }

    #[test]
    fn exec_boolean() {
        let mut engine = ScriptEngine::new().unwrap();
        let result = engine.exec("true").unwrap();
        assert_eq!(result, OmiValue::Bool(true));
    }

    #[test]
    fn exec_null() {
        let mut engine = ScriptEngine::new().unwrap();
        let result = engine.exec("null").unwrap();
        assert_eq!(result, OmiValue::Null);
    }

    #[test]
    fn exec_syntax_error() {
        let mut engine = ScriptEngine::new().unwrap();
        let result = engine.exec("let x = ;");
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::Execution(msg) => assert!(!msg.is_empty()),
            other => panic!("expected Execution error, got: {:?}", other),
        }
    }

    #[test]
    fn global_state_persists() {
        let mut engine = ScriptEngine::new().unwrap();
        engine.exec("let counter = 10;").unwrap();
        let result = engine.exec("counter = counter + 5; counter").unwrap();
        assert_eq!(result, OmiValue::Number(15.0));
    }

    #[test]
    fn event_object_setup_and_readback() {
        let mut engine = ScriptEngine::new().unwrap();
        let event = engine.mk_object();
        let val = engine.mk_number(42.0);
        engine.set_property(event, "value", val);
        let path = engine.mk_string("/Dev/Temp");
        engine.set_property(event, "path", path);

        let global = engine.global();
        engine.set_property(global, "event", event);

        let result = engine.exec("event.value").unwrap();
        assert_eq!(result, OmiValue::Number(42.0));

        let result = engine.exec("event.path").unwrap();
        assert_eq!(result, OmiValue::Str("/Dev/Temp".into()));
    }

    #[test]
    fn value_conversion_round_trips() {
        let mut engine = ScriptEngine::new().unwrap();

        let cases = vec![
            OmiValue::Number(3.14),
            OmiValue::Bool(true),
            OmiValue::Bool(false),
            OmiValue::Str("test string".into()),
            OmiValue::Null,
        ];

        for original in cases {
            let mjs_val = engine.from_omi_value(&original);
            let back = engine.to_omi_value(mjs_val);
            assert_eq!(original, back, "round-trip failed for {:?}", original);
        }
    }

    #[test]
    fn script_too_large() {
        let mut engine = ScriptEngine::new().unwrap();
        let big_script = "x".repeat(MAX_SCRIPT_LEN + 1);
        match engine.exec(&big_script) {
            Err(ScriptError::ScriptTooLarge(len)) => assert_eq!(len, MAX_SCRIPT_LEN + 1),
            other => panic!("expected ScriptTooLarge, got: {:?}", other),
        }
    }

    #[test]
    fn infinite_loop_hits_op_limit() {
        let mut engine = ScriptEngine::new().unwrap();
        match engine.exec("while(true){}") {
            Err(ScriptError::OpLimitExceeded) => {}
            other => panic!("expected OpLimitExceeded, got: {:?}", other),
        }
    }

    #[test]
    fn finite_loop_completes_within_limit() {
        let mut engine = ScriptEngine::new().unwrap();
        let result = engine.exec("let s = 0; for (let i = 0; i < 100; i++) { s = s + i; } s");
        assert!(result.is_ok(), "small loop should succeed: {:?}", result);
    }

    #[test]
    fn op_limit_resets_between_calls() {
        let mut engine = ScriptEngine::new().unwrap();
        // First call: use some budget
        engine.exec("let x = 0; for (let i = 0; i < 100; i++) { x = x + 1; }").unwrap();
        // Second call: should get a fresh budget, not continue from the previous
        engine.exec("let y = 0; for (let i = 0; i < 100; i++) { y = y + 1; }").unwrap();
    }
}
