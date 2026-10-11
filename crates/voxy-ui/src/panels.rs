//! The dockable panels.

use egui::{Id, RichText, Ui, WidgetText};
use egui_dock::TabViewer;
use voxy_core::command::{
    AddKeyframe, ApplyEasePreset, DuplicateEffects, MoveEffect, PasteEffects, RemoveEffects,
    RemoveKeyframes, RenameEffect, RenameLayer, ResetEffect, SetAnimated, SetEffectsEnabled,
    SetInterpolation, SetProperty, SetWorkingSpace,
};
use voxy_core::glam::DVec2;
use voxy_core::registry::{EffectParamDef, ParamVisibility};
use voxy_core::{
    EasePreset, EffectInstance, FrameRate, Interp, Layer, LayerId, LayerKind, LinearRgba, Property,
    Time, Unit, Value, WorkingSpace,
};

use crate::state::EditorState;
use crate::viewport::ViewportRenderer;

/// A dockable panel. Saved by its title; [`Tab::from_saved`] also reads the
/// names from before the Media panel became the Project panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Viewport,
    Timeline,
    Graph,
    Properties,
    /// The Project panel (called the Media panel before it listed
    /// compositions and folders).
    Project,
    /// The Effects & Presets panel (Milestone E-C).
    Effects,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Viewport,
        Tab::Timeline,
        Tab::Graph,
        Tab::Properties,
        Tab::Project,
        Tab::Effects,
    ];

    /// Panels added after layouts were first saved. A saved layout without
    /// them gets them next to the Project panel instead of being refused.
    pub const ADDED_LATER: [Tab; 1] = [Tab::Effects];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Tab::Viewport => "Viewport",
            Tab::Timeline => "Timeline",
            Tab::Graph => "Graph Editor",
            Tab::Properties => "Properties",
            Tab::Project => "Project",
            Tab::Effects => "Effects & Presets",
        }
    }

    /// The panel a saved layout's panel name stands for, including names
    /// from before the rename ("Media").
    pub fn from_saved(name: &str) -> Option<Tab> {
        let current = crate::compat::panel_name(name)?;
        Tab::ALL.into_iter().find(|t| t.title() == current)
    }
}

impl serde::Serialize for Tab {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.title())
    }
}

impl<'de> serde::Deserialize<'de> for Tab {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Tab, D::Error> {
        let name = String::deserialize(d)?;
        Tab::from_saved(&name)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown panel {name:?}")))
    }
}

/// Default layout: Project panel left, viewport center, properties right,
/// timeline along the bottom.
pub fn default_layout() -> egui_dock::DockState<Tab> {
    use egui_dock::NodeIndex;
    let mut dock = egui_dock::DockState::new(vec![Tab::Viewport]);
    let tree = dock.main_surface_mut();
    let [top, _timeline] =
        tree.split_below(NodeIndex::root(), 0.68, vec![Tab::Timeline, Tab::Graph]);
    let [center, _properties] = tree.split_right(top, 0.76, vec![Tab::Properties]);
    let [_project, _viewport] = tree.split_left(center, 0.26, vec![Tab::Project, Tab::Effects]);
    dock
}

pub struct Panels<'a> {
    pub state: &'a mut EditorState,
    pub viewport: Option<&'a mut ViewportRenderer>,
}

impl TabViewer for Panels<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(("voxy-tab", tab.title()))
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        tab.title().into()
    }

    fn is_closeable(&self, _tab: &Tab) -> bool {
        // Panels cannot be reopened yet (Window menu arrives later), so keep
        // them all docked somewhere.
        false
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        match tab {
            Tab::Viewport => self.viewport_ui(ui),
            Tab::Timeline => self.timeline_ui(ui),
            Tab::Graph => crate::graph_editor::show(ui, self.state),
            Tab::Properties => self.properties_ui(ui),
            Tab::Project => self.project_ui(ui),
            Tab::Effects => crate::effects_panel::show(ui, self.state),
        }
        // A press inside a panel gives it keyboard focus, which decides what
        // Delete, Enter and Select All mean (Project panel or editor).
        let pressed =
            ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(ui.max_rect());
        if pressed {
            self.state.project.focused = matches!(tab, Tab::Project);
        }
    }
}

