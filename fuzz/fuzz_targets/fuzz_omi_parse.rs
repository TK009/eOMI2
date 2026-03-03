#![no_main]

use libfuzzer_sys::fuzz_target;
use reconfigurable_device::omi::OmiMessage;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // We only care about panics / UB — the Ok/Err result is discarded.
        let _ = OmiMessage::parse(s);
    }
});
