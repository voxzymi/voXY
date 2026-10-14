//! voXY's OpenFX host.
//!
//! - [`channel`]: the local-socket control channel to a sandbox.
//! - [`client`]: the editor side of a sandbox: launch, timeouts, crashes,
//!   blocklist.
//! - [`clip`]: clip descriptors and instances.
//! - [`discovery`]: the plugin search paths and the `.ofx.bundle` layout.
//! - [`props`]: property sets, in safe Rust.
//! - [`protocol`]: the sandbox wire protocol.
//! - [`runtime`]: the editor's OpenFX runtime: scanning bundles, running
//!   effects for the renderer.
//! - [`shm`]: the shared memory images travel in.
//! - [`host`]: the host property set.
//! - [`image`]: images handed to plugins, and the host's image source.
//! - [`mapping`]: plugin descriptions as voXY effect definitions.
//! - [`params`]: parameter sets and their value store, in safe Rust.
//! - [`effect`]: the host's record of an effect handle (messages,
//!   progress, time line, abort).
//! - [`suites`]: the C side (`OfxHost`, `fetchSuite`, the suites, the
//!   action driver). With [`shm`]'s mapping code, the only unsafe code.
//!
//! The editor never loads a plugin binary: that happens in
//! `voxy-ofx-sandbox`, one helper process per bundle (design §6.3).

pub mod channel;
pub mod client;
pub mod clip;
pub mod discovery;
pub mod effect;
pub mod host;
pub mod image;
pub mod mapping;
pub mod params;
pub mod props;
pub mod protocol;
pub mod runtime;
pub mod shm;
pub mod suites;
