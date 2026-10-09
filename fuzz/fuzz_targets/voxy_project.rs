//! The `.voxy` loader never panics, and anything it accepts saves and
//! reloads to the same document.
#![no_main]

use libfuzzer_sys::fuzz_target;
use voxy_core::persist;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    if let Ok(doc) = persist::from_json(text) {
        let json = persist::to_json(&doc).expect("valid documents encode");
        assert_eq!(persist::from_json(&json).expect("re-load"), doc);
    }
});
