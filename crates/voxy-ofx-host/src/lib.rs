//! voXY's OpenFX host.
//!
//! - [`discovery`]: the plugin search paths and the `.ofx.bundle` layout.
//! - [`props`]: property sets, in safe Rust.
//! - [`suites`]: the C side (`OfxHost`, `fetchSuite`, the suites); the only
//!   unsafe code.
//!
//! Still to come in O-A: the host suites, tested in-process in a harness
//! only. The editor never loads a plugin binary; from O-B that happens in
//! `voxy-ofx-sandbox`, one helper process per bundle (design §6.3).

pub mod discovery;
pub mod props;
pub mod suites;
