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

/// The most taps one side of a convolution pass may have (the shader's
/// weight array holds `MAX_TAPS + 1` weights: the centre and one side).
pub const MAX_TAPS: usize = 1023;

/// One pass of a kernel, as the shader reads it (`EffectUniform` in
/// `shaders/effects/common.wgsl`, after its `frame` header).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KernelParams {
    pub p: [[f32; 4]; 4],
    pub i: [u32; 4],
    /// Convolution weights, centre first; at most `MAX_TAPS + 1`.
    pub w: Vec<f32>,
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

fn flag(fx: &EvaluatedEffect, name: &str) -> bool {
    matches!(fx.param(name), Some(Value::Bool(true)))
}

/// Which axes a blur's "Blur Dimensions" selects: `(x, y)`.
fn axes(fx: &EvaluatedEffect) -> (bool, bool) {
    match choice(fx, "dimensions") {
        1 => (true, false),
        2 => (false, true),
        _ => (true, true),
    }
}

/// Normalized gaussian weights, centre first, for a blur of `radius`
/// texels (about three standard deviations). Computed in `f64` with the
/// pure-Rust `libm` exponential, so they are the same on every platform.
pub fn gaussian_weights(radius: f64) -> Vec<f32> {
    let sigma = radius / 3.0;
    if !(sigma.is_finite() && sigma > 0.25) {
        return vec![1.0];
    }
    let taps = ((3.0 * sigma).ceil() as usize).clamp(1, MAX_TAPS);
    let raw: Vec<f64> = (0..=taps)
        .map(|k| libm::exp(-((k * k) as f64) / (2.0 * sigma * sigma)))
        .collect();
    let total = raw[0] + 2.0 * raw[1..].iter().sum::<f64>();
    raw.iter().map(|w| (w / total) as f32).collect()
}

/// Flat weights averaging `2·taps + 1` texels.
pub fn box_weights(taps: usize) -> Vec<f32> {
    let taps = taps.min(MAX_TAPS);
    vec![(1.0 / (2 * taps + 1) as f64) as f32; taps + 1]
}

/// One separable convolution pass along `axis` (0 = x, 1 = y).
fn convolve(axis: u32, repeat_edges: bool, w: Vec<f32>) -> KernelParams {
    KernelParams {
        i: [axis, (w.len() - 1) as u32, u32::from(repeat_edges), 0],
        w,
        ..KernelParams::default()
    }
}

/// The built-in effects the renderer can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kernel {
    Invert,
    GaussianBlur,
    BoxBlur,
    DirectionalBlur,
}

impl Kernel {
    /// Every kernel.
    pub const ALL: [Kernel; 4] = [
        Kernel::Invert,
        Kernel::GaussianBlur,
        Kernel::BoxBlur,
        Kernel::DirectionalBlur,
    ];

    /// The kernel running `type_id` at `version`, if this build has one.
    pub fn find(type_id: &EffectTypeId, version: u32) -> Option<Kernel> {
        Kernel::ALL
            .into_iter()
            .find(|k| k.type_id() == type_id.as_str() && version == 1)
    }

    /// Whether the kernel's output depends on the frame number (seeded
    /// randomness, design §3.4). Only then is the frame part of the cache
    /// key, so static effects render once.
    pub fn uses_frame(self) -> bool {
        false
    }

