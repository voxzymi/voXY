//! Effects that run outside the GPU kernels: OpenFX plugins (design
//! §6.9). The renderer reads the layer's image back to the CPU, hands it
//! to an [`ExternalEffects`] provider, and uploads the result.
//!
//! The trait is plain data in and out, so `voxy-core` stays free of GPU
//! and plugin code; the OpenFX host implements it, the renderer calls it.

use std::fmt::Debug;

use glam::DVec2;

use crate::color::WorkingSpace;
use crate::effect::EffectTypeId;
use crate::evaluate::EvaluatedEffect;

/// One layer-local image: premultiplied RGBA floats, rows top-down, in
/// the project's working space.
#[derive(Debug, Clone, PartialEq)]
pub struct CpuFrame {
    pub width: u32,
    pub height: u32,
    /// Where the buffer's top-left pixel lies in layer pixels.
    pub origin: DVec2,
    /// Buffer pixels per layer pixel (preview resolution).
    pub texel_scale: f64,
    /// The layer's own content size in layer pixels, from its origin (the
    /// OpenFX host's project size).
    pub layer_size: DVec2,
    pub space: WorkingSpace,
    /// `width × height × 4` values.
    pub pixels: Vec<f32>,
}

/// A rectangle in layer pixels (y down), `min` inclusive, `max` exclusive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub min: DVec2,
    pub max: DVec2,
}

/// Runs external effects. Implementations must be safe to call from the
/// render thread and must never block it forever (the OpenFX host has
/// timeouts).
pub trait ExternalEffects: Send + Sync + Debug {
    /// Whether this provider runs `type_id`.
    fn supports(&self, type_id: &EffectTypeId) -> bool;

    /// Renders `fx` (its parameters already evaluated at the frame) in
    /// place on `frame`. On an error the renderer passes the image through
    /// and reports the message.
    fn render(&self, fx: &EvaluatedEffect, frame: &mut CpuFrame) -> Result<(), String>;

    /// How far `fx`'s output reaches, given its input's region (layer
    /// pixels) and the layer's size. The renderer keeps the input region
    /// too, as one buffer holds every stage. The default is the input.
    fn region_of_definition(
        &self,
        fx: &EvaluatedEffect,
        input: Region,
        layer_size: DVec2,
    ) -> Region {
        let _ = (fx, layer_size);
        input
    }
}
