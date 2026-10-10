//! Built-in effect kernels: the CPU-side rules of each WGSL effect.
//!
//! A kernel is the renderer's half of a built-in effect: the definition
//! (parameters, ranges) lives in `voxy_core::registry`, the formula in a
//! WGSL shader, and the region rules here. The region rules are pure `f64`
//! functions, so planning stays deterministic and testable without a GPU
//! (`docs/design-effects-ofx.md` §3.2).
//!
//! Regions are in layer-local pixels. `scale` is the preview resolution
//! scale; pixel parameters are multiplied by it, so the look does not change
//! at reduced resolution (§3.6).

use voxy_core::glam::DVec2;
use voxy_core::registry::ids;
use voxy_core::{EffectTypeId, EvaluatedEffect, Value};

/// A kernel's parameters as the shader reads them (`EffectUniform` in
/// `shaders/effects/common.wgsl`, after its `frame` header).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct KernelParams {
    pub p: [[f32; 4]; 4],
    pub i: [u32; 4],
}

fn scalar(fx: &EvaluatedEffect, name: &str, default: f64) -> f64 {
    fx.scalar(name).filter(|v| v.is_finite()).unwrap_or(default)
}

fn choice(fx: &EvaluatedEffect, name: &str) -> u32 {
    match fx.param(name) {
        Some(Value::Enum(e)) => *e,
        _ => 0,
    }
}

/// An axis-aligned rectangle, `min` inclusive, `max` exclusive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: DVec2,
    pub max: DVec2,
}

impl Rect {
    pub fn new(min: DVec2, max: DVec2) -> Self {
        Rect { min, max }
    }

    /// `(0, 0)` to `size`.
    pub fn of_size(size: DVec2) -> Self {
        Rect::new(DVec2::ZERO, size)
    }

    pub fn size(&self) -> DVec2 {
        (self.max - self.min).max(DVec2::ZERO)
    }

    pub fn is_empty(&self) -> bool {
        self.max.x <= self.min.x || self.max.y <= self.min.y
    }

    /// Grown by `d` on every side (`d` may differ per axis).
    #[must_use]
    pub fn pad(&self, d: DVec2) -> Self {
        Rect::new(self.min - d, self.max + d)
    }

    #[must_use]
    pub fn intersect(&self, o: &Rect) -> Self {
        Rect::new(self.min.max(o.min), self.max.min(o.max))
    }

    #[must_use]
    pub fn union(&self, o: &Rect) -> Self {
        Rect::new(self.min.min(o.min), self.max.max(o.max))
    }

    /// The smallest rectangle on whole pixels containing this one.
    #[must_use]
    pub fn round_out(&self) -> Self {
        Rect::new(self.min.floor(), self.max.ceil())
    }
}

/// The built-in effects the renderer can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kernel {
    Invert,
}

impl Kernel {
    /// Every kernel, for tests.
    pub const ALL: [Kernel; 1] = [Kernel::Invert];

    /// The kernel running `type_id` at `version`, if this build has one.
    pub fn find(type_id: &EffectTypeId, version: u32) -> Option<Kernel> {
        match (type_id.as_str(), version) {
            (ids::INVERT, 1) => Some(Kernel::Invert),
            _ => None,
        }
    }

    /// Whether the kernel's output depends on the frame number (seeded
    /// randomness, design §3.4). Only then is the frame part of the cache
    /// key, so static effects render once.
    pub fn uses_frame(self) -> bool {
        match self {
            Kernel::Invert => false,
        }
    }

    pub fn type_id(self) -> &'static str {
        match self {
            Kernel::Invert => ids::INVERT,
        }
    }

    /// The shader parameters for `fx`. `texel_scale` is texels per layer
    /// pixel (the preview scale, possibly reduced to fit the GPU), by which
    /// pixel parameters are multiplied.
    pub fn params(self, fx: &EvaluatedEffect, texel_scale: f64) -> KernelParams {
        let _ = texel_scale;
        let mut k = KernelParams::default();
        match self {
            Kernel::Invert => {
                k.p[0][0] = scalar(fx, "mix", 0.0).clamp(0.0, 1.0) as f32;
                k.i[0] = choice(fx, "channel").min(4);
            }
        }
        k
    }

    /// Region of definition: how far the output can reach, given the input
    /// region (layer pixels).
    pub fn rod(self, fx: &EvaluatedEffect, input: Rect, scale: f64) -> Rect {
        let _ = (fx, scale);
        match self {
            Kernel::Invert => input,
        }
    }

    /// Region of interest: the input pixels needed to compute `output`.
    pub fn roi(self, fx: &EvaluatedEffect, output: Rect, scale: f64) -> Rect {
        let _ = (fx, scale);
        match self {
            Kernel::Invert => output,
        }
    }
}

/// Region of definition of a whole chain: `source` grown by every effect in
/// order.
pub fn chain_rod(effects: &[(Kernel, &EvaluatedEffect)], source: Rect, scale: f64) -> Rect {
    effects
        .iter()
        .fold(source, |r, (k, fx)| k.rod(fx, r, scale))
}

/// The input region the first effect needs so that `wanted` of the chain's
/// output is correct: `wanted` pulled back through every effect, last first.
pub fn chain_roi(effects: &[(Kernel, &EvaluatedEffect)], wanted: Rect, scale: f64) -> Rect {
    effects
        .iter()
        .rev()
        .fold(wanted, |r, (k, fx)| k.roi(fx, r, scale))
}
