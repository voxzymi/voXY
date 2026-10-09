//! Editor state shared by the menus and panels.

use std::path::{Path, PathBuf};

use voxy_core::command::{AddImageLayer, AddSolidLayer, RemoveLayers, SetAssetPath};
use voxy_core::{Command, CompSettings, Document, History, LayerId, LinearRgba, Time, persist};

/// A message for the status bar.
#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub is_error: bool,
}

#[derive(Debug)]
pub struct EditorState {
    pub history: History,
    pub project_path: Option<PathBuf>,
    pub selection: Option<LayerId>,
    pub time: Time,
    pub status: Option<Status>,
}

impl EditorState {
    pub fn new(doc: Document) -> Self {
        EditorState {
            history: History::new(doc),
            project_path: None,
            selection: None,
            time: Time::ZERO,
            status: None,
        }
    }

    pub fn untitled() -> Document {
        Document::new("Untitled", CompSettings::default())
    }

    /// Runs a command, reporting failures in the status bar.
    pub fn execute(&mut self, cmd: impl Command) {
        if let Err(e) = self.history.execute(cmd) {
            self.error(e.to_string());
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

    pub fn new_document(&mut self) {
        self.history.reset(Self::untitled());
        self.project_path = None;
        self.selection = None;
        self.time = Time::ZERO;
        self.info("New project");
    }

    pub fn open_path(&mut self, path: &Path) {
        match persist::load(path) {
            Ok(doc) => {
                self.history.reset(doc);
                self.project_path = Some(path.to_path_buf());
                self.selection = None;
                self.time = Time::ZERO;
                self.info(format!("Opened {}", path.display()));
            }
            Err(e) => self.error(format!("Could not open {}: {e}", path.display())),
        }
    }

    pub fn open_dialog(&mut self) {
        if let Some(path) = project_dialog().pick_file() {
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
        let Some(mut path) = project_dialog().set_file_name(name).save_file() else {
            return false;
        };
        if path.extension().is_none() {
            path.set_extension(persist::PROJECT_EXTENSION);
        }
        self.rebase_assets(path.parent());
        self.save_to(&path)
    }

    fn save_to(&mut self, path: &Path) -> bool {
        match persist::save(self.history.current(), path) {
            Ok(()) => {
                self.history.mark_saved();
                self.project_path = Some(path.to_path_buf());
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

    pub fn add_solid(&mut self) {
        let comp = self.history.current().root_comp;
        self.execute(AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::from_srgb8(124, 92, 255, 255),
            index: 0,
        });
        self.selection = self.history.current().root().layers.first().map(|l| l.id);
    }

    pub fn import_image_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import Image")
            .add_filter("Images", &["png", "jpg", "jpeg"])
            .pick_file()
        else {
            return;
        };
        match voxy_media::image_dimensions(&path) {
            Ok((w, h)) => {
                let comp = self.history.current().root_comp;
                let stored = portable_path(&path, self.asset_root().as_deref());
                self.execute(AddImageLayer {
                    comp,
                    path: stored,
                    pixel_size: [w, h],
                    index: 0,
                });
                self.selection = self.history.current().root().layers.first().map(|l| l.id);
            }
            Err(e) => self.error(format!("Could not import {}: {e}", path.display())),
        }
    }

    pub fn delete_selection(&mut self) {
        if let Some(id) = self.selection.take() {
            self.execute(RemoveLayers { layers: vec![id] });
        }
    }

    /// Drops the selection if its layer no longer exists (after undo etc.).
    pub fn fix_selection(&mut self) {
        if let Some(id) = self.selection
            && self.history.current().find_layer(id).is_none()
        {
            self.selection = None;
        }
    }
}

fn project_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter("voXY project", &[persist::PROJECT_EXTENSION])
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
    fn add_and_delete_selection() {
        let mut s = EditorState::new(EditorState::untitled());
        s.add_solid();
        assert!(s.selection.is_some());
        s.delete_selection();
        assert!(s.history.current().root().layers.is_empty());
        s.history.undo();
        assert_eq!(s.history.current().root().layers.len(), 1);
        s.history.undo();
        s.fix_selection();
        assert!(s.selection.is_none());
    }
}
