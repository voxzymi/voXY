//! The dockable panels.

use std::fmt::Write as _;

use egui::{Id, RichText, Ui, WidgetText};
use egui_dock::TabViewer;
use voxy_core::command::{
    MoveLayer, RemoveLayers, RenameLayer, SetLayerVisible, SetProperty, SetWorkingSpace,
};
use voxy_core::glam::DVec2;
use voxy_core::{LayerKind, LinearRgba, Property, Unit, Value, WorkingSpace};

use crate::state::EditorState;
use crate::viewport::ViewportRenderer;

/// A dockable panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Viewport,
    Timeline,
    Properties,
    Media,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Viewport => "Viewport",
            Tab::Timeline => "Timeline",
            Tab::Properties => "Properties",
            Tab::Media => "Media",
        }
    }
}

/// Default layout: media left, viewport center, properties right, timeline
/// along the bottom.
pub fn default_layout() -> egui_dock::DockState<Tab> {
    use egui_dock::NodeIndex;
    let mut dock = egui_dock::DockState::new(vec![Tab::Viewport]);
    let tree = dock.main_surface_mut();
    let [top, _timeline] = tree.split_below(NodeIndex::root(), 0.68, vec![Tab::Timeline]);
    let [center, _properties] = tree.split_right(top, 0.76, vec![Tab::Properties]);
    let [_media, _viewport] = tree.split_left(center, 0.22, vec![Tab::Media]);
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
            Tab::Properties => self.properties_ui(ui),
            Tab::Media => self.media_ui(ui),
        }
    }
}

impl Panels<'_> {
    fn viewport_ui(&mut self, ui: &mut Ui) {
        let Some(viewport) = self.viewport.as_deref_mut() else {
            ui.centered_and_justified(|ui| ui.label("GPU viewport unavailable"));
            return;
        };
        let avail = ui.available_size();
        let ppp = ui.ctx().pixels_per_point();
        let doc = self.state.history.current().clone();
        let frame = viewport.frame(
            &doc,
            self.state.history.revision(),
            self.state.time,
            avail * ppp,
            self.state.asset_root(),
        );
        let Some((id, _)) = frame else { return };
        // Fit the whole composition into the panel, preserving aspect ratio.
        // The frame itself is rendered at most at full resolution.
        let comp = doc.root();
        let scale = (avail.x / comp.width as f32).min(avail.y / comp.height as f32);
        let size = egui::vec2(comp.width as f32, comp.height as f32) * scale;
        ui.centered_and_justified(|ui| {
            ui.add(egui::Image::new((id, size)));
        });
    }

    fn timeline_ui(&mut self, ui: &mut Ui) {
        let doc = self.state.history.current().clone();
        let comp = doc.root();
        ui.horizontal(|ui| {
            ui.label(RichText::new(&comp.name).strong());
            ui.label(format!(
                "{}×{} · {} · {:.2}s",
                comp.width,
                comp.height,
                comp.frame_rate,
                comp.duration.as_secs_f64()
            ));
            ui.weak("(keyframes and the full timeline arrive in Milestone 2)");
        });
        ui.separator();
        if comp.layers.is_empty() {
            ui.weak("No layers. Use Layer ▸ New Solid or Layer ▸ Import Image…");
            return;
        }
        let count = comp.layers.len();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (index, layer) in comp.layers.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let mut visible = layer.flags.visible;
                        if ui
                            .checkbox(&mut visible, "")
                            .on_hover_text("Visible")
                            .changed()
                        {
                            self.state.execute(SetLayerVisible {
                                layer: layer.id,
                                visible,
                            });
                        }
                        ui.weak(format!("{:>2}", index + 1));
                        let selected = self.state.selection == Some(layer.id);
                        let label = format!("{}  ·  {}", layer.name, layer.kind.type_name());
                        if ui.selectable_label(selected, label).clicked() {
                            self.state.selection = Some(layer.id);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("🗑").on_hover_text("Delete layer").clicked() {
                                self.state.execute(RemoveLayers {
                                    layers: vec![layer.id],
                                });
                                if selected {
                                    self.state.selection = None;
                                }
                            }
                            if ui
                                .add_enabled(index + 1 < count, egui::Button::new("⏷").small())
                                .on_hover_text("Move down")
                                .clicked()
                            {
                                self.state.execute(MoveLayer {
                                    layer: layer.id,
                                    to_index: index + 1,
                                });
                            }
                            if ui
                                .add_enabled(index > 0, egui::Button::new("⏶").small())
                                .on_hover_text("Move up")
                                .clicked()
                            {
                                self.state.execute(MoveLayer {
                                    layer: layer.id,
                                    to_index: index - 1,
                                });
                            }
                        });
                    });
                }
            });
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
                            self.property_row(ui, p);
                        }
                    });
                if let LayerKind::Solid { color, size } = &layer.kind {
                    ui.separator();
                    ui.label(RichText::new("Solid").strong());
                    egui::Grid::new("solid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            self.property_row(ui, color);
                            self.property_row(ui, size);
                        });
                }
                if let LayerKind::Image { asset } = &layer.kind
                    && let Some(a) = doc.assets.get(*asset)
                {
                    ui.separator();
                    ui.label(RichText::new("Source").strong());
                    ui.label(&a.path);
                }
            });
    }

    fn composition_ui(&mut self, ui: &mut Ui, doc: &voxy_core::Document) {
        let comp = doc.root();
        ui.label(RichText::new("Composition").strong());
        egui::Grid::new("comp")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                ui.label("Size");
                ui.label(format!("{} × {} px", comp.width, comp.height));
                ui.end_row();
                ui.label("Frame rate");
                ui.label(comp.frame_rate.to_string());
                ui.end_row();
                ui.label("Duration");
                ui.label(format!("{:.3} s", comp.duration.as_secs_f64()));
                ui.end_row();
                self.property_row(ui, &comp.background);
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
    fn property_row(&mut self, ui: &mut Ui, p: &Property) {
        ui.label(p.desc.label.as_ref());
        let value = p.value_at(self.state.time).clone();
        let new = ui.horizontal(|ui| edit_value(ui, p, &value)).inner;
        if let Some(v) = new
            && v != value
        {
            self.state.execute(SetProperty {
                prop: p.id,
                value: v,
            });
        }
        ui.end_row();
    }

    fn media_ui(&mut self, ui: &mut Ui) {
        if ui.button("Import Image…").clicked() {
            self.state.import_image_dialog();
        }
        ui.separator();
        let doc = self.state.history.current().clone();
        if doc.assets.0.is_empty() {
            ui.weak("No media yet.");
        }
        let missing = &self
            .viewport
            .as_ref()
            .map(|v| v.last_report.missing_assets.clone())
            .unwrap_or_default();
        for a in doc.assets.iter() {
            let mut text = RichText::new(&a.name);
            if missing.contains(&a.id) {
                text = text.color(ui.visuals().warn_fg_color);
            }
            let r = ui.label(text);
            let mut tip = a.path.clone();
            if let Some([w, h]) = a.pixel_size {
                let _ = write!(tip, "\n{w} × {h} px");
            }
            if missing.contains(&a.id) {
                tip.push_str("\nMissing: the file could not be loaded");
            }
            r.on_hover_text(tip);
        }
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
fn edit_value(ui: &mut Ui, p: &Property, value: &Value) -> Option<Value> {
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
        other => {
            ui.weak(format!("{other:?}"));
            None
        }
    }
}
