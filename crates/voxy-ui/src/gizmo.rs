//! On-canvas handles for effect point parameters (design §4.2): each enabled
//! effect on the selected layer draws one handle per parameter whose
//! definition has `gizmo: Point`. Values are in layer pixels, mapped through
//! the layer's world transform; dragging issues the usual `SetProperty`,
//! whose merge key makes one drag one undo step.

use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui};
use voxy_core::command::SetProperty;
use voxy_core::glam::{DAffine2, DVec2};
use voxy_core::registry::GizmoKind;
use voxy_core::{CompId, Document, EffectId, PropId, Value};

use crate::state::EditorState;

/// One draggable point, in composition pixels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Handle {
    pub effect: EffectId,
    pub prop: PropId,
    /// "Effect name: parameter label", for the tooltip.
    pub label: String,
    pub comp: DVec2,
    /// The layer's layer-to-composition transform at this time.
    pub world: DAffine2,
}

/// The handles of the selected layer's enabled effects in `comp` at the
/// current time. Layers outside their time span have none.
pub(crate) fn handles(state: &EditorState, doc: &Document, comp: CompId) -> Vec<Handle> {
    let Some(layer) = state.selection else {
        return Vec::new();
    };
    let Some((_, model)) = doc.find_layer(layer) else {
        return Vec::new();
    };
    let Some(eval) = voxy_core::evaluate(doc, comp, state.time) else {
        return Vec::new();
    };
    let Some(l) = eval.layers.iter().find(|l| l.id == layer) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for fx in &l.effects {
        let (Some(def), Some(inst)) = (
            state.effects.get(&fx.type_id, fx.version),
            model.effect(fx.id),
        ) else {
            continue;
        };
        for p in def
            .params
            .iter()
            .filter(|p| p.gizmo == Some(GizmoKind::Point))
        {
            let value = fx.params.iter().find(|(n, _)| **n == *p.name);
            let prop = inst.params.iter().find(|q| q.desc.key == p.desc.key);
            if let (Some((_, Value::Vec2(v))), Some(prop)) = (value, prop) {
                out.push(Handle {
                    effect: fx.id,
                    prop: prop.id,
                    label: format!("{}: {}", inst.name, p.desc.label),
                    comp: l.world.transform_point2(*v),
                    world: l.world,
                });
            }
        }
    }
    out
}

/// Moves `handle` to `to` (composition pixels). Repeated calls during one
/// drag merge into one undo step. Does nothing for a layer scaled to zero.
pub(crate) fn drag_to(state: &mut EditorState, handle: &Handle, to: DVec2) {
    if handle.world.matrix2.determinant().abs() < 1e-12 {
        return;
    }
    let local = handle.world.inverse().transform_point2(to);
    if !local.is_finite() {
        return;
    }
    state.execute(SetProperty {
        prop: handle.prop,
        time: state.time,
        value: Value::Vec2(local),
    });
}

