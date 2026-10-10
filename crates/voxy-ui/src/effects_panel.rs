//! The Effects & Presets panel: every effect the session can apply, by
//! category, with search (`docs/design-effects-ofx.md` §4.1).
//!
//! Double-click applies an effect to the selected layer. Dragging one onto a
//! layer in the timeline applies it there. Dropped on the viewport, it
//! applies to the selected layer, because the viewport has no layer picking
//! yet.

use std::sync::Arc;

use egui::{Color32, DragAndDrop, Pos2, Rect, Sense, Stroke, Ui, vec2};
use voxy_core::LayerId;
use voxy_core::command::AddEffect;
use voxy_core::registry::{Category, EffectDef};

use crate::state::EditorState;

/// View state of the panel. Never a command.
#[derive(Debug, Clone, Default)]
pub struct EffectsPanel {
    pub search: String,
}

/// What an effect drag carries.
#[derive(Debug, Clone)]
pub struct EffectDrag(pub Arc<EffectDef>);

/// Whether `def` matches every word of `search` (case-insensitive, in its
/// name, category or description).
pub fn matches(def: &EffectDef, search: &str) -> bool {
    let hay = format!("{} {} {}", def.name, def.category.label(), def.description).to_lowercase();
    search
        .split_whitespace()
        .all(|w| hay.contains(&w.to_lowercase()))
}

/// The registry's effects matching `search`, grouped by category (in
/// category order) and sorted by name.
pub fn listing(state: &EditorState, search: &str) -> Vec<(Category, Vec<Arc<EffectDef>>)> {
    let mut groups: std::collections::BTreeMap<Category, Vec<Arc<EffectDef>>> =
        std::collections::BTreeMap::new();
    for def in state.effects.latest_all() {
        if matches(def, search) {
            groups
                .entry(def.category.clone())
                .or_default()
                .push(def.clone());
        }
    }
    groups
        .into_iter()
        .map(|(c, mut defs)| {
            defs.sort_by(|a, b| a.name.cmp(&b.name));
            (c, defs)
        })
        .collect()
}

/// Applies `def` to `layer` (one undo step), at the end of its stack.
pub fn apply(state: &mut EditorState, layer: LayerId, def: Arc<EffectDef>) {
    let name = def.name.clone();
    state.last_effect = Some(def.clone());
    if state
        .execute_reported(AddEffect {
            layer,
            index: None,
            def,
        })
        .is_some()
    {
        state.selection = Some(layer);
        state.info(format!("Added {name}"));
    }
}

/// Applies `def` to the selected layer, or says why it cannot.
pub fn apply_to_selection(state: &mut EditorState, def: Arc<EffectDef>) {
    match state.selection {
        Some(layer) if state.history.current().find_layer(layer).is_some() => {
            apply(state, layer, def);
        }
        _ => state.info("Select a layer to apply an effect to."),
    }
}

pub fn show(ui: &mut Ui, state: &mut EditorState) {
    ui.horizontal(|ui| {
        ui.label("Search");
        ui.add(
            egui::TextEdit::singleline(&mut state.effects_panel.search)
                .hint_text("Name or category")
                .desired_width(f32::INFINITY),
        );
    });
    ui.separator();
    let search = state.effects_panel.search.clone();
    let groups = listing(state, &search);
    let presets: Vec<String> = state
        .effect_presets
        .iter()
        .filter(|p| {
            let hay = p.name.to_lowercase();
            search
                .split_whitespace()
                .all(|w| hay.contains(&w.to_lowercase()))
        })
        .map(|p| p.name.clone())
        .collect();
    if groups.is_empty() && presets.is_empty() {
        ui.weak("No effects or presets match.");
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            if !presets.is_empty() {
                let id = ui.make_persistent_id("effects-presets");
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                    .show_header(ui, |ui| {
                        preset_icon(ui);
                        ui.strong("Presets");
                    })
                    .body(|ui| {
                        for name in presets {
                            let r = ui
                                .add(egui::Label::new(&name).sense(Sense::click()))
                                .on_hover_text("Double-click to apply to the selected layer.");
                            if r.double_clicked() {
                                state.apply_effect_preset(&name);
                            }
                            r.context_menu(|ui| {
                                if ui.button("Apply to Selected Layer").clicked() {
                                    state.apply_effect_preset(&name);
                                    ui.close();
                                }
                                if ui.button("Delete Preset").clicked() {
                                    state.remove_effect_preset(&name);
                                    ui.close();
                                }
                            });
                        }
                    });
            }
            for (category, defs) in groups {
                let id = ui.make_persistent_id(("effects-category", category.label()));
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    true,
                )
                .show_header(ui, |ui| {
                    category_icon(ui, &category);
                    ui.strong(category.label());
                })
                .body(|ui| {
                    for def in defs {
                        let r = ui
                            .add(egui::Label::new(def.name.as_ref()).sense(Sense::click_and_drag()))
                            .on_hover_text(format!(
                                "{}\n\nDouble-click to apply to the selected layer, or drag onto a layer.",
                                def.description
                            ));
                        if r.double_clicked() {
                            apply_to_selection(state, def.clone());
                        }
                        if r.drag_started() {
                            DragAndDrop::set_payload(ui.ctx(), EffectDrag(def.clone()));
                        }
                    }
                });
            }
        });
}

