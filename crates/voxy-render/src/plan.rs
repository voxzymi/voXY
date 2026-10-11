//! Render planning: turn an [`EvaluatedComp`] into GPU-ready draw operations.
//!
//! This is the `build_graph` stage of the pipeline (design proposal §4.1). In
//! Milestone 1 the graph is a single chain: clear → one composite per layer →
//! output transform. A nested composition is a sub-plan rendered offscreen
//! (a tree of plans, each with a content hash for caching); effects, masks
//! and mattes (Milestones 4–5) will add more node kinds.
//!
//! Planning is pure CPU work in `f64`, rounded to `f32` only at the end, so it
//! is deterministic and testable without a GPU.

use std::collections::HashMap;
use std::sync::Arc;

use voxy_core::glam::{DAffine2, DVec2};
use voxy_core::{
    AssetId, BlendMode, EvaluatedComp, EvaluatedContent, EvaluatedEffect, LinearRgba, Nested, Time,
    Value, WorkingSpace,
};

use crate::kernels::Kernel;

/// Settings that differ between preview and export. Everything else is
/// shared, so a full-quality preview frame equals the exported frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSettings {
    /// Output resolution relative to the composition (1.0 = full size).
    pub scale: f64,
    /// Which effects are drawn. Preview only: export always uses
    /// [`EffectsView::All`] (design §4.2, solo and before/after).
    pub effects: EffectsView,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            scale: 1.0,
            effects: EffectsView::All,
        }
    }
}

impl RenderSettings {
    /// Full-quality settings at `scale`.
    pub fn at_scale(scale: f64) -> Self {
        RenderSettings {
            scale,
            ..RenderSettings::default()
        }
    }
}

/// Which effects a preview draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EffectsView {
    /// Every enabled effect (always for export).
    #[default]
    All,
    /// No effects at all: the "before" side of the before/after split.
    Bypass,
    /// Only this effect on its layer, the layer's other effects bypassed.
    /// Other layers draw as usual.
    Solo(voxy_core::EffectId),
}

impl RenderSettings {
    /// Output size in pixels for a composition size (never zero).
    pub fn output_size(&self, width: u32, height: u32) -> (u32, u32) {
        let s = |v: u32| ((f64::from(v) * self.scale).round() as u32).max(1);
        (s(width), s(height))
    }
}

/// What a draw fills its quad with.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawSource {
    /// Premultiplied color, already encoded for the working space, with
    /// layer opacity applied.
    Solid([f32; 4]),
    /// An image asset; the shader samples, premultiplies and applies `opacity`.
    Image { asset: AssetId, opacity: f32 },
    /// The frame of a video asset at source time `time`, drawn like an image.
    Video {
        asset: AssetId,
        time: Time,
        opacity: f32,
    },
    /// A nested composition: `plan` is rendered offscreen (transparent
    /// background, at the child's size times the preview scale), then drawn
    /// like an image through the layer's transform with `opacity`.
    Comp { plan: Arc<RenderPlan>, opacity: f32 },
    /// A layer with effects: `chain` renders its source and effects in
    /// layer-local space, and the result is drawn through the layer's
    /// transform with `opacity` (design-effects-ofx.md §3.1).
    Chain { chain: Arc<ChainPlan>, opacity: f32 },
    /// An adjustment layer (design §1.5): `chain`'s effects run on the
    /// composite of everything drawn before it (its source is a stand-in),
    /// over the whole composition, and the result replaces what was
    /// there, mixed by `opacity` and the draw's blend mode.
    Adjustment { chain: Arc<ChainPlan>, opacity: f32 },
}

