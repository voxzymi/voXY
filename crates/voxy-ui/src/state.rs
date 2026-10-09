//! Editor state shared by the menus and panels.
//!
//! Everything that changes the document goes through [`EditorState::execute`]
//! or [`EditorState::execute_reported`] and so through `History`. View state
//! (selection, the open composition, scroll, folder expansion) never does.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use voxy_core::command::{
    AddImageLayer, AddSolidLayer, AddVideoLayer, CreateComposition, CreateFolder,
    DuplicateComposition, ImportAssets, MoveToFolder, RemoveItems, RemoveLayers, RemoveMode,
    RenameComposition, RenameFolder, RenameMedia, ReplaceMedia, SetAssetPath,
};
use voxy_core::{
    Asset, AssetId, AssetKind, Command, CommandReport, CompId, CompSettings, Composition, Document,
    FolderId, FrameRate, History, LayerId, LinearRgba, PoolItem, Time, persist,
};

use crate::actions::{Action, ActionContext, Keymap};
use crate::dialogs::{Dialogs, IMAGE_EXTENSIONS, NativeDialogs, VIDEO_EXTENSIONS};
use crate::project_view::{PendingRemove, ProjectState};
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
        self.history.reset(Self::untitled());
        self.project_path = None;
        self.reset_view();
        self.info("New project");
    }

    pub fn open_path(&mut self, path: &Path) {
        match persist::load_with_ui(path) {
            Ok((doc, ui_state)) => {
                self.history.reset(doc);
                self.project_path = Some(path.to_path_buf());
                self.reset_view();
                if let Some(state) = ui_state {
                    self.project.view = crate::compat::from_ui_state(&state);
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
        let ui_state = crate::compat::to_ui_state(&self.project.view);
        match persist::save_with_ui(self.history.current(), Some(&ui_state), path) {
            Ok(()) => {
                self.history.mark_saved();
                self.project_path = Some(path.to_path_buf());
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
    fn show_created(&mut self, report: &CommandReport) {
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

    /// Creates a composition from the project defaults, opens it, selects
    /// it in the Project panel and, for the button, starts renaming it. The
    /// Composition Settings dialog (Milestone C) will sit in front of this.
    pub fn new_composition(&mut self, rename: bool) -> Option<CompId> {
        let doc = self.history.current().clone();
        let mut settings: CompSettings = doc.meta.settings.new_comp.clone();
        settings.name = next_name(&settings.name, |n| doc.comps.values().any(|c| c.name == n));
        let folder = self.target_folder();
        let report = self.execute_reported(CreateComposition { settings, folder })?;
        let id = *report.created_comps.first()?;
        self.show_created(&report);
        self.open_comp(id);
        if rename {
            let doc = self.history.current().clone();
            self.project.begin_rename(&doc, PoolItem::Comp(id));
        }
        Some(id)
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
        }
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
            Action::FileNewComposition => {
                self.new_composition(true);
            }
            Action::FileSave => {
                self.save();
            }
            Action::FileSaveAs => {
                self.save_as_dialog();
            }
            Action::FileImport => self.import_dialog(),
            Action::EditUndo => {
                self.history.undo();
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
        }
        None
    }
}

/// `"<base> 1"`, or the smallest number that is not `taken`.
fn next_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
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
fn probe_asset(path: &Path, root: Option<&Path>) -> Result<Asset, String> {
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

    fn state() -> EditorState {
        let mut s = EditorState::new(EditorState::untitled());
        s.dialogs = Box::new(NoDialogs);
        s
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