impl Panels<'_> {
    fn viewport_ui(&mut self, ui: &mut Ui) {
        let Some(comp_id) = self.state.comp_id() else {
            crate::empty_state::show_viewport_message(ui);
            return;
        };
        if let Some(comp) = self.state.comp() {
            crate::drops::viewport_drop(
                ui,
                self.state,
                comp_id,
                [comp.width, comp.height],
                ui.max_rect(),
            );
        }
        crate::effects_panel::drop_on_rect(ui, self.state, ui.max_rect());
        let Some(viewport) = self.viewport.as_deref_mut() else {
            ui.centered_and_justified(|ui| ui.label("GPU viewport unavailable"));
            return;
        };
        let avail = ui.available_size();
        let ppp = ui.ctx().pixels_per_point();
        let (doc, revision) = self.state.view_document();
        let preview = self.state.effects_preview;
        let playing = self.state.transport.is_playing();
        let mut shot = crate::viewport::Shot {
            doc: &doc,
            comp: comp_id,
            revision,
            time: self.state.time,
            effects: preview.view(),
        };
        let frame = viewport.frame(
            &shot,
            avail * ppp,
            self.state.asset_root(),
            playing,
            crate::viewport::Slot::Main,
        );
        let Some((id, _)) = frame else { return };
        let before = preview.split.and_then(|_| {
            shot.effects = voxy_render::EffectsView::Bypass;
            viewport.frame(
                &shot,
                avail * ppp,
                self.state.asset_root(),
                playing,
                crate::viewport::Slot::Before,
            )
        });
        // Fit the whole composition into the panel, preserving aspect ratio.
        // The frame itself is rendered at most at full resolution.
        let Some(comp) = doc.comp(comp_id) else {
            return;
        };
        let scale = (avail.x / comp.width as f32).min(avail.y / comp.height as f32);
        let size = egui::vec2(comp.width as f32, comp.height as f32) * scale;
        let rect = ui
            .centered_and_justified(|ui| ui.add(egui::Image::new((id, size))))
            .inner
            .rect;
        if let (Some(pos), Some((before, _))) = (preview.split, before) {
            self.split_overlay(ui, rect, before, pos);
        }
        crate::gizmo::show(ui, self.state, &doc, comp_id, rect);
    }

    /// The before/after split over the viewport frame at `rect`: the frame
    /// without effects left of a draggable divider at `pos` (0 to 1).
    fn split_overlay(&mut self, ui: &mut Ui, rect: egui::Rect, before: egui::TextureId, pos: f32) {
        let x = rect.left() + rect.width() * pos;
        let left = egui::Rect::from_min_max(rect.min, egui::pos2(x, rect.bottom()));
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(pos, 1.0));
        let painter = ui.painter_at(rect);
        painter.image(before, left, uv, egui::Color32::WHITE);
        let handle = egui::Rect::from_center_size(
            egui::pos2(x, rect.center().y),
            egui::vec2(8.0, rect.height()),
        );
        let response = ui
            .interact(handle, ui.id().with("effects-split"), egui::Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
            .on_hover_text("Left: without effects. Right: with effects. Drag to move.");
        if response.dragged()
            && let Some(p) = response.interact_pointer_pos()
            && rect.width() > 0.0
        {
            self.state.effects_preview.split =
                Some(((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
        }
        let stroke = ui.visuals().selection.stroke;
        painter.vline(x, rect.y_range(), egui::Stroke::new(2.0, stroke.color));
        for (text, align, at) in [
            (
                "Before",
                egui::Align2::RIGHT_TOP,
                egui::pos2(x - 6.0, rect.top() + 4.0),
            ),
            (
                "After",
                egui::Align2::LEFT_TOP,
                egui::pos2(x + 6.0, rect.top() + 4.0),
            ),
        ] {
            painter.text(
                at,
                align,
                text,
                egui::FontId::proportional(12.0),
                ui.visuals().strong_text_color(),
            );
        }
    }

    fn timeline_ui(&mut self, ui: &mut Ui) {
        crate::tabs_ui::show(ui, self.state);
        let Some(comp) = self.state.comp() else {
            crate::empty_state::show(ui, self.state);
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(&comp.name).strong());
            ui.label(format!(
                "{}×{} · {} · {:.2}s",
                comp.width,
                comp.height,
                comp.frame_rate,
                comp.duration.as_secs_f64()
            ));
        });
        self.time_scrubber(ui, &comp);
        ui.separator();
        crate::timeline::show(ui, self.state);
        crate::effects_panel::drop_on_rows(ui, self.state);
    }

    fn properties_ui(&mut self, ui: &mut Ui) {
        let doc = self.state.history.current().clone();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                let layer = self
                    .state
                    .selection
                    .and_then(|id| doc.find_layer(id))
                    .map(|(_, l)| l);
                let Some(layer) = layer else {
                    self.composition_ui(ui, &doc);
                    return;
                };
                ui.horizontal(|ui| {
                    ui.label("Name");
                    let mut name = layer.name.clone();
                    if ui.text_edit_singleline(&mut name).changed() {
                        self.state.execute(RenameLayer {
                            layer: layer.id,
                            name,
                        });
                    }
                });
                ui.weak(layer.kind.type_name());
                ui.separator();
                ui.label(RichText::new("Transform").strong());
                egui::Grid::new("transform")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for p in layer.transform.properties() {
                            self.property_row(ui, p, None, &[]);
                        }
                    });
                if let LayerKind::Solid { color, size } = &layer.kind {
                    ui.separator();
                    ui.label(RichText::new("Solid").strong());
                    egui::Grid::new("solid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            self.property_row(ui, color, None, &[]);
                            self.property_row(ui, size, None, &[]);
                        });
                }
                if let Some(asset) = layer.kind.asset()
                    && let Some(a) = doc.assets.get(asset)
                {
                    ui.separator();
                    ui.label(RichText::new("Source").strong());
                    ui.label(&a.path);
                }
                self.effects_ui(ui, layer, &doc);
            });
    }

    /// Effect Controls: one collapsible section per effect, in stack order.
    fn effects_ui(&mut self, ui: &mut Ui, layer: &Layer, doc: &voxy_core::Document) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Effects").strong());
            self.add_effect_menu(ui, layer.id);
            let paste = !self.state.effect_clipboard.is_empty();
            if ui
                .add_enabled(paste, egui::Button::new("Paste").small())
                .on_hover_text("Paste the copied effects at the end of the stack")
                .clicked()
            {
                self.paste_effects(layer.id, None);
            }
            let split = self.state.effects_preview.split.is_some();
            if ui
                .add(egui::Button::new("Before/After").small().selected(split))
                .on_hover_text(
                    "Split the viewport: without effects on the left, with them on the right",
                )
                .clicked()
            {
                self.state.effects_preview.toggle_split();
            }
        });
        if layer.effects.is_empty() {
            ui.weak("No effects. Add one with Add Effect.");
            return;
        }
        // Layers a layer-reference parameter can point at.
        let others: Vec<(LayerId, String)> = doc
            .find_layer(layer.id)
            .and_then(|(comp, _)| doc.comp(comp))
            .map(|c| {
                c.layers
                    .iter()
                    .filter(|l| l.id != layer.id)
                    .map(|l| (l.id, l.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let count = layer.effects.len();
        for (i, fx) in layer.effects.iter().enumerate() {
            let def = self.state.effects.get(&fx.type_id, fx.version).cloned();
            let id = ui.make_persistent_id(("effect-controls", fx.id));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| self.effect_header(ui, layer.id, fx, i, count))
                .body(|ui| {
                    if let Some(ph) = &fx.placeholder {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!(
                                "Not available: {}. It is kept as it is and has no effect.",
                                ph.reason
                            ),
                        );
                    }
                    self.effect_params(ui, fx, def.as_deref(), None, &others, 0);
                });
        }
    }

    /// The parameters of `fx` in `group` (`None`: the top level), then its
    /// subgroups as collapsible sections. Without a definition (a
    /// placeholder) every parameter is shown at the top level.
    fn effect_params(
        &mut self,
        ui: &mut Ui,
        fx: &EffectInstance,
        def: Option<&voxy_core::registry::EffectDef>,
        group: Option<u32>,
        others: &[(LayerId, String)],
        depth: usize,
    ) {
        egui::Grid::new(("effect-params", fx.id, group))
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for p in &fx.params {
                    let pd = def.and_then(|d| d.param(fx.param_name(p)));
                    if pd.map_or(group.is_some(), |pd| pd.group != group)
                        || pd.is_some_and(|pd| pd.flags.visibility == ParamVisibility::Hidden)
                    {
                        continue;
                    }
                    if fx.is_placeholder()
                        || pd.is_some_and(|pd| pd.flags.visibility == ParamVisibility::ReadOnly)
                    {
                        ui.label(p.desc.label.as_ref());
                        ui.weak(crate::timeline::value_text(&p.value_at(self.state.time)));
                        ui.end_row();
                    } else {
                        self.property_row(ui, p, pd, others);
                    }
                }
            });
        // Nesting is capped, so a damaged group tree cannot recurse forever.
        let Some(def) = def.filter(|_| depth < 8) else {
            return;
        };
        for g in def.groups.iter().filter(|g| g.parent == group) {
            egui::CollapsingHeader::new(g.label.as_ref())
                .id_salt(("effect-group", fx.id, g.id))
                .default_open(g.open_by_default)
                .show(ui, |ui| {
                    self.effect_params(ui, fx, Some(def), Some(g.id), others, depth + 1);
                });
        }
    }

    /// The header of one effect's section: enable toggle, name, reorder,
    /// reset and remove, and the right-click menu.
    fn effect_header(
        &mut self,
        ui: &mut Ui,
        layer: LayerId,
        fx: &EffectInstance,
        index: usize,
        count: usize,
    ) {
        let mut enabled = fx.enabled;
        if ui
            .checkbox(&mut enabled, "")
            .on_hover_text(if fx.enabled {
                "Turn this effect off"
            } else {
                "Turn this effect on"
            })
            .changed()
        {
            self.state.execute(SetEffectsEnabled {
                layer,
                ids: vec![fx.id],
                enabled,
            });
        }
        let mut name = RichText::new(&fx.name);
        if !fx.enabled {
            name = name.weak();
        }
        if fx.is_placeholder() {
            name = name.color(ui.visuals().warn_fg_color);
        }
        let selected = self.state.selected_effect == Some(fx.id);
        let label = ui.selectable_label(selected, name);
        if label.clicked() || label.secondary_clicked() {
            self.state.selected_effect = Some(fx.id);
        }
        label.context_menu(|ui| {
            let mut rename = fx.name.clone();
            ui.horizontal(|ui| {
                ui.label("Name");
                if ui.text_edit_singleline(&mut rename).lost_focus() && rename != fx.name {
                    self.state.execute(RenameEffect {
                        layer,
                        id: fx.id,
                        name: rename.clone(),
                    });
                }
            });
            ui.separator();
            if ui.button("Copy").clicked() {
                self.state.effect_clipboard = vec![fx.clone()];
                ui.close();
            }
            if ui
                .add_enabled(
                    !self.state.effect_clipboard.is_empty(),
                    egui::Button::new("Paste Below"),
                )
                .clicked()
            {
                self.paste_effects(layer, Some(index + 1));
                ui.close();
            }
            if ui
                .add_enabled(!fx.is_placeholder(), egui::Button::new("Duplicate"))
                .clicked()
            {
                self.state.execute(DuplicateEffects {
                    layer,
                    ids: vec![fx.id],
                });
                ui.close();
            }
            if ui
                .add_enabled(!fx.is_placeholder(), egui::Button::new("Reset"))
                .clicked()
            {
                self.state.execute(ResetEffect { layer, id: fx.id });
                ui.close();
            }
            if ui.button("Remove").clicked() {
                self.state.execute(RemoveEffects {
                    layer,
                    ids: vec![fx.id],
                });
                ui.close();
            }
            ui.separator();
            ui.add_enabled_ui(!fx.is_placeholder(), |ui| {
                ui.label(RichText::new("Save as preset").weak());
                ui.horizontal(|ui| {
                    let key = egui::Id::new(("preset-name", fx.id));
                    let mut name = ui
                        .data_mut(|d| d.get_temp::<String>(key))
                        .unwrap_or_else(|| fx.name.clone());
                    ui.text_edit_singleline(&mut name);
                    ui.data_mut(|d| d.insert_temp(key, name.clone()));
                    if ui.button("Save").clicked() {
                        self.state.save_effect_preset(&name, vec![fx.clone()]);
                        ui.close();
                    }
                });
                if ui.button("Save Whole Stack as Preset").clicked() {
                    let all = self.state.selected_layer_effects();
                    let all: Vec<_> = all.into_iter().filter(|e| !e.is_placeholder()).collect();
                    self.state
                        .save_effect_preset(&format!("{} stack", fx.name), all);
                    ui.close();
                }
            });
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("✕")
                .on_hover_text("Remove effect")
                .clicked()
            {
                self.state.execute(RemoveEffects {
                    layer,
                    ids: vec![fx.id],
                });
            }
            if ui
                .add_enabled(!fx.is_placeholder(), egui::Button::new("⟲").small())
                .on_hover_text("Reset all parameters")
                .clicked()
            {
                self.state.execute(ResetEffect { layer, id: fx.id });
            }
            let solo = self.state.effects_preview.solo == Some(fx.id);
            if ui
                .add_enabled(
                    !fx.is_placeholder(),
                    egui::Button::new("Solo").small().selected(solo),
                )
                .on_hover_text(if solo {
                    "Show all effects again (viewport only)"
                } else {
                    "Show only this effect on this layer (viewport only)"
                })
                .clicked()
            {
                self.state.effects_preview.toggle_solo(fx.id);
            }
            if ui
                .add_enabled(index + 1 < count, egui::Button::new("▼").small())
                .on_hover_text("Apply later")
                .clicked()
            {
                self.state.execute(MoveEffect {
                    layer,
                    id: fx.id,
                    to_index: index + 1,
                });
            }
            if ui
                .add_enabled(index > 0, egui::Button::new("▲").small())
                .on_hover_text("Apply earlier")
                .clicked()
            {
                self.state.execute(MoveEffect {
                    layer,
                    id: fx.id,
                    to_index: index - 1,
                });
            }
        });
    }

    /// "Add Effect": every effect in the registry, by category.
    fn add_effect_menu(&mut self, ui: &mut Ui, layer: LayerId) {
        let registry = self.state.effects.clone();
        let mut by_category: std::collections::BTreeMap<_, Vec<_>> =
            std::collections::BTreeMap::default();
        for def in registry.latest_all() {
            by_category
                .entry(def.category.clone())
                .or_default()
                .push(def.clone());
        }
        ui.menu_button("Add Effect", |ui| {
            for (category, mut defs) in by_category {
                defs.sort_by(|a, b| a.name.cmp(&b.name));
                ui.menu_button(category.label(), |ui| {
                    for def in defs {
                        if ui
                            .button(def.name.as_ref())
                            .on_hover_text(def.description.as_ref())
                            .clicked()
                        {
                            crate::effects_panel::apply(self.state, layer, def);
                            ui.close();
                        }
                    }
                });
            }
        });
    }

    fn paste_effects(&mut self, layer: LayerId, index: Option<usize>) {
        let items = self.state.effect_clipboard.clone();
        self.state.execute_reported(PasteEffects {
            layers: vec![layer],
            index,
            items,
        });
    }

    fn composition_ui(&mut self, ui: &mut Ui, doc: &voxy_core::Document) {
        let comp = self.state.comp();
        ui.label(RichText::new("Composition").strong());
        egui::Grid::new("comp")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                if let Some(comp) = &comp {
                    ui.label("Size");
                    ui.label(format!("{} × {} px", comp.width, comp.height));
                    ui.end_row();
                    ui.label("Frame rate");
                    ui.label(comp.frame_rate.to_string());
                    ui.end_row();
                    ui.label("Duration");
                    ui.label(format!("{:.3} s", comp.duration.as_secs_f64()));
                    ui.end_row();
                    self.property_row(ui, &comp.background, None, &[]);
                }
                ui.label("Working space");
                let mut space = doc.meta.working_space;
                egui::ComboBox::from_id_salt("working-space")
                    .selected_text(working_space_label(space))
                    .show_ui(ui, |ui| {
                        for s in [WorkingSpace::Linear, WorkingSpace::SrgbCompat] {
                            ui.selectable_value(&mut space, s, working_space_label(s));
                        }
                    });
                if space != doc.meta.working_space {
                    self.state.execute(SetWorkingSpace(space));
                }
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.weak("Select a layer to edit its properties.");
    }

    /// One editable grid row for a property.
    /// Current-time control. The full timeline with keyframe lanes and the
    /// graph editor replace this in the next step of Milestone 2.
    fn time_scrubber(&mut self, ui: &mut Ui, comp: &voxy_core::Composition) {
        let rate = comp.frame_rate;
        let last = (rate.time_to_frame(comp.duration) - 1).max(0);
        let mut frame = rate.time_to_frame(self.state.time).clamp(0, last);
        ui.horizontal(|ui| {
            let label = if self.state.transport.is_playing() {
                "Pause"
            } else {
                "Play"
            };
            if ui.button(label).on_hover_text("Space").clicked() {
                self.state.toggle_playback();
            }
            ui.label("Time");
            let slider = egui::Slider::new(&mut frame, 0..=last)
                .suffix(" f")
                .clamping(egui::SliderClamping::Always);
            if ui.add(slider).changed() {
                self.state.time = rate.frame_to_time(frame);
            }
            ui.weak(comp_timecode(comp, self.state.time));
            ui.weak("· Space plays · Left/Right step one frame · Home to start");
        });
    }

    /// One editable grid row for a property: keyframe controls and label,
    /// then the value editor. `param` adds an effect parameter's choices and
    /// display flags; `layers` are the targets a layer reference can name.
    fn property_row(
        &mut self,
        ui: &mut Ui,
        p: &Property,
        param: Option<&EffectParamDef>,
        layers: &[(LayerId, String)],
    ) {
        let time = self.state.time;
        let value = p.value_at(time);
        let label = ui.horizontal(|ui| {
            if p.desc.animatable {
                self.keyframe_controls(ui, p, time);
            }
            ui.label(p.desc.label.as_ref())
        });
        if let Some(hint) = param.and_then(|pd| pd.hint.as_deref()) {
            label.inner.on_hover_text(hint);
        }
        let new = ui
            .horizontal(|ui| edit_value(ui, p, &value, param, layers))
            .inner;
        if let Some(v) = new
            && v != value
        {
            // On an animated property this sets (or creates) the key at the
            // current time.
            self.state.execute(SetProperty {
                prop: p.id,
                time,
                value: v,
            });
        }
        ui.end_row();
    }

    fn keyframe_controls(&mut self, ui: &mut Ui, p: &Property, time: voxy_core::Time) {
        let animated = p.is_animated();
        let stopwatch = ui
            .selectable_label(animated, "⏱")
            .on_hover_text(if animated {
                "Animated: click to remove all keyframes"
            } else {
                "Animate this property"
            });
        if stopwatch.clicked() {
            self.state.execute(SetAnimated {
                prop: p.id,
                time,
                animated: !animated,
            });
        }
        let Some(track) = p.track() else {
            return;
        };
        let (prev, next) = track.neighbors(time);
        if ui
            .add_enabled(prev.is_some(), egui::Button::new("◀").small())
            .on_hover_text("Previous keyframe")
            .clicked()
            && let Some(k) = prev
        {
            self.state.time = k.time;
        }
        let key = track.key_at(time);
        let diamond = keyframe_button(ui, key.is_some()).on_hover_text(if key.is_some() {
            "Keyframe here: click to delete, right-click for easing"
        } else {
            "Add a keyframe at the current time"
        });
        if diamond.clicked() {
            if key.is_some() {
                self.state.execute(RemoveKeyframes {
                    prop: p.id,
                    times: vec![time],
                });
            } else {
                self.state.execute(AddKeyframe { prop: p.id, time });
            }
        }
        if let Some(k) = key {
            diamond.context_menu(|ui| {
                ui.label(RichText::new("Interpolation").weak());
                let mut choices = vec![Interp::Linear, Interp::Bezier, Interp::Hold];
                choices.extend(Interp::parametric_defaults());
                for interp in choices {
                    // A parametric type is "selected" whatever its parameters.
                    let selected =
                        std::mem::discriminant(&k.interp) == std::mem::discriminant(&interp);
                    if ui.radio(selected, interp.label()).clicked() && !selected {
                        self.state.execute(SetInterpolation {
                            prop: p.id,
                            times: vec![time],
                            interp,
                        });
                        ui.close();
                    }
                }
                ui.separator();
                ui.label(RichText::new("Ease to next keyframe").weak());
                for preset in EasePreset::ALL {
                    if ui.button(preset.label()).clicked() {
                        self.state.execute(ApplyEasePreset {
                            prop: p.id,
                            times: vec![time],
                            preset,
                        });
                        ui.close();
                    }
                }
            });
        }
        if ui
            .add_enabled(next.is_some(), egui::Button::new("▶").small())
            .on_hover_text("Next keyframe")
            .clicked()
            && let Some(k) = next
        {
            self.state.time = k.time;
        }
    }

    fn project_ui(&mut self, ui: &mut Ui) {
        // Footage the viewport could not decode, with the reason.
        let problems: std::collections::HashMap<voxy_core::AssetId, String> = self
            .viewport
            .as_ref()
            .map(|v| {
                self.state
                    .history
                    .current()
                    .assets
                    .iter()
                    .filter_map(|a| v.media.failure(a.id).map(|e| (a.id, e.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        crate::project_panel::show(ui, self.state, &problems);
    }
}

pub fn working_space_label(space: WorkingSpace) -> &'static str {
    match space {
        WorkingSpace::Linear => "Linear light",
        WorkingSpace::SrgbCompat => "sRGB (compatibility)",
    }
}

/// Edits a value according to its descriptor; returns the new value if the
/// user changed it this frame.
fn edit_value(
    ui: &mut Ui,
    p: &Property,
    value: &Value,
    param: Option<&EffectParamDef>,
    layers: &[(LayerId, String)],
) -> Option<Value> {
    let (factor, suffix) = match p.desc.unit {
        Unit::Factor | Unit::Normalized => (100.0, " %"),
        Unit::Degrees => (1.0, "°"),
        Unit::Pixels => (1.0, " px"),
        Unit::Seconds => (1.0, " s"),
        Unit::None => (1.0, ""),
    };
    let range = p.desc.range.map(|(lo, hi)| lo * factor..=hi * factor);
    let drag = |ui: &mut Ui, v: f64| -> Option<f64> {
        let mut shown = v * factor;
        let mut w = egui::DragValue::new(&mut shown)
            .suffix(suffix)
            .speed(if factor > 1.0 { 0.5 } else { 1.0 })
            .max_decimals(3);
        if let Some(r) = range.clone() {
            w = w.range(r);
        }
        ui.add(w).changed().then_some(shown / factor)
    };
    match value {
        Value::Scalar(v) => drag(ui, *v).map(Value::Scalar),
        Value::Vec2(v) => {
            let x = drag(ui, v.x);
            let y = drag(ui, v.y);
            (x.is_some() || y.is_some())
                .then(|| Value::Vec2(DVec2::new(x.unwrap_or(v.x), y.unwrap_or(v.y))))
        }
        Value::Color(c) if param.is_some_and(|pd| pd.flags.hide_alpha) => {
            let mut rgb = [c.r, c.g, c.b];
            ui.color_edit_button_rgb(&mut rgb)
                .changed()
                .then(|| Value::Color(LinearRgba::new(rgb[0], rgb[1], rgb[2], c.a)))
        }
        Value::Color(c) => {
            let mut rgba = c.to_array();
            ui.color_edit_button_rgba_unmultiplied(&mut rgba)
                .changed()
                .then(|| Value::Color(LinearRgba::new(rgba[0], rgba[1], rgba[2], rgba[3])))
        }
        Value::Bool(b) => {
            let mut b = *b;
            ui.checkbox(&mut b, "").changed().then_some(Value::Bool(b))
        }
        Value::Int(i) => {
            let mut v = *i;
            let mut w = egui::DragValue::new(&mut v).suffix(suffix).speed(0.1);
            if let Some((lo, hi)) = p.desc.range {
                w = w.range(lo as i64..=hi as i64);
            }
            ui.add(w).changed().then_some(Value::Int(v))
        }
        Value::Enum(e) => {
            let labels = param.and_then(|pd| pd.choices.clone());
            let name = |i: u32| {
                labels
                    .as_ref()
                    .and_then(|l| l.get(i as usize).map(std::string::ToString::to_string))
                    .unwrap_or_else(|| i.to_string())
            };
            let mut chosen = *e;
            egui::ComboBox::from_id_salt(("enum", p.id))
                .selected_text(name(*e))
                .show_ui(ui, |ui| {
                    let n = labels.as_ref().map_or(0, |l| l.len() as u32);
                    for i in 0..n {
                        ui.selectable_value(&mut chosen, i, name(i));
                    }
                });
            (chosen != *e).then_some(Value::Enum(chosen))
        }
        Value::Layer(target) => {
            let label = |t: Option<LayerId>| match t {
                None => "None".to_owned(),
                Some(id) => layers
                    .iter()
                    .find(|(l, _)| *l == id)
                    .map_or_else(|| "(missing layer)".to_owned(), |(_, n)| n.clone()),
            };
            let mut chosen = *target;
            egui::ComboBox::from_id_salt(("layer", p.id))
                .selected_text(label(*target))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, "None");
                    for (id, name) in layers {
                        ui.selectable_value(&mut chosen, Some(*id), name);
                    }
                });
            (chosen != *target).then_some(Value::Layer(chosen))
        }
        Value::Curve(c) => crate::curve_editor::show(ui, egui::Id::new(("curve", p.id)), c)
            .map(|c| Value::Curve(std::sync::Arc::new(c))),
        other => {
            ui.weak(format!("{other:?}"));
            None
        }
    }
}

