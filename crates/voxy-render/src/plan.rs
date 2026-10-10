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

use std::sync::Arc;

use voxy_core::glam::{DAffine2, DVec2};
use voxy_core::{AssetId, EvaluatedComp, EvaluatedContent, LinearRgba, Nested, Time, WorkingSpace};

/// Settings that differ between preview and export. Everything else is
/// shared, so a full-quality preview frame equals the exported frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSettings {
    /// Output resolution relative to the composition (1.0 = full size).
    pub scale: f64,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings { scale: 1.0 }
    }
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
struct Builder {
    settings: RenderSettings,
    space: WorkingSpace,
    draws: Vec<DrawOp>,
    skipped: usize,
}

impl Builder {
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
            let opacity = effective as f32;
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
                    Nested::Shown(child) if *collapse => {
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
                        let plan = RenderPlan::nested(child, self.settings);
                        self.skipped += plan.skipped_precomps;
                        (
                            DrawSource::Comp {
                                plan: Arc::new(plan),
                                opacity,
                            },
                            Some(*size),
                        )
                    }
                    Nested::TooDeep => {
                        self.skipped += 1;
                        continue;
                    }
                    Nested::OutsideChild | Nested::Missing => continue,
                },
            };
            self.draws.push(DrawOp {
                source,
                local_size,
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
        )
    }

    /// A nested composition's plan: transparent where the child draws
    /// nothing. As in After Effects, a composition's background colour shows
    /// only when it is viewed or exported directly, never when nested.
    fn nested(eval: &EvaluatedComp, settings: RenderSettings) -> RenderPlan {
        Self::build_with(eval, settings, [0.0; 4])
    }

    fn build_with(eval: &EvaluatedComp, settings: RenderSettings, clear: [f64; 4]) -> RenderPlan {
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
        };
        b.layers(eval, output_from_comp, 1.0);
        RenderPlan {
            width,
            height,
            working_space: eval.working_space,
            clear,
            draws: b.draws,
            skipped_precomps: b.skipped,
        }
    }

    /// A stable 64-bit hash of everything that decides this plan's pixels:
    /// size, working space, clear colour and every draw (sources, matrices,
    /// colours as exact bits, video frame identity), including nested plans.
    /// Two plans with equal keys render identically, so a nested composition
    /// that resolves the same at two times shares one cached picture, and any
    /// change to settings changes the key (no stale frame is possible).
    pub fn content_key(&self) -> u64 {
        let mut h = Fnv::new();
        self.hash_into(&mut h);
        h.0
    }

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
            match &d.source {
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
                    plan.hash_into(h);
                }
            }
            match d.local_size {
                Some(s) => {
                    h.u32(1);
                    h.f64(s.x);
                    h.f64(s.y);
                }
                None => h.u32(0),
            }
            let m = d.output_from_local;
            for v in m.to_cols_array() {
                h.f64(v);
            }
        }
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
        let plan = RenderPlan::build(&eval, &RenderSettings { scale: 0.5 });
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
}
