//! Vector geometry and rasterization front-end for voXY.
//!
//! Shapes, masks and text arrive in Milestone 4, built on `kurbo` (geometry),
//! `parley` (text layout), `i_overlay` (boolean operations) and `vello`
//! (GPU rasterization). In Milestone 1 this crate only fixes the vello
//! dependency so the whole workspace resolves to a single `wgpu` version.

/// vello's re-export of `wgpu`. `voxy-render` statically asserts that this is
/// the very same crate as the workspace `wgpu` (see the README's
/// "Pinned GPU stack" section).
pub use vello::wgpu;
