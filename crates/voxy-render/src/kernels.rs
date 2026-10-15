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

use voxy_core::glam::{DAffine2, DMat2, DVec2};
use voxy_core::registry::ids;
use voxy_core::{Curve, EffectTypeId, EvaluatedEffect, LinearRgba, Value, WorkingSpace};

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
    /// Convolution weights, centre first, or other tables (Curves); at
    /// most `MAX_TAPS + 1`.
    pub w: Vec<f32>,
    /// Which of the kernel's shaders runs this pass (0 for one-shader
    /// kernels).
    pub stage: u8,
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
    /// The size of the layer's own content in layer pixels, from its
    /// origin (for effects relative to the layer's edges).
    pub layer_size: DVec2,
    /// The layer part of the seed ([`crate::EffectNode::seed`]).
    pub seed: u64,
}

impl PassContext {
    pub fn new(texel_scale: f64, space: WorkingSpace) -> Self {
        PassContext {
            texel_scale,
            space,
            origin: DVec2::ZERO,
            layer_size: DVec2::ZERO,
            seed: 0,
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

/// Entries per channel table of Curves.
pub const CURVE_TABLE: usize = 256;

fn curve(fx: &EvaluatedEffect, name: &str) -> std::sync::Arc<Curve> {
    match fx.param(name) {
        Some(Value::Curve(c)) => c.clone(),
        _ => std::sync::Arc::new(Curve::identity()),
    }
}

/// The four tables of Curves, one after another: red, green and blue, each
/// with the master curve applied after it, then alpha.
/// [`CURVE_TABLE`] samples each, at `i / (CURVE_TABLE − 1)`.
pub fn curve_tables(master: &Curve, channels: [&Curve; 4]) -> Vec<f32> {
    let last = (CURVE_TABLE - 1) as f64;
    let mut w = Vec::with_capacity(4 * CURVE_TABLE);
    for (i, c) in channels.iter().enumerate() {
        for k in 0..CURVE_TABLE {
            let x = k as f64 / last;
            let y = if i < 3 {
                master.eval(c.eval(x))
            } else {
                c.eval(x)
            };
            w.push(y as f32);
        }
    }
    w
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

/// The base seed of Grain: an FNV-1a hash of the layer, the seed
/// parameter and the frame, folded to 32 bits (design §3.4). Integer
/// arithmetic only, so it is the same everywhere.
pub fn grain_seed(layer: u64, seed: i64, frame: i64) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for word in [layer, seed as u64, frame as u64] {
        for byte in word.to_le_bytes() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    (h ^ (h >> 32)) as u32
}

/// The shadow's offset in layer pixels: Distance along Direction (0° right,
/// clockwise on screen).
fn shadow_offset(fx: &EvaluatedEffect) -> DVec2 {
    let a = scalar(fx, "direction", 45.0).to_radians();
    let d = scalar(fx, "distance", 0.0).clamp(0.0, 1000.0);
    DVec2::new(libm::cos(a), libm::sin(a)) * d
}

/// Transform's map in layer pixels: `position + R·S·(p − anchor)`.
fn transform_map(fx: &EvaluatedEffect) -> DAffine2 {
    let scale = match fx.param("scale") {
        Some(Value::Vec2(v)) if v.is_finite() => v.clamp(DVec2::splat(-100.0), DVec2::splat(100.0)),
        _ => DVec2::ONE,
    };
    let a = scalar(fx, "rotation", 0.0).to_radians();
    let (sin, cos) = (libm::sin(a), libm::cos(a));
    let rotate = DMat2::from_cols(DVec2::new(cos, sin), DVec2::new(-sin, cos));
    DAffine2::from_translation(point(fx, "position"))
        * DAffine2::from_mat2(rotate * DMat2::from_diagonal(scale))
        * DAffine2::from_translation(-point(fx, "anchor"))
}

/// The bounding box of `r` mapped by `m`.
fn mapped(r: Rect, m: DAffine2) -> Rect {
    let corners = [
        r.min,
        r.max,
        DVec2::new(r.min.x, r.max.y),
        DVec2::new(r.max.x, r.min.y),
    ]
    .map(|p| m.transform_point2(p));
    corners
        .iter()
        .skip(1)
        .fold(Rect::new(corners[0], corners[0]), |acc, p| {
            Rect::new(acc.min.min(*p), acc.max.max(*p))
        })
}

/// Mirror's normal: pixels on its side are replaced by their reflection.
fn mirror_normal(fx: &EvaluatedEffect) -> DVec2 {
    let a = scalar(fx, "angle", 0.0).to_radians();
    DVec2::new(libm::cos(a), libm::sin(a))
}

/// The reflection across Mirror's line.
fn mirror_map(fx: &EvaluatedEffect) -> DAffine2 {
    let n = mirror_normal(fx);
    let c = point(fx, "center");
    let householder = DMat2::IDENTITY - 2.0 * DMat2::from_cols(n * n.x, n * n.y);
    DAffine2::from_mat2_translation(householder, c - householder * c)
}

fn bulge_radius(fx: &EvaluatedEffect) -> f64 {
    scalar(fx, "radius", 100.0).clamp(1.0, 10_000.0)
}

/// `r` and, when they meet, Bulge's circle: pixels inside it come from
/// inside it.
fn with_circle(r: Rect, fx: &EvaluatedEffect) -> Rect {
    if scalar(fx, "amount", 0.0) == 0.0 {
        return r;
    }
    let c = point(fx, "center");
    let circle = Rect::new(c, c).pad(DVec2::splat(bulge_radius(fx) + 1.0));
    if r.intersect(&circle).is_empty() {
        r
    } else {
        r.union(&circle)
    }
}

/// Degrees as radians in `[0, 2π)`, reduced in `f64` so large phases stay
/// precise in `f32`.
fn turns_rad(deg: f64) -> f32 {
    ((deg / 360.0).rem_euclid(1.0) * std::f64::consts::TAU) as f32
}

/// Displacement Map's largest horizontal and vertical shifts, layer px.
fn displacement(fx: &EvaluatedEffect) -> (f64, f64) {
    let amount = |name: &str| scalar(fx, name, 0.0).clamp(-1000.0, 1000.0);
    (amount("h_amount"), amount("v_amount"))
}

/// A transition's progress, 0 to 1: the layer's own progress with Auto on.
fn transition_progress(fx: &EvaluatedEffect) -> f64 {
    let t = if flag(fx, "auto") {
        fx.layer_progress
    } else {
        scalar(fx, "progress", 0.0)
    };
    if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn factor2(fx: &EvaluatedEffect, name: &str, default: f64) -> DVec2 {
    match fx.param(name) {
        Some(Value::Vec2(v)) if v.is_finite() => v.clamp(DVec2::splat(0.01), DVec2::splat(10.0)),
        _ => DVec2::splat(default),
    }
}

/// One tile's size in layer pixels.
fn tile_size(fx: &EvaluatedEffect, layer: Rect) -> DVec2 {
    (layer.size() * factor2(fx, "tile_size", 0.5)).max(DVec2::splat(1e-3))
}

/// The tiled area: the layer scaled by Output Size around its centre.
fn tile_output(fx: &EvaluatedEffect, layer: Rect) -> Rect {
    let centre = (layer.min + layer.max) / 2.0;
    let half = layer.size() * factor2(fx, "output_size", 1.0) / 2.0;
    Rect::new(centre - half, centre + half)
}

/// Stroke's width in layer pixels.
fn stroke_width(fx: &EvaluatedEffect) -> f64 {
    scalar(fx, "width", 0.0).clamp(0.0, 500.0)
}

/// How far the shadow's blur reaches, in layer pixels (as Gaussian Blur).
fn shadow_reach(fx: &EvaluatedEffect) -> f64 {
    scalar(fx, "softness", 0.0).clamp(0.0, 500.0) + 1.0
}

/// Chromatic Aberration's scale difference `k` (red `1 + k`, blue `1 − k`).
fn aberration(fx: &EvaluatedEffect) -> f64 {
    scalar(fx, "amount", 0.0).clamp(0.0, 0.2)
}

/// Pixelate's block size in layer pixels.
fn pixel_cell(fx: &EvaluatedEffect) -> f64 {
    scalar(fx, "cell", 10.0).clamp(1.0, 1000.0)
}

/// `r` grown to whole blocks of `cell` on the layer's grid.
fn cells(r: Rect, cell: f64) -> Rect {
    Rect::new((r.min / cell).floor() * cell, (r.max / cell).ceil() * cell)
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
    Levels,
    Posterize,
    Curves,
    ColorBalance,
    GradientMap,
    Lut,
    Vignette,
    Pixelate,
    Grain,
    ChromaticAberration,
    DropShadow,
    InnerShadow,
    Glow,
    Stroke,
    ChromaKey,
    LumaKey,
    MatteChoker,
    Transform,
    Mirror,
    Wave,
    Ripple,
    Bulge,
    Tile,
    Crossfade,
    Wipe,
    Slide,
    Zoom,
    DisplacementMap,
    /// An effect run by an [`ExternalEffects`](voxy_core::external::ExternalEffects)
    /// provider on the CPU (OpenFX plugins, design §6.9). Not in
    /// [`Kernel::ALL`]: it has no shader.
    External,
}

impl Kernel {
    /// Every kernel.
    pub const ALL: [Kernel; 38] = [
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
        Kernel::Levels,
        Kernel::Posterize,
        Kernel::Curves,
        Kernel::ColorBalance,
        Kernel::GradientMap,
        Kernel::Lut,
        Kernel::Vignette,
        Kernel::Pixelate,
        Kernel::Grain,
        Kernel::ChromaticAberration,
        Kernel::DropShadow,
        Kernel::InnerShadow,
        Kernel::Glow,
        Kernel::Stroke,
        Kernel::ChromaKey,
        Kernel::LumaKey,
        Kernel::MatteChoker,
        Kernel::Transform,
        Kernel::Mirror,
        Kernel::Wave,
        Kernel::Ripple,
        Kernel::Bulge,
        Kernel::Tile,
        Kernel::Crossfade,
        Kernel::Wipe,
        Kernel::Slide,
        Kernel::Zoom,
        Kernel::DisplacementMap,
    ];

    /// The kernel running `type_id` at `version`, if this build has one.
    pub fn find(type_id: &EffectTypeId, version: u32) -> Option<Kernel> {
        if type_id.is_ofx() {
            return Some(Kernel::External);
        }
        Kernel::ALL
            .into_iter()
            .find(|k| k.type_id() == type_id.as_str() && version == 1)
    }

    /// Whether the kernel uses the deterministic seed (design §3.4).
    pub fn seeded(self) -> bool {
        self == Kernel::Grain
    }

    /// Whether the output of `fx` depends on the frame number (animated
    /// seeded randomness, or an OpenFX plugin, which is told the time).
    /// Only then is the frame part of the cache key, so static effects
    /// render once.
    pub fn uses_frame(self, fx: &EvaluatedEffect) -> bool {
        (self == Kernel::Grain && flag(fx, "animate")) || self == Kernel::External
    }

    /// The layer-reference parameter naming the layer the kernel reads.
    pub fn layer_input(self) -> Option<&'static str> {
        match self {
            Kernel::DisplacementMap => Some("map_layer"),
            k if k.is_transition() => Some("layer_b"),
            _ => None,
        }
    }

    /// A transition (design §5): Progress, Auto and a B layer.
    pub fn is_transition(self) -> bool {
        matches!(
            self,
            Kernel::Crossfade | Kernel::Wipe | Kernel::Slide | Kernel::Zoom
        )
    }

    /// The layer-reference parameter of `fx` naming the layer it reads:
    /// [`Kernel::layer_input`], or for an OpenFX effect its first extra
    /// clip's parameter.
    pub fn layer_param(self, fx: &EvaluatedEffect) -> Option<&str> {
        match self {
            Kernel::External => fx
                .params
                .iter()
                .map(|(n, _)| &**n)
                .find(|n| n.starts_with(voxy_core::external::CLIP_PARAM_PREFIX)),
            k => k.layer_input(),
        }
    }

    /// Whether the output of `fx` depends on the layer's progress through
    /// its duration (a transition with Auto on).
    pub fn uses_layer_progress(self, fx: &EvaluatedEffect) -> bool {
        match self {
            Kernel::External => flag(fx, voxy_core::external::AUTO_PARAM),
            k => k.is_transition() && flag(fx, "auto"),
        }
    }

    /// Whether the kernel combines its result with its own input, so the
    /// input is kept (bound as `orig`) while its passes run.
    pub fn keeps_input(self) -> bool {
        matches!(
            self,
            Kernel::DropShadow | Kernel::InnerShadow | Kernel::Glow | Kernel::Stroke
        )
    }

    /// Whether the kernel reads a LUT asset (bind group 2).
    pub fn uses_lut(self) -> bool {
        self == Kernel::Lut
    }

    /// The asset `fx` reads, for kernels that read one.
    pub fn asset(self, fx: &EvaluatedEffect) -> Option<voxy_core::AssetId> {
        match (self, fx.param("lut")) {
            (Kernel::Lut, Some(Value::Asset(a))) => *a,
            _ => None,
        }
    }

    pub fn type_id(self) -> &'static str {
        match self {
            // OpenFX types are matched by prefix in `find`.
            Kernel::External => "",
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
            Kernel::Levels => ids::LEVELS,
            Kernel::Posterize => ids::POSTERIZE,
            Kernel::Curves => ids::CURVES,
            Kernel::ColorBalance => ids::COLOR_BALANCE,
            Kernel::GradientMap => ids::GRADIENT_MAP,
            Kernel::Lut => ids::LUT,
            Kernel::Vignette => ids::VIGNETTE,
            Kernel::Pixelate => ids::PIXELATE,
            Kernel::Grain => ids::GRAIN,
            Kernel::ChromaticAberration => ids::CHROMATIC_ABERRATION,
            Kernel::DropShadow => ids::DROP_SHADOW,
            Kernel::InnerShadow => ids::INNER_SHADOW,
            Kernel::Glow => ids::GLOW,
            Kernel::Stroke => ids::STROKE,
            Kernel::ChromaKey => ids::CHROMA_KEY,
            Kernel::LumaKey => ids::LUMA_KEY,
            Kernel::MatteChoker => ids::MATTE_CHOKER,
            Kernel::Transform => ids::TRANSFORM,
            Kernel::Mirror => ids::MIRROR,
            Kernel::Wave => ids::WAVE,
            Kernel::Ripple => ids::RIPPLE,
            Kernel::Bulge => ids::BULGE,
            Kernel::Tile => ids::TILE,
            Kernel::Crossfade => ids::CROSSFADE,
            Kernel::Wipe => ids::WIPE,
            Kernel::Slide => ids::SLIDE,
            Kernel::Zoom => ids::ZOOM,
            Kernel::DisplacementMap => ids::DISPLACEMENT_MAP,
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
            // Runs on the CPU; the compositor handles it, with no passes.
            Kernel::External => Vec::new(),
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
            Kernel::Levels => {
                let ib = scalar(fx, "input_black", 0.0).clamp(0.0, 1.0);
                let iw = scalar(fx, "input_white", 1.0).clamp(0.0, 1.0);
                let gamma = scalar(fx, "gamma", 1.0).clamp(0.1, 10.0);
                let ob = scalar(fx, "output_black", 0.0).clamp(0.0, 1.0);
                let ow = scalar(fx, "output_white", 1.0).clamp(0.0, 1.0);
                if ib == 0.0 && iw == 1.0 && gamma == 1.0 && ob == 0.0 && ow == 1.0 {
                    return Vec::new();
                }
                // Equal points make a hard step at that point.
                let span = if (iw - ib).abs() < 1e-4 {
                    1e-4
                } else {
                    iw - ib
                };
                k.p[0] = [ib as f32, (1.0 / span) as f32, (1.0 / gamma) as f32, 0.0];
                k.p[1] = [ob as f32, ow as f32, 0.0, 0.0];
                k.i[0] = choice(fx, "channel").min(4);
                vec![k]
            }
            Kernel::Curves => {
                let master = curve(fx, "master");
                let [red, green, blue, alpha] =
                    ["red", "green", "blue", "alpha"].map(|n| curve(fx, n));
                if [&master, &red, &green, &blue, &alpha]
                    .iter()
                    .all(|c| c.is_identity())
                {
                    return Vec::new();
                }
                k.w = curve_tables(&master, [&red, &green, &blue, &alpha]);
                vec![k]
            }
            Kernel::Vignette => {
                let amount = scalar(fx, "amount", 0.5).clamp(0.0, 1.0);
                if amount == 0.0 {
                    return Vec::new();
                }
                let a = ctx.texel(DVec2::ZERO);
                let b = ctx.texel(ctx.layer_size);
                k.p[0] = [a.x as f32, a.y as f32, b.x as f32, b.y as f32];
                k.p[1] = [
                    amount as f32,
                    scalar(fx, "size", 0.6).clamp(0.0, 1.5) as f32,
                    scalar(fx, "feather", 0.6).clamp(0.0, 1.5) as f32,
                    0.0,
                ];
                k.p[2] = working_rgb(color(fx, "color", LinearRgba::BLACK), space);
                vec![k]
            }
            Kernel::Transform => {
                let opacity = scalar(fx, "opacity", 1.0).clamp(0.0, 1.0);
                let map = transform_map(fx);
                if map == DAffine2::IDENTITY && opacity == 1.0 {
                    return Vec::new();
                }
                // Output texel to input texel: through layer pixels.
                let texel =
                    DAffine2::from_scale(DVec2::splat(s)) * DAffine2::from_translation(-ctx.origin);
                let (inv, opacity) = if map.matrix2.determinant().abs() < 1e-12 {
                    (DAffine2::IDENTITY, 0.0)
                } else {
                    (texel * map.inverse() * texel.inverse(), opacity)
                };
                let (col0, col1) = (inv.matrix2.x_axis, inv.matrix2.y_axis);
                k.p[0] = [col0.x as f32, col0.y as f32, col1.x as f32, col1.y as f32];
                k.p[1] = [
                    inv.translation.x as f32,
                    inv.translation.y as f32,
                    opacity as f32,
                    0.0,
                ];
                vec![k]
            }
            Kernel::Mirror => {
                let c = ctx.texel(point(fx, "center"));
                let n = mirror_normal(fx);
                k.p[0] = [c.x as f32, c.y as f32, n.x as f32, n.y as f32];
                vec![k]
            }
            Kernel::Wave => {
                let amp = scalar(fx, "amplitude", 0.0).clamp(0.0, 500.0);
                if amp == 0.0 {
                    return Vec::new();
                }
                let o = ctx.texel(DVec2::ZERO);
                let length = scalar(fx, "wavelength", 60.0).clamp(1.0, 5000.0);
                let a = scalar(fx, "direction", 0.0).to_radians();
                k.p[0] = [
                    o.x as f32,
                    o.y as f32,
                    (length * s) as f32,
                    (amp * s) as f32,
                ];
                k.p[1] = [
                    libm::cos(a) as f32,
                    libm::sin(a) as f32,
                    turns_rad(scalar(fx, "phase", 0.0)),
                    0.0,
                ];
                vec![k]
            }
            Kernel::Ripple => {
                let amp = scalar(fx, "amplitude", 0.0).clamp(0.0, 500.0);
                if amp == 0.0 {
                    return Vec::new();
                }
                let c = ctx.texel(point(fx, "center"));
                let length = scalar(fx, "wavelength", 30.0).clamp(1.0, 5000.0);
                k.p[0] = [
                    c.x as f32,
                    c.y as f32,
                    (length * s) as f32,
                    (amp * s) as f32,
                ];
                k.p[1][0] = turns_rad(scalar(fx, "phase", 0.0));
                vec![k]
            }
            Kernel::Bulge => {
                let amount = scalar(fx, "amount", 0.0).clamp(-1.0, 1.0);
                if amount == 0.0 {
                    return Vec::new();
                }
                let c = ctx.texel(point(fx, "center"));
                let radius = bulge_radius(fx);
                k.p[0] = [
                    c.x as f32,
                    c.y as f32,
                    (radius * s) as f32,
                    libm::exp2(amount) as f32,
                ];
                vec![k]
            }
            Kernel::Crossfade | Kernel::Wipe | Kernel::Slide | Kernel::Zoom => {
                let progress = transition_progress(fx);
                if progress == 0.0 {
                    return Vec::new();
                }
                let layer = Rect::of_size(ctx.layer_size);
                let (lo, hi) = (ctx.texel(layer.min), ctx.texel(layer.max));
                k.p[1] = [lo.x as f32, lo.y as f32, hi.x as f32, hi.y as f32];
                match self {
                    Kernel::Crossfade => k.p[0][0] = (1.0 - progress) as f32,
                    Kernel::Wipe => {
                        let angle = scalar(fx, "angle", 0.0).to_radians();
                        let dir = DVec2::new(libm::cos(angle), libm::sin(angle));
                        // The layer's extent along d, in texels.
                        let along = [lo, hi, DVec2::new(lo.x, hi.y), DVec2::new(hi.x, lo.y)]
                            .map(|c| c.dot(dir));
                        let first = along.iter().copied().fold(f64::INFINITY, f64::min);
                        let last = along.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                        let feather = scalar(fx, "feather", 20.0).clamp(0.0, 1000.0) * s;
                        let edge = (first - feather / 2.0) + progress * (last - first + feather);
                        k.p[0] = [
                            dir.x as f32,
                            dir.y as f32,
                            edge as f32,
                            feather.max(1e-3) as f32,
                        ];
                    }
                    Kernel::Slide => {
                        let angle = scalar(fx, "direction", 0.0).to_radians();
                        let dir = DVec2::new(libm::cos(angle), libm::sin(angle));
                        let size = hi - lo;
                        let extent = dir.x.abs() * size.x + dir.y.abs() * size.y;
                        // A leaves along d; B follows it in from behind.
                        let off = dir * extent * progress;
                        let rest = dir * extent * (1.0 - progress);
                        k.p[0] = [off.x as f32, off.y as f32, rest.x as f32, rest.y as f32];
                    }
                    _ => {
                        let zoom = scalar(fx, "zoom", 4.0).clamp(1.0, 100.0);
                        let c = (lo + hi) / 2.0;
                        let scale = 1.0 + progress * (zoom - 1.0);
                        k.p[0] = [
                            c.x as f32,
                            c.y as f32,
                            scale as f32,
                            (1.0 - progress) as f32,
                        ];
                    }
                }
                vec![k]
            }
            Kernel::DisplacementMap => {
                let (h, v) = displacement(fx);
                if h == 0.0 && v == 0.0 {
                    return Vec::new();
                }
                k.p[0] = [(h * s) as f32, (v * s) as f32, 0.0, 0.0];
                k.i[0] = choice(fx, "h_channel").min(4);
                k.i[1] = choice(fx, "v_channel").min(4);
                vec![k]
            }
            Kernel::Tile => {
                let layer = Rect::of_size(ctx.layer_size);
                let (tile, out) = (tile_size(fx, layer), tile_output(fx, layer));
                let (lo, hi) = (ctx.texel(layer.min), ctx.texel(layer.max));
                let c = ctx.texel(point(fx, "center"));
                let (o0, o1) = (ctx.texel(out.min), ctx.texel(out.max));
                k.p[0] = [lo.x as f32, lo.y as f32, hi.x as f32, hi.y as f32];
                k.p[1] = [
                    c.x as f32,
                    c.y as f32,
                    (tile.x * s) as f32,
                    (tile.y * s) as f32,
                ];
                k.p[2] = [o0.x as f32, o0.y as f32, o1.x as f32, o1.y as f32];
                k.i[0] = u32::from(flag(fx, "mirror"));
                vec![k]
            }
            Kernel::ChromaKey => {
                k.p[0] = working_rgb(
                    color(fx, "key_color", LinearRgba::from_srgb8(0, 177, 64, 255)),
                    space,
                );
                k.p[1] = [
                    scalar(fx, "tolerance", 0.15).clamp(0.0, 1.0) as f32,
                    scalar(fx, "softness", 0.1).clamp(0.0, 1.0) as f32,
                    scalar(fx, "spill", 0.5).clamp(0.0, 1.0) as f32,
                    0.0,
                ];
                vec![k]
            }
            Kernel::LumaKey => {
                k.p[0] = [
                    scalar(fx, "threshold", 0.1).clamp(0.0, 1.0) as f32,
                    scalar(fx, "softness", 0.1).clamp(0.0, 1.0) as f32,
                    0.0,
                    0.0,
                ];
                k.i[0] = choice(fx, "mode").min(1);
                vec![k]
            }
            Kernel::MatteChoker => {
                let choke = scalar(fx, "choke", 0.0).clamp(-100.0, 100.0);
                let taps = (choke.abs() * s).round() as usize;
                let mut out = Vec::new();
                if taps > 0 {
                    for axis in [0, 1] {
                        out.push(KernelParams {
                            i: [axis, taps.min(MAX_TAPS) as u32, u32::from(choke < 0.0), 0],
                            ..KernelParams::default()
                        });
                    }
                }
                let w = gaussian_weights(scalar(fx, "softness", 0.0).clamp(0.0, 100.0) * s);
                if w.len() > 1 {
                    for axis in [0, 1] {
                        out.push(KernelParams {
                            stage: 1,
                            ..convolve(axis, false, w.clone())
                        });
                    }
                }
                out
            }
            Kernel::Stroke => {
                let width = stroke_width(fx) * s;
                let opacity = scalar(fx, "opacity", 1.0).clamp(0.0, 1.0);
                let only = flag(fx, "outline_only");
                if width <= 0.0 || (opacity == 0.0 && !only) {
                    return Vec::new();
                }
                // Stage 0 marks the opaque pixels; stage 1 floods each
                // pixel with the offset to its nearest one, in steps that
                // halve down to 1 (jump flooding).
                let mut out = vec![KernelParams::default()];
                let mut step = (width.ceil() as u32 + 1).next_power_of_two();
                while step >= 1 {
                    let mut p = KernelParams {
                        stage: 1,
                        ..KernelParams::default()
                    };
                    p.p[0][0] = step as f32;
                    out.push(p);
                    step /= 2;
                }
                // Stage 2: the outline, behind the kept input.
                k.stage = 2;
                k.p[0] = working_rgb(color(fx, "color", LinearRgba::WHITE), space);
                k.p[1] = [width as f32, opacity as f32, 0.0, 0.0];
                k.i[0] = u32::from(only);
                out.push(k);
                out
            }
            Kernel::Glow => {
                let intensity = scalar(fx, "intensity", 1.0).clamp(0.0, 10.0);
                if intensity == 0.0 {
                    return Vec::new();
                }
                // Stage 0: the light above the threshold, in linear light.
                let mut bright = KernelParams::default();
                bright.p[0][0] = scalar(fx, "threshold", 0.5).clamp(0.0, 2.0) as f32;
                let mut out = vec![bright];
                // Stage 1: blurred.
                let w = gaussian_weights(scalar(fx, "radius", 0.0).clamp(0.0, 500.0) * s);
                if w.len() > 1 {
                    for axis in [0, 1] {
                        out.push(KernelParams {
                            stage: 1,
                            ..convolve(axis, false, w.clone())
                        });
                    }
                }
                // Stage 2: added to the input.
                k.stage = 2;
                k.p[0][0] = intensity as f32;
                out.push(k);
                out
            }
            Kernel::DropShadow | Kernel::InnerShadow => {
                let opacity = scalar(fx, "opacity", 0.5).clamp(0.0, 1.0);
                let only = self == Kernel::DropShadow && flag(fx, "shadow_only");
                if opacity == 0.0 && !only {
                    return Vec::new();
                }
                let off = shadow_offset(fx) * s;
                // Stage 0: the input's alpha, moved by the offset.
                let mut mask = KernelParams::default();
                mask.p[0] = [off.x as f32, off.y as f32, 0.0, 0.0];
                let mut out = vec![mask];
                // Stage 1: blurred, like Gaussian Blur.
                let w = gaussian_weights(scalar(fx, "softness", 0.0).clamp(0.0, 500.0) * s);
                if w.len() > 1 {
                    for axis in [0, 1] {
                        out.push(KernelParams {
                            stage: 1,
                            ..convolve(axis, false, w.clone())
                        });
                    }
                }
                // Stage 2: coloured and combined with the input.
                k.stage = 2;
                k.p[0] = working_rgb(color(fx, "color", LinearRgba::BLACK), space);
                k.p[1][0] = opacity as f32;
                k.i[0] = match self {
                    Kernel::InnerShadow => 2,
                    _ => u32::from(only),
                };
                out.push(k);
                out
            }
            Kernel::ChromaticAberration => {
                let amount = aberration(fx);
                if amount == 0.0 {
                    return Vec::new();
                }
                let c = ctx.texel(point(fx, "center"));
                k.p[0] = [c.x as f32, c.y as f32, amount as f32, 0.0];
                vec![k]
            }
            Kernel::Grain => {
                let amount = scalar(fx, "amount", 0.1).clamp(0.0, 1.0);
                if amount == 0.0 {
                    return Vec::new();
                }
                let o = ctx.texel(DVec2::ZERO);
                let size = scalar(fx, "size", 1.0).clamp(0.5, 50.0) * s;
                k.p[0] = [o.x as f32, o.y as f32, size as f32, amount as f32];
                let user = match fx.param("seed") {
                    Some(Value::Int(n)) => *n,
                    _ => 0,
                };
                let frame = if flag(fx, "animate") { fx.frame } else { 0 };
                k.i[0] = u32::from(flag(fx, "monochrome"));
                k.i[2] = grain_seed(ctx.seed, user, frame);
                vec![k]
            }
            Kernel::Pixelate => {
                let cell = pixel_cell(fx);
                if cell <= 1.0 {
                    return Vec::new();
                }
                let o = ctx.texel(DVec2::ZERO);
                let c = cell * s;
                k.p[0] = [o.x as f32, o.y as f32, c as f32, 0.0];
                // One sample per texel of a block, at most 16 per side.
                k.i[1] = (c.ceil() as u32).clamp(1, 16);
                vec![k]
            }
            Kernel::Lut => {
                let amount = scalar(fx, "amount", 1.0).clamp(0.0, 1.0);
                if amount == 0.0 || self.asset(fx).is_none() {
                    return Vec::new();
                }
                // The table's size and domain are filled in by the
                // compositor once the file is loaded (`p1`, `p2`, `i0.y`).
                k.p[0][0] = amount as f32;
                k.i[0] = choice(fx, "input").min(1);
                vec![k]
            }
            Kernel::GradientMap => {
                let amount = scalar(fx, "amount", 1.0).clamp(0.0, 1.0);
                if amount == 0.0 {
                    return Vec::new();
                }
                k.p[0] = working_rgb(color(fx, "shadow", LinearRgba::BLACK), space);
                k.p[1] = working_rgb(
                    color(fx, "midtone", LinearRgba::from_srgb8(128, 128, 128, 255)),
                    space,
                );
                k.p[2] = working_rgb(color(fx, "highlight", LinearRgba::WHITE), space);
                k.p[3] = [
                    scalar(fx, "midpoint", 0.5).clamp(0.01, 0.99) as f32,
                    amount as f32,
                    0.0,
                    0.0,
                ];
                vec![k]
            }
            Kernel::ColorBalance => {
                let shift = |range: &str| {
                    let v = |axis: &str| {
                        scalar(fx, &format!("{range}_{axis}"), 0.0).clamp(-1.0, 1.0) as f32
                    };
                    [v("red"), v("green"), v("blue"), 0.0]
                };
                k.p[0] = shift("shadow");
                k.p[1] = shift("mid");
                k.p[2] = shift("high");
                if k.p[..3].iter().flatten().all(|v| *v == 0.0) {
                    return Vec::new();
                }
                k.i[0] = u32::from(flag(fx, "preserve_luminosity"));
                vec![k]
            }
            Kernel::Posterize => {
                let levels = match fx.param("levels") {
                    Some(Value::Int(n)) => (*n).clamp(2, 256) as u32,
                    _ => 6,
                };
                k.i[0] = levels;
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
            // OpenFX filters: the region stays the input's for now (the
            // plugin's own GetRegionOfDefinition joins in a later step).
            Kernel::External
            | Kernel::Invert
            | Kernel::BrightnessContrast
            | Kernel::Exposure
            | Kernel::Tint
            | Kernel::HueSaturation
            | Kernel::Levels
            | Kernel::Posterize
            | Kernel::Curves
            | Kernel::ColorBalance
            | Kernel::GradientMap
            | Kernel::Lut
            | Kernel::Vignette
            | Kernel::Grain
            // Position-dependent: `rod` and `roi` handle them.
            | Kernel::RadialBlur
            | Kernel::ZoomBlur
            | Kernel::Pixelate
            | Kernel::ChromaticAberration
            | Kernel::DropShadow
            | Kernel::InnerShadow
            | Kernel::ChromaKey
            | Kernel::LumaKey
            | Kernel::Transform
            | Kernel::Mirror
            | Kernel::Bulge
            | Kernel::Tile
            | Kernel::Crossfade
            | Kernel::Wipe
            | Kernel::Slide
            | Kernel::Zoom => DVec2::ZERO,
            Kernel::Wave | Kernel::Ripple => DVec2::splat(r("amplitude").min(500.0) + 1.0),
            Kernel::DisplacementMap => {
                let (h, v) = displacement(fx);
                DVec2::new(h.abs(), v.abs()) + DVec2::ONE
            }
            Kernel::Glow => DVec2::splat(r("radius").min(500.0) + 1.0),
            Kernel::Stroke => DVec2::splat(stroke_width(fx) + 1.0),
            // Growing reaches out by the choke; softening by its radius.
            Kernel::MatteChoker => {
                let grow = (-scalar(fx, "choke", 0.0)).clamp(0.0, 100.0);
                DVec2::splat(grow + r("softness").min(100.0) + 1.0)
            }
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
    /// region (layer pixels). It always contains the input region, because
    /// one buffer holds every stage of a chain.
    /// `layer` is the layer's own content rectangle, for kernels relative
    /// to it (Tile).
    pub fn rod(self, fx: &EvaluatedEffect, input: Rect, layer: Rect) -> Rect {
        let c = point(fx, "center");
        match self {
            Kernel::RadialBlur if spin_half(fx) > 0.0 => spun(input, c, spin_half(fx)),
            // A pixel at `p` averages `c + (p − c)·k` for k in [1 − a, 1],
            // so input reaches out to `c + (input − c) / (1 − a)`.
            Kernel::ZoomBlur if zoom_amount(fx) > 0.0 => input
                .union(&scaled(input, c, 1.0 / (1.0 - zoom_amount(fx))))
                .pad(DVec2::ONE),
            Kernel::Pixelate if pixel_cell(fx) > 1.0 => cells(input, pixel_cell(fx)),
            // Red is drawn scaled by 1 + k around c, blue by 1 − k.
            Kernel::ChromaticAberration if aberration(fx) > 0.0 => {
                let k = aberration(fx);
                input
                    .union(&scaled(input, c, 1.0 + k))
                    .union(&scaled(input, c, 1.0 - k))
                    .pad(DVec2::ONE)
            }
            // The moved, blurred alpha (Inner Shadow needs it beyond the
            // layer too, to shade near the edges correctly).
            Kernel::DropShadow | Kernel::InnerShadow => input.union(
                &Rect::new(input.min + shadow_offset(fx), input.max + shadow_offset(fx))
                    .pad(DVec2::splat(shadow_reach(fx))),
            ),
            // The chain's buffer holds every stage, so a region always
            // keeps the input too (scaled to nothing, the pass draws
            // transparency).
            Kernel::Transform => {
                let m = transform_map(fx);
                if m.matrix2.determinant().abs() < 1e-12 {
                    input
                } else {
                    input.union(&mapped(input, m).pad(DVec2::ONE))
                }
            }
            Kernel::Mirror => input.union(&mapped(input, mirror_map(fx))).pad(DVec2::ONE),
            Kernel::Bulge => with_circle(input, fx),
            Kernel::Tile => input.union(&tile_output(fx, layer).pad(DVec2::ONE)),
            _ if self.repeats_edges(fx) => input,
            _ => input.pad(self.reach(fx)),
        }
    }

    /// Region of interest: the input pixels needed to compute `output`.
    pub fn roi(self, fx: &EvaluatedEffect, output: Rect, layer: Rect) -> Rect {
        let c = point(fx, "center");
        match self {
            Kernel::RadialBlur if spin_half(fx) > 0.0 => spun(output, c, spin_half(fx)),
            Kernel::ZoomBlur if zoom_amount(fx) > 0.0 => output
                .union(&scaled(output, c, 1.0 - zoom_amount(fx)))
                .pad(DVec2::ONE),
            Kernel::Pixelate if pixel_cell(fx) > 1.0 => cells(output, pixel_cell(fx)),
            Kernel::ChromaticAberration if aberration(fx) > 0.0 => {
                let k = aberration(fx);
                output
                    .union(&scaled(output, c, 1.0 / (1.0 + k)))
                    .union(&scaled(output, c, 1.0 / (1.0 - k)))
                    .pad(DVec2::ONE)
            }
            Kernel::DropShadow | Kernel::InnerShadow => output.union(
                &Rect::new(
                    output.min - shadow_offset(fx),
                    output.max - shadow_offset(fx),
                )
                .pad(DVec2::splat(shadow_reach(fx))),
            ),
            Kernel::Transform => {
                let m = transform_map(fx);
                if m.matrix2.determinant().abs() < 1e-12 {
                    output
                } else {
                    mapped(output, m.inverse()).pad(DVec2::ONE)
                }
            }
            Kernel::Mirror => output
                .union(&mapped(output, mirror_map(fx)))
                .pad(DVec2::ONE),
            Kernel::Bulge => with_circle(output, fx),
            // Every tile shows the whole layer; Slide and Zoom read from
            // anywhere in the layer's frame.
            Kernel::Tile | Kernel::Slide | Kernel::Zoom => output.union(&layer.pad(DVec2::ONE)),
            _ => output.pad(self.reach(fx)),
        }
    }
}

/// Region of definition of a whole chain: `source` grown by every effect in
/// order.
pub fn chain_rod(effects: &[(Kernel, &EvaluatedEffect)], source: Rect, layer: Rect) -> Rect {
    effects
        .iter()
        .fold(source, |r, (k, fx)| k.rod(fx, r, layer))
}

/// The input region the first effect needs so that `wanted` of the chain's
/// output is correct: `wanted` pulled back through every effect, last first.
pub fn chain_roi(effects: &[(Kernel, &EvaluatedEffect)], wanted: Rect, layer: Rect) -> Rect {
    effects
        .iter()
        .rev()
        .fold(wanted, |r, (k, fx)| k.roi(fx, r, layer))
}