/// Applies a dragged effect released over one of the timeline's layer rows.
pub fn drop_on_rows(ui: &Ui, state: &mut EditorState) {
    let Some(at) = released_at(ui) else { return };
    let row = state
        .timeline
        .rows
        .iter()
        .find(|(_, r)| r.contains(at))
        .map(|(id, _)| *id);
    if let Some(layer) = row
        && let Some(EffectDrag(def)) = take(ui)
    {
        apply(state, layer, def);
    }
}

/// Applies a dragged effect released over `rect` (the viewport) to the
/// selected layer.
pub fn drop_on_rect(ui: &Ui, state: &mut EditorState, rect: Rect) {
    let Some(at) = released_at(ui) else { return };
    if rect.contains(at)
        && let Some(EffectDrag(def)) = take(ui)
    {
        apply_to_selection(state, def);
    }
}

/// Where an effect drag was released this frame, if one was.
fn released_at(ui: &Ui) -> Option<Pos2> {
    if !DragAndDrop::has_payload_of_type::<EffectDrag>(ui.ctx()) {
        return None;
    }
    ui.input(|i| {
        if i.pointer.any_released() {
            i.pointer.interact_pos()
        } else {
            None
        }
    })
}

fn take(ui: &Ui) -> Option<EffectDrag> {
    DragAndDrop::take_payload::<EffectDrag>(ui.ctx()).map(|p| (*p).clone())
}

/// The presets group's glyph: two stacked cards.
fn preset_icon(ui: &mut Ui) {
    let size = ui.spacing().interact_size.y * 0.8;
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let painter = ui.painter_at(rect);
    let stroke = Stroke::new(1.4, ui.visuals().text_color());
    let card = Rect::from_center_size(rect.center(), vec2(size * 0.55, size * 0.4));
    for k in [-0.12, 0.12] {
        painter.rect_stroke(
            card.translate(vec2(size * k, -size * k)),
            1.5,
            stroke,
            egui::StrokeKind::Middle,
        );
    }
}

