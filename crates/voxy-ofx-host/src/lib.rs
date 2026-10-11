//! voXY's OpenFX host.
//!
//! - [`discovery`]: the plugin search paths and the `.ofx.bundle` layout.
//!
//! Still to come in O-A: the host suites, tested in-process in a harness
//! only. The editor never loads a plugin binary; from O-B that happens in
//! `voxy-ofx-sandbox`, one helper process per bundle (design §6.3).

pub mod discovery;
