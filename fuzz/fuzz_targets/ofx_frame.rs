//! Sandbox frames (design §6.7): the editor decodes frames written by a
//! process running third-party code, so no input may panic.
#![no_main]

use libfuzzer_sys::fuzz_target;
use voxy_ofx_host::protocol::{Reply, Request, read_frame};

fuzz_target!(|data: &[u8]| {
    let _ = read_frame::<Reply>(&mut &data[..]);
    let _ = read_frame::<Request>(&mut &data[..]);
});
