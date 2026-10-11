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
use voxy_core::{EffectTypeId, EvaluatedEffect, LinearRgba, Value, WorkingSpace};

/// The most samples Radial and Zoom Blur average per pixel; each pixel
/// takes one per texel of its sweep, up to this (odd, so one sits on the
/// pixel itself).
pub const SWEEP_SAMPLES: u32 = 255;

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

fn color(fx: &EvaluatedEffect, name: &str, default: LinearRgba) -> LinearRgba {
    match fx.param(name) {
        Some(Value::Color(c)) if c.is_finite() => *c,
        _ => default,
    }
}

/// A linear colour as working-space RGB (alpha dropped).
fn working_rgb(c: LinearRgba, space: WorkingSpace) -> [f32; 4] {
    let [r, g, b, _] = match space {
        WorkingSpace::Linear => c.to_array(),
        WorkingSpace::SrgbCompat => c.to_srgb(),
    };
    [r, g, b, 0.0]
}

/// The contrast factor `k` for a contrast setting `c` in `[-1, 1]`:
/// `1 + c` below zero (down to a flat grey at −1), `1 / (1 − 0.99·c)` above
/// (up to 100 at 1). Continuous, with `k = 1` at 0.
pub fn contrast_factor(c: f64) -> f64 {
    let c = c.clamp(-1.0, 1.0);
    if c < 0.0 {
        1.0 + c
    } else {
        1.0 / (1.0 - 0.99 * c)
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

/// Where a kernel's passes run: the chain buffer they read and write.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassContext {
    /// Texels per layer pixel (the preview scale, possibly reduced to fit
    /// the GPU); pixel parameters are multiplied by it.
    pub texel_scale: f64,
    /// The working space colour parameters are encoded for.
    pub space: WorkingSpace,
    /// The layer pixel at the buffer's top-left corner (texel `(0, 0)`).
    pub origin: DVec2,
}

impl PassContext {
    pub fn new(texel_scale: f64, space: WorkingSpace) -> Self {
        PassContext {
            texel_scale,
            space,
            origin: DVec2::ZERO,
        }
    }

    /// A layer-pixel position in buffer texels.
    pub fn texel(&self, layer_px: DVec2) -> DVec2 {
        (layer_px - self.origin) * self.texel_scale
    }
}

fn point(fx: &EvaluatedEffect, name: &str) -> DVec2 {
    match fx.param(name) {
        Some(Value::Vec2(v)) if v.is_finite() => *v,
        _ => DVec2::ZERO,
    }
}

/// The largest distance from `c` to `r`'s corners.
fn farthest(r: Rect, c: DVec2) -> f64 {
    [
        r.min,
        r.max,
        DVec2::new(r.min.x, r.max.y),
        DVec2::new(r.max.x, r.min.y),
    ]
    .iter()
    .map(|p| p.distance(c))
    .fold(0.0, f64::max)
}

/// `r` scaled by `k` around `c`.
fn scaled(r: Rect, centre: DVec2, k: f64) -> Rect {
    let from = centre + (r.min - centre) * k;
    let to = centre + (r.max - centre) * k;
    Rect::new(from.min(to), from.max(to))
}

/// Half the spin of Radial Blur, in radians.
fn spin_half(fx: &EvaluatedEffect) -> f64 {
    scalar(fx, "angle", 0.0).clamp(0.0, 360.0).to_radians() / 2.0
}

fn zoom_amount(fx: &EvaluatedEffect) -> f64 {
    scalar(fx, "amount", 0.0).clamp(0.0, 0.9)
}

/// The region a spin of `±half` radians around `c` can carry `r` to: `r`
/// grown by the longest chord a corner travels, kept within the circle
/// through the farthest corner.
fn spun(r: Rect, c: DVec2, half: f64) -> Rect {
    let reach = farthest(r, c);
    let circle = Rect::new(c - DVec2::splat(reach), c + DVec2::splat(reach));
    r.pad(DVec2::splat(reach * half.min(2.0) + 1.0))
        .intersect(&circle.pad(DVec2::ONE))
        .union(&r)
}

/// The built-in effects the renderer can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kernel {
    Invert,
    GaussianBlur,
    BoxBlur,
    DirectionalBlur,
    RadialBlur,
    ZoomBlur,
    BrightnessContrast,
    Exposure,
    Tint,
    HueSaturation,
}