impl DrawSource {
    /// The same source drawn with its opacity multiplied by `factor`.
    #[must_use]
    pub fn with_opacity(&self, factor: f32) -> DrawSource {
        match self {
            DrawSource::Solid(c) => DrawSource::Solid(c.map(|v| v * factor)),
            DrawSource::Image { asset, opacity } => DrawSource::Image {
                asset: *asset,
                opacity: opacity * factor,
            },
            DrawSource::Video {
                asset,
                time,
                opacity,
            } => DrawSource::Video {
                asset: *asset,
                time: *time,
                opacity: opacity * factor,
            },
            DrawSource::Comp { plan, opacity } => DrawSource::Comp {
                plan: Arc::clone(plan),
                opacity: opacity * factor,
            },
            DrawSource::Adjustment { chain, opacity } => DrawSource::Adjustment {
                chain: Arc::clone(chain),
                opacity: opacity * factor,
            },
            DrawSource::Chain { chain, opacity } => DrawSource::Chain {
                chain: Arc::clone(chain),
                opacity: opacity * factor,
            },
        }
    }
}

/// One effect of a chain: the kernel that runs it and its parameters at the
/// frame time.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectNode {
    pub kernel: Kernel,
    pub effect: EvaluatedEffect,
    /// The layer part of a seeded kernel's seed (its layer's ID; design
    /// §3.4), and 0 for every other kernel, so identical unseeded chains
    /// on different layers share one rendered result.
    pub seed: u64,
}

/// A layer's source and effect stack, rendered in layer-local pixels at the
/// preview scale before the layer's transform applies.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainPlan {
    /// The layer's content at full opacity (never a `Chain`).
    pub source: DrawSource,
    /// Size of the source in layer pixels; `None` for images and video
    /// means the decoded size.
    pub source_size: Option<DVec2>,
    /// In application order.
    pub effects: Vec<EffectNode>,
    /// Preview resolution scale.
    pub scale: f64,
    pub working_space: WorkingSpace,
    /// [`ChainPlan::content_key`].
    key: u64,
}

impl ChainPlan {
    pub fn new(
        source: DrawSource,
        source_size: Option<DVec2>,
        effects: Vec<EffectNode>,
        scale: f64,
        working_space: WorkingSpace,
    ) -> Self {
        let mut chain = ChainPlan {
            source,
            source_size,
            effects,
            scale,
            working_space,
            key: 0,
        };
        let mut h = Fnv::new();
        chain.hash_into(&mut h);
        chain.key = h.0;
        chain
    }

    /// A stable hash of everything that decides the chain's pixels: the
    /// source, every effect's type, version and evaluated parameters (as
    /// exact bits), the scale and the working space (design §3.3). It does
    /// not depend on where the layer is placed, so moving a static layer
    /// with static effects reuses its rendered chain.
    pub fn content_key(&self) -> u64 {
        self.key
    }

    fn hash_into(&self, h: &mut Fnv) {
        h.u32(0xC4A1);
        hash_source(&self.source, h);
        hash_size(self.source_size, h);
        h.u64(self.effects.len() as u64);
        for node in &self.effects {
            let fx = &node.effect;
            h.str(fx.type_id.as_str());
            h.u32(fx.version);
            h.u64(fx.params.len() as u64);
            for (name, value) in &fx.params {
                h.str(name);
                h.value(value);
            }
            match fx.matte {
                Some(m) => {
                    h.u32(1);
                    h.u64(m.layer.raw());
                    h.u32(u32::from(m.invert) | (m.channel as u32) << 1);
                }
                None => h.u32(0),
            }
            // Zero unless the kernel uses them (see the planner).
            h.u64(fx.frame as u64);
            h.f64(fx.layer_progress);
            h.u64(node.seed);
        }
        h.f64(self.scale);
        h.u32(match self.working_space {
            WorkingSpace::Linear => 0,
            WorkingSpace::SrgbCompat => 1,
        });
    }
}

