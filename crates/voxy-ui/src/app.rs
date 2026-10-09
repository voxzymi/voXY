//! The eframe application: menus, shortcuts, docking and dialogs.

use std::path::PathBuf;

use egui::{RichText, Ui};
use egui_dock::{DockArea, DockState};
use voxy_core::WorkingSpace;
use voxy_core::command::SetWorkingSpace;

use crate::actions::Action;
use crate::menu::{action_item, action_item_labelled};
use crate::panels::{Panels, Tab, default_layout, working_space_label};
use crate::state::{AppRequest, EditorState};
use crate::viewport::ViewportRenderer;

/// Product name, exactly as it must appear to users.
pub const PRODUCT_NAME: &str = "voXY";

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
        state.prefs_path = crate::prefs::default_path();
        if let Some(path) = &state.prefs_path {
            state.prefs = crate::prefs::Preferences::load_from(path);
        }
        state.choose_startup_tip(clock_seed());
        state.gpu_max_texture = viewport
            .as_ref()
            .map(ViewportRenderer::max_texture_dimension);
        if let Some(dir) = crate::prefs::recovery_dir() {
            state.enable_recovery(dir);
        }
        for c in state.keymap.conflicts() {
            tracing::warn!(
                "shortcut {:?} is bound to more than one action: {:?}",
                c.shortcut,
                c.actions
            );
        }
        if let Some(path) = open {
            state.open_path(&path);
        } else {
            state.offer_orphaned_recovery();
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

    /// Runs the actions whose shortcuts were pressed this frame. An action
    /// that cannot run says why in the status bar and does nothing.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        // Text fields keep their own editing shortcuts.
        let typing = ctx.egui_wants_keyboard_input();
        let project_focused = self.state.project.focused;
        let fired = ctx.input_mut(|i| self.state.keymap.dispatch(i, project_focused, typing));
        for action in fired {
            if let Err(why) = self.state.availability(action) {
                self.state.info(why);
            } else {
                self.run(action);
            }
        }
    }

    /// Runs `action`, passing the requests that need confirmation to the
    /// unsaved-changes flow.
    fn run(&mut self, action: Action) {
        if let Some(request) = self.state.perform(action) {
            self.handle(request);
        }
    }

    fn handle(&mut self, request: AppRequest) {
        self.request(match request {
            AppRequest::NewProject => Discarding::New,
            AppRequest::Open => Discarding::Open,
            AppRequest::Quit => Discarding::Quit,
        });
    }

    fn item(&mut self, ui: &mut Ui, action: Action) {
        if let Some(request) = action_item(ui, &mut self.state, action) {
            self.handle(request);
        }
    }

    fn item_labelled(&mut self, ui: &mut Ui, action: Action, label: &str) {
        if let Some(request) = action_item_labelled(ui, &mut self.state, action, label) {
            self.handle(request);
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                ui.menu_button("New", |ui| {
                    self.item_labelled(ui, Action::FileNewProject, "Project");
                    self.item(ui, Action::FileNewComposition);
                    self.item(ui, Action::FileNewCompositionFromFootage);
                    self.item(ui, Action::ProjectNewFolder);
                });
                self.item(ui, Action::FileOpen);
                self.item(ui, Action::FileImport);
                ui.separator();
                self.item(ui, Action::FileSave);
                self.item(ui, Action::FileSaveAs);
                ui.separator();
                self.item(ui, Action::FileQuit);
            });
            ui.menu_button("Edit", |ui| {
                let undo = self
                    .state
                    .history
                    .undo_label()
                    .map_or_else(|| "Undo".to_owned(), |l| format!("Undo {l}"));
                self.item_labelled(ui, Action::EditUndo, &undo);
                let redo = self
                    .state
                    .history
                    .redo_label()
                    .map_or_else(|| "Redo".to_owned(), |l| format!("Redo {l}"));
                self.item_labelled(ui, Action::EditRedo, &redo);
            });
            ui.menu_button("Layer", |ui| {
                self.item(ui, Action::LayerNewSolid);
                self.item(ui, Action::LayerImportImage);
                self.item(ui, Action::LayerImportVideo);
                ui.separator();
                self.item(ui, Action::LayerDelete);
                ui.separator();
                self.item(ui, Action::RevealSourceInProject);
                self.item(ui, Action::RevealInFileManager);
            });
            ui.menu_button("Keyframe", |ui| {
                self.item(ui, Action::EaseCopy);
                self.item(ui, Action::EasePaste);
                ui.separator();
                ui.label(RichText::new("Apply to selected keyframes").weak());
                self.item(ui, Action::EaseApplySpring);
                self.item(ui, Action::EaseApplyBounce);
                self.item(ui, Action::EaseApplyOscillation);
                self.item(ui, Action::EaseApplySteps);
                self.item(ui, Action::EaseApplySteppedRamp);
            });
            ui.menu_button("Composition", |ui| {
                self.item(ui, Action::CompositionSettings);
                ui.separator();
                crate::project_panel::reveal_in_composition_menu(ui, &mut self.state);
                ui.separator();
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
            ui.menu_button("Window", |ui| {
                for tab in Tab::ALL {
                    if ui.button(tab.title()).clicked() {
                        if let Some(path) = self.dock.find_tab(&tab) {
                            let _ = self.dock.set_active_tab(path);
                        }
                        ui.close();
                    }
                }
                ui.separator();
                let mut tips = self.state.prefs.show_tips;
                if ui.checkbox(&mut tips, "Show Tips").changed() {
                    self.state.prefs.show_tips = tips;
                    self.state.save_prefs();
                }
                if ui.button("Reset Layout").clicked() {
                    self.dock = default_layout();
                    ui.close();
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
        crate::comp_dialog::show(ctx, &mut self.state);
        crate::footage::show_prompt(ctx, &mut self.state);
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
        if let Some(offer) = self.state.pending_recovery.clone() {
            let what = offer.project.as_ref().map_or_else(
                || "an untitled project".to_owned(),
                |p| {
                    format!(
                        "“{}”",
                        p.file_stem()
                            .map_or_else(|| p.to_string_lossy(), |s| s.to_string_lossy())
                    )
                },
            );
            let when = crate::recovery::describe_age(std::time::SystemTime::now(), offer.modified);
            let mut choice = None;
            egui::Modal::new(egui::Id::new("voxy-recovery")).show(ctx, |ui| {
                ui.heading("Recover unsaved changes?");
                ui.label(format!(
                    "voXY did not shut down cleanly last time. It kept a copy of {what} saved {when}."
                ));
                if offer.project.is_some() {
                    ui.label("It is newer than the project file, which is untouched until you save.");
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Recover").clicked() {
                        choice = Some(true);
                    }
                    if ui
                        .button("Discard")
                        .on_hover_text("Delete the autosaved copy. This cannot be undone.")
                        .clicked()
                    {
                        choice = Some(false);
                    }
                });
            });
            match choice {
                Some(true) => self.state.recover(),
                Some(false) => self.state.discard_recovery(),
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
    /// A clean exit removes the autosave. A close the user did not confirm
    /// while there were unsaved changes (the system ending the session, say)
    /// keeps it, like a crash.
    fn on_exit(&mut self) {
        self.state.finish_session(self.allow_close);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && self.state.history.is_dirty()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Discarding::Quit);
        }

        if self.pending.is_none()
            && !self.show_about
            && self.state.comp_dialog.is_none()
            && self.state.pending_recovery.is_none()
        {
            self.shortcuts(&ctx);
        }

        // Autosave: not mid-drag, while typing, or behind a dialog; and wake
        // up now and then while there are unsaved changes, since egui only
        // draws frames on input.
        let busy = ctx.input(|i| i.pointer.any_down())
            || ctx.egui_wants_keyboard_input()
            || self.pending.is_some()
            || self.state.comp_dialog.is_some();
        self.state.autosave_tick(std::time::Instant::now(), busy);
        if self.state.history.is_dirty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
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

        if let Some(text) = self.state.outgoing_clipboard.take() {
            ctx.copy_text(text);
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

/// A seed from the clock, for picking the Tip of the Day (and nothing else).
fn clock_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}