/// `H:MM:SS:FF` of `time`, shifted by the composition's start timecode and
/// in drop-frame (`;`) when the composition uses it. Layers and keyframes
/// are not affected: only this display.
pub(crate) fn comp_timecode(comp: &voxy_core::Composition, time: Time) -> String {
    timecode_at(time, comp.frame_rate, comp.start_offset, comp.drop_frame)
}

pub(crate) fn timecode_at(time: Time, rate: FrameRate, start: Time, drop: bool) -> String {
    let frames = rate.time_to_frame(time).max(0) + rate.time_to_frame(start);
    voxy_core::timecode::format_frames(frames, rate, drop)
}

/// Non-drop timecode of a bare duration at `rate`.
pub(crate) fn timecode(time: Time, rate: FrameRate) -> String {
    timecode_at(time, rate, Time::ZERO, false)
}

/// A keyframe diamond: filled when a key sits at the current time, hollow
/// otherwise. Painted rather than taken from a font so it renders the same
/// everywhere.
fn keyframe_button(ui: &mut Ui, filled: bool) -> egui::Response {
    let size = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let c = rect.center();
    let r = size * 0.32;
    let points = vec![
        c + egui::vec2(0.0, -r),
        c + egui::vec2(r, 0.0),
        c + egui::vec2(0.0, r),
        c + egui::vec2(-r, 0.0),
    ];
    let accent = ui.visuals().selection.stroke.color;
    let fill = if filled {
        accent
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = egui::Stroke::new(
        1.5,
        if filled {
            accent
        } else {
            visuals.fg_stroke.color
        },
    );
    ui.painter()
        .add(egui::Shape::convex_polygon(points, fill, stroke));
    response
}
