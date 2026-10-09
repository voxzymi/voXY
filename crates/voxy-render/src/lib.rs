//! voXY GPU compositor.
//!
//! Pipeline (design proposal §4.1):
//!
//! ```text
//! Document ──evaluate()──▶ EvaluatedComp ──RenderPlan::build()──▶ RenderPlan
//!          (voxy-core)                    (CPU, f64)
//!   ──Compositor::render()──▶ RGBA16F working buffer ──output transform──▶ RGBA8
//! ```
//!
//! Preview and export call the same functions; only [`RenderSettings`]
//! differ.

mod compositor;
mod gpu;
mod plan;

pub use compositor::{
    Compositor, OUTPUT_FORMAT, OutputAlpha, RenderError, RenderReport, RgbaImage, WORKING_FORMAT,
};
pub use gpu::{GpuContext, GpuError};
pub use plan::{DrawOp, DrawSource, RenderPlan, RenderSettings, working_premultiplied};

pub use wgpu;

use voxy_core::{CompId, Document, Time};

/// Compile-time proof that vello (via `voxy-vector`) and voxy-render use the
/// same `wgpu` crate. If the pinned versions ever drift apart, this stops
/// compiling instead of silently linking two wgpu copies.
#[allow(dead_code)]
fn same_wgpu_as_vello(device: &wgpu::Device) -> &voxy_vector::wgpu::Device {
    device
}

/// Which frame to render, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRequest {
    pub comp: CompId,
    pub time: Time,
    pub settings: RenderSettings,
    pub alpha: OutputAlpha,
}

impl FrameRequest {
    /// Full resolution, straight alpha: what export uses.
    pub fn export(comp: CompId, time: Time) -> Self {
        FrameRequest {
            comp,
            time,
            settings: RenderSettings::default(),
            alpha: OutputAlpha::Straight,
        }
    }
}

/// Evaluates, plans and renders one frame to CPU memory. Returns `None` if
/// the composition does not exist.
pub fn render_frame(
    compositor: &mut Compositor,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    doc: &Document,
    request: &FrameRequest,
) -> Option<Result<(RgbaImage, RenderReport), RenderError>> {
    let eval = voxy_core::evaluate(doc, request.comp, request.time)?;
    let plan = RenderPlan::build(&eval, &request.settings);
    Some(compositor.render_to_image(device, queue, &doc.assets, &plan, request.alpha))
}
