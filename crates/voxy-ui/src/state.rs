//! Editor state shared by the menus and panels.
//!
//! Everything that changes the document goes through [`EditorState::execute`]
//! or [`EditorState::execute_reported`] and so through `History`. View state
//! (selection, the open composition, scroll, folder expansion) never does.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use voxy_core::command::{
    AddImageLayer, AddSolidLayer, AddVideoLayer, CreateComposition, CreateFolder,
    DuplicateComposition, ImportAssets, MoveToFolder, RemoveItems, RemoveLayers, RemoveMode,
    RenameComposition, RenameFolder, RenameMedia, ReplaceMedia, ResizeMode, SetAssetPath,
    SetCompositionSettings,
};
use voxy_core::{
    Asset, AssetId, AssetKind, Command, CommandReport, CompId, CompSettings, Composition, Document,
    FolderId, FrameRate, History, InterpKind, LayerId, LinearRgba, PoolItem, PropId, SegmentEase,
    Time, persist,
};

use crate::actions::{Action, ActionContext, Keymap};
use crate::comp_draft::{Context as DraftContext, Draft, Mode, Stage, Tab as DialogTab};
use crate::dialogs::{Dialogs, IMAGE_EXTENSIONS, NativeDialogs, VIDEO_EXTENSIONS};
use crate::prefs::Preferences;
use crate::project_view::{PendingRemove, ProjectState};
use crate::recovery::{self, Offer, Session};
use crate::reveal::{self, CompTarget};

/// A message for the status bar.
#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub is_error: bool,
}

/// Actions that replace the whole document or end the program, so the
/// application shell asks about unsaved changes before running them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRequest {
    NewProject,
    Open,
    Quit,
}

#[derive(Debug)]
struct CreatedComp {
    comp: CompId,
    selection: std::collections::BTreeSet<PoolItem>,
    active: Option<CompId>,
}

/// The document as it would be if the open dialog were confirmed.
#[derive(Debug)]
struct Preview {
    doc: Arc<Document>,
    key: (CompId, CompSettings, ResizeMode),
    revision: u64,
}

/// Preview revisions have this bit set, so they never equal a real one.
const PREVIEW_REVISION_BIT: u64 = 1 << 63;

#[derive(Debug)]
pub struct EditorState {
    pub history: History,
    pub project_path: Option<PathBuf>,
    pub selection: Option<LayerId>,
    pub time: Time,
    pub status: Option<Status>,
    pub timeline: crate::timeline::TimelineView,
    pub graph: crate::graph_editor::GraphView,
    pub transport: crate::transport::Transport,
    pub project: ProjectState,
    pub keymap: Keymap,
    pub dialogs: Box<dyn Dialogs>,
    /// The Composition Settings dialog, while it is open.
    pub comp_dialog: Option<Draft>,
    /// The last tab the dialog showed (saved in `ui_state`).
    pub dialog_tab: DialogTab,
    /// The "items differ" question of New Composition From Footage.
    pub footage_prompt: Option<crate::footage::Prompt>,
    pub prefs: Preferences,
    /// The easing last copied with "Copy Easing" (or the graph editor's Copy
    /// button). Lives for the session.
    pub ease_clipboard: Option<SegmentEase>,
    /// Autosave bookkeeping; `None` until [`EditorState::enable_recovery`]
    /// (tests and tools that never autosave leave it off).
    recovery: Option<Autosaver>,
    /// An autosave found for the project that was just opened (or left by a
    /// crashed session), waiting for the user to recover or discard it.
    pub pending_recovery: Option<Offer>,
    /// Text for the shell to put on the system clipboard (the copied
    /// easing as JSON), so it can be pasted elsewhere, e.g. into a bug
    /// report. Taken by the app each frame.
    pub outgoing_clipboard: Option<String>,
    /// Where preferences are saved; `None` keeps them in memory (tests).
    pub prefs_path: Option<PathBuf>,
    /// The GPU's largest texture side, once the viewport knows it.
    pub gpu_max_texture: Option<u32>,
    preview: Option<Preview>,
    preview_counter: u64,
    /// What a just-created composition replaced, so undo can put it back.
    last_create: Option<CreatedComp>,
    /// The composition being edited. View state until composition tabs land
    /// (Milestone E); falls back to the main composition when it is `None`
    /// or no longer exists.
    active: Option<CompId>,
}

impl EditorState {
    pub fn new(doc: Document) -> Self {
        EditorState {
            history: History::new(doc),
            project_path: None,
            selection: None,
            time: Time::ZERO,
            status: None,
            timeline: crate::timeline::TimelineView::default(),
            graph: crate::graph_editor::GraphView::default(),
            transport: crate::transport::Transport::default(),
            project: ProjectState::default(),
            keymap: Keymap::default(),
            dialogs: Box::new(NativeDialogs),
            comp_dialog: None,
            dialog_tab: DialogTab::Basic,
            footage_prompt: None,
            prefs: Preferences::default(),
            ease_clipboard: None,
            recovery: None,
            pending_recovery: None,
            outgoing_clipboard: None,
            prefs_path: None,
            gpu_max_texture: None,
            preview: None,
            preview_counter: 0,
            last_create: None,
            active: None,
        }
    }

    /// A new project: no compositions, no media. Nothing creates one for
    /// the user.
    pub fn untitled() -> Document {
        Document::empty("Untitled")
    }

    /// Runs a command, reporting failures in the status bar.
    pub fn execute(&mut self, cmd: impl Command) {
        if let Err(e) = self.history.execute(cmd) {
            self.error(e.to_string());
        }
    }

    /// [`EditorState::execute`] for commands that report what they created.
    /// Warnings go to the status bar.
    pub fn execute_reported(&mut self, cmd: impl Command) -> Option<CommandReport> {
        match self.history.execute_reported(cmd) {
            Ok(report) => {
                if !report.warnings.is_empty() {
                    self.info(report.warnings.join(" "));
                }
                Some(report)
            }
            Err(e) => {
                self.error(e.to_string());
                None
            }
        }
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            is_error: false,
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::warn!("{text}");
        self.status = Some(Status {
            text,
            is_error: true,
        });
    }

    /// The directory relative asset paths resolve against.
    pub fn asset_root(&self) -> Option<PathBuf> {
        self.project_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
    }

    /// Display name: the file name if saved, otherwise the project name.
    pub fn display_name(&self) -> String {
        self.project_path
            .as_deref()
            .and_then(Path::file_name)
            .map_or_else(
                || self.history.current().meta.name.clone(),
                |n| n.to_string_lossy().into_owned(),
            )
    }

    /// The composition the editor shows: the one opened from the Project
    /// panel or by a Reveal, else the main one, else none. `None` is a
    /// normal state: every caller must handle it.
    pub fn comp_id(&self) -> Option<CompId> {
        let doc = self.history.current();
        self.active
            .filter(|id| doc.comps.contains_key(id))
            .or(doc.main_comp)
            .filter(|id| doc.comps.contains_key(id))
    }

    pub fn comp(&self) -> Option<Arc<Composition>> {
        let id = self.comp_id()?;
        self.history.current().comps.get(&id).cloned()
    }

    /// Shows `id` in the timeline and viewport. View state only.
    pub fn open_comp(&mut self, id: CompId) {
        if self.history.current().comps.contains_key(&id) && self.comp_id() != Some(id) {
            let rate = self.frame_rate();
            self.transport.pause(rate);
            self.active = Some(id);
            self.selection = None;
            self.time = Time::ZERO;
        }
    }

    fn frame_rate(&self) -> FrameRate {
        self.comp().map_or(FrameRate::FPS_30, |c| c.frame_rate)
    }

    /// The open composition, or an error in the status bar if there is none.
    fn require_comp(&mut self) -> Option<CompId> {
        let id = self.comp_id();
        if id.is_none() {
            self.error(crate::actions::NEEDS_COMP);
        }
        id
    }

    fn select_top_layer(&mut self) {
        self.selection = self.comp().and_then(|c| c.layers.first().map(|l| l.id));
    }

    /// Starts or stops playback from the playhead.
    pub fn toggle_playback(&mut self) {
        let rate = self.frame_rate();
        if let Some(t) = self.transport.toggle(self.time, rate) {
            self.time = t;
        }
    }

    /// Moves the playhead by `frames`, staying inside the composition.
    pub fn step_frames(&mut self, frames: i64) {
        let Some(comp) = self.comp() else { return };
        let rate = comp.frame_rate;
        let last = (rate.time_to_frame(comp.duration) - 1).max(0);
        let frame = (rate.time_to_frame(self.time) + frames).clamp(0, last);
        self.time = rate.frame_to_time(frame);
    }

    fn reset_view(&mut self) {
        let rate = self.frame_rate();
        self.transport.pause(rate);
        self.project = ProjectState::default();
        self.selection = None;
        self.active = None;
        self.time = Time::ZERO;
    }

    pub fn new_document(&mut self) {
        // The caller has already asked about unsaved changes: whatever was
        // autosaved for the old project is no longer wanted.
        self.discard_autosave();
        self.history.reset(Self::untitled());
        self.project_path = None;
        self.reset_view();
        self.info("New project");
    }

    pub fn open_path(&mut self, path: &Path) {
        match persist::load_with_ui(path) {
            Ok((doc, ui_state)) => {
                self.discard_autosave();
                self.history.reset(doc);
                self.project_path = Some(path.to_path_buf());
                self.reset_view();
                // A newer autosave next to it means the last session ended
                // without saving; the user decides (see `recover`).
                self.pending_recovery = recovery::offer_for_project(path);
                if let Some(state) = ui_state {
                    self.project.view = crate::compat::from_ui_state(&state);
                    self.dialog_tab = crate::compat::dialog_tab_from(&state);
                }
                self.project.forget_missing(self.history.current());
                self.info(format!("Opened {}", path.display()));
            }
            Err(e) => self.error(format!("Could not open {}: {e}", path.display())),
        }
    }

    pub fn open_dialog(&mut self) {
        if let Some(path) = self.dialogs.open_project() {
            self.open_path(&path);
        }
    }

