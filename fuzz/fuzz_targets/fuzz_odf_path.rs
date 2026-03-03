#![no_main]

use libfuzzer_sys::fuzz_target;
use reconfigurable_device::odf::{ObjectTree, OmiValue};

fuzz_target!(|data: &[u8]| {
    if let Ok(path) = std::str::from_utf8(data) {
        let mut tree = ObjectTree::new();

        // Pre-populate so resolve / delete have something to walk.
        let _ = tree.write_value("/A/B", OmiValue::Number(1.0), None);

        // Exercise all public path-based APIs (each calls parse_path internally).
        let _ = tree.resolve(path);
        let _ = tree.write_value(path, OmiValue::Null, None);
        let _ = tree.delete(path);
    }
});
