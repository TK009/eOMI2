#![no_main]

use libfuzzer_sys::fuzz_target;
use reconfigurable_device::scripting::ScriptEngine;

fuzz_target!(|data: &[u8]| {
    if let Ok(src) = std::str::from_utf8(data) {
        // Fresh engine per input to avoid state accumulation masking bugs.
        if let Ok(mut engine) = ScriptEngine::new() {
            let _ = engine.exec(src);
        }
    }
});