/// One layer composited with premultiplied "over".
#[derive(Debug, Clone, PartialEq)]
pub struct DrawOp {
    pub source: DrawSource,
    /// Quad size in layer-local pixels. `None` for images and video means
    /// "use the decoded size".
    pub local_size: Option<DVec2>,
    /// Maps layer-local pixels to output pixels (resolution scale applied).
    pub output_from_local: DAffine2,
    /// How the layer combines with what is below it. Normal uses the
    /// fixed-function "over"; every other mode the blend shader.
    pub blend: BlendMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderPlan {
    pub width: u32,
    pub height: u32,
    pub working_space: WorkingSpace,
    /// Premultiplied background, encoded for the working space.
    pub clear: [f64; 4],
    /// Paint order: first is bottom.
    pub draws: Vec<DrawOp>,
    /// Precomp layers left out because the nesting was too deep or went in a
    /// circle (validation should make this unreachable). Counts the whole
    /// tree.
    pub skipped_precomps: usize,
    /// Effects passed through because this build has no kernel for them yet
    /// (whole tree).
    pub unsupported_effects: usize,
    /// Placeholder effects passed through (whole tree).
    pub missing_effects: usize,
    /// [`RenderPlan::content_key`], computed once when the plan is built (a
    /// nested plan is hashed by its own key, so hashing is not repeated down
    /// the tree).
    key: u64,
}

/// Fill of a placeholder layer (a removed source): mid grey.
pub const PLACEHOLDER_COLOR: LinearRgba = LinearRgba {
    r: 0.18,
    g: 0.18,
    b: 0.18,
    a: 1.0,
};

/// Encodes a linear straight-alpha color for the working space and
/// premultiplies it.
pub fn working_premultiplied(color: LinearRgba, space: WorkingSpace) -> [f32; 4] {
    let [r, g, b, a] = match space {
        WorkingSpace::Linear => color.to_array(),
        WorkingSpace::SrgbCompat => color.to_srgb(),
    };
    [r * a, g * a, b * a, a]
}

/// Collects the draws of one plan.
struct Builder<'a> {
    settings: RenderSettings,
    space: WorkingSpace,
    draws: Vec<DrawOp>,
    skipped: usize,
    unsupported: usize,
    missing: usize,
    /// Plans already built for a shared evaluated child (by its address), so
    /// a composition used by several layers is planned once.
    memo: &'a mut HashMap<usize, Arc<RenderPlan>>,
}