/// A small painted glyph per category (original artwork, no icon font).
#[allow(clippy::many_single_char_names)] // p, c, r, d: painter, centre, radius, offset.
fn category_icon(ui: &mut Ui, category: &Category) {
    let size = ui.spacing().interact_size.y * 0.8;
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter_at(rect);
    let c = rect.center();
    let r = size * 0.36;
    let ink = ui.visuals().text_color();
    let stroke = Stroke::new(1.4, ink);
    match category {
        Category::Blur => {
            for (k, alpha) in [(1.0, 40), (0.75, 90), (0.5, 200)] {
                p.circle_filled(c, r * k, Color32::from_white_alpha(alpha));
            }
        }
        Category::Color => {
            let d = r * 0.45;
            for (dx, dy, col) in [
                (-d, d * 0.6, Color32::from_rgb(220, 70, 60)),
                (d, d * 0.6, Color32::from_rgb(70, 170, 90)),
                (0.0, -d * 0.7, Color32::from_rgb(70, 120, 220)),
            ] {
                p.circle_filled(c + vec2(dx, dy), r * 0.55, col.linear_multiply(0.85));
            }
        }
        Category::Stylize => {
            let pts: Vec<Pos2> = (0..10)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 10.0 - std::f32::consts::FRAC_PI_2;
                    let rr = if i % 2 == 0 { r } else { r * 0.45 };
                    c + vec2(a.cos(), a.sin()) * rr
                })
                .collect();
            p.add(egui::Shape::closed_line(pts, stroke));
        }
        Category::Keying => {
            p.circle_stroke(c + vec2(-r * 0.4, 0.0), r * 0.45, stroke);
            p.line_segment([c + vec2(0.05 * r, 0.0), c + vec2(r, 0.0)], stroke);
            p.line_segment([c + vec2(0.7 * r, 0.0), c + vec2(0.7 * r, r * 0.4)], stroke);
        }
        Category::Distort => {
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    c + vec2(
                        (t - 0.5) * 2.0 * r,
                        (t * std::f32::consts::TAU).sin() * r * 0.5,
                    )
                })
                .collect();
            p.add(egui::Shape::line(pts, stroke));
        }
        Category::Transition => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(r * 2.0, r * 1.6)),
                1.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            p.line_segment(
                [c + vec2(-r * 0.4, r * 0.8), c + vec2(r * 0.4, -r * 0.8)],
                stroke,
            );
        }
        Category::Generate => {
            p.circle_stroke(c, r * 0.5, stroke);
            for i in 0..4 {
                let a = std::f32::consts::FRAC_PI_2 * i as f32;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * r * 0.7, c + d * r], stroke);
            }
        }
        Category::Plugin(_) => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(r * 1.6, r * 1.6)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            p.circle_filled(c, r * 0.3, ink);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxy_core::LinearRgba;
    use voxy_core::command::AddSolidLayer;

    fn state_with_layer() -> (EditorState, LayerId) {
        let mut s = EditorState::new(EditorState::untitled());
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
        (s, layer)
    }

    #[test]
    fn search_matches_every_word_in_name_category_or_description() {
        let (s, _) = state_with_layer();
        let names = |q: &str| -> Vec<String> {
            listing(&s, q)
                .into_iter()
                .flat_map(|(_, d)| d.into_iter().map(|d| d.name.to_string()))
                .collect()
        };
        assert_eq!(names("gauss"), ["Gaussian Blur"]);
        assert_eq!(names("BLUR box"), ["Box Blur"]);
        assert!(names("color").len() >= 5, "{:?}", names("color"));
        assert_eq!(names("no such effect"), Vec::<String>::new());
        // Everything, grouped, with each category's effects sorted.
        let all = listing(&s, "");
        assert_eq!(
            all.iter().map(|(_, d)| d.len()).sum::<usize>(),
            s.effects.len()
        );
        for (_, defs) in &all {
            assert!(defs.windows(2).all(|w| w[0].name <= w[1].name));
        }
    }

    #[test]
    fn applying_needs_a_selection_and_is_one_undo_step() {
        let (mut s, layer) = state_with_layer();
        let def = listing(&s, "invert")[0].1[0].clone();
        s.selection = None;
        let before = s.history.revision();
        apply_to_selection(&mut s, def.clone());
        assert_eq!(s.history.revision(), before, "nothing selected");
        s.selection = Some(layer);
        apply_to_selection(&mut s, def);
        let l = s
            .history
            .current()
            .find_layer(layer)
            .expect("layer")
            .1
            .clone();
        assert_eq!(l.effects.len(), 1);
        assert!(s.history.undo());
        assert_eq!(
            s.history
                .current()
                .find_layer(layer)
                .expect("layer")
                .1
                .effects
                .len(),
            0
        );
    }

    #[test]
    fn effect_actions_act_on_the_selected_effect_one_step_each() {
        use crate::actions::Action;
        let (mut s, layer) = state_with_layer();
        let blur = listing(&s, "gaussian")[0].1[0].clone();
        let invert = listing(&s, "invert")[0].1[0].clone();
        apply(&mut s, layer, blur);
        apply(&mut s, layer, invert);
        let effects = |s: &EditorState| s.selected_layer_effects();
        let ids: Vec<_> = effects(&s).iter().map(|e| e.id).collect();

        // Without a selected effect, the single-effect actions say why.
        assert!(s.availability(Action::EffectToggle).is_err());
        s.selected_effect = Some(ids[1]);
        assert!(
            s.availability(Action::EffectMoveDown).is_err(),
            "already last"
        );

        s.perform(Action::EffectMoveUp);
        assert_eq!(effects(&s)[0].id, ids[1]);
        s.perform(Action::EffectToggle);
        assert!(!effects(&s)[0].enabled);
        s.perform(Action::EffectDuplicate);
        assert_eq!(effects(&s).len(), 3);
        s.perform(Action::EffectCopy);
        assert_eq!(s.effect_clipboard.len(), 1);
        s.perform(Action::EffectPaste);
        assert_eq!(effects(&s).len(), 4);
        s.perform(Action::EffectApplyLast);
        assert_eq!(effects(&s).len(), 5);
        assert_eq!(effects(&s)[4].type_id, effects(&s)[0].type_id);
        s.perform(Action::EffectRemove);
        assert_eq!(effects(&s).len(), 4);
        s.perform(Action::EffectRemoveAll);
        assert_eq!(effects(&s).len(), 0);
        // Seven edits (copying is not one), seven undo steps.
        for _ in 0..7 {
            assert!(s.history.undo());
        }
        assert_eq!(effects(&s).iter().map(|e| e.id).collect::<Vec<_>>(), ids);
    }

    #[test]
    fn presets_save_from_one_layer_and_apply_to_another_in_one_step() {
        let (mut s, first) = state_with_layer();
        let comp = s.comp_id().expect("comp");
        s.execute(AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        });
        let second = s.history.current().comp(comp).expect("comp").layers[0].id;
        let blur = listing(&s, "gaussian")[0].1[0].clone();
        apply(&mut s, first, blur);
        s.selection = Some(first);
        let stack = s.selected_layer_effects();
        s.save_effect_preset("Dreamy", stack.clone());
        assert_eq!(s.effect_presets.len(), 1);

        s.selection = Some(second);
        s.apply_effect_preset("Dreamy");
        let got = s.selected_layer_effects();
        assert_eq!(got.len(), 1);
        assert_ne!(got[0].id, stack[0].id, "fresh IDs");
        assert_eq!(
            got[0].params[0].value_at(voxy_core::Time::ZERO),
            stack[0].params[0].value_at(voxy_core::Time::ZERO)
        );
        assert!(s.history.undo());
        assert_eq!(s.selected_layer_effects().len(), 0);

        s.remove_effect_preset("Dreamy");
        assert_eq!(s.effect_presets.len(), 0);
    }
}