    /// Saves to the current path, asking for one if there is none. Returns
    /// whether the project was saved.
    pub fn save(&mut self) -> bool {
        match self.project_path.clone() {
            Some(path) => self.save_to(&path),
            None => self.save_as_dialog(),
        }
    }

    pub fn save_as_dialog(&mut self) -> bool {
        let name = format!(
            "{}.{}",
            self.history.current().meta.name,
            persist::PROJECT_EXTENSION
        );
        let Some(mut path) = self.dialogs.save_project(&name) else {
            return false;
        };
        if path.extension().is_none() {
            path.set_extension(persist::PROJECT_EXTENSION);
        }
        self.rebase_assets(path.parent());
        self.save_to(&path)
    }

    fn save_to(&mut self, path: &Path) -> bool {
        let ui_state = crate::compat::to_ui_state(&self.project.view, self.dialog_tab);
        match persist::save_with_ui(self.history.current(), Some(&ui_state), path) {
            Ok(()) => {
                // What was autosaved (under the old or the new name) is now
                // in the saved file.
                self.discard_autosave();
                self.history.mark_saved();
                self.project_path = Some(path.to_path_buf());
                self.discard_autosave();
                self.project.missing.invalidate();
                self.info(format!("Saved {}", path.display()));
                true
            }
            Err(e) => {
                self.error(format!("Could not save {}: {e}", path.display()));
                false
            }
        }
    }