impl Builder<'_> {
    /// Adds the draws for `eval`'s layers. `base` maps `eval`'s composition
    /// pixels to output pixels; `inherited` is the opacity of collapsed
    /// precomp layers above (1.0 at the top).
    fn layers(&mut self, eval: &EvaluatedComp, base: DAffine2, inherited: f64) {
        let space = self.space;
        for l in &eval.layers {
            let effective = l.opacity * inherited;
            if effective <= 0.0 {
                continue;
            }
            self.missing += l.missing_effects;
            let mut nodes = Vec::new();
            let solo = match self.settings.effects {
                EffectsView::Solo(id) if l.effects.iter().any(|fx| fx.id == id) => Some(id),
                _ => None,
            };
            for fx in &l.effects {
                if self.settings.effects == EffectsView::Bypass
                    || solo.is_some_and(|id| id != fx.id)
                {
                    continue;
                }
                match Kernel::find(&fx.type_id, fx.version) {
                    Some(kernel) => {
                        let mut effect = fx.clone();
                        // A kernel without seeds must not tell frames apart,
                        // or a static chain would never be reused.
                        if !kernel.uses_frame(fx) {
                            effect.frame = 0;
                        }
                        // Likewise the layer's progress, used only by Auto.
                        if !kernel.uses_layer_progress(fx) {
                            effect.layer_progress = 0.0;
                        }
                        let seed = if kernel.seeded() { l.id.raw() } else { 0 };
                        nodes.push(EffectNode {
                            kernel,
                            effect,
                            seed,
                        });
                    }
                    None => self.unsupported += 1,
                }
            }
            let chain = !nodes.is_empty();
            // With effects, the source is rendered at full opacity; the
            // layer's opacity applies after them.
            let layer_opacity = effective as f32;
            let opacity = if chain { 1.0 } else { layer_opacity };
            let (source, local_size) = match &l.content {
                EvaluatedContent::Solid { color, size } => {
                    if size.x <= 0.0 || size.y <= 0.0 {
                        continue;
                    }
                    let c = working_premultiplied(*color, space).map(|v| v * opacity);
                    (DrawSource::Solid(c), Some(*size))
                }
                EvaluatedContent::Image { asset, size } => (
                    DrawSource::Image {
                        asset: *asset,
                        opacity,
                    },
                    *size,
                ),
                EvaluatedContent::Video {
                    asset,
                    source_time,
                    size,
                } => (
                    DrawSource::Video {
                        asset: *asset,
                        time: *source_time,
                        opacity,
                    },
                    *size,
                ),
                EvaluatedContent::Placeholder { size } => {
                    let Some(size) = (*size).filter(|s| s.x > 0.0 && s.y > 0.0) else {
                        continue;
                    };
                    let c = working_premultiplied(PLACEHOLDER_COLOR, space).map(|v| v * opacity);
                    (DrawSource::Solid(c), Some(size))
                }
                EvaluatedContent::Precomp {
                    size,
                    collapse,
                    nested,
                    ..
                } => match nested {
                    // Effects need the child as one picture, so a collapsed
                    // precomp with effects renders offscreen like any other.
                    Nested::Shown(child) if *collapse && !chain => {
                        // The child's layers join this plan, through the
                        // precomp layer's transform; its background is not
                        // drawn and content outside its bounds stays visible.
                        self.layers(child, base * l.world, effective);
                        continue;
                    }
                    Nested::Shown(child) => {
                        if size.x <= 0.0 || size.y <= 0.0 {
                            continue;
                        }
                        let at = Arc::as_ptr(child) as usize;
                        let plan = if let Some(done) = self.memo.get(&at) {
                            Arc::clone(done)
                        } else {
                            let built = Arc::new(RenderPlan::build_with(
                                child,
                                self.settings,
                                [0.0; 4],
                                self.memo,
                            ));
                            self.memo.insert(at, Arc::clone(&built));
                            built
                        };
                        self.skipped += plan.skipped_precomps;
                        self.unsupported += plan.unsupported_effects;
                        self.missing += plan.missing_effects;
                        (DrawSource::Comp { plan, opacity }, Some(*size))
                    }
                    Nested::TooDeep => {
                        self.skipped += 1;
                        continue;
                    }
                    Nested::OutsideChild | Nested::Missing => continue,
                },
                // Its effects apply to what is below, over the whole
                // composition; without effects it does nothing.
                EvaluatedContent::Adjustment => {
                    if !chain {
                        continue;
                    }
                    let comp = DVec2::new(f64::from(eval.width), f64::from(eval.height));
                    self.draws.push(DrawOp {
                        source: DrawSource::Adjustment {
                            chain: Arc::new(ChainPlan::new(
                                DrawSource::Solid([0.0; 4]),
                                Some(comp),
                                nodes,
                                self.settings.scale,
                                space,
                            )),
                            opacity: layer_opacity,
                        },
                        local_size: Some(comp),
                        blend: l.blend,
                        output_from_local: base,
                    });
                    continue;
                }
            };
            let source = if chain {
                DrawSource::Chain {
                    chain: Arc::new(ChainPlan::new(
                        source,
                        local_size,
                        nodes,
                        self.settings.scale,
                        space,
                    )),
                    opacity: layer_opacity,
                }
            } else {
                source
            };
            self.draws.push(DrawOp {
                source,
                local_size,
                blend: l.blend,
                output_from_local: base * l.world,
            });
        }
    }
}

impl RenderPlan {
    pub fn build(eval: &EvaluatedComp, settings: &RenderSettings) -> RenderPlan {
        Self::build_with(
            eval,
            *settings,
            working_premultiplied(eval.background, eval.working_space).map(f64::from),
            &mut HashMap::new(),
        )
    }