    pub fn type_id(self) -> &'static str {
        match self {
            Kernel::Invert => ids::INVERT,
            Kernel::GaussianBlur => ids::GAUSSIAN_BLUR,
            Kernel::BoxBlur => ids::BOX_BLUR,
            Kernel::DirectionalBlur => ids::DIRECTIONAL_BLUR,
        }
    }

    /// The shader passes for `fx`, in order. `texel_scale` is texels per
    /// layer pixel (the preview scale, possibly reduced to fit the GPU), by
    /// which pixel parameters are multiplied. An empty list means the effect
    /// does nothing with these parameters.
    pub fn passes(self, fx: &EvaluatedEffect, texel_scale: f64) -> Vec<KernelParams> {
        let s = texel_scale;
        match self {
            Kernel::Invert => {
                let mut k = KernelParams::default();
                k.p[0][0] = scalar(fx, "mix", 0.0).clamp(0.0, 1.0) as f32;
                k.i[0] = choice(fx, "channel").min(4);
                vec![k]
            }
            Kernel::GaussianBlur => {
                let w = gaussian_weights(scalar(fx, "radius", 0.0).max(0.0) * s);
                if w.len() == 1 {
                    return Vec::new();
                }
                let repeat = flag(fx, "repeat_edges");
                let (x, y) = axes(fx);
                let mut out = Vec::new();
                if x {
                    out.push(convolve(0, repeat, w.clone()));
                }
                if y {
                    out.push(convolve(1, repeat, w));
                }
                out
            }
            Kernel::BoxBlur => {
                let taps = (scalar(fx, "radius", 0.0).max(0.0) * s).round() as usize;
                if taps == 0 {
                    return Vec::new();
                }
                let repeat = flag(fx, "repeat_edges");
                let passes = scalar(fx, "passes", 1.0).clamp(1.0, 5.0) as usize;
                let (x, y) = axes(fx);
                let mut out = Vec::new();
                for _ in 0..passes {
                    if x {
                        out.push(convolve(0, repeat, box_weights(taps)));
                    }
                    if y {
                        out.push(convolve(1, repeat, box_weights(taps)));
                    }
                }
                out
            }
            Kernel::DirectionalBlur => {
                let length = scalar(fx, "length", 0.0).max(0.0) * s;
                // One sample per texel of length, odd so one sits at the centre.
                let samples = (length.ceil() as usize).min(2 * MAX_TAPS) | 1;
                if samples <= 1 {
                    return Vec::new();
                }
                let angle = scalar(fx, "direction", 0.0).to_radians();
                let step = length / (samples - 1) as f64;
                let mut k = KernelParams::default();
                k.p[0] = [
                    (libm::cos(angle) * step) as f32,
                    (libm::sin(angle) * step) as f32,
                    0.0,
                    0.0,
                ];
                k.i[1] = samples as u32;
                vec![k]
            }
        }
    }

    /// How far the kernel reaches, in layer pixels, per axis.
    fn reach(self, fx: &EvaluatedEffect) -> DVec2 {
        // One layer pixel more than the parameter: the taps cover
        // `ceil(r · scale)` texels, which never exceeds `(r + 1) · scale`
        // for preview scales up to 1.
        let r = |name: &str| scalar(fx, name, 0.0).max(0.0);
        match self {
            Kernel::Invert => DVec2::ZERO,
            Kernel::GaussianBlur | Kernel::BoxBlur => {
                let passes = if self == Kernel::BoxBlur {
                    scalar(fx, "passes", 1.0).clamp(1.0, 5.0)
                } else {
                    1.0
                };
                let d = r("radius") * passes + 1.0;
                let (x, y) = axes(fx);
                DVec2::new(if x { d } else { 0.0 }, if y { d } else { 0.0 })
            }
            Kernel::DirectionalBlur => {
                let half = r("length") / 2.0 + 1.0;
                let a = scalar(fx, "direction", 0.0).to_radians();
                DVec2::new(libm::cos(a).abs(), libm::sin(a).abs()) * half
            }
        }
    }

    /// Whether the kernel repeats edge pixels instead of reading
    /// transparency past the input (its output then stays inside it).
    fn repeats_edges(self, fx: &EvaluatedEffect) -> bool {
        matches!(self, Kernel::GaussianBlur | Kernel::BoxBlur) && flag(fx, "repeat_edges")
    }

    /// Region of definition: how far the output can reach, given the input
    /// region (layer pixels).
    pub fn rod(self, fx: &EvaluatedEffect, input: Rect, scale: f64) -> Rect {
        let _ = scale;
        if self.repeats_edges(fx) {
            return input;
        }
        input.pad(self.reach(fx))
    }

    /// Region of interest: the input pixels needed to compute `output`.
    pub fn roi(self, fx: &EvaluatedEffect, output: Rect, scale: f64) -> Rect {
        let _ = scale;
        output.pad(self.reach(fx))
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
