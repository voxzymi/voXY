//! Viewport layer picking (U-B): the topmost layer whose pixels lie under a
//! composition-space point. Used to target an effect drop at the layer under
//! the cursor. Pure over the evaluated composition, so it is unit-tested
//! headlessly; the screen↔composition mapping lives in `drops`.

use voxy_core::glam::{DAffine2, DVec2};
use voxy_core::{CompId, Document, EvaluatedContent, LayerId, Time};

/// A picked layer and its outline in composition pixels (the four corners of
/// its pixel rectangle, mapped through its world transform).
pub struct Pick {
    pub layer: LayerId,
    pub quad: [DVec2; 4],
}

/// The topmost layer at composition-pixel `point` in `comp` at `time`, with
/// its outline, or `None` if the point hits no layer (or falls on a layer
/// with no pixels, like an adjustment layer).
pub fn pick(doc: &Document, comp: CompId, time: Time, point: DVec2) -> Option<Pick> {
    let eval = voxy_core::evaluate(doc, comp, time)?;
    // `layers` is bottom-first; the topmost hit wins, so walk it in reverse.
    eval.layers.iter().rev().find_map(|l| {
        let size = content_size(&l.content)?;
        let local = l.world.inverse().transform_point2(point);
        let inside = local.x >= 0.0 && local.y >= 0.0 && local.x <= size.x && local.y <= size.y;
        inside.then(|| Pick {
            layer: l.id,
            quad: corners(l.world, size),
        })
    })
}

/// The outline of `layer` in composition pixels, if it has pixels.
pub fn layer_quad(doc: &Document, comp: CompId, time: Time, layer: LayerId) -> Option<[DVec2; 4]> {
    let eval = voxy_core::evaluate(doc, comp, time)?;
    let l = eval.layers.iter().find(|l| l.id == layer)?;
    Some(corners(l.world, content_size(&l.content)?))
}

/// The four corners of the `size` rectangle (layer-local, origin top-left)
/// mapped through `world` into composition pixels.
fn corners(world: DAffine2, size: DVec2) -> [DVec2; 4] {
    [
        DVec2::ZERO,
        DVec2::new(size.x, 0.0),
        size,
        DVec2::new(0.0, size.y),
    ]
    .map(|p| world.transform_point2(p))
}

/// A layer's local pixel size, when it has pixels. An adjustment layer (no
/// pixels of its own) and a source whose size is unknown return `None`.
pub fn content_size(content: &EvaluatedContent) -> Option<DVec2> {
    match content {
        EvaluatedContent::Solid { size, .. } | EvaluatedContent::Precomp { size, .. } => {
            Some(*size)
        }
        EvaluatedContent::Image { size, .. }
        | EvaluatedContent::Video { size, .. }
        | EvaluatedContent::Placeholder { size } => *size,
        EvaluatedContent::Adjustment => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxy_core::command::{AddImageLayer, AddSolidLayer, SetProperty};
    use voxy_core::{CompSettings, Document, History, LinearRgba, Value};

    fn comp_doc() -> (History, CompId) {
        let settings = CompSettings {
            name: "Main".into(),
            width: 200,
            height: 100,
            ..CompSettings::default()
        };
        let h = History::new(Document::with_comp("Hit", settings));
        let c = h.current().main_comp.expect("comp");
        (h, c)
    }

    #[test]
    fn picks_the_topmost_layer_under_the_point() {
        let (mut h, c) = comp_doc();
        // Bottom: a 200x100 image covering the whole comp.
        h.execute(AddImageLayer {
            comp: c,
            path: "media/bg.png".into(),
            pixel_size: [200, 100],
            index: 0,
        })
        .unwrap();
        let bottom = h.current().comp(c).unwrap().layers[0].id;
        // Top: a small 40x40 solid at the layer origin.
        h.execute(AddSolidLayer {
            comp: c,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        })
        .unwrap();
        let top = h.current().comp(c).unwrap().layers[0].id;
        // Shrink the solid to 40x40 so it does not cover everything.
        let size_prop = h
            .current()
            .find_layer(top)
            .and_then(|(_, m)| match &m.kind {
                voxy_core::LayerKind::Solid { size, .. } => Some(size.id),
                _ => None,
            })
            .expect("solid size prop");
        h.execute(SetProperty {
            prop: size_prop,
            time: Time::ZERO,
            value: Value::Vec2(DVec2::new(40.0, 40.0)),
        })
        .unwrap();

        let doc = h.current();
        // A point inside the small solid (near the origin) hits the top layer.
        assert_eq!(
            pick(doc, c, Time::ZERO, DVec2::new(10.0, 10.0)).map(|p| p.layer),
            Some(top)
        );
        // A point outside the solid but inside the image hits the image.
        assert_eq!(
            pick(doc, c, Time::ZERO, DVec2::new(150.0, 80.0)).map(|p| p.layer),
            Some(bottom)
        );
        // A point outside the composition hits nothing.
        assert_eq!(
            pick(doc, c, Time::ZERO, DVec2::new(400.0, 400.0)).map(|p| p.layer),
            None
        );
    }

    #[test]
    fn an_adjustment_layer_is_not_picked() {
        use voxy_core::command::AddAdjustmentLayer;
        let (mut h, c) = comp_doc();
        h.execute(AddAdjustmentLayer {
            comp: c,
            name: None,
            index: 0,
        })
        .unwrap();
        // Nothing with pixels under the point → None, even though a layer
        // covers it conceptually.
        assert_eq!(
            pick(h.current(), c, Time::ZERO, DVec2::new(50.0, 50.0)).map(|p| p.layer),
            None
        );
    }
}