    /// A plan for `eval`. A nested composition's plan has a transparent
    /// `clear`: as in After Effects, a composition's background colour shows
    /// only when it is viewed or exported directly, never when nested.
    fn build_with(
        eval: &EvaluatedComp,
        settings: RenderSettings,
        clear: [f64; 4],
        memo: &mut HashMap<usize, Arc<RenderPlan>>,
    ) -> RenderPlan {
        let (width, height) = settings.output_size(eval.width, eval.height);
        let output_from_comp = DAffine2::from_scale(DVec2::new(
            f64::from(width) / f64::from(eval.width),
            f64::from(height) / f64::from(eval.height),
        ));
        let mut b = Builder {
            settings,
            space: eval.working_space,
            draws: Vec::new(),
            skipped: 0,
            unsupported: 0,
            missing: 0,
            memo,
        };
        b.layers(eval, output_from_comp, 1.0);
        let mut plan = RenderPlan {
            width,
            height,
            working_space: eval.working_space,
            clear,
            draws: b.draws,
            skipped_precomps: b.skipped,
            unsupported_effects: b.unsupported,
            missing_effects: b.missing,
            key: 0,
        };
        plan.key = plan.compute_key();
        plan
    }

    /// The image and video assets this plan and its nested plans draw.
    pub fn assets(&self) -> Vec<AssetId> {
        let mut out = Vec::new();
        self.collect_assets(&mut out);
        out.sort_unstable();
        out.dedup();
        out
    }

    fn collect_assets(&self, out: &mut Vec<AssetId>) {
        for d in &self.draws {
            collect_source_assets(&d.source, out);
        }
    }

    /// A stable 64-bit hash of everything that decides this plan's pixels:
    /// size, working space, clear colour and every draw (sources, matrices,
    /// colours as exact bits, video frame identity), including nested plans.
    /// Two plans with equal keys render identically, so a nested composition
    /// that resolves the same at two times shares one cached picture, and any
    /// change to settings changes the key (no stale frame is possible).
    pub fn content_key(&self) -> u64 {
        self.key
    }

    fn compute_key(&self) -> u64 {
        let mut h = Fnv::new();
        self.hash_into(&mut h);
        h.0
    }

    /// Hashes this plan's own content; nested plans by their stored keys.
    fn hash_into(&self, h: &mut Fnv) {
        h.u32(self.width);
        h.u32(self.height);
        h.u32(match self.working_space {
            WorkingSpace::Linear => 0,
            WorkingSpace::SrgbCompat => 1,
        });
        for c in self.clear {
            h.f64(c);
        }
        h.u64(self.draws.len() as u64);
        for d in &self.draws {
            hash_source(&d.source, h);
            hash_size(d.local_size, h);
            h.u32(crate::blend::mode_index(d.blend));
            let m = d.output_from_local;
            for v in m.to_cols_array() {
                h.f64(v);
            }
        }
    }
}

fn collect_source_assets(source: &DrawSource, out: &mut Vec<AssetId>) {
    match source {
        DrawSource::Image { asset, .. } | DrawSource::Video { asset, .. } => out.push(*asset),
        DrawSource::Comp { plan, .. } => plan.collect_assets(out),
        DrawSource::Chain { chain, .. } => collect_source_assets(&chain.source, out),
        DrawSource::Solid(_) | DrawSource::Adjustment { .. } => {}
    }
}

/// Hashes a draw source; nested plans and chains by their stored keys.
fn hash_source(source: &DrawSource, h: &mut Fnv) {
    match source {
        DrawSource::Solid(c) => {
            h.u32(1);
            for v in c {
                h.f32(*v);
            }
        }
        DrawSource::Image { asset, opacity } => {
            h.u32(2);
            h.u64(asset.raw());
            h.f32(*opacity);
        }
        DrawSource::Video {
            asset,
            time,
            opacity,
        } => {
            h.u32(3);
            h.u64(asset.raw());
            h.u64(time.flicks() as u64);
            h.f32(*opacity);
        }
        DrawSource::Comp { plan, opacity } => {
            h.u32(4);
            h.f32(*opacity);
            h.u64(plan.key);
        }
        DrawSource::Chain { chain, opacity } => {
            h.u32(5);
            h.f32(*opacity);
            h.u64(chain.key);
        }
        DrawSource::Adjustment { chain, opacity } => {
            h.u32(6);
            h.f32(*opacity);
            h.u64(chain.key);
        }
    }
}

fn hash_size(size: Option<DVec2>, h: &mut Fnv) {
    match size {
        Some(s) => {
            h.u32(1);
            h.f64(s.x);
            h.f64(s.y);
        }
        None => h.u32(0),
    }
}

