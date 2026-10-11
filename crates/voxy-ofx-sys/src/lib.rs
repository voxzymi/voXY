//! Generated Rust FFI bindings for the OpenFX 1.5.1 C API.
//!
//! `bindings.rs` is written by `cargo xtask bindgen` from the vendored,
//! pinned BSD-3 OpenFX headers in `third_party/openfx/` and checked in. CI
//! regenerates it on Linux and fails on any diff. The headers only use
//! fixed-width C types, `size_t` and pointers, so one file fits all four
//! 64-bit targets; the layout assertions in it are checked at compile time
//! on every CI platform.
//!
//! Left out: the GPU and OpenGL render suites (owner decision D5), the
//! deprecated `ofxOld.h`, and the plugin's three exported entry points,
//! which the host resolves from the loaded binary instead.
//!
//! The draw-suite enums are plain integer constants. Their C type is
//! `unsigned int` on Linux and macOS and `int` on Windows; both are 32 bits,
//! so the checked-in aliases have the same ABI everywhere.

#[allow(
    unsafe_code,
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    missing_debug_implementations,
    clippy::all,
    clippy::pedantic
)]
mod bindings;

pub use bindings::*;
