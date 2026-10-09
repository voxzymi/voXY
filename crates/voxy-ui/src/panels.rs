//! The dockable panels.

use std::fmt::Write as _;

use egui::{Id, RichText, Ui, WidgetText};
use egui_dock::TabViewer;
use voxy_core::command::{
    AddKeyframe, ApplyEasePreset, RemoveKeyframes, RenameLayer, SetAnimated, SetInterpolation,
    SetProperty, SetWorkingSpace,
};
use voxy_core::glam::DVec2;
use voxy_core::{
    EasePreset, FrameRate, Interp, LayerKind, LinearRgba, Property, Time, Unit, Value, WorkingSpace,
};

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
        });
        self.time_scrubber(ui, comp);
        ui.separator();
        crate::timeline::show(ui, self.state);
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
    /// Current-time control. The full timeline with keyframe lanes and the
    /// graph editor replace this in the next step of Milestone 2.
    fn time_scrubber(&mut self, ui: &mut Ui, comp: &voxy_core::Composition) {
        let rate = comp.frame_rate;
        let last = (rate.time_to_frame(comp.duration) - 1).max(0);
        let mut frame = rate.time_to_frame(self.state.time).clamp(0, last);
        ui.horizontal(|ui| {
            ui.label("Time");
            let slider = egui::Slider::new(&mut frame, 0..=last)
                .suffix(" f")
                .clamping(egui::SliderClamping::Always);
            if ui.add(slider).changed() {
                self.state.time = rate.frame_to_time(frame);
            }
            ui.weak(timecode(self.state.time, rate));
            ui.weak("· Left/Right arrow keys step one frame");
        });
    }

    /// One editable grid row for a property: keyframe controls and label,
    /// then the value editor.
    fn property_row(&mut self, ui: &mut Ui, p: &Property) {
        let time = self.state.time;
        ui.horizontal(|ui| {
            if p.desc.animatable {
                self.keyframe_controls(ui, p, time);
            }
            ui.label(p.desc.label.as_ref());
        });
        let value = p.value_at(time);
        let new = ui.horizontal(|ui| edit_value(ui, p, &value)).inner;
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
                for (interp, label) in [
                    (Interp::Linear, "Linear"),
                    (Interp::Bezier, "Bézier"),
                    (Interp::Hold, "Hold"),
                ] {
                    if ui.radio(k.interp == interp, label).clicked() {
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

/// `HH:MM:SS:FF` (frames counted at the nominal integer rate).
pub(crate) fn timecode(time: Time, rate: FrameRate) -> String {
    let frame = rate.time_to_frame(time).max(0);
    let fps = (rate.as_f64().round() as i64).max(1);
    let (secs, ff) = (frame / fps, frame % fps);
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60,
        ff
    )
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