impl Kernel {
    /// Every kernel.
    pub const ALL: [Kernel; 10] = [
        Kernel::Invert,
        Kernel::GaussianBlur,
        Kernel::BoxBlur,
        Kernel::DirectionalBlur,
        Kernel::RadialBlur,
        Kernel::ZoomBlur,
        Kernel::BrightnessContrast,
        Kernel::Exposure,
        Kernel::Tint,
        Kernel::HueSaturation,
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
            Kernel::RadialBlur => ids::RADIAL_BLUR,
            Kernel::ZoomBlur => ids::ZOOM_BLUR,
            Kernel::BrightnessContrast => ids::BRIGHTNESS_CONTRAST,
            Kernel::Exposure => ids::EXPOSURE,
            Kernel::Tint => ids::TINT,
            Kernel::HueSaturation => ids::HUE_SATURATION,
        }
    }

    /// The shader passes for `fx`, in order, run in the buffer `ctx`
    /// describes. An empty list means the effect does nothing with these
    /// parameters, so defaults pass through exactly.
    pub fn passes(self, fx: &EvaluatedEffect, ctx: &PassContext) -> Vec<KernelParams> {
        let s = ctx.texel_scale;
        let space = ctx.space;
        let mut k = KernelParams::default();
        match self {
            Kernel::BrightnessContrast => {
                let brightness = scalar(fx, "brightness", 0.0).clamp(-1.0, 1.0);
                let contrast = scalar(fx, "contrast", 0.0).clamp(-1.0, 1.0);
                if brightness == 0.0 && contrast == 0.0 {
                    return Vec::new();
                }
                k.p[0] = [
                    brightness as f32,
                    contrast_factor(contrast) as f32,
                    0.0,
                    0.0,
                ];
                vec![k]
            }
            Kernel::Exposure => {
                let stops = scalar(fx, "exposure", 0.0).clamp(-20.0, 20.0);
                let offset = scalar(fx, "offset", 0.0).clamp(-1.0, 1.0);
                let gamma = scalar(fx, "gamma", 1.0).clamp(0.01, 10.0);
                if stops == 0.0 && offset == 0.0 && gamma == 1.0 {
                    return Vec::new();
                }
                k.p[0] = [
                    libm::exp2(stops) as f32,
                    offset as f32,
                    (1.0 / gamma) as f32,
                    0.0,
                ];
                vec![k]
            }
            Kernel::Tint => {
                let amount = scalar(fx, "amount", 1.0).clamp(0.0, 1.0);
                if amount == 0.0 {
                    return Vec::new();
                }
                k.p[0] = working_rgb(color(fx, "map_black", LinearRgba::BLACK), space);
                k.p[1] = working_rgb(color(fx, "map_white", LinearRgba::WHITE), space);
                k.p[2][0] = amount as f32;
                vec![k]
            }
            Kernel::HueSaturation => {
                let hue = scalar(fx, "hue", 0.0);
                let sat = scalar(fx, "saturation", 0.0).clamp(-1.0, 1.0);
                let light = scalar(fx, "lightness", 0.0).clamp(-1.0, 1.0);
                let colorize = flag(fx, "colorize");
                if hue == 0.0 && sat == 0.0 && light == 0.0 && !colorize {
                    return Vec::new();
                }
                // Turns, reduced exactly in f64 so large angles stay precise.
                let turns = |deg: f64| (deg / 360.0).rem_euclid(1.0) as f32;
                k.p[0] = [turns(hue), sat as f32, light as f32, 0.0];
                k.p[1] = [
                    turns(scalar(fx, "colorize_hue", 0.0)),
                    scalar(fx, "colorize_saturation", 0.25).clamp(0.0, 1.0) as f32,
                    0.0,
                    0.0,
                ];
                k.i[0] = u32::from(colorize);
                vec![k]
            }
            Kernel::Invert => {
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
                let (along_x, along_y) = axes(fx);
                let mut out = Vec::new();
                if along_x {
                    out.push(convolve(0, repeat, w.clone()));
                }
                if along_y {
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
                let (along_x, along_y) = axes(fx);
                let mut out = Vec::new();
                for _ in 0..passes {
                    if along_x {
                        out.push(convolve(0, repeat, box_weights(taps)));
                    }
                    if along_y {
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
                k.p[0] = [
                    (libm::cos(angle) * step) as f32,
                    (libm::sin(angle) * step) as f32,
                    0.0,
                    0.0,
                ];
                k.i[1] = samples as u32;
                vec![k]
            }
            Kernel::RadialBlur | Kernel::ZoomBlur => {
                let amount = if self == Kernel::RadialBlur {
                    spin_half(fx)
                } else {
                    zoom_amount(fx)
                };
                if amount == 0.0 {
                    return Vec::new();
                }
                let c = ctx.texel(point(fx, "center"));
                k.p[0] = [c.x as f32, c.y as f32, amount as f32, 0.0];
                k.i[1] = SWEEP_SAMPLES;
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
            Kernel::Invert
            | Kernel::BrightnessContrast
            | Kernel::Exposure
            | Kernel::Tint
            | Kernel::HueSaturation
            // Position-dependent: `rod` and `roi` handle them.
            | Kernel::RadialBlur
            | Kernel::ZoomBlur => DVec2::ZERO,
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
        let c = point(fx, "center");
        match self {
            Kernel::RadialBlur if spin_half(fx) > 0.0 => spun(input, c, spin_half(fx)),
            // A pixel at `p` averages `c + (p − c)·k` for k in [1 − a, 1],
            // so input reaches out to `c + (input − c) / (1 − a)`.
            Kernel::ZoomBlur if zoom_amount(fx) > 0.0 => input
                .union(&scaled(input, c, 1.0 / (1.0 - zoom_amount(fx))))
                .pad(DVec2::ONE),
            _ if self.repeats_edges(fx) => input,
            _ => input.pad(self.reach(fx)),
        }
    }

    /// Region of interest: the input pixels needed to compute `output`.
    pub fn roi(self, fx: &EvaluatedEffect, output: Rect, scale: f64) -> Rect {
        let _ = scale;
        let c = point(fx, "center");
        match self {
            Kernel::RadialBlur if spin_half(fx) > 0.0 => spun(output, c, spin_half(fx)),
            Kernel::ZoomBlur if zoom_amount(fx) > 0.0 => output
                .union(&scaled(output, c, 1.0 - zoom_amount(fx)))
                .pad(DVec2::ONE),
            _ => output.pad(self.reach(fx)),
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
