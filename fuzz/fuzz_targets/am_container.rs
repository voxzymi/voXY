//! Opening and inspecting any file (zip or not) never panics, and an
//! unedited package re-exports byte-for-byte.
#![no_main]

use libfuzzer_sys::fuzz_target;
use voxy_am_compat::inspect::inspect;

fuzz_target!(|data: &[u8]| {
    if let Ok(report) = inspect("fuzz", data.to_vec()) {
        if let Some(ok) = report.package_round_trips {
            assert!(ok);
        }
    }
});