/// 64-bit FNV-1a over canonical little-endian bytes.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }

    fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }

    /// Length-prefixed, so `("ab", "c")` and `("a", "bc")` differ.
    fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }

    /// A value with its kind, every float as exact bits.
    fn value(&mut self, v: &Value) {
        match v {
            Value::Bool(b) => {
                self.u32(1);
                self.u32(u32::from(*b));
            }
            Value::Int(i) => {
                self.u32(2);
                self.u64(*i as u64);
            }
            Value::Scalar(x) => {
                self.u32(3);
                self.f64(*x);
            }
            Value::Vec2(p) => {
                self.u32(4);
                self.f64(p.x);
                self.f64(p.y);
            }
            Value::Vec3(p) => {
                self.u32(5);
                self.f64(p.x);
                self.f64(p.y);
                self.f64(p.z);
            }
            Value::Color(c) => {
                self.u32(6);
                for x in c.to_array() {
                    self.f32(x);
                }
            }
            Value::Enum(e) => {
                self.u32(7);
                self.u32(*e);
            }
            Value::String(s) => {
                self.u32(8);
                self.str(s);
            }
            Value::Layer(l) => {
                self.u32(9);
                self.u64(l.map_or(u64::MAX, voxy_core::LayerId::raw));
            }
            Value::Curve(c) => {
                self.u32(10);
                self.u64(c.points().len() as u64);
                for p in c.points() {
                    self.f64(p.x);
                    self.f64(p.y);
                }
            }
            Value::Asset(a) => {
                self.u32(11);
                self.u64(a.map_or(u64::MAX, voxy_core::AssetId::raw));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxy_core::command::{AddSolidLayer, Command, SetProperty};
    use voxy_core::{CompSettings, Document, Time, Value, evaluate};

    #[test]
    fn plan_applies_scale_space_and_opacity() {
        let mut doc = Document::with_comp(
            "Plan",
            CompSettings {
                width: 100,
                height: 50,
                background: LinearRgba::new(0.5, 0.5, 0.5, 1.0),
                ..CompSettings::default()
            },
        );
        let comp = doc.main_comp.unwrap();
        AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::new(1.0, 0.0, 0.0, 1.0),
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let opacity = doc.main().unwrap().layers[0].transform.opacity.id;
        SetProperty {
            time: Time::ZERO,
            prop: opacity,
            value: Value::Scalar(0.5),
        }
        .apply(&mut doc)
        .unwrap();

        let eval = evaluate(&doc, comp, Time::ZERO).unwrap();
        let plan = RenderPlan::build(&eval, &RenderSettings::at_scale(0.5));
        assert_eq!((plan.width, plan.height), (50, 25));
        assert_eq!(plan.clear, [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(plan.draws.len(), 1);
        assert_eq!(
            plan.draws[0].source,
            DrawSource::Solid([0.5, 0.0, 0.0, 0.5])
        );
        let corner = plan.draws[0]
            .output_from_local
            .transform_point2(DVec2::new(100.0, 50.0));
        assert_eq!(corner, DVec2::new(50.0, 25.0));

        doc.meta.working_space = WorkingSpace::SrgbCompat;
        let eval = evaluate(&doc, comp, Time::ZERO).unwrap();
        let plan = RenderPlan::build(&eval, &RenderSettings::default());
        let encoded = voxy_core::linear_to_srgb(0.5);
        assert!((plan.clear[0] - f64::from(encoded)).abs() < 1e-6);
    }

    #[test]
    fn invisible_layers_are_dropped() {
        let mut doc = Document::with_comp("Plan", CompSettings::default());
        let comp = doc.main_comp.unwrap();
        AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let opacity = doc.main().unwrap().layers[0].transform.opacity.id;
        SetProperty {
            time: Time::ZERO,
            prop: opacity,
            value: Value::Scalar(0.0),
        }
        .apply(&mut doc)
        .unwrap();
        let eval = evaluate(&doc, comp, Time::ZERO).unwrap();
        assert_eq!(
            RenderPlan::build(&eval, &RenderSettings::default()).draws,
            []
        );
    }

    #[test]
    fn placeholders_draw_grey_and_a_precomp_becomes_a_nested_plan() {
        use voxy_core::PoolItem;
        use voxy_core::command::{
            AddImageLayer, AddSourceLayer, CreateComposition, RemoveItems, RemoveMode,
        };
        let mut doc = Document::with_comp("Plan", CompSettings::default());
        let comp = doc.main_comp.unwrap();
        AddImageLayer {
            comp,
            path: "a.png".into(),
            pixel_size: [40, 20],
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let asset = doc.main().unwrap().layers[0].kind.asset().unwrap();
        RemoveItems {
            items: vec![PoolItem::Asset(asset)],
            mode: RemoveMode::PlaceholderLayers,
        }
        .apply(&mut doc)
        .unwrap();
        CreateComposition {
            settings: CompSettings::default(),
            folder: None,
        }
        .apply(&mut doc)
        .unwrap();
        let child = *doc.comps.keys().last().unwrap();
        AddSourceLayer {
            comp,
            source: voxy_core::SourceRef::Comp(child),
            index: 0,
            in_point: Time::ZERO,
        }
        .apply(&mut doc)
        .unwrap();
        let eval = evaluate(&doc, comp, Time::ZERO).unwrap();
        let plan = RenderPlan::build(&eval, &RenderSettings::default());
        assert_eq!(plan.draws.len(), 2, "the placeholder and the precomp");
        // The precomp layer was added last, so it is on top.
        let DrawSource::Comp {
            plan: nested,
            opacity,
        } = &plan.draws[1].source
        else {
            panic!("{:?}", plan.draws[1].source)
        };
        assert_eq!(*opacity, 1.0);
        assert_eq!((nested.width, nested.height), (1920, 1080));
        assert_eq!(nested.clear, [0.0; 4], "a nested comp is transparent");
        assert_eq!(plan.draws[1].local_size, Some(DVec2::new(1920.0, 1080.0)));
        let grey = working_premultiplied(PLACEHOLDER_COLOR, WorkingSpace::Linear);
        assert_eq!(plan.draws[0].source, DrawSource::Solid(grey));
        assert_eq!(plan.draws[0].local_size, Some(DVec2::new(40.0, 20.0)));
    }

    #[test]
    fn a_composition_used_twice_is_planned_once_and_keys_follow_content() {
        use voxy_core::command::{AddSourceLayer, CreateComposition};
        let mut doc = Document::with_comp("Plan", CompSettings::default());
        let comp = doc.main_comp.unwrap();
        CreateComposition {
            settings: CompSettings::default(),
            folder: None,
        }
        .apply(&mut doc)
        .unwrap();
        let child = *doc.comps.keys().last().unwrap();
        AddSolidLayer {
            comp: child,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        for _ in 0..2 {
            AddSourceLayer {
                comp,
                source: voxy_core::SourceRef::Comp(child),
                index: 0,
                in_point: Time::ZERO,
            }
            .apply(&mut doc)
            .unwrap();
        }
        let eval = evaluate(&doc, comp, Time::ZERO).unwrap();
        let plan = RenderPlan::build(&eval, &RenderSettings::default());
        let [
            DrawSource::Comp { plan: a, .. },
            DrawSource::Comp { plan: b, .. },
        ] = [&plan.draws[0].source, &plan.draws[1].source]
        else {
            panic!("two nested draws")
        };
        assert!(Arc::ptr_eq(a, b), "one shared sub-plan");
        // The key is a function of content: equal plans share it, a change
        // (here the preview scale) does not.
        let again = RenderPlan::build(&eval, &RenderSettings::default());
        assert_eq!(plan.content_key(), again.content_key());
        assert_eq!(plan, again);
        let half = RenderPlan::build(&eval, &RenderSettings::at_scale(0.5));
        assert_ne!(plan.content_key(), half.content_key());
        assert_ne!(a.content_key(), plan.content_key());
        assert!(plan.assets().is_empty(), "solids have no assets");
    }
}
