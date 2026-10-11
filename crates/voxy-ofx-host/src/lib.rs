//! voXY's OpenFX host.
//!
//! - [`clip`]: clip descriptors and instances.
//! - [`discovery`]: the plugin search paths and the `.ofx.bundle` layout.
//! - [`props`]: property sets, in safe Rust.
//! - [`host`]: the host property set.
//! - [`image`]: images handed to plugins, and the host's image source.
//! - [`params`]: parameter sets and their value store, in safe Rust.
//! - [`effect`]: the host's record of an effect handle (messages,
//!   progress, time line, abort).
//! - [`suites`]: the C side (`OfxHost`, `fetchSuite`, the suites); the only
//!   unsafe code.
//!
//! Still to come in O-A: the host suites, tested in-process in a harness
//! only. The editor never loads a plugin binary; from O-B that happens in
//! `voxy-ofx-sandbox`, one helper process per bundle (design §6.3).

pub mod clip;
pub mod discovery;
pub mod effect;
pub mod host;
pub mod image;
pub mod params;
pub mod props;
pub mod suites;
