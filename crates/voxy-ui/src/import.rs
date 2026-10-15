//! Background import (U-A §1): probing a file's metadata and content hash
//! reads the file, which is slow for large videos. That work runs on a worker
//! thread so the UI never freezes; the document is touched only once every
//! file is probed, as one undo step. Cancelling stops the worker and applies
//! nothing, so the document is left exactly as it was.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;

use voxy_core::Asset;
use voxy_core::command::{DropItem, DropTarget, Placement};

use crate::state::{EditorState, expand_dropped_paths, import_summary, probe_asset};

/// Where a finished import puts its assets.
pub enum ImportDest {
    /// Into the Project panel only.
    Project,
    /// Added as layers by a drop (timeline / viewport / paste into a comp).
    Drop {
        target: DropTarget,
        placement: Placement,
    },
}

/// One probed file, in the order the files were submitted.
struct Probed(Result<Asset, String>);

/// A running background import.
pub struct ImportJob {
    dest: ImportDest,
    /// How many files the worker will probe in total.
    total: usize,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Probed>,
    assets: Vec<Asset>,
    failures: Vec<String>,
    /// Unsupported files found while expanding dropped folders.
    skipped: usize,
    /// How many probe results have arrived so far.
    received: usize,
    /// The worker; dropping it detaches the (by then idle) thread.
    handle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for ImportJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportJob")
            .field("total", &self.total)
            .field("received", &self.received)
            .field("assets", &self.assets.len())
            .field("skipped", &self.skipped)
            .finish_non_exhaustive()
    }
}

impl ImportJob {
    /// Files probed so far and the total, for a progress indicator.
    pub fn progress(&self) -> (usize, usize) {
        (self.received, self.total)
    }
}

impl EditorState {
    /// Starts importing `paths` in the background. A dropped folder is
    /// expanded first (fast directory listing); the slow per-file probing
    /// runs on a worker thread.
    pub fn begin_import(&mut self, paths: &[PathBuf], dest: ImportDest) {
        if self.import_job.is_some() {
            self.info("An import is already in progress");
            return;
        }
        let (files, skipped) = expand_dropped_paths(paths);
        if files.is_empty() {
            if skipped > 0 {
                self.info(import_summary(0, 0, skipped, &[]));
            }
            return;
        }
        let total = files.len();
        let root = self.asset_root();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = channel();
        let handle = std::thread::spawn(move || {
            for path in files {
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                let probed = Probed(probe_asset(&path, root.as_deref()));
                // A send error means the job was dropped (cancelled): stop.
                if tx.send(probed).is_err() {
                    break;
                }
            }
        });
        self.import_job = Some(ImportJob {
            dest,
            total,
            cancel,
            rx,
            assets: Vec::new(),
            failures: Vec::new(),
            skipped,
            received: 0,
            handle: Some(handle),
        });
    }

    /// Cancels a running import. The document is left unchanged; the worker
    /// is signalled to stop and then detached.
    pub fn cancel_import(&mut self) {
        if let Some(job) = self.import_job.take() {
            job.cancel.store(true, Ordering::Relaxed);
            self.info("Import cancelled");
        }
    }

    /// Drains probe results and, once every file is probed, applies the
    /// import as one undo step. Called every frame while a job is running;
    /// requests a repaint so progress keeps advancing without user input.
    pub fn poll_import(&mut self, ctx: &egui::Context) {
        let Some(job) = self.import_job.as_mut() else {
            return;
        };
        while let Ok(Probed(result)) = job.rx.try_recv() {
            job.received += 1;
            match result {
                Ok(asset) => job.assets.push(asset),
                Err(e) => job.failures.push(e),
            }
        }
        if job.received < job.total {
            ctx.request_repaint();
            return;
        }
        // Every file is in: apply on the UI thread, then clear the job.
        let job = self.import_job.take().expect("checked above");
        let ImportJob {
            dest,
            assets,
            failures,
            skipped,
            handle,
            ..
        } = job;
        // The worker has sent everything and is exiting; join it so no thread
        // lingers past the import.
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        match dest {
            ImportDest::Project => self.apply_import(assets, skipped, &failures),
            ImportDest::Drop { target, placement } => {
                let items: Vec<DropItem> = assets.into_iter().map(DropItem::File).collect();
                self.perform_drop_inner(target, items, placement, skipped, &failures);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use voxy_core::Document;

    fn write_png(path: &Path, tint: u8) {
        let mut img = image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([tint, tint, tint, 255]));
        img.save(path).unwrap();
    }

    /// Pumps the background job to completion (worker threads are quick on
    /// these tiny files), with a bound so a hang fails instead of spinning.
    fn run_to_completion(state: &mut EditorState) {
        let ctx = egui::Context::default();
        for _ in 0..2000 {
            state.poll_import(&ctx);
            if state.import_job.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("import did not finish");
    }

    #[test]
    fn background_import_applies_as_one_undo_step() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.png");
        write_png(&a, 10);
        write_png(&b, 20);
        let mut state = EditorState::new(Document::empty("t"));
        state.begin_import(&[a, b], ImportDest::Project);
        assert!(state.import_job.is_some());
        run_to_completion(&mut state);
        assert_eq!(state.history.current().assets.iter().count(), 2);
        state.history.undo();
        assert!(
            state.history.current().assets.iter().next().is_none(),
            "the whole import is one undo step"
        );
    }

    #[test]
    fn cancelling_leaves_the_document_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        write_png(&a, 10);
        let mut state = EditorState::new(Document::empty("t"));
        let before = state.history.current().clone();
        state.begin_import(&[a], ImportDest::Project);
        state.cancel_import();
        assert!(state.import_job.is_none());
        // Polling after a cancel must not apply anything.
        let ctx = egui::Context::default();
        state.poll_import(&ctx);
        assert_eq!(state.history.current(), &before);
        assert!(state.history.current().assets.iter().next().is_none());
    }

    #[test]
    fn a_folder_of_only_unsupported_files_imports_nothing_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        let mut state = EditorState::new(Document::empty("t"));
        state.begin_import(&[dir.path().to_path_buf()], ImportDest::Project);
        // The folder expands to zero supported files, so no job even starts.
        assert!(state.import_job.is_none());
        assert!(state.history.current().assets.iter().next().is_none());
        assert!(
            state
                .status
                .as_ref()
                .is_some_and(|s| s.text.contains("unsupported")),
            "it says what was skipped"
        );
    }
}