    /// Before saving into `new_dir`, rewrites relative asset paths so they
    /// still point at the same files.
    fn rebase_assets(&mut self, new_dir: Option<&Path>) {
        let old_dir = self.asset_root();
        if old_dir.as_deref() == new_dir {
            return;
        }
        let doc = self.history.current().clone();
        let changes: Vec<SetAssetPath> = doc
            .assets
            .iter()
            .filter(|a| Path::new(&a.path).is_relative())
            .map(|a| {
                let absolute = a.resolve(old_dir.as_deref());
                SetAssetPath {
                    asset: a.id,
                    path: portable_path(&absolute, new_dir),
                }
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        self.history.begin("Rebase Asset Paths");
        for c in changes {
            self.execute(c);
        }
        self.history.commit();
    }

    // ---------------------------------------------------------- layers

    pub fn add_solid(&mut self) {
        let Some(comp) = self.require_comp() else {
            return;
        };
        self.execute(AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::from_srgb8(124, 92, 255, 255),
            index: 0,
        });
        self.select_top_layer();
    }

    pub fn import_image_dialog(&mut self) {
        let Some(path) = self.dialogs.pick_image() else {
            return;
        };
        match voxy_media::image_dimensions(&path) {
            Ok((w, h)) => {
                let Some(comp) = self.require_comp() else {
                    return;
                };
                let stored = portable_path(&path, self.asset_root().as_deref());
                self.execute(AddImageLayer {
                    comp,
                    path: stored,
                    pixel_size: [w, h],
                    index: 0,
                });
                self.select_top_layer();
            }
            Err(e) => self.error(format!("Could not import {}: {e}", path.display())),
        }
    }

    pub fn import_video_dialog(&mut self) {
        if let Some(path) = self.dialogs.pick_video() {
            self.import_video(&path);
        }
    }

    /// Adds a layer showing the video at `path`.
    pub fn import_video(&mut self, path: &Path) {
        let info = match voxy_media::video::probe(path) {
            Ok(info) => info,
            Err(e) => {
                self.error(format!("Could not import {}: {e}", path.display()));
                return;
            }
        };
        let Some(video) = info.video else {
            self.error(format!("{} has no video stream", path.display()));
            return;
        };
        let Some(comp) = self.require_comp() else {
            return;
        };
        let stored = portable_path(path, self.asset_root().as_deref());
        self.execute(AddVideoLayer {
            comp,
            path: stored,
            pixel_size: [video.width, video.height],
            duration: video.duration.or(info.duration),
            frame_rate: video.frame_rate,
            index: 0,
        });
        self.select_top_layer();
    }

    /// Deletes selected keyframes if there are any, otherwise the selected layer.
    pub fn delete_selection(&mut self) {
        if crate::timeline::delete_selected_keys(self) {
            return;
        }
        if let Some(id) = self.selection.take() {
            self.execute(RemoveLayers { layers: vec![id] });
        }
    }

    /// Drops selections whose targets no longer exist (after undo etc.).
    pub fn fix_selection(&mut self) {
        if let Some(id) = self.selection
            && self.history.current().find_layer(id).is_none()
        {
            self.selection = None;
        }
        self.project.forget_missing(self.history.current());
    }

    // ------------------------------------------------- Project panel

    /// The folder new items go in: the selected folder, or the folder of the
    /// one selected item, else the top level.
    pub fn target_folder(&self) -> Option<FolderId> {
        let doc = self.history.current();
        let mut it = self.project.selection.items.iter();
        match (it.next(), it.next()) {
            (Some(&PoolItem::Folder(f)), None) => Some(f),
            (Some(&item), None) => doc.item_folder(item),
            _ => None,
        }
    }

    /// Selects what a command just created in the Project panel.
    pub(crate) fn show_created(&mut self, report: &CommandReport) {
        let items: Vec<PoolItem> = report
            .created_folders
            .iter()
            .map(|&f| PoolItem::Folder(f))
            .chain(report.created_comps.iter().map(|&c| PoolItem::Comp(c)))
            .chain(report.created_assets.iter().map(|&a| PoolItem::Asset(a)))
            .collect();
        if items.is_empty() {
            return;
        }
        let rev = self.history.revision();
        let doc = self.history.current().clone();
        self.project.show_items(&doc, rev, &items);
    }

    /// Creates a composition (one undoable command), opens it and selects
    /// it in the Project panel.
    pub fn create_composition(
        &mut self,
        settings: CompSettings,
        folder: Option<FolderId>,
    ) -> Option<CompId> {
        let before = (self.project.selection.items.clone(), self.comp_id());
        let report = self.execute_reported(CreateComposition { settings, folder })?;
        let id = *report.created_comps.first()?;
        self.last_create = Some(CreatedComp {
            comp: id,
            selection: before.0,
            active: before.1,
        });
        self.show_created(&report);
        self.open_comp(id);
        Some(id)
    }

    fn dialog_context(&self) -> DraftContext {
        let doc = self.history.current();
        DraftContext {
            existing_names: doc
                .comps
                .values()
                .map(|c| c.name.clone())
                .chain(doc.assets.iter().map(|a| a.name.clone()))
                .chain(doc.folders.values().map(|f| f.name.clone()))
                .collect(),
            max_texture: self.gpu_max_texture,
        }
    }

    /// Opens the Composition Settings dialog for a new composition, from the
    /// project's defaults ("Comp N" with the next unused number).
    pub fn open_new_comp_dialog(&mut self) {
        let doc = self.history.current();
        let mut settings: CompSettings = doc.meta.settings.new_comp.clone();
        settings.name = next_name(&settings.name, |n| doc.comps.values().any(|c| c.name == n));
        let folder = self.target_folder();
        let draft = Draft::new(
            Mode::New { folder },
            &settings,
            &self.prefs.comp_dialog,
            self.dialog_tab,
            self.dialog_context(),
        );
        self.comp_dialog = Some(draft);
        self.preview = None;
    }

    /// Opens the dialog to edit `comp`.
    pub fn open_comp_settings(&mut self, comp: CompId) {
        let Some(c) = self.history.current().comp(comp) else {
            return;
        };
        let settings = CompSettings::of(c);
        let draft = Draft::new(
            Mode::Edit(comp),
            &settings,
            &self.prefs.comp_dialog,
            self.dialog_tab,
            self.dialog_context(),
        );
        self.comp_dialog = Some(draft);
        self.preview = None;
    }

    /// Undo. When it removes the composition the last create made, the
    /// Project panel selection and the open composition come back too.
    pub fn undo(&mut self) -> bool {
        let done = self.history.undo();
        if let Some(c) = self
            .last_create
            .take_if(|c| !self.history.current().comps.contains_key(&c.comp))
        {
            self.project.selection.items = c.selection;
            self.project.selection.anchor = self.project.selection.items.iter().next().copied();
            self.active = c.active;
        }
        done
    }

    /// Closes the dialog without changing the document.
    pub fn cancel_comp_dialog(&mut self) {
        if let Some(d) = self.comp_dialog.take() {
            self.dialog_tab = d.tab;
        }
        self.preview = None;
    }

    /// OK. A new composition is created at once. An edit that needs a
    /// decision (layers to resize, keyframes off the new grid) first shows
    /// the prompt; otherwise it is applied. Either way the document gets one
    /// command, so one undo step.
    pub fn confirm_comp_dialog(&mut self) {
        let Some(d) = self.comp_dialog.as_ref() else {
            return;
        };
        let Ok(settings) = d.settings() else {
            return;
        };
        match d.mode {
            Mode::New { folder } => {
                let memory = d.memory();
                let tab = d.tab;
                self.comp_dialog = None;
                self.remember(memory, tab);
                self.create_composition(settings, folder);
            }
            Mode::Edit(id) => {
                let Some(comp) = self.history.current().comp(id).cloned() else {
                    self.cancel_comp_dialog();
                    return;
                };
                if d.stage == Stage::Editing && d.needs_confirm(&comp) {
                    if let Some(d) = self.comp_dialog.as_mut() {
                        d.stage = Stage::Confirm;
                    }
                    return;
                }
                let (resize, snap) = (d.resize_mode(), d.snap_keys);
                let (memory, tab) = (d.memory(), d.tab);
                self.comp_dialog = None;
                self.preview = None;
                self.remember(memory, tab);
                self.execute_reported(SetCompositionSettings {
                    comp: id,
                    settings,
                    resize,
                    snap_to_frames: snap,
                });
            }
        }
    }

    fn remember(&mut self, memory: crate::prefs::DialogMemory, tab: DialogTab) {
        self.dialog_tab = tab;
        if self.prefs.comp_dialog != memory {
            self.prefs.comp_dialog = memory;
            self.save_prefs();
        }
    }

    pub fn save_prefs(&mut self) {
        if let Some(path) = self.prefs_path.clone()
            && let Err(e) = self.prefs.save_to(&path)
        {
            self.error(format!("Could not save preferences: {e}"));
        }
    }

    /// Keeps the live preview in step with the dialog: the viewport shows
    /// what OK would produce, without touching the document.
    pub fn update_comp_preview(&mut self) {
        let wanted = self.comp_dialog.as_ref().and_then(|d| {
            let Mode::Edit(id) = d.mode else { return None };
            (d.live_preview && d.stage == Stage::Editing).then_some(())?;
            Some((id, d.settings().ok()?, d.resize_mode()))
        });
        let Some(key) = wanted else {
            self.preview = None;
            return;
        };
        if self.preview.as_ref().is_some_and(|p| p.key == key) {
            return;
        }
        let mut doc = (**self.history.current()).clone();
        let cmd = SetCompositionSettings {
            comp: key.0,
            settings: key.1.clone(),
            resize: key.2,
            snap_to_frames: false,
        };
        if cmd.apply(&mut doc).is_err() {
            self.preview = None;
            return;
        }
        self.preview_counter += 1;
        self.preview = Some(Preview {
            doc: Arc::new(doc),
            key,
            revision: PREVIEW_REVISION_BIT | self.preview_counter,
        });
    }

    /// The document and revision the viewport draws: the live preview of an
    /// open dialog, else the real document.
    pub fn view_document(&self) -> (Arc<Document>, u64) {
        match &self.preview {
            Some(p) => (p.doc.clone(), p.revision),
            None => (self.history.current().clone(), self.history.revision()),
        }
    }

    /// Makes a folder in the selected folder (or the top level) and starts
    /// renaming it.
    pub fn new_folder(&mut self) {
        let doc = self.history.current().clone();
        let parent = self.target_folder();
        let name = next_name("Folder", |n| doc.folders.values().any(|f| f.name == n));
        let Some(report) = self.execute_reported(CreateFolder { name, parent }) else {
            return;
        };
        self.show_created(&report);
        if let Some(&f) = report.created_folders.first() {
            let doc = self.history.current().clone();
            self.project.begin_rename(&doc, PoolItem::Folder(f));
        }
    }

    pub fn import_dialog(&mut self) {
        let paths = self.dialogs.pick_media();
        // Cancelling the picker changes nothing, not even the status bar.
        if !paths.is_empty() {
            self.import_paths(&paths);
        }
    }

    /// Adds files to the project as one undo step. No composition is
    /// created or changed. Files that cannot be read are skipped and named
    /// in the status bar.
    pub fn import_paths(&mut self, paths: &[PathBuf]) {
        let root = self.asset_root();
        let mut assets = Vec::new();
        let mut failures = Vec::new();
        for path in paths {
            match probe_asset(path, root.as_deref()) {
                Ok(asset) => assets.push(asset),
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        if assets.is_empty() {
            if !failures.is_empty() {
                self.error(format!("Could not import {}", failures.join("; ")));
            }
            return;
        }
        let folder = self.target_folder();
        let count = assets.len();
        let Some(report) = self.execute_reported(ImportAssets { assets, folder }) else {
            return;
        };
        self.show_created(&report);
        self.project.missing.invalidate();
        if failures.is_empty() {
            self.info(format!(
                "Imported {count} {}",
                if count == 1 { "file" } else { "files" }
            ));
        } else {
            self.error(format!(
                "Imported {count}; could not import {}",
                failures.join("; ")
            ));
        }
    }

    pub fn rename_item(&mut self, item: PoolItem, name: String) {
        match item {
            PoolItem::Asset(asset) => self.execute(RenameMedia { asset, name }),
            PoolItem::Comp(comp) => self.execute(RenameComposition { comp, name }),
            PoolItem::Folder(folder) => self.execute(RenameFolder { folder, name }),
        }
    }

    /// Moves items into `folder` (`None`: the top level) as one undo step.
    /// Items already there, and a folder dropped on itself, are skipped.
    pub fn move_items(&mut self, items: Vec<PoolItem>, folder: Option<FolderId>) {
        let doc = self.history.current();
        let moving: Vec<PoolItem> = items
            .into_iter()
            .filter(|&i| doc.item_folder(i) != folder && Some(i) != folder.map(PoolItem::Folder))
            .collect();
        if moving.is_empty() {
            return;
        }
        self.execute(MoveToFolder {
            items: moving,
            folder,
        });
        if let Some(f) = folder {
            self.project.view.expanded.insert(f);
        }
    }

    /// Removes items, asking first when layers use them or a folder holds
    /// other items. Never deletes files.
    pub fn request_remove(&mut self, items: Vec<PoolItem>) {
        if items.is_empty() {
            return;
        }
        let doc = self.history.current();
        let impact = doc.removal_impact(&items);
        let ex = doc.expand_items(&items);
        let chosen = items
            .iter()
            .filter(|i| !matches!(i, PoolItem::Folder(_)))
            .count();
        let contained = (ex.assets.len() + ex.comps.len()).saturating_sub(chosen);
        if impact.is_empty() && contained == 0 {
            self.remove_items(items, RemoveMode::PlaceholderLayers);
        } else {
            self.project.pending_remove = Some(PendingRemove {
                layers: impact.total(),
                comps: impact.comp_count(),
                contained,
                items,
            });
        }
    }

    pub fn remove_items(&mut self, items: Vec<PoolItem>, mode: RemoveMode) {
        self.project.pending_remove = None;
        self.execute(RemoveItems { items, mode });
        self.project.missing.invalidate();
    }

    /// Duplicates the selected compositions in one undo step.
    pub fn duplicate_selected(&mut self) {
        let comps: Vec<CompId> = self
            .project
            .selection
            .items
            .iter()
            .filter_map(|i| crate::project_view::comp_of(*i))
            .collect();
        if comps.is_empty() {
            self.error("Only compositions can be duplicated.");
            return;
        }
        self.history.begin("Duplicate Composition");
        let mut created = CommandReport::default();
        for comp in comps {
            match self.history.execute_reported(DuplicateComposition { comp }) {
                Ok(r) => created.created_comps.extend(r.created_comps),
                Err(e) => {
                    self.history.rollback();
                    self.error(e.to_string());
                    return;
                }
            }
        }
        self.history.commit();
        self.show_created(&created);
    }

    /// Relinks a missing or moved file: the item keeps its identity, so
    /// every layer showing it follows.
    pub fn relink(&mut self, asset: AssetId) {
        let Some(path) = self.dialogs.pick_relink() else {
            return;
        };
        let stored = portable_path(&path, self.asset_root().as_deref());
        match probe_asset(&path, self.asset_root().as_deref()) {
            Ok(new) => {
                if self.history.current().assets.get(asset).map(|a| a.kind) != Some(new.kind) {
                    self.error("The new file must be the same kind of media (image or video).");
                    return;
                }
                self.execute_reported(ReplaceMedia {
                    asset,
                    path: stored,
                    pixel_size: new.pixel_size,
                    duration: new.duration,
                    frame_rate: new.frame_rate,
                    audio: None,
                    content_hash: None,
                });
                self.project.missing.invalidate();
            }
            Err(e) => self.error(format!("Could not relink to {}: {e}", path.display())),
        }
    }

    /// Re-checks which files are missing (cheap when nothing changed).
    pub fn refresh_missing(&mut self) {
        let root = self.asset_root();
        let rev = self.history.revision();
        let doc = self.history.current().clone();
        self.project
            .missing
            .refresh(&doc, rev, root.as_deref(), Instant::now(), Path::exists);
    }

    // ----------------------------------------------------------- reveal

    fn selected_layer_ids(&self) -> Vec<LayerId> {
        self.selection.into_iter().collect()
    }

    /// Layer → project (§6.1).
    pub fn reveal_source_in_project(&mut self) {
        let layers = self.selected_layer_ids();
        match reveal::source_targets(self.history.current(), &layers) {
            Ok(items) => {
                let rev = self.history.revision();
                let doc = self.history.current().clone();
                if let Some(msg) = self.project.show_items(&doc, rev, &items) {
                    self.info(msg);
                }
                self.project.focused = true;
            }
            Err(why) => self.error(why),
        }
    }

    /// Where the selected project items are used.
    pub fn comp_targets(&self) -> Vec<CompTarget> {
        let items: Vec<PoolItem> = self.project.selection.items.iter().copied().collect();
        reveal::comp_targets(self.history.current(), &items)
    }

    /// Project → composition (§6.2): opens `target`'s composition and
    /// selects the first of its layers in stack order (the single layer
    /// selection; multi-layer selection arrives with tabs). The playhead
    /// moves to the layer's in-point when it is outside the layer.
    pub fn reveal_in_composition(&mut self, target: &CompTarget) {
        let Some(first) = target.layers.first().copied() else {
            return;
        };
        self.open_comp(target.comp);
        if self.comp_id() != Some(target.comp) {
            return;
        }
        self.selection = Some(first);
        self.timeline.reveal = Some(first);
        if let Some((_, layer)) = self.history.current().find_layer(first)
            && !(layer.in_point <= self.time && self.time < layer.out_point)
        {
            self.time = layer.in_point;
        }
    }

    /// The keyboard command: the composition using the most selected layers.
    pub fn reveal_in_primary_composition(&mut self) {
        let targets = self.comp_targets();
        match reveal::primary_target(&targets) {
            Some(t) => {
                let t = t.clone();
                self.reveal_in_composition(&t);
            }
            None => self.error(reveal::NOT_USED),
        }
    }

    /// The file to show in the file manager, or why there is none. The
    /// selected project items come first, else the selected layer's source.
    pub fn file_manager_target(&self) -> Result<PathBuf, String> {
        let doc = self.history.current();
        let no_file = || "Compositions and folders have no file.".to_owned();
        // The panel with focus decides: items in the Project panel, else the
        // selected layer, else whatever is selected in the panel.
        let from_layer = self.selection.is_some()
            && (self.project.selection.items.is_empty() || !self.project.focused);
        let asset = if from_layer || self.project.selection.items.is_empty() {
            let layer = self.selection.ok_or("Select an item or a layer first.")?;
            let items = reveal::source_targets(doc, &[layer]).map_err(str::to_owned)?;
            match items.first() {
                Some(&PoolItem::Asset(a)) => a,
                _ => return Err(no_file()),
            }
        } else {
            self.project
                .selection
                .items
                .iter()
                .find_map(|i| match i {
                    PoolItem::Asset(a) => Some(*a),
                    _ => None,
                })
                .ok_or_else(no_file)?
        };
        let path = doc
            .assets
            .get(asset)
            .map(|a| a.resolve(self.asset_root().as_deref()))
            .ok_or("File not found")?;
        if path.exists() {
            Ok(path)
        } else {
            Err("File not found".to_owned())
        }
    }

    pub fn reveal_in_file_manager(&mut self) {
        match self.file_manager_target() {
            Ok(path) => {
                let plan = reveal::reveal_plan(reveal::Os::current(), &path);
                if plan.is_empty() {
                    self.error(format!(
                        "Cannot show {} in the file manager.",
                        path.display()
                    ));
                    return;
                }
                std::thread::spawn(move || {
                    if let Err(e) = reveal::run(&plan) {
                        tracing::warn!("could not open the file manager: {e}");
                    }
                });
            }
            Err(why) => self.error(why),
        }
    }

    // ---------------------------------------------------------- actions

    /// What the actions' availability depends on.
    pub fn action_context(&self) -> ActionContext {
        ActionContext {
            has_comp: self.comp_id().is_some(),
            has_layer: self
                .selection
                .is_some_and(|l| self.history.current().find_layer(l).is_some()),
            project_selection: self.project.selection.items.len(),
            selected_segments: self.selected_segments().iter().map(|(_, t)| t.len()).sum(),
        }
    }

    /// The segments the selected keyframes start, grouped by property, in a
    /// fixed order (property id, then time). Keys that no longer exist and
    /// the last key of each property (which starts no segment) are left out.
    pub fn selected_segments(&self) -> Vec<(PropId, Vec<Time>)> {
        let Some((_, layer)) = self
            .selection
            .and_then(|id| self.history.current().find_layer(id))
        else {
            return Vec::new();
        };
        segments_of(&self.timeline.selected_keys, &layer.properties())
    }

    /// Where an easing edit made in the graph editor for the segment
    /// starting at `time` applies: every selected segment if that one is
    /// part of a larger selection, otherwise just that segment.
    pub fn easing_targets(&self, prop: PropId, time: Time) -> Vec<(PropId, Vec<Time>)> {
        let selected = self.selected_segments();
        let in_selection = selected
            .iter()
            .any(|(p, times)| *p == prop && times.contains(&time));
        let many = selected.iter().map(|(_, t)| t.len()).sum::<usize>() > 1;
        if in_selection && many {
            selected
        } else {
            vec![(prop, vec![time])]
        }
    }

    /// Puts `ease` on the clipboard (and its JSON on the system clipboard).
    pub fn copy_easing(&mut self, ease: SegmentEase) {
        self.outgoing_clipboard = serde_json::to_string_pretty(&ease).ok();
        self.ease_clipboard = Some(ease);
        self.info(format!("Copied {} easing.", ease.label()));
    }

    /// Writes `ease` onto `targets` as one undoable step.
    pub fn apply_easing(&mut self, targets: Vec<(PropId, Vec<Time>)>, ease: SegmentEase) {
        self.execute(voxy_core::command::ApplyEasing { targets, ease });
    }

    /// The easing of the first selected segment (the one "Copy Easing"
    /// takes).
    fn first_selected_easing(&self) -> Option<SegmentEase> {
        let (prop, times) = self.selected_segments().into_iter().next()?;
        let (_, layer) = self
            .selection
            .and_then(|id| self.history.current().find_layer(id))?;
        let track = layer
            .properties()
            .into_iter()
            .find(|p| p.id == prop)?
            .track()?;
        track.segment_ease(track.search(*times.first()?).ok()?)
    }

    /// `Ok` if `action` can run now, otherwise the tooltip saying why not.
    pub fn availability(&self, action: Action) -> Result<(), String> {
        action
            .availability(&self.action_context())
            .map_err(str::to_owned)?;
        let doc = self.history.current();
        let selected = &self.project.selection.items;
        match action {
            Action::RevealSourceInProject => {
                reveal::source_targets(doc, &self.selected_layer_ids())
                    .map(|_| ())
                    .map_err(str::to_owned)
            }
            Action::RevealInComposition if self.comp_targets().is_empty() => {
                Err(reveal::NOT_USED.to_owned())
            }
            Action::RevealInFileManager => self.file_manager_target().map(|_| ()),
            Action::EasePaste if self.ease_clipboard.is_none() => {
                Err("Copy an easing first.".to_owned())
            }
            Action::EditUndo if !self.history.can_undo() => Err("Nothing to undo.".to_owned()),
            Action::EditRedo if !self.history.can_undo() && !self.history.can_redo() => {
                Err("Nothing to redo.".to_owned())
            }
            Action::EditRedo if !self.history.can_redo() => Err("Nothing to redo.".to_owned()),
            Action::ProjectRename if selected.len() != 1 => {
                Err("Select one item to rename.".to_owned())
            }
            Action::ProjectDuplicate
                if !selected.iter().any(|i| matches!(i, PoolItem::Comp(_))) =>
            {
                Err("Only compositions can be duplicated.".to_owned())
            }
            _ => Ok(()),
        }
    }

    /// Runs an action. Actions that discard the document or end the
    /// program are returned for the application shell to confirm first.
    /// An action that cannot run reports why in the status bar and does
    /// nothing; no action panics for lack of a composition.
    pub fn perform(&mut self, action: Action) -> Option<AppRequest> {
        if let Err(why) = self.availability(action) {
            self.error(why);
            return None;
        }
        match action {
            Action::FileNewProject => return Some(AppRequest::NewProject),
            Action::FileOpen => return Some(AppRequest::Open),
            Action::FileQuit => return Some(AppRequest::Quit),
            Action::FileNewComposition => self.open_new_comp_dialog(),
            Action::FileNewCompositionFromFootage => self.new_comp_from_footage(),
            Action::CompositionSettings => {
                if let Some(id) = self.comp_id() {
                    self.open_comp_settings(id);
                }
            }
            Action::FileSave => {
                self.save();
            }
            Action::FileSaveAs => {
                self.save_as_dialog();
            }
            Action::FileImport => self.import_dialog(),
            Action::EditUndo => {
                self.undo();
            }
            Action::EditRedo => {
                self.history.redo();
            }
            Action::SelectionClear => {
                if self.project.focused {
                    self.project.selection.clear();
                } else {
                    self.selection = None;
                }
            }
            Action::LayerNewSolid => self.add_solid(),
            Action::LayerImportImage => self.import_image_dialog(),
            Action::LayerImportVideo => self.import_video_dialog(),
            Action::LayerDelete => self.delete_selection(),
            Action::TransportPlayPause => self.toggle_playback(),
            Action::TransportStart => self.time = Time::ZERO,
            Action::TransportStepForward => self.step_frames(1),
            Action::TransportStepBack => self.step_frames(-1),
            Action::ProjectNewFolder => self.new_folder(),
            Action::ProjectRename => {
                if let Some(&item) = self.project.selection.items.iter().next() {
                    let doc = self.history.current().clone();
                    self.project.begin_rename(&doc, item);
                }
            }
            Action::ProjectRemove => {
                let items: Vec<PoolItem> = self.project.selection.items.iter().copied().collect();
                self.request_remove(items);
            }
            Action::ProjectDuplicate => self.duplicate_selected(),
            Action::ProjectSelectAll => {
                let rev = self.history.revision();
                let doc = self.history.current().clone();
                let rows = self.project.rows(&doc, rev);
                self.project.selection.select_all(&rows);
            }
            Action::RevealSourceInProject => self.reveal_source_in_project(),
            Action::RevealInComposition => self.reveal_in_primary_composition(),
            Action::RevealInFileManager => self.reveal_in_file_manager(),
            Action::EaseCopy => {
                if let Some(ease) = self.first_selected_easing() {
                    self.copy_easing(ease);
                }
            }
            Action::EasePaste => {
                if let Some(ease) = self.ease_clipboard {
                    self.apply_easing(self.selected_segments(), ease);
                }
            }
            Action::EaseApplySpring => self.apply_kind(InterpKind::Spring),
            Action::EaseApplyBounce => self.apply_kind(InterpKind::Bounce),
            Action::EaseApplyOscillation => self.apply_kind(InterpKind::Oscillation),
            Action::EaseApplySteps => self.apply_kind(InterpKind::Steps),
            Action::EaseApplySteppedRamp => self.apply_kind(InterpKind::SteppedRamp),
        }
        None
    }
}

/// What autosave remembers between frames.
#[derive(Debug)]
struct Autosaver {
    session: Session,
    /// The revision last written to an autosave file.
    saved_revision: Option<u64>,
    /// When the current stretch of unsaved edits was first noticed.
    dirty_since: Option<Instant>,
    /// A dead session's autosave that this one has recovered and may delete
    /// once its own autosave holds the same edits.
    superseded: Option<Offer>,
}

impl EditorState {
    /// Turns autosave and recovery on, keeping untitled projects' recovery
    /// files in `dir`.
    pub fn enable_recovery(&mut self, dir: PathBuf) {
        self.recovery = Some(Autosaver {
            session: Session::new(dir),
            saved_revision: None,
            dirty_since: None,
            superseded: None,
        });
    }

    /// At launch with no project to open: offers the newest unsaved project
    /// a crashed session left behind.
    pub fn offer_orphaned_recovery(&mut self) {
        if let Some(rec) = &self.recovery
            && self.pending_recovery.is_none()
        {
            self.pending_recovery = recovery::orphans(rec.session.dir()).into_iter().next();
        }
    }

    /// Writes an autosave if the project has been modified for the autosave
    /// interval and nothing is mid-edit. Call once per frame; `busy` is true
    /// while a drag or text entry is in progress or a dialog is open.
    pub fn autosave_tick(&mut self, now: Instant, busy: bool) {
        let secs = self.prefs.autosave_seconds;
        let rev = self.history.revision();
        let dirty = self.history.is_dirty();
        let waiting = self.pending_recovery.is_some();
        let Some(rec) = self.recovery.as_mut() else {
            return;
        };
        if !dirty {
            // Back at the saved state (saved, or everything undone): an
            // autosave of the edits in between must not be offered later.
            rec.dirty_since = None;
            if rec.saved_revision.take().is_some() {
                self.discard_autosave();
            }
            return;
        }
        // While an offer is waiting its file may be the only copy of the
        // user's work, and a titled project's autosave is the same file:
        // never write over it before they decide.
        if secs == 0 || waiting || rec.saved_revision == Some(rev) {
            return;
        }
        let since = *rec.dirty_since.get_or_insert(now);
        if busy || now.duration_since(since) < Duration::from_secs(u64::from(secs)) {
            return;
        }
        let target = match &self.project_path {
            Some(project) => Ok(recovery::autosave_path(project)),
            None => rec.session.untitled_path(),
        };
        let ui_state = crate::compat::to_ui_state(&self.project.view, self.dialog_tab);
        let written = target.map_err(|e| e.to_string()).and_then(|path| {
            persist::save_with_ui(self.history.current(), Some(&ui_state), &path)
                .map(|()| path)
                .map_err(|e| e.to_string())
        });
        match written {
            Ok(_) => {
                rec.saved_revision = Some(rev);
                rec.dirty_since = None;
                if let Some(old) = rec.superseded.take() {
                    recovery::discard(&old);
                }
            }
            Err(e) => {
                tracing::warn!("autosave failed: {e}");
                rec.dirty_since = Some(now); // try again after another interval
            }
        }
    }

    /// Deletes the autosave of the current project (saved: next to it;
    /// untitled: this session's). Used when the project is saved, replaced
    /// or deliberately discarded.
    pub fn discard_autosave(&mut self) {
        if let Some(project) = &self.project_path {
            let _ = std::fs::remove_file(recovery::autosave_path(project));
        }
        if let Some(rec) = self.recovery.as_mut() {
            rec.session.forget_untitled();
            rec.saved_revision = None;
            rec.dirty_since = None;
        }
    }

    /// Loads the offered autosave in place of what is open. The recovered
    /// project counts as unsaved; the file stays until it is saved or
    /// discarded, being the only copy of those edits.
    pub fn recover(&mut self) {
        let Some(offer) = self.pending_recovery.take() else {
            return;
        };
        match persist::load_with_ui(&offer.autosave) {
            Ok((doc, ui_state)) => {
                self.history.reset_unsaved(doc);
                self.project_path.clone_from(&offer.project);
                self.reset_view();
                if let Some(state) = ui_state {
                    self.project.view = crate::compat::from_ui_state(&state);
                    self.dialog_tab = crate::compat::dialog_tab_from(&state);
                }
                self.project.forget_missing(self.history.current());
                if offer.project.is_none()
                    && let Some(rec) = self.recovery.as_mut()
                {
                    // Untitled: this session autosaves under its own name; the
                    // dead session's file goes once ours has the same edits.
                    rec.superseded = Some(offer.clone());
                }
                self.info("Recovered unsaved changes. Save the project to keep them.");
            }
            Err(e) => {
                // Keep the file: a loader bug must not cost the user their work.
                self.error(format!(
                    "Could not recover {}: {e}. The file was left in place.",
                    offer.autosave.display()
                ));
            }
        }
    }

    /// Throws the offered autosave away.
    pub fn discard_recovery(&mut self) {
        if let Some(offer) = self.pending_recovery.take() {
            recovery::discard(&offer);
        }
    }

    /// A clean exit: nothing of this session should be offered next time. Does
    /// nothing while the project has unsaved changes the user has not
    /// explicitly given up (`confirmed`), so a close that was not confirmed
    /// is treated like a crash and recoverable.
    pub fn finish_session(&mut self, confirmed: bool) {
        if self.history.is_dirty() && !confirmed {
            return;
        }
        self.discard_autosave();
        if let Some(rec) = self.recovery.as_mut() {
            rec.session.finish();
        }
    }

    /// Applies `kind` with default parameters to every selected segment.
    fn apply_kind(&mut self, kind: InterpKind) {
        let targets = self.selected_segments();
        self.apply_easing(targets, SegmentEase::new(kind.default_interp()));
    }
}

/// The segments that `keys` start on `props`: for each property that has
/// selected keys, the times of those keys that exist and are not the last
/// key. Properties come out in id order and times ascending, so the result
/// is deterministic.
pub(crate) fn segments_of(
    keys: &std::collections::HashSet<(PropId, Time)>,
    props: &[&voxy_core::Property],
) -> Vec<(PropId, Vec<Time>)> {
    let mut out = Vec::new();
    for prop in props {
        let Some(track) = prop.track() else { continue };
        let last = track.last().time;
        let mut times: Vec<Time> = keys
            .iter()
            .filter(|(p, t)| *p == prop.id && *t != last && track.search(*t).is_ok())
            .map(|(_, t)| *t)
            .collect();
        if !times.is_empty() {
            times.sort();
            out.push((prop.id, times));
        }
    }
    out.sort_by_key(|(p, _)| p.raw());
    out
}

/// `"<base> 1"`, or the smallest number that is not `taken`.
pub(crate) fn next_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut n = 1u64;
    loop {
        let candidate = format!("{base} {n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// How a file is imported, from its extension.
fn media_kind(path: &Path) -> Option<AssetKind> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Some(AssetKind::Image)
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(AssetKind::Video)
    } else {
        None
    }
}

/// Reads a media file's facts into an [`Asset`] (path stored relative to
/// `root` when inside it). Fails with a short reason.
pub(crate) fn probe_asset(path: &Path, root: Option<&Path>) -> Result<Asset, String> {
    let stored = portable_path(path, root);
    let name = path
        .file_name()
        .map_or_else(|| stored.clone(), |n| n.to_string_lossy().into_owned());
    match media_kind(path).ok_or("unsupported file type")? {
        AssetKind::Image => {
            let (w, h) = voxy_media::image_dimensions(path).map_err(|e| e.to_string())?;
            Ok(Asset {
                pixel_size: Some([w, h]),
                ..Asset::new(AssetKind::Image, name, stored)
            })
        }
        AssetKind::Video => {
            let info = voxy_media::video::probe(path).map_err(|e| e.to_string())?;
            let video = info.video.ok_or("no video stream")?;
            Ok(Asset {
                pixel_size: Some([video.width, video.height]),
                duration: video.duration.or(info.duration),
                frame_rate: video.frame_rate,
                ..Asset::new(AssetKind::Video, name, stored)
            })
        }
    }
}

/// Stores `path` relative to `base` with `/` separators when it lies inside
/// it, otherwise as an absolute path.
fn portable_path(path: &Path, base: Option<&Path>) -> String {
    match base.and_then(|b| path.strip_prefix(b).ok()) {
        Some(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        None => path.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::NoDialogs;

    impl EditorState {
        /// A composition from the project defaults, as the dialog would make
        /// with no edits.
        fn new_composition(&mut self, rename: bool) -> Option<CompId> {
            let doc = self.history.current().clone();
            let mut settings = doc.meta.settings.new_comp.clone();
            settings.name = next_name(&settings.name, |n| doc.comps.values().any(|c| c.name == n));
            let folder = self.target_folder();
            let id = self.create_composition(settings, folder)?;
            if rename {
                let doc = self.history.current().clone();
                self.project.begin_rename(&doc, PoolItem::Comp(id));
            }
            Some(id)
        }
    }

    fn state() -> EditorState {
        let mut s = EditorState::new(EditorState::untitled());
        s.dialogs = Box::new(NoDialogs);
        s
    }

    /// A project whose one solid has rotation and opacity keyed at 0 s, 1 s
    /// and 2 s, with the layer selected. Returns the two property ids.
    fn keyed_state() -> (EditorState, PropId, PropId) {
        use voxy_core::command::{AddKeyframe, AddSolidLayer, SetProperty};
        let mut s = state();
        s.new_composition(false).unwrap();
        let comp = s.comp_id().unwrap();
        s.execute(AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        });
        let layer = s.history.current().comp(comp).unwrap().layers[0].clone();
        s.selection = Some(layer.id);
        let (rot, opa) = (layer.transform.rotation.id, layer.transform.opacity.id);
        for prop in [rot, opa] {
            s.execute(AddKeyframe {
                prop,
                time: Time::ZERO,
            });
            for (secs, v) in [(1, 0.5), (2, 1.0)] {
                s.execute(SetProperty {
                    prop,
                    time: Time::from_secs(secs),
                    value: voxy_core::Value::Scalar(v),
                });
            }
        }
        (s, rot, opa)
    }

    fn select(s: &mut EditorState, keys: &[(PropId, i64)]) {
        s.timeline.selected_keys = keys
            .iter()
            .map(|&(p, secs)| (p, Time::from_secs(secs)))
            .collect();
    }

    fn easing_of(s: &EditorState, prop: PropId, segment: usize) -> SegmentEase {
        let layer = &s
            .history
            .current()
            .comp(s.comp_id().unwrap())
            .unwrap()
            .layers[0];
        let p = layer
            .properties()
            .into_iter()
            .find(|p| p.id == prop)
            .unwrap();
        p.track().unwrap().segment_ease(segment).unwrap()
    }

    #[test]
    fn selected_segments_skip_last_keys_and_unknown_keys_in_a_fixed_order() {
        let (mut s, rot, opa) = keyed_state();
        assert_eq!(s.selected_segments().len(), 0);
        // The last key of each property starts no segment; time 7 s has no
        // key at all.
        select(
            &mut s,
            &[(opa, 1), (rot, 2), (rot, 1), (rot, 0), (opa, 2), (rot, 7)],
        );
        let segs = s.selected_segments();
        let expect: Vec<(PropId, Vec<Time>)> = {
            let mut v = vec![
                (rot, vec![Time::ZERO, Time::from_secs(1)]),
                (opa, vec![Time::from_secs(1)]),
            ];
            v.sort_by_key(|(p, _)| p.raw());
            v
        };
        assert_eq!(segs, expect);
        assert_eq!(s.action_context().selected_segments, 3);
        // Selecting only last keys leaves the actions unavailable.
        select(&mut s, &[(rot, 2), (opa, 2)]);
        assert_eq!(s.action_context().selected_segments, 0);
        assert!(s.availability(Action::EaseApplySpring).is_err());
    }

    #[test]
    fn easing_targets_follow_the_selection_only_when_the_segment_is_part_of_it() {
        let (mut s, rot, opa) = keyed_state();
        let t = Time::from_secs;
        select(&mut s, &[(rot, 0), (opa, 0)]);
        // Segment inside a multi-segment selection: all of them.
        assert_eq!(
            s.easing_targets(rot, t(0))
                .iter()
                .map(|(_, v)| v.len())
                .sum::<usize>(),
            2
        );
        // A segment outside the selection, or a selection of one: just it.
        assert_eq!(s.easing_targets(rot, t(1)), vec![(rot, vec![t(1)])]);
        select(&mut s, &[(rot, 0)]);
        assert_eq!(s.easing_targets(rot, t(0)), vec![(rot, vec![t(0)])]);
    }

    #[test]
    fn apply_actions_copy_and_paste_work_on_the_selection_in_one_undo_step() {
        use voxy_core::{Interp, InterpKind};
        let (mut s, rot, opa) = keyed_state();
        // Nothing selected: refused with the reason, nothing changes.
        assert!(s.perform(Action::EaseApplySpring).is_none());
        assert!(s.status.as_ref().is_some_and(|st| st.is_error));
        assert!(s.availability(Action::EasePaste).is_err());

        select(&mut s, &[(rot, 0), (rot, 1), (opa, 0)]);
        let before = s.history.current().clone();
        s.perform(Action::EaseApplyBounce);
        for (prop, seg) in [(rot, 0), (rot, 1), (opa, 0)] {
            assert_eq!(easing_of(&s, prop, seg).interp.kind(), InterpKind::Bounce);
        }
        assert_eq!(easing_of(&s, opa, 1).interp, Interp::Linear, "unselected");
        assert!(s.history.undo());
        assert_eq!(s.history.current(), &before, "one undo reverts all three");

        // Copy the first selected segment (rotation 0 s) and paste it elsewhere.
        select(&mut s, &[(rot, 0)]);
        s.perform(Action::EaseApplySteps);
        s.perform(Action::EaseCopy);
        let copied = s.ease_clipboard.unwrap();
        assert_eq!(copied.interp.kind(), InterpKind::Steps);
        let json = s.outgoing_clipboard.take().unwrap();
        assert_eq!(serde_json::from_str::<SegmentEase>(&json).unwrap(), copied);
        select(&mut s, &[(opa, 0), (opa, 1)]);
        s.perform(Action::EasePaste);
        assert_eq!(easing_of(&s, opa, 0), copied);
        assert_eq!(easing_of(&s, opa, 1), copied);
    }

    #[test]
    fn every_ease_action_is_in_the_menu_table_with_a_unique_id_and_no_default_shortcut() {
        for a in [
            Action::EaseCopy,
            Action::EasePaste,
            Action::EaseApplySpring,
            Action::EaseApplyBounce,
            Action::EaseApplyOscillation,
            Action::EaseApplySteps,
            Action::EaseApplySteppedRamp,
        ] {
            assert!(Action::ALL.contains(&a));
            assert!(a.id().starts_with("ease."), "{a:?}");
            assert_eq!(Action::from_id(a.id()), Some(a));
            assert!(a.def().defaults.is_empty(), "rebindable, not bound");
        }
    }

    // ───────────────────────── autosave and recovery ─────────────────────────

    /// A state with recovery on (folder under `dir`), a 10 s interval and one
    /// composition. `saved` also gives it a project file `P.voxy` in `dir`.
    fn recovering(dir: &Path, saved: bool) -> EditorState {
        let mut s = state();
        s.enable_recovery(dir.join("recovery"));
        s.prefs.autosave_seconds = 10;
        s.new_composition(false).unwrap();
        if saved {
            let path = dir.join("P.voxy");
            persist::save(s.history.current(), &path).unwrap();
            s.open_path(&path);
        }
        s
    }

    fn edit_something(s: &mut EditorState) {
        let comp = s.comp_id().unwrap();
        s.execute(voxy_core::command::AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        });
    }

    #[test]
    fn autosave_waits_for_the_interval_then_writes_beside_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = recovering(dir.path(), true);
        let auto = recovery::autosave_path(&dir.path().join("P.voxy"));
        let t0 = Instant::now();
        let secs = Duration::from_secs;

        // Clean project: nothing to autosave, however long we wait.
        s.autosave_tick(t0 + secs(100), false);
        assert!(!auto.exists());

        edit_something(&mut s);
        s.autosave_tick(t0, false); // first noticed
        s.autosave_tick(t0 + secs(9), false);
        assert!(!auto.exists(), "not before the interval");
        s.autosave_tick(t0 + secs(10), true);
        assert!(
            !auto.exists(),
            "not while something is being dragged or typed"
        );
        s.autosave_tick(t0 + secs(11), false);
        assert!(auto.exists());
        let (saved, _) = persist::load_with_ui(&auto).unwrap();
        assert_eq!(&saved, &**s.history.current(), "it is the open document");

        // The project file itself is untouched; the same state is not rewritten.
        let project = std::fs::read_to_string(dir.path().join("P.voxy")).unwrap();
        assert!(!project.contains("Solid"), "{project}");
        let first = std::fs::metadata(&auto).unwrap().modified().unwrap();
        s.autosave_tick(t0 + secs(200), false);
        assert_eq!(std::fs::metadata(&auto).unwrap().modified().unwrap(), first);

        // A later edit starts a new interval.
        edit_something(&mut s);
        s.autosave_tick(t0 + secs(300), false);
        s.autosave_tick(t0 + secs(309), false);
        assert_eq!(std::fs::metadata(&auto).unwrap().modified().unwrap(), first);
        s.autosave_tick(t0 + secs(310), false);
        let (saved, _) = persist::load_with_ui(&auto).unwrap();
        assert_eq!(&saved, &**s.history.current());
    }

    #[test]
    fn autosave_off_means_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = recovering(dir.path(), true);
        s.prefs.autosave_seconds = 0;
        edit_something(&mut s);
        let t0 = Instant::now();
        s.autosave_tick(t0, false);
        s.autosave_tick(t0 + Duration::from_secs(10_000), false);
        assert!(!recovery::autosave_path(&dir.path().join("P.voxy")).exists());
    }

    #[test]
    fn saving_or_undoing_back_to_clean_deletes_the_autosave() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = recovering(dir.path(), true);
        let auto = recovery::autosave_path(&dir.path().join("P.voxy"));
        let t0 = Instant::now();
        let make = |s: &mut EditorState, at: u64| {
            edit_something(s);
            s.autosave_tick(t0 + Duration::from_secs(at), false);
            s.autosave_tick(t0 + Duration::from_secs(at + 10), false);
        };
        make(&mut s, 0);
        assert!(auto.exists());
        // Everything undone: the document equals the saved file again, so an
        // autosave of the edits in between must not be offered after a crash.
        assert!(s.history.undo());
        s.autosave_tick(t0 + Duration::from_secs(50), false);
        assert!(!auto.exists());
        // Saving removes it too.
        s.history.redo();
        make(&mut s, 100);
        assert!(auto.exists());
        assert!(s.save());
        assert!(!auto.exists());
    }