/// Draws and drags the handles over the viewport frame shown at `rect`,
/// which shows all of `comp`.
pub(crate) fn show(
    ui: &mut Ui,
    state: &mut EditorState,
    doc: &Arc<Document>,
    comp: CompId,
    rect: Rect,
) {
    let Some(c) = doc.comp(comp) else { return };
    if c.width == 0 || rect.width() <= 0.0 {
        return;
    }
    let scale = f64::from(rect.width()) / f64::from(c.width);
    let to_screen = |p: DVec2| {
        Pos2::new(
            rect.left() + (p.x * scale) as f32,
            rect.top() + (p.y * scale) as f32,
        )
    };
    let painter = ui.painter_at(rect);
    let accent = ui.visuals().selection.stroke.color;
    for h in handles(state, doc, comp) {
        let at = to_screen(h.comp);
        if !rect.expand(8.0).contains(at) {
            continue;
        }
        let id = ui.id().with(("effect-point", h.prop));
        let response = ui
            .interact(
                Rect::from_center_size(at, egui::vec2(14.0, 14.0)),
                id,
                Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_text(&h.label);
        if response.dragged() {
            let d = response.drag_delta();
            if d != egui::Vec2::ZERO {
                let delta = DVec2::new(f64::from(d.x), f64::from(d.y)) / scale;
                drag_to(state, &h, h.comp + delta);
            }
        }
        let selected = state.selected_effect == Some(h.effect);
        let hot = response.hovered() || response.dragged();
        let fill = if selected || hot {
            accent
        } else {
            Color32::WHITE
        };
        // A ring with a cross: readable over light and dark frames.
        painter.circle_stroke(at, 6.0, Stroke::new(3.0, Color32::from_black_alpha(160)));
        painter.circle_stroke(at, 6.0, Stroke::new(1.5, fill));
        for (a, b) in [((-3.0, 0.0), (3.0, 0.0)), ((0.0, -3.0), (0.0, 3.0))] {
            painter.line_segment(
                [at + egui::vec2(a.0, a.1), at + egui::vec2(b.0, b.1)],
                Stroke::new(1.0, fill),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxy_core::LinearRgba;
    use voxy_core::command::{AddEffect, AddSolidLayer};
    use voxy_core::registry::{
        Category, EffectDef, EffectOrigin, EffectParamDef, EffectRegistry, EffectSpace, ParamFlags,
    };
    use voxy_core::{EffectTypeId, PropertyDesc, Unit};

    /// Group one has no point parameters, so the tests bring their own.
    fn point_def() -> Arc<EffectDef> {
        let type_id = EffectTypeId::new("voxy.test.point");
        let param = |name: &str, default: Value, gizmo| EffectParamDef {
            name: name.into(),
            desc: Arc::new(PropertyDesc {
                key: type_id.param_key(name).into(),
                label: name.into(),
                kind: default.kind(),
                default,
                range: None,
                soft_range: None,
                unit: Unit::Pixels,
                animatable: true,
                spatial: true,
            }),
            choices: None,
            group: None,
            gizmo,
            hint: None,
            flags: ParamFlags::default(),
            check: None,
        };
        Arc::new(EffectDef {
            type_id: type_id.clone(),
            version: 1,
            name: "Point Test".into(),
            category: Category::Distort,
            description: "".into(),
            params: vec![
                param(
                    "center",
                    Value::Vec2(DVec2::new(10.0, 20.0)),
                    Some(GizmoKind::Point),
                ),
                param("amount", Value::Scalar(1.0), None),
            ]
            .into(),
            groups: Vec::new().into(),
            space: EffectSpace::Working,
            seeded: false,
            origin: EffectOrigin::Builtin,
        })
    }

    fn state_with_point_effect() -> (EditorState, CompId, voxy_core::LayerId, Arc<EffectDef>) {
        let mut s = EditorState::new(EditorState::untitled());
        let def = point_def();
        let mut reg = EffectRegistry::with_builtins();
        reg.register((*def).clone()).expect("register");
        s.effects = Arc::new(reg);
        let settings = s.history.current().meta.settings.new_comp.clone();
        s.create_composition(settings, None).expect("composition");
        let comp = s.comp_id().expect("comp");
        s.execute(AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        });
        let layer = s.history.current().comp(comp).expect("comp").layers[0].id;
        s.execute(AddEffect {
            layer,
            index: None,
            def: def.clone(),
        });
        (s, comp, layer, def)
    }

    #[test]
    fn handles_follow_the_layer_transform_and_drags_are_one_undo_step() {
        let (mut s, comp, layer, _) = state_with_point_effect();
        let doc = s.history.current().clone();
        // Nothing selected: no handles.
        assert_eq!(handles(&s, &doc, comp).len(), 0);
        s.selection = Some(layer);
        let hs = handles(&s, &doc, comp);
        assert_eq!(hs.len(), 1, "only the point parameter");
        let h = hs[0].clone();
        let world = doc
            .find_layer(layer)
            .and_then(|_| voxy_core::evaluate(&doc, comp, s.time))
            .and_then(|e| e.layers.into_iter().find(|l| l.id == layer))
            .expect("evaluated")
            .world;
        // New point parameters start at the centre of the layer's content.
        let start = doc.content_size(layer).expect("solid size") / 2.0;
        assert_eq!(h.comp, world.transform_point2(start));
        assert_eq!(h.label, "Point Test: center");

        drag_to(&mut s, &h, h.comp + DVec2::new(5.0, 0.0));
        drag_to(&mut s, &h, h.comp + DVec2::new(7.0, -2.0));
        let fx = &s.selected_layer_effects()[0];
        let v = fx.params[0].value_at(s.time);
        let Value::Vec2(v) = v else { panic!("{v:?}") };
        assert!((v - (start + DVec2::new(7.0, -2.0))).length() < 1e-9, "{v}");
        let doc = s.history.current().clone();
        assert_eq!(
            handles(&s, &doc, comp)[0].comp,
            h.comp + DVec2::new(7.0, -2.0)
        );

        // One undo takes back the whole drag, and the effect stays.
        assert!(s.history.undo());
        let v = s.selected_layer_effects()[0].params[0].value_at(s.time);
        assert_eq!(v, Value::Vec2(start));
    }

    #[test]
    fn disabled_effects_and_zero_scale_layers_have_no_working_handles() {
        let (mut s, comp, layer, _) = state_with_point_effect();
        s.selection = Some(layer);
        let doc = s.history.current().clone();
        let mut h = handles(&s, &doc, comp)[0].clone();
        // A layer scaled to zero cannot be inverted: the drag is ignored.
        h.world = DAffine2::from_scale(DVec2::ZERO);
        let revision = s.history.revision();
        drag_to(&mut s, &h, DVec2::new(1.0, 1.0));
        assert_eq!(s.history.revision(), revision);

        let fx = s.selected_layer_effects()[0].id;
        s.execute(voxy_core::command::SetEffectsEnabled {
            layer,
            ids: vec![fx],
            enabled: false,
        });
        let doc = s.history.current().clone();
        assert_eq!(handles(&s, &doc, comp).len(), 0);
    }
}
