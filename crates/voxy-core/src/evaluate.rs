//! Evaluation: resolve a composition at one point in time.
//!
//! This is the first stage of the render pipeline (design proposal §4.1). It
//! is pure and deterministic: the same document, composition and time always
//! produce the same [`EvaluatedComp`].

use std::collections::HashMap;
use std::sync::Arc;

use glam::{DAffine2, DVec2};

use crate::asset::AssetTable;
use crate::color::{LinearRgba, WorkingSpace};
use crate::comp::Composition;
use crate::document::Document;
use crate::id::{AssetId, CompId, LayerId};
use crate::layer::{BlendMode, Layer, LayerKind, Transform};
use crate::time::Time;

/// A composition with every property resolved at one time.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedComp {
    pub comp: CompId,
    pub time: Time,
    pub width: u32,
    pub height: u32,
    pub working_space: WorkingSpace,
    pub background: LinearRgba,
    /// Active layers in paint order: bottom first.
    pub layers: Vec<EvaluatedLayer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedLayer {
    pub id: LayerId,
    pub content: EvaluatedContent,
    /// Maps layer-local pixels to composition pixels, parenting included.
    pub world: DAffine2,
    /// Layer opacity in `0..=1` (not inherited from parents).
    pub opacity: f64,
    pub blend: BlendMode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EvaluatedContent {
    /// A rectangle from the layer-local origin to `size`.
    Solid { color: LinearRgba, size: DVec2 },
    /// An image drawn at native size from the layer-local origin.
    Image {
        asset: AssetId,
        /// Size recorded with the asset, if any; the renderer uses the decoded
        /// image's size.
        size: Option<DVec2>,
    },
    /// A video frame drawn at native size from the layer-local origin.
    Video {
        asset: AssetId,
        /// The source time to show.
        source_time: Time,
        /// Size recorded with the asset, if any; the renderer uses the
        /// decoded frame's size.
        size: Option<DVec2>,
    },
    /// A nested composition at `child_time` (see [`Layer::child_time`]),
    /// drawn at its own size from the layer-local origin.
    Precomp {
        comp: CompId,
        child_time: Time,
        size: DVec2,
        /// "Collapse transformations": the child's layers are drawn as if
        /// they were part of the parent, instead of through an offscreen
        /// picture of the child.
        collapse: bool,
        /// The child, evaluated at `child_time`.
        nested: Nested,
    },
    /// A placeholder for a removed source, at its remembered size.
    Placeholder { size: Option<DVec2> },
    /// An adjustment layer: it draws nothing itself, and its effects apply
    /// to the layers below it (rendered from Milestone E-D).
    Adjustment,
}

/// The deepest chain of nested compositions that is evaluated: the root and
/// 15 levels of precomps. A longer chain (or a cycle that slipped past
/// validation) draws nothing for the layer that would go deeper.
pub const MAX_NESTING: usize = 16;

/// What a precomp layer's child evaluated to.
#[derive(Debug, Clone, PartialEq)]
pub enum Nested {
    /// The child at the layer's child time. Shared: a composition used by
    /// several layers at the same child time is evaluated once.
    Shown(Arc<EvaluatedComp>),
    /// The child time is outside the child's duration: nothing to show.
    OutsideChild,
    /// Too deep, or the composition contains itself: nothing is drawn.
    TooDeep,
    /// The composition no longer exists.
    Missing,
}

/// Evaluates `comp` at `time`. Returns `None` if the composition is missing.
pub fn evaluate(doc: &Document, comp: CompId, time: Time) -> Option<EvaluatedComp> {
    let mut scope = Scope {
        chain: vec![comp],
        done: HashMap::new(),
    };
    evaluate_in(doc, comp, time, &mut scope)
}

/// What one `evaluate` call is in the middle of.
struct Scope {
    /// The compositions being evaluated, outermost first.
    chain: Vec<CompId>,
    /// Compositions already evaluated in this call, by composition, child
    /// time and depth. Without this, a composition used by several layers
    /// (or a diamond of them) would be evaluated once per use, which grows
    /// exponentially with the nesting. The depth is part of the key because
    /// it decides where [`MAX_NESTING`] cuts.
    done: HashMap<(CompId, Time, usize), Arc<EvaluatedComp>>,
}

fn evaluate_in(
    doc: &Document,
    comp: CompId,
    time: Time,
    scope: &mut Scope,
) -> Option<EvaluatedComp> {
    let c = doc.comp(comp)?;
    let background = c
        .background
        .value_at(time)
        .as_color()
        .unwrap_or(LinearRgba::BLACK);
    let mut worlds = WorldCache::new(c, time);
    let layers = c
        .layers
        .iter()
        .rev()
        .filter(|l| l.is_active_at(time))
        .map(|l| EvaluatedLayer {
            id: l.id,
            content: content(doc, l, time, scope),
            world: worlds.world(l.id),
            opacity: scalar(&l.transform.opacity, time, 1.0).clamp(0.0, 1.0),
            blend: l.blend_mode,
        })
        .collect();
    Some(EvaluatedComp {
        comp,
        time,
        width: c.width,
        height: c.height,
        working_space: doc.meta.working_space,
        background,
        layers,
    })
}

fn content(doc: &Document, layer: &Layer, time: Time, scope: &mut Scope) -> EvaluatedContent {
    let assets = &doc.assets;
    match &layer.kind {
        LayerKind::Solid { color, size } => EvaluatedContent::Solid {
            color: color.value_at(time).as_color().unwrap_or(LinearRgba::WHITE),
            size: size.value_at(time).as_vec2().unwrap_or(DVec2::ZERO),
        },
        LayerKind::Image { asset } => EvaluatedContent::Image {
            asset: *asset,
            size: asset_size(assets, *asset),
        },
        LayerKind::Video { asset } => EvaluatedContent::Video {
            asset: *asset,
            source_time: layer.source_time(time),
            size: asset_size(assets, *asset),
        },
        LayerKind::Precomp { comp, collapse, .. } => {
            let child = doc.comp(*comp);
            let duration = child.map_or(Time::ZERO, |c| c.duration);
            let child_time = layer.child_time(time, duration).unwrap_or(Time::ZERO);
            let nested = match child {
                None => Nested::Missing,
                Some(_) if scope.chain.contains(comp) || scope.chain.len() >= MAX_NESTING => {
                    Nested::TooDeep
                }
                Some(_) if child_time < Time::ZERO || child_time >= duration => {
                    Nested::OutsideChild
                }
                Some(_) => {
                    let key = (*comp, child_time, scope.chain.len());
                    if let Some(done) = scope.done.get(&key) {
                        Nested::Shown(Arc::clone(done))
                    } else {
                        scope.chain.push(*comp);
                        let eval = evaluate_in(doc, *comp, child_time, scope);
                        scope.chain.pop();
                        match eval {
                            Some(e) => {
                                let shared = Arc::new(e);
                                scope.done.insert(key, Arc::clone(&shared));
                                Nested::Shown(shared)
                            }
                            None => Nested::Missing,
                        }
                    }
                }
            };
            EvaluatedContent::Precomp {
                comp: *comp,
                child_time,
                size: child.map_or(DVec2::ZERO, |c| {
                    DVec2::new(f64::from(c.width), f64::from(c.height))
                }),
                collapse: *collapse,
                nested,
            }
        }
        LayerKind::Adjustment => EvaluatedContent::Adjustment,
        LayerKind::Placeholder { was } => EvaluatedContent::Placeholder {
            size: was
                .pixel_size
                .map(|[w, h]| DVec2::new(f64::from(w), f64::from(h))),
        },
    }
}

fn asset_size(assets: &AssetTable, asset: AssetId) -> Option<DVec2> {
    assets
        .get(asset)
        .and_then(|a| a.pixel_size)
        .map(|[w, h]| DVec2::new(f64::from(w), f64::from(h)))
}

fn scalar(p: &crate::Property, time: Time, fallback: f64) -> f64 {
    p.value_at(time).as_scalar().unwrap_or(fallback)
}

fn vec2(p: &crate::Property, time: Time, fallback: DVec2) -> DVec2 {
    p.value_at(time).as_vec2().unwrap_or(fallback)
}

/// The layer-to-parent matrix `T(position) · R(rotation) · S(scale) · T(−anchor)`.
pub fn local_matrix(t: &Transform, time: Time) -> DAffine2 {
    let anchor = vec2(&t.anchor, time, DVec2::ZERO);
    let position = vec2(&t.position, time, DVec2::ZERO);
    let scale = vec2(&t.scale, time, DVec2::ONE);
    let rotation = scalar(&t.rotation, time, 0.0).to_radians();
    DAffine2::from_scale_angle_translation(scale, rotation, position)
        * DAffine2::from_translation(-anchor)
}

/// The matrix mapping layer `id`'s local pixels to composition pixels at
/// `time`, parenting included. Identity if the layer is missing.
pub fn layer_world(comp: &Composition, id: LayerId, time: Time) -> DAffine2 {
    WorldCache::new(comp, time).world(id)
}

/// Memoized world matrices for one composition at one time.
struct WorldCache<'a> {
    comp: &'a Composition,
    time: Time,
    cache: HashMap<LayerId, DAffine2>,
}

