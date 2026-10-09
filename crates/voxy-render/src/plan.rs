//! Render planning: turn an [`EvaluatedComp`] into GPU-ready draw operations.
//!
//! This is the `build_graph` stage of the pipeline (design proposal §4.1). In
//! Milestone 1 the graph is a single chain: clear → one composite per layer →
//! output transform. Effects, masks, mattes and precomps (Milestones 4–5) turn
//! it into a DAG with per-node content hashes for caching.
//!
//! Planning is pure CPU work in `f64`, rounded to `f32` only at the end, so it
//! is deterministic and testable without a GPU.

use voxy_core::glam::{DAffine2, DVec2};
use voxy_core::{AssetId, EvaluatedComp, EvaluatedContent, LinearRgba, Time, WorkingSpace};

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
#[derive(Debug, Clone, Copy, PartialEq)]
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

impl RenderPlan {
    pub fn build(eval: &EvaluatedComp, settings: &RenderSettings) -> RenderPlan {
        let (width, height) = settings.output_size(eval.width, eval.height);
        let output_from_comp = DAffine2::from_scale(DVec2::new(
            f64::from(width) / f64::from(eval.width),
            f64::from(height) / f64::from(eval.height),
        ));
        let space = eval.working_space;
        let clear = working_premultiplied(eval.background, space).map(f64::from);
        let draws = eval
            .layers
            .iter()
            .filter(|l| l.opacity > 0.0)
            .filter_map(|l| {
                let opacity = l.opacity as f32;
                let (source, local_size) = match &l.content {
                    EvaluatedContent::Solid { color, size } => {
                        if size.x <= 0.0 || size.y <= 0.0 {
                            return None;
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
                        let size = (*size).filter(|s| s.x > 0.0 && s.y > 0.0)?;
                        let c =
                            working_premultiplied(PLACEHOLDER_COLOR, space).map(|v| v * opacity);
                        (DrawSource::Solid(c), Some(size))
                    }
                    // Nested compositions render with Milestone E
                    // (docs/deferred.md); until then they draw nothing.
                    EvaluatedContent::Precomp { .. } => return None,
                };
                Some(DrawOp {
                    source,
                    local_size,
                    output_from_local: output_from_comp * l.world,
                })
            })
            .collect();
        RenderPlan {
            width,
            height,
            working_space: space,
            clear,
            draws,
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
    fn placeholders_draw_grey_and_precomps_wait_for_milestone_e() {
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
        assert_eq!(plan.draws.len(), 1, "the precomp draws nothing yet");
        let grey = working_premultiplied(PLACEHOLDER_COLOR, WorkingSpace::Linear);
        assert_eq!(plan.draws[0].source, DrawSource::Solid(grey));
        assert_eq!(plan.draws[0].local_size, Some(DVec2::new(40.0, 20.0)));
    }
}
