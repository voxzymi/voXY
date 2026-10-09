//! Evaluation: resolve a composition at one point in time.
//!
//! This is the first stage of the render pipeline (design proposal §4.1). It
//! is pure and deterministic: the same document, composition and time always
//! produce the same [`EvaluatedComp`].

use std::collections::HashMap;

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
}

/// Evaluates `comp` at `time`. Returns `None` if the composition is missing.
pub fn evaluate(doc: &Document, comp: CompId, time: Time) -> Option<EvaluatedComp> {
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
            content: content(l, time, &doc.assets),
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

fn content(layer: &Layer, time: Time, assets: &AssetTable) -> EvaluatedContent {
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
        let mut doc = Document::new(
            "Eval",
            CompSettings {
                width: 200,
                height: 100,
                ..CompSettings::default()
            },
        );
        let comp = doc.root_comp;
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
        let e = evaluate(&doc, doc.root_comp, Time::ZERO).unwrap();
        assert_eq!(e.layers.len(), 2);
        assert_eq!(e.layers[0].id, doc.root().layers[1].id);
        assert_eq!(e.layers[1].id, doc.root().layers[0].id);
    }

    #[test]
    fn default_solid_covers_comp() {
        let doc = doc_with_two_solids();
        let e = evaluate(&doc, doc.root_comp, Time::ZERO).unwrap();
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
        let parent = doc.root().layers[1].clone();
        let child_id = doc.root().layers[0].id;
        // Parent: rotate 90° clockwise about its anchor (the comp center).
        SetProperty {
            time: Time::ZERO,
            prop: parent.transform.rotation.id,
            value: Value::Scalar(90.0),
        }
        .apply(&mut doc)
        .unwrap();
        doc.layer_mut(child_id).unwrap().parent = Some(parent.id);
        let e = evaluate(&doc, doc.root_comp, Time::ZERO).unwrap();
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
            prop: doc.root().layers[0].transform.position.id,
            value: Value::Vec2(DVec2::new(110.0, 50.0)),
        }
        .apply(&mut doc)
        .unwrap();
        let e = evaluate(&doc, doc.root_comp, Time::ZERO).unwrap();
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
        let top = doc.root().layers[0].id;
        SetLayerVisible {
            layer: top,
            visible: false,
        }
        .apply(&mut doc)
        .unwrap();
        let e = evaluate(&doc, doc.root_comp, Time::ZERO).unwrap();
        assert_eq!(e.layers.len(), 1);
        let end = doc.root().duration;
        assert!(
            evaluate(&doc, doc.root_comp, end)
                .unwrap()
                .layers
                .is_empty()
        );
    }

    #[test]
    fn evaluation_is_deterministic() {
        let doc = doc_with_two_solids();
        let a = evaluate(&doc, doc.root_comp, Time::from_millis(500));
        let b = evaluate(&doc, doc.root_comp, Time::from_millis(500));
        assert_eq!(a, b);
    }

    #[test]
    fn video_layers_map_composition_time_to_source_time() {
        use crate::command::{AddVideoLayer, SetLayerTiming};
        let mut doc = Document::new("Video", CompSettings::default());
        let comp = doc.root_comp;
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
        let layer = doc.root().layers[0].id;
        // In at 1 s, showing the source from 0.5 s.
        SetLayerTiming {
            layer,
            in_point: Time::from_secs(1),
            out_point: Time::from_secs(3),
            start_offset: Time::from_millis(500),
        }
        .apply(&mut doc)
        .unwrap();
        assert!(
            evaluate(&doc, comp, Time::from_millis(999))
                .unwrap()
                .layers
                .is_empty()
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