impl<'a> WorldCache<'a> {
    fn new(comp: &'a Composition, time: Time) -> Self {
        WorldCache {
            comp,
            time,
            cache: HashMap::new(),
        }
    }

    fn world(&mut self, id: LayerId) -> DAffine2 {
        // Collect the parent chain iteratively; validation forbids cycles,
        // and the length bound keeps a corrupt document from looping.
        let mut chain = Vec::new();
        let mut cur = Some(id);
        while let Some(l) = cur {
            if self.cache.contains_key(&l) || chain.len() > self.comp.layers.len() {
                break;
            }
            chain.push(l);
            cur = self.comp.layer(l).and_then(|layer| layer.parent);
        }
        let mut acc = cur
            .and_then(|l| self.cache.get(&l).copied())
            .unwrap_or(DAffine2::IDENTITY);
        for &l in chain.iter().rev() {
            if let Some(layer) = self.comp.layer(l) {
                acc *= local_matrix(&layer.transform, self.time);
            }
            self.cache.insert(l, acc);
        }
        self.cache.get(&id).copied().unwrap_or(DAffine2::IDENTITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{AddSolidLayer, Command, SetLayerVisible, SetProperty};
    use crate::document::CompSettings;
    use crate::value::Value;

    fn doc_with_two_solids() -> Document {
        let mut doc = Document::with_comp(
            "Eval",
            CompSettings {
                width: 200,
                height: 100,
                ..CompSettings::default()
            },
        );
        let comp = doc.main_comp.unwrap();
        for color in [LinearRgba::WHITE, LinearRgba::BLACK] {
            AddSolidLayer {
                comp,
                name: None,
                color,
                index: 0,
            }
            .apply(&mut doc)
            .unwrap();
        }
        doc
    }

    #[test]
    fn paint_order_is_bottom_first() {
        let doc = doc_with_two_solids();
        let e = evaluate(&doc, doc.main_comp.unwrap(), Time::ZERO).unwrap();
        assert_eq!(e.layers.len(), 2);
        assert_eq!(e.layers[0].id, doc.main().unwrap().layers[1].id);
        assert_eq!(e.layers[1].id, doc.main().unwrap().layers[0].id);
    }

    #[test]
    fn default_solid_covers_comp() {
        let doc = doc_with_two_solids();
        let e = evaluate(&doc, doc.main_comp.unwrap(), Time::ZERO).unwrap();
        let l = &e.layers[0];
        assert_eq!(l.world.transform_point2(DVec2::ZERO), DVec2::ZERO);
        assert_eq!(
            l.world.transform_point2(DVec2::new(200.0, 100.0)),
            DVec2::new(200.0, 100.0)
        );
    }

    #[test]
    fn transform_order_and_parenting() {
        let mut doc = doc_with_two_solids();
        let parent = doc.main().unwrap().layers[1].clone();
        let child_id = doc.main().unwrap().layers[0].id;
        // Parent: rotate 90° clockwise about its anchor (the comp center).
        SetProperty {
            time: Time::ZERO,
            prop: parent.transform.rotation.id,
            value: Value::Scalar(90.0),
        }
        .apply(&mut doc)
        .unwrap();
        doc.layer_mut(child_id).unwrap().parent = Some(parent.id);
        let e = evaluate(&doc, doc.main_comp.unwrap(), Time::ZERO).unwrap();
        let p = e.layers.iter().find(|l| l.id == parent.id).unwrap();
        // Top-left corner (0,0) is 100 px left and 50 px above the anchor;
        // rotating 90° clockwise in y-down space moves it to 50 px right and
        // 100 px above.
        let tl = p.world.transform_point2(DVec2::ZERO);
        assert!((tl - DVec2::new(150.0, -50.0)).length() < 1e-9, "{tl}");
        // The child's own local matrix is the identity (anchor == position),
        // so its world matrix is exactly the parent's.
        let c = e.layers.iter().find(|l| l.id == child_id).unwrap();
        assert!((c.world.transform_point2(DVec2::ZERO) - tl).length() < 1e-9);
        // Moving the child moves it in the parent's rotated frame: +x local
        // becomes +y on screen.
        SetProperty {
            time: Time::ZERO,
            prop: doc.main().unwrap().layers[0].transform.position.id,
            value: Value::Vec2(DVec2::new(110.0, 50.0)),
        }
        .apply(&mut doc)
        .unwrap();
        let e = evaluate(&doc, doc.main_comp.unwrap(), Time::ZERO).unwrap();
        let c = e.layers.iter().find(|l| l.id == child_id).unwrap();
        let moved = c.world.transform_point2(DVec2::ZERO);
        assert!(
            (moved - (tl + DVec2::new(0.0, 10.0))).length() < 1e-9,
            "{moved}"
        );
    }

    #[test]
    fn hidden_and_out_of_range_layers_are_skipped() {
        let mut doc = doc_with_two_solids();
        let top = doc.main().unwrap().layers[0].id;
        SetLayerVisible {
            layer: top,
            visible: false,
        }
        .apply(&mut doc)
        .unwrap();
        let e = evaluate(&doc, doc.main_comp.unwrap(), Time::ZERO).unwrap();
        assert_eq!(e.layers.len(), 1);
        let end = doc.main().unwrap().duration;
        assert_eq!(
            evaluate(&doc, doc.main_comp.unwrap(), end).unwrap().layers,
            []
        );
    }

    #[test]
    fn evaluation_is_deterministic() {
        let doc = doc_with_two_solids();
        let a = evaluate(&doc, doc.main_comp.unwrap(), Time::from_millis(500));
        let b = evaluate(&doc, doc.main_comp.unwrap(), Time::from_millis(500));
        assert_eq!(a, b);
    }

    #[test]
    fn video_layers_map_composition_time_to_source_time() {
        use crate::command::{AddVideoLayer, SetLayerTiming};
        let mut doc = Document::with_comp("Video", CompSettings::default());
        let comp = doc.main_comp.unwrap();
        AddVideoLayer {
            comp,
            path: "clip.mp4".into(),
            pixel_size: [64, 32],
            duration: None,
            frame_rate: None,
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let layer = doc.main().unwrap().layers[0].id;
        // In at 1 s, showing the source from 0.5 s.
        SetLayerTiming {
            layer,
            in_point: Time::from_secs(1),
            out_point: Time::from_secs(3),
            start_offset: Time::from_millis(500),
        }
        .apply(&mut doc)
        .unwrap();
        assert_eq!(
            evaluate(&doc, comp, Time::from_millis(999)).unwrap().layers,
            []
        );
        let eval = evaluate(&doc, comp, Time::from_millis(1250)).unwrap();
        let EvaluatedContent::Video {
            source_time, size, ..
        } = eval.layers[0].content
        else {
            panic!("not a video layer");
        };
        assert_eq!(source_time, Time::from_millis(750));
        assert_eq!(size, Some(DVec2::new(64.0, 32.0)));
    }
}