    #[test]
    fn untitled_projects_autosave_in_the_recovery_folder_and_clean_up_on_exit() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = recovering(dir.path(), false);
        let rec = dir.path().join("recovery");
        edit_something(&mut s);
        let t0 = Instant::now();
        s.autosave_tick(t0, false);
        s.autosave_tick(t0 + Duration::from_secs(10), false);
        let files: Vec<_> = std::fs::read_dir(&rec).unwrap().flatten().collect();
        assert_eq!(files.len(), 2, "autosave and its session lock: {files:?}");
        // This window is alive, so it is not a crash to recover.
        assert_eq!(recovery::orphans(&rec).len(), 0);
        // An unconfirmed close with unsaved changes keeps the file.
        s.finish_session(false);
        assert_eq!(std::fs::read_dir(&rec).unwrap().count(), 2);
        // A confirmed one (saved or "Don't Save") removes everything.
        s.finish_session(true);
        assert_eq!(std::fs::read_dir(&rec).unwrap().count(), 0);
    }

    #[test]
    fn a_newer_autosave_is_offered_on_open_and_never_overwritten_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = recovering(dir.path(), true);
        let project = dir.path().join("P.voxy");
        let auto = recovery::autosave_path(&project);
        edit_something(&mut s);
        let t0 = Instant::now();
        s.autosave_tick(t0, false);
        s.autosave_tick(t0 + Duration::from_secs(10), false);
        let crashed_doc = Document::clone(s.history.current());
        // The window dies; a new one opens the project.
        std::thread::sleep(Duration::from_millis(20));
        let mut s2 = recovering(dir.path(), false);
        s2.pending_recovery = None;
        // (recovering() made a fresh composition; open_path replaces it.)
        s2.open_path(&project);
        assert!(s2.pending_recovery.is_some(), "newer than the project file");
        // The user has not decided yet: edits must not overwrite the file.
        let before = std::fs::read(&auto).unwrap();
        edit_something(&mut s2);
        s2.autosave_tick(t0, false);
        s2.autosave_tick(t0 + Duration::from_secs(1000), false);
        assert_eq!(std::fs::read(&auto).unwrap(), before);
        // Undo that edit and recover: the autosaved document comes back, as
        // an unsaved change to the same project.
        s2.history.undo();
        s2.recover();
        assert!(s2.pending_recovery.is_none());
        assert_eq!(&**s2.history.current(), &crashed_doc);
        assert!(s2.history.is_dirty());
        assert_eq!(s2.project_path.as_deref(), Some(project.as_path()));
        assert!(auto.exists(), "kept until saved");
        assert!(s2.save());
        assert!(!auto.exists());
    }

    #[test]
    fn discarding_an_offer_deletes_it_and_loading_a_bad_one_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        // The project exists on disk but is not open (as at launch).
        let mut s = recovering(dir.path(), false);
        let project = dir.path().join("P.voxy");
        persist::save(s.history.current(), &project).unwrap();
        let auto = recovery::autosave_path(&project);
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&auto, "this is not a project").unwrap();
        s.open_path(&project);
        assert!(s.pending_recovery.is_some());
        let before = Document::clone(s.history.current());
        s.recover();
        assert!(s.status.as_ref().is_some_and(|st| st.is_error));
        assert_eq!(&**s.history.current(), &before, "nothing was replaced");
        assert!(
            auto.exists(),
            "an unreadable autosave is left for inspection"
        );
        s.open_path(&project);
        s.discard_recovery();
        assert!(!auto.exists());
        assert!(s.pending_recovery.is_none());
    }

    #[test]
    fn a_crashed_untitled_project_is_recovered_and_the_old_file_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let rec = dir.path().join("recovery");
        let crashed_doc;
        {
            let mut a = recovering(dir.path(), false);
            edit_something(&mut a);
            let t0 = Instant::now();
            a.autosave_tick(t0, false);
            a.autosave_tick(t0 + Duration::from_secs(10), false);
            crashed_doc = Document::clone(a.history.current());
            // `a` goes away without a clean exit: the process died.
        }
        let mut b = recovering(dir.path(), false);
        b.offer_orphaned_recovery();
        let offer = b
            .pending_recovery
            .clone()
            .expect("the crashed session's copy");
        b.recover();
        assert_eq!(&**b.history.current(), &crashed_doc);
        assert!(b.history.is_dirty() && b.project_path.is_none());
        assert!(
            offer.autosave.exists(),
            "kept until this session has its own copy"
        );
        let t0 = Instant::now();
        b.autosave_tick(t0, false);
        b.autosave_tick(t0 + Duration::from_secs(10), false);
        assert!(
            !offer.autosave.exists(),
            "superseded by this session's autosave"
        );
        assert_eq!(recovery::orphans(&rec).len(), 0);
        b.finish_session(true);
    }

    fn write_png(path: &Path) {
        image::RgbaImage::new(2, 3).save(path).unwrap();
    }

    #[test]
    fn portable_paths() {
        let base = Path::new("/projects/demo");
        assert_eq!(
            portable_path(Path::new("/projects/demo/demo.assets/a.png"), Some(base)),
            "demo.assets/a.png"
        );
        assert_eq!(
            portable_path(Path::new("/elsewhere/b.png"), Some(base)),
            "/elsewhere/b.png"
        );
        assert_eq!(
            portable_path(Path::new("/elsewhere/b.png"), None),
            "/elsewhere/b.png"
        );
    }

    #[test]
    fn a_new_project_has_no_composition() {
        // `untitled()` must never create a composition: the user does.
        let doc = EditorState::untitled();
        assert!(doc.comps.is_empty(), "no default composition");
        assert_eq!(doc.main_comp, None);
        assert!(doc.assets.iter().next().is_none() && doc.folders.is_empty());
        let mut s = state();
        assert!(s.comp_id().is_none() && s.comp().is_none());
        s.new_document();
        assert!(s.history.current().comps.is_empty());
    }

    #[test]
    fn add_and_delete_selection() {
        let mut s = state();
        s.new_composition(false).unwrap();
        s.add_solid();
        assert!(s.selection.is_some());
        s.delete_selection();
        assert_eq!(s.comp().unwrap().layers, []);
        s.history.undo();
        assert_eq!(s.comp().unwrap().layers.len(), 1);
        s.history.undo();
        s.fix_selection();
        assert!(s.selection.is_none());
    }

    #[test]
    fn adding_a_solid_without_a_composition_only_reports() {
        let mut s = state();
        s.add_solid();
        assert!(s.history.current().comps.is_empty());
        assert_eq!(s.status.as_ref().unwrap().text, crate::actions::NEEDS_COMP);
        assert!(!s.history.can_undo());
    }

    #[test]
    fn new_composition_is_numbered_opened_selected_and_renaming() {
        let mut s = state();
        let a = s.new_composition(true).unwrap();
        assert_eq!(s.history.current().comp(a).unwrap().name, "Comp 1");
        assert_eq!(s.comp_id(), Some(a), "opened");
        assert!(
            s.project.selection.items.contains(&PoolItem::Comp(a)),
            "selected"
        );
        assert_eq!(
            s.project.scroll_to,
            Some(PoolItem::Comp(a)),
            "scrolled into view"
        );
        assert_eq!(
            s.project.rename.as_ref().map(|r| r.item),
            Some(PoolItem::Comp(a))
        );
        let b = s.new_composition(false).unwrap();
        assert_eq!(s.history.current().comp(b).unwrap().name, "Comp 2");
        assert_eq!(s.comp_id(), Some(b));
        assert_eq!(
            s.history.current().main_comp,
            Some(a),
            "the first stays main"
        );
        s.history.undo();
        assert_eq!(
            s.comp_id(),
            Some(a),
            "undo falls back to a composition that exists"
        );
    }

    #[test]
    fn deleting_the_last_composition_leaves_none() {
        let mut s = state();
        let a = s.new_composition(false).unwrap();
        s.remove_items(vec![PoolItem::Comp(a)], RemoveMode::PlaceholderLayers);
        assert!(s.history.current().comps.is_empty());
        assert!(s.comp_id().is_none(), "nothing is created in its place");
    }

    #[test]
    fn importing_into_an_empty_project_creates_no_composition() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("pic.png");
        write_png(&img);
        let mut s = state();
        s.import_paths(&[img, dir.path().join("notes.txt")]);
        let doc = s.history.current();
        assert_eq!(doc.assets.iter().count(), 1);
        assert!(doc.comps.is_empty() && doc.main_comp.is_none());
        let status = s.status.as_ref().unwrap();
        assert!(
            status.is_error && status.text.contains("notes.txt"),
            "{status:?}"
        );
        let asset = doc.assets.iter().next().unwrap();
        assert_eq!(
            (asset.name.as_str(), asset.pixel_size),
            ("pic.png", Some([2, 3]))
        );
        assert_eq!(s.project.selection.items.len(), 1, "the import is selected");
        s.history.undo();
        assert!(
            s.history.current().assets.iter().next().is_none(),
            "one undo"
        );
    }

    #[test]
    fn cancelling_the_import_picker_changes_nothing() {
        let mut s = state();
        let before = s.history.current().clone();
        let rev = s.history.revision();
        s.import_dialog();
        assert_eq!(s.history.current(), &before);
        assert_eq!(s.history.revision(), rev);
        assert!(s.status.is_none(), "no message either");
        assert!(s.perform(Action::FileImport).is_none());
        assert!(!s.history.can_undo());
    }

    #[test]
    fn duplicate_move_and_remove_are_single_undo_steps() {
        let mut s = state();
        let c1 = s.new_composition(false).unwrap();
        let c2 = s.new_composition(false).unwrap();
        s.project.selection.set_one(PoolItem::Comp(c1));
        s.project.selection.items.insert(PoolItem::Comp(c2));
        s.duplicate_selected();
        assert_eq!(s.history.current().comps.len(), 4);
        s.history.undo();
        assert_eq!(
            s.history.current().comps.len(),
            2,
            "one undo for both copies"
        );

        s.project.selection.clear();
        s.new_folder();
        let f = *s.history.current().folders.keys().next().unwrap();
        assert!(s.project.rename.is_some(), "a new folder starts renaming");
        s.rename_item(PoolItem::Folder(f), "Shots".into());
        s.move_items(vec![PoolItem::Comp(c1)], Some(f));
        assert_eq!(s.history.current().comp(c1).unwrap().folder, Some(f));
        assert!(
            s.project.view.expanded.contains(&f),
            "the destination opens"
        );
        s.request_remove(vec![PoolItem::Folder(f)]);
        assert!(
            s.project.pending_remove.is_some(),
            "a folder with contents asks first"
        );
        s.remove_items(vec![PoolItem::Folder(f)], RemoveMode::PlaceholderLayers);
        assert!(s.history.current().comp(c1).is_none());
        s.history.undo();
        assert!(
            s.history.current().comp(c1).is_some(),
            "undo restores the folder's contents"
        );
    }

    #[test]
    fn moving_skips_items_already_there_and_a_folder_into_itself() {
        let mut s = state();
        let c = s.new_composition(false).unwrap();
        s.new_folder();
        let f = *s.history.current().folders.keys().next().unwrap();
        let rev = s.history.revision();
        s.move_items(vec![PoolItem::Comp(c)], None);
        s.move_items(vec![PoolItem::Folder(f)], Some(f));
        assert_eq!(s.history.revision(), rev, "nothing to do, nothing recorded");
    }

    #[test]
    fn removing_an_unused_item_needs_no_confirmation() {
        let mut s = state();
        let c = s.new_composition(false).unwrap();
        s.request_remove(vec![PoolItem::Comp(c)]);
        assert!(s.project.pending_remove.is_none());
        assert!(s.history.current().comps.is_empty());
    }

    #[test]
    fn new_items_go_into_the_selected_folder() {
        let mut s = state();
        s.new_folder();
        let f = *s.history.current().folders.keys().next().unwrap();
        s.project.selection.set_one(PoolItem::Folder(f));
        let c = s.new_composition(false).unwrap();
        assert_eq!(s.history.current().comp(c).unwrap().folder, Some(f));
        assert!(
            s.project.view.expanded.contains(&f),
            "the folder opens to show it"
        );
    }

    #[test]
    fn project_panel_settings_save_and_load_with_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.voxy");
        let mut s = state();
        s.new_folder();
        let f = *s.history.current().folders.keys().next().unwrap();
        s.project.view.expanded.insert(f);
        s.project.view.sort = crate::project_view::SortKey::Type;
        assert!(s.save_to(&path));
        let mut t = state();
        t.open_path(&path);
        assert_eq!(t.project.view.sort, crate::project_view::SortKey::Type);
        assert!(t.project.view.expanded.contains(&f));
        assert!(
            t.history.current().comps.is_empty(),
            "saved empty, opens empty"
        );
        assert!(t.comp_id().is_none());
    }

    #[test]
    fn reveal_in_composition_opens_it_selects_the_layer_and_moves_the_playhead() {
        use voxy_core::SourceRef;
        use voxy_core::command::{AddSolidLayer, AddSourceLayer};
        let mut s = state();
        let inner = s.new_composition(false).unwrap();
        let main = s.new_composition(false).unwrap();
        s.execute_reported(AddSourceLayer {
            comp: main,
            source: SourceRef::Comp(inner),
            index: 0,
            in_point: Time::from_secs(2),
        })
        .unwrap();
        s.execute(AddSolidLayer {
            comp: inner,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        });
        s.open_comp(inner);
        s.project.selection.set_one(PoolItem::Comp(inner));
        let before = s.history.revision();
        assert!(s.availability(Action::RevealInComposition).is_ok());
        s.perform(Action::RevealInComposition);
        assert_eq!(s.comp_id(), Some(main));
        assert!(s.selection.is_some());
        assert_eq!(
            s.time,
            Time::from_secs(2),
            "the playhead was outside the layer"
        );
        assert_eq!(s.history.revision(), before, "reveal never enters History");
    }

    #[test]
    fn reveal_source_selects_the_item_in_the_panel() {
        use voxy_core::SourceRef;
        use voxy_core::command::AddSourceLayer;
        let mut s = state();
        let inner = s.new_composition(false).unwrap();
        let main = s.new_composition(false).unwrap();
        s.new_folder();
        let f = *s.history.current().folders.keys().next().unwrap();
        s.move_items(vec![PoolItem::Comp(inner)], Some(f));
        s.project.view.expanded.clear();
        s.project.view.search = "nothing matches".into();
        s.execute_reported(AddSourceLayer {
            comp: main,
            source: SourceRef::Comp(inner),
            index: 0,
            in_point: Time::ZERO,
        })
        .unwrap();
        s.selection = s.comp().unwrap().layers.first().map(|l| l.id);
        s.perform(Action::RevealSourceInProject);
        assert!(s.project.selection.items.contains(&PoolItem::Comp(inner)));
        assert!(
            s.project.view.expanded.contains(&f),
            "ancestor folders open"
        );
        assert!(
            s.project.view.search.is_empty(),
            "a hiding search is cleared"
        );
        assert_eq!(
            s.status.as_ref().unwrap().text,
            crate::project_view::SEARCH_CLEARED
        );
        assert_eq!(s.project.scroll_to, Some(PoolItem::Comp(inner)));
    }

    #[test]
    fn dialog_ok_creates_one_undo_step_and_undo_restores_the_selection() {
        let mut s = state();
        let first = s.new_composition(false).unwrap();
        s.project.selection.set_one(PoolItem::Comp(first));
        s.open_new_comp_dialog();
        let d = s.comp_dialog.as_mut().unwrap();
        assert_eq!(d.name, "Comp 2", "next unused number");
        d.name = "Reel".into();
        d.set_width(1080);
        d.set_height(1920);
        let before = s.history.current().clone();
        s.confirm_comp_dialog();
        assert!(s.comp_dialog.is_none());
        let id = s.comp_id().unwrap();
        let c = s.history.current().comp(id).unwrap();
        assert_eq!((c.name.as_str(), c.width, c.height), ("Reel", 1080, 1920));
        assert!(
            s.project.selection.items.contains(&PoolItem::Comp(id)),
            "selected in the panel"
        );
        assert!(s.project.rename.is_none(), "the name was already chosen");
        assert!(s.undo());
        assert_eq!(s.history.current(), &before, "one undo");
        assert_eq!(s.comp_id(), Some(first));
        assert_eq!(
            s.project.selection.items,
            [PoolItem::Comp(first)].into(),
            "the earlier selection is back"
        );
    }

    #[test]
    fn cancel_changes_nothing() {
        let mut s = state();
        s.new_composition(false).unwrap();
        let before = s.history.current().clone();
        let rev = s.history.revision();
        s.open_new_comp_dialog();
        s.comp_dialog.as_mut().unwrap().name = "Nope".into();
        s.cancel_comp_dialog();
        assert!(s.comp_dialog.is_none());
        assert_eq!(s.history.current(), &before);
        assert_eq!(s.history.revision(), rev);
        let id = s.comp_id().unwrap();
        s.open_comp_settings(id);
        s.comp_dialog.as_mut().unwrap().set_width(640);
        s.update_comp_preview();
        s.cancel_comp_dialog();
        assert_eq!(s.history.current(), &before, "Preview reverts on Cancel");
        assert_eq!(
            s.view_document().1,
            rev,
            "the viewport is back on the real document"
        );
    }

    #[test]
    fn dialog_validation_blocks_ok() {
        let mut s = state();
        s.open_new_comp_dialog();
        s.comp_dialog.as_mut().unwrap().name = "  ".into();
        s.confirm_comp_dialog();
        assert!(s.comp_dialog.is_some(), "still open");
        assert!(s.history.current().comps.is_empty());
    }

    #[test]
    fn preview_shows_the_edit_without_touching_history() {
        let mut s = state();
        let id = s.new_composition(false).unwrap();
        s.add_solid();
        let rev = s.history.revision();
        let before = s.history.current().clone();
        s.open_comp_settings(id);
        let d = s.comp_dialog.as_mut().unwrap();
        d.set_width(1280);
        d.set_height(720);
        d.keep_layers = false;
        s.update_comp_preview();
        let (doc, prev_rev) = s.view_document();
        assert_eq!(
            doc.comp(id).unwrap().width,
            1280,
            "the viewport sees the edit"
        );
        assert_ne!(prev_rev, rev);
        assert!(prev_rev & (1 << 63) != 0, "never equals a real revision");
        assert_eq!(s.history.current(), &before, "the document is untouched");
        // Same draft: same revision. A new edit: a new revision.
        s.update_comp_preview();
        assert_eq!(s.view_document().1, prev_rev);
        s.comp_dialog.as_mut().unwrap().set_width(1000);
        s.update_comp_preview();
        assert_ne!(s.view_document().1, prev_rev);
        // Turning Preview off goes back to the document.
        s.comp_dialog.as_mut().unwrap().live_preview = false;
        s.update_comp_preview();
        assert_eq!(s.view_document().1, rev);
    }

    #[test]
    fn edit_ok_after_preview_is_one_undo_step_and_scales_layers() {
        use voxy_core::LayerKind;
        let mut s = state();
        let id = s.new_composition(false).unwrap();
        s.add_solid();
        let before = s.history.current().clone();
        s.open_comp_settings(id);
        let d = s.comp_dialog.as_mut().unwrap();
        d.set_width(960);
        d.set_height(540);
        d.keep_layers = false;
        s.update_comp_preview();
        s.confirm_comp_dialog();
        let d = s
            .comp_dialog
            .as_ref()
            .expect("a resize with layers asks first");
        assert_eq!(d.stage, Stage::Confirm);
        s.confirm_comp_dialog();
        assert!(s.comp_dialog.is_none());
        let c = s.history.current().comp(id).unwrap();
        assert_eq!((c.width, c.height), (960, 540));
        assert!(matches!(c.layers[0].kind, LayerKind::Solid { .. }));
        assert!(s.history.undo());
        assert_eq!(
            s.history.current(),
            &before,
            "preview plus OK is one undo step"
        );
    }

    #[test]
    fn edit_without_layers_or_keyframes_applies_without_asking() {
        let mut s = state();
        let id = s.new_composition(false).unwrap();
        s.open_comp_settings(id);
        s.comp_dialog.as_mut().unwrap().set_width(800);
        s.confirm_comp_dialog();
        assert!(s.comp_dialog.is_none());
        assert_eq!(s.history.current().comp(id).unwrap().width, 800);
    }

    #[test]
    fn frame_rate_change_keeps_keyframes_in_real_time_and_can_snap() {
        use voxy_core::FrameRate;
        let mut s = state();
        let id = s.new_composition(false).unwrap();
        s.add_solid();
        let layer = s.selection.unwrap();
        let prop = s
            .history
            .current()
            .find_layer(layer)
            .unwrap()
            .1
            .transform
            .opacity
            .id;
        s.history.begin("key");
        s.execute(voxy_core::command::SetAnimated {
            prop,
            time: Time::ZERO,
            animated: true,
        });
        s.execute(voxy_core::command::AddKeyframe {
            prop,
            time: Time::from_millis(500),
        });
        s.history.commit();
        let times = |s: &EditorState| -> Vec<Time> {
            s.history
                .current()
                .property(prop)
                .unwrap()
                .track()
                .unwrap()
                .keys()
                .iter()
                .map(|k| k.time)
                .collect()
        };
        let before = times(&s);
        s.open_comp_settings(id);
        s.comp_dialog.as_mut().unwrap().set_fps_text("29.97".into());
        s.confirm_comp_dialog();
        let d = s
            .comp_dialog
            .as_ref()
            .expect("off-grid keyframes ask first");
        assert_eq!(d.stage, Stage::Confirm);
        s.confirm_comp_dialog();
        assert_eq!(
            s.history.current().comp(id).unwrap().frame_rate,
            FrameRate::FPS_29_97
        );
        assert_eq!(times(&s), before, "keyframes did not move in real time");
        s.history.undo();
        s.open_comp_settings(id);
        let d = s.comp_dialog.as_mut().unwrap();
        d.set_fps_text("29.97".into());
        d.snap_keys = true;
        s.confirm_comp_dialog();
        s.confirm_comp_dialog();
        assert_ne!(times(&s), before, "snapped to the nearest frame");
    }

    #[test]
    fn unavailable_reveals_say_why() {
        let mut s = state();
        assert!(s.availability(Action::RevealSourceInProject).is_err());
        let c = s.new_composition(false).unwrap();
        s.add_solid();
        assert_eq!(
            s.availability(Action::RevealSourceInProject),
            Err("Solids are not project items.".to_owned())
        );
        s.project.selection.set_one(PoolItem::Comp(c));
        s.project.focused = true;
        assert_eq!(
            s.availability(Action::RevealInComposition),
            Err(reveal::NOT_USED.to_owned())
        );
        assert_eq!(
            s.availability(Action::RevealInFileManager),
            Err("Compositions and folders have no file.".to_owned())
        );
    }
}
