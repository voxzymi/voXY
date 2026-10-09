//! The eframe application: menus, shortcuts, docking and dialogs.

use std::path::PathBuf;

use egui::{Key, KeyboardShortcut, Modifiers, RichText, Ui};
use egui_dock::{DockArea, DockState};
use voxy_core::command::SetWorkingSpace;
use voxy_core::{Time, WorkingSpace};

use crate::panels::{Panels, Tab, default_layout, working_space_label};
use crate::state::EditorState;
use crate::viewport::ViewportRenderer;

/// Product name, exactly as it must appear to users.
pub const PRODUCT_NAME: &str = "voXY";

const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const REDO: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
const REDO_ALT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
const QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);

/// Actions that discard the current document and therefore ask first if it
/// has unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Discarding {
    New,
    Open,
    Quit,
}

pub struct VoxyApp {
    state: EditorState,
    dock: DockState<Tab>,
    viewport: Option<ViewportRenderer>,
    pending: Option<Discarding>,
    allow_close: bool,
    show_about: bool,
}

impl std::fmt::Debug for VoxyApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoxyApp")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl VoxyApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        crate::theme::apply(&cc.egui_ctx);
        let viewport = cc
            .wgpu_render_state
            .clone()
            .map(|rs| ViewportRenderer::new(rs, cc.egui_ctx.clone()));
        if viewport.is_none() {
            tracing::error!("eframe did not provide a wgpu render state; the viewport is disabled");
        }
        let mut state = EditorState::new(EditorState::untitled());
        if let Some(path) = open {
            state.open_path(&path);
        }
        VoxyApp {
            state,
            dock: default_layout(),
            viewport,
            pending: None,
            allow_close: false,
            show_about: false,
        }
    }

    fn request(&mut self, action: Discarding) {
        if self.state.history.is_dirty() {
            self.pending = Some(action);
        } else {
            self.perform(action, None);
        }
    }

    fn perform(&mut self, action: Discarding, ctx: Option<&egui::Context>) {
        match action {
            Discarding::New => self.state.new_document(),
            Discarding::Open => self.state.open_dialog(),
            Discarding::Quit => {
                self.allow_close = true;
                if let Some(ctx) = ctx {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // Text fields and drag values keep their own editing shortcuts.
        let typing = ctx.egui_wants_keyboard_input();
        let consume = |s: &KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(s));
        // More specific shortcuts first: Ctrl+Shift+Z before Ctrl+Z.
        if consume(&SAVE_AS) {
            self.state.save_as_dialog();
        } else if consume(&SAVE) {
            self.state.save();
        }
        if consume(&NEW) {
            self.request(Discarding::New);
        }
        if consume(&OPEN) {
            self.request(Discarding::Open);
        }
        if consume(&QUIT) {
            self.request(Discarding::Quit);
        }
        if typing {
            return;
        }
        if consume(&REDO) || consume(&REDO_ALT) {
            self.state.history.redo();
        } else if consume(&UNDO) {
            self.state.history.undo();
        }
        if ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
            self.state.delete_selection();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.state.selection = None;
        }
        if ctx.input(|i| i.key_pressed(Key::Space)) {
            self.state.toggle_playback();
        }
        if ctx.input(|i| i.key_pressed(Key::Home)) {
            self.state.time = Time::ZERO;
        }
        // Count every press (and auto-repeat) this frame, not just whether a
        // key went down, so fast stepping never drops frames.
        let step: i64 = ctx.input(|i| {
            i.events
                .iter()
                .map(|e| match e {
                    egui::Event::Key {
                        key: Key::ArrowRight,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.is_none() => 1,
                    egui::Event::Key {
                        key: Key::ArrowLeft,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.is_none() => -1,
                    _ => 0,
                })
                .sum()
        });
        if step != 0
            && let Some(comp) = self.state.comp()
        {
            let rate = comp.frame_rate;
            let last = (rate.time_to_frame(comp.duration) - 1).max(0);
            let frame = (rate.time_to_frame(self.state.time) + step).clamp(0, last);
            self.state.time = rate.frame_to_time(frame);
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui
                    .add(egui::Button::new("New Project").shortcut_text(ctx.format_shortcut(&NEW)))
                    .clicked()
                {
                    self.request(Discarding::New);
                }
                if ui
                    .add(egui::Button::new("Open…").shortcut_text(ctx.format_shortcut(&OPEN)))
                    .clicked()
                {
                    self.request(Discarding::Open);
                }
                ui.separator();
                if ui
                    .add(egui::Button::new("Save").shortcut_text(ctx.format_shortcut(&SAVE)))
                    .clicked()
                {
                    self.state.save();
                }
                if ui
                    .add(egui::Button::new("Save As…").shortcut_text(ctx.format_shortcut(&SAVE_AS)))
                    .clicked()
                {
                    self.state.save_as_dialog();
                }
                ui.separator();
                if ui
                    .add(egui::Button::new("Quit").shortcut_text(ctx.format_shortcut(&QUIT)))
                    .clicked()
                {
                    self.request(Discarding::Quit);
                }
            });
            ui.menu_button("Edit", |ui| {
                let undo = self
                    .state
                    .history
                    .undo_label()
                    .map_or_else(|| "Undo".to_owned(), |l| format!("Undo {l}"));
                if ui
                    .add_enabled(
                        self.state.history.can_undo(),
                        egui::Button::new(undo).shortcut_text(ctx.format_shortcut(&UNDO)),
                    )
                    .clicked()
                {
                    self.state.history.undo();
                }
                let redo = self
                    .state
                    .history
                    .redo_label()
                    .map_or_else(|| "Redo".to_owned(), |l| format!("Redo {l}"));
                if ui
                    .add_enabled(
                        self.state.history.can_redo(),
                        egui::Button::new(redo).shortcut_text(ctx.format_shortcut(&REDO)),
                    )
                    .clicked()
                {
                    self.state.history.redo();
                }
            });
            ui.menu_button("Layer", |ui| {
                if ui.button("New Solid").clicked() {
                    self.state.add_solid();
                }
                if ui.button("Import Image…").clicked() {
                    self.state.import_image_dialog();
                }
                if ui.button("Import Video…").clicked() {
                    self.state.import_video_dialog();
                }
                ui.separator();
                if ui
                    .add_enabled(
                        self.state.selection.is_some(),
                        egui::Button::new("Delete").shortcut_text("Del"),
                    )
                    .clicked()
                {
                    self.state.delete_selection();
                }
            });
            ui.menu_button("Composition", |ui| {
                ui.label(RichText::new("Working color space").weak());
                let current = self.state.history.current().meta.working_space;
                for space in [WorkingSpace::Linear, WorkingSpace::SrgbCompat] {
                    if ui
                        .radio(current == space, working_space_label(space))
                        .clicked()
                        && current != space
                    {
                        self.state.execute(SetWorkingSpace(space));
                    }
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button(format!("About {PRODUCT_NAME}")).clicked() {
                    self.show_about = true;
                }
            });
        });
    }

    fn status_bar(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let dirty = if self.state.history.is_dirty() {
                " •"
            } else {
                ""
            };
            ui.label(RichText::new(format!("{}{dirty}", self.state.display_name())).strong());
            let doc = self.state.history.current().clone();
            ui.separator();
            if let Some(comp) = self.state.comp() {
                ui.label(format!(
                    "{}×{} · {}",
                    comp.width, comp.height, comp.frame_rate
                ));
            } else {
                ui.label("No composition");
            }
            ui.separator();
            ui.label(working_space_label(doc.meta.working_space));
            if let Some(v) = &self.viewport
                && !v.last_report.missing_assets.is_empty()
            {
                ui.separator();
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!(
                        "{} missing media file(s)",
                        v.last_report.missing_assets.len()
                    ),
                );
            }
            if let Some(v) = &self.viewport {
                let cache = v.media.cache_stats();
                if cache.frames > 0 {
                    ui.separator();
                    ui.weak(format!(
                        "Video cache: {} frames, {} of {} MB",
                        cache.frames,
                        cache.bytes >> 20,
                        cache.budget >> 20
                    ))
                    .on_hover_text(format!(
                        "Decoded video frames kept in memory.\n{} hits, {} misses, {} evicted",
                        cache.hits, cache.misses, cache.evictions
                    ));
                }
            }
            if let Some(label) = self.state.transport.output_label() {
                ui.separator();
                ui.weak(label);
            }
            if let Some(s) = &self.state.status {
                ui.separator();
                if s.is_error {
                    ui.colored_label(ui.visuals().error_fg_color, &s.text);
                } else {
                    ui.weak(&s.text);
                }
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(action) = self.pending {
            let name = self.state.display_name();
            let mut choice = None;
            egui::Modal::new(egui::Id::new("voxy-unsaved")).show(ctx, |ui| {
                ui.heading("Unsaved changes");
                ui.label(format!("Save changes to “{name}” before continuing?"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Don’t Save").clicked() {
                        choice = Some(2);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(0);
                    }
                });
            });
            match choice {
                Some(1) => {
                    self.pending = None;
                    if self.state.save() {
                        self.perform(action, Some(ctx));
                    }
                }
                Some(2) => {
                    self.pending = None;
                    self.perform(action, Some(ctx));
                }
                Some(_) => self.pending = None,
                None => {}
            }
        }
        if self.show_about {
            egui::Modal::new(egui::Id::new("voxy-about")).show(ctx, |ui| {
                ui.heading(PRODUCT_NAME);
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label("A desktop motion-graphics and video editor.");
                ui.add_space(8.0);
                if ui.button("Close").clicked() {
                    self.show_about = false;
                }
            });
        }
    }
}

impl eframe::App for VoxyApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && self.state.history.is_dirty()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Discarding::Quit);
        }

        if self.pending.is_none() && !self.show_about {
            self.shortcuts(&ctx);
        }

        // Playback: the master clock moves the playhead.
        if let Some(comp) = self.state.comp()
            && let Some(t) =
                self.state
                    .transport
                    .tick(self.state.time, comp.duration, comp.frame_rate)
        {
            self.state.time = t;
        }
        if self.state.transport.is_playing() {
            ctx.request_repaint();
        }

        egui::Panel::top("voxy-menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("voxy-status").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            let mut panels = Panels {
                state: &mut self.state,
                viewport: self.viewport.as_mut(),
            };
            DockArea::new(&mut self.dock)
                .style(egui_dock::Style::from_egui(ui.style().as_ref()))
                .show_leaf_close_all_buttons(false)
                .show_close_buttons(false)
                .show_inside(ui, &mut panels);
        });

        self.dialogs(&ctx);
        self.state.fix_selection();

        // A drag or text edit has ended: the next edit starts a new undo step.
        if !ctx.input(|i| i.pointer.any_down()) && !ctx.egui_wants_keyboard_input() {
            self.state.history.end_interaction();
        }
    }
}
