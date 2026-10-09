//! Snapshot-based undo/redo.

use std::borrow::Cow;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::command::{Command, CommandError, MergeKey};
use crate::document::Document;

/// A lock-free handle other threads use to read the latest document.
pub type SharedDocument = Arc<ArcSwap<Document>>;

#[derive(Debug, Clone)]
struct State {
    doc: Arc<Document>,
    /// Monotonic identifier of this state; changes whenever the document does.
    revision: u64,
}

#[derive(Debug)]
struct Entry {
    state: State,
    label: Cow<'static, str>,
}

#[derive(Debug)]
struct Transaction {
    before: State,
    label: Cow<'static, str>,
    depth: u32,
}

/// The undo history of one document.
///
/// Each step stores the *previous document snapshot*. Snapshots share all
/// unchanged compositions and layers, so a step costs roughly the size of what
/// it changed. Undo swaps the old snapshot back in; there are no hand-written
/// inverse operations to get wrong.
#[derive(Debug)]
pub struct History {
    current: State,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    txn: Option<Transaction>,
    /// Merge key of the most recent step while an interaction is open.
    merge: Option<MergeKey>,
    next_revision: u64,
    saved_revision: Option<u64>,
    max_steps: usize,
    shared: SharedDocument,
}

impl History {
    pub const DEFAULT_MAX_STEPS: usize = 1000;

    /// Starts a history at `doc`, which counts as saved.
    pub fn new(doc: Document) -> Self {
        let doc = Arc::new(doc);
        History {
            shared: Arc::new(ArcSwap::new(doc.clone())),
            current: State { doc, revision: 0 },
            undo: Vec::new(),
            redo: Vec::new(),
            txn: None,
            merge: None,
            next_revision: 1,
            saved_revision: Some(0),
            max_steps: Self::DEFAULT_MAX_STEPS,
        }
    }

    pub fn set_max_steps(&mut self, max: usize) {
        self.max_steps = max.max(1);
        self.trim();
    }

    pub fn current(&self) -> &Arc<Document> {
        &self.current.doc
    }

    /// Identifies the current state. Changes on every edit, undo and redo.
    pub fn revision(&self) -> u64 {
        self.current.revision
    }

    /// Lock-free reader handle, updated on every change.
    pub fn shared(&self) -> SharedDocument {
        self.shared.clone()
    }

    /// Applies `cmd`. On error the document is unchanged.
    ///
    /// If `cmd` has the same [`MergeKey`] as the previous step and
    /// [`History::end_interaction`] has not been called since, the two
    /// collapse into one undo step.
    // Commands are taken by value: history will own them once the command
    // log (bug-report replay) lands.
    #[allow(clippy::needless_pass_by_value)]
    pub fn execute(&mut self, cmd: impl Command) -> Result<(), CommandError> {
        let mut next = Document::clone(&self.current.doc);
        cmd.apply(&mut next)?;
        if cfg!(debug_assertions) {
            next.validate()?;
        }
        if next == *self.current.doc {
            return Ok(());
        }
        let key = cmd.merge_key();
        let merges =
            self.txn.is_none() && key.is_some() && key == self.merge && !self.undo.is_empty();
        let prev = self.replace_current(next);
        if self.txn.is_none() {
            if !merges {
                self.undo.push(Entry {
                    state: prev,
                    label: cmd.label(),
                });
                self.trim();
            }
            self.merge = key;
        }
        self.redo.clear();
        Ok(())
    }

    /// Ends the current interaction: the next mergeable command starts a new
    /// undo step. Call when a drag ends or a field loses focus.
    pub fn end_interaction(&mut self) {
        self.merge = None;
    }

    /// Groups every command until the matching [`History::commit`] into one
    /// undo step. Transactions nest; only the outermost one records a step.
    pub fn begin(&mut self, label: impl Into<Cow<'static, str>>) {
        self.merge = None;
        match &mut self.txn {
            Some(t) => t.depth += 1,
            None => {
                self.txn = Some(Transaction {
                    before: self.current.clone(),
                    label: label.into(),
                    depth: 1,
                });
            }
        }
    }

    pub fn commit(&mut self) {
        let Some(t) = &mut self.txn else { return };
        t.depth -= 1;
        if t.depth > 0 {
            return;
        }
        let t = self.txn.take().expect("checked above");
        if t.before.revision != self.current.revision {
            self.undo.push(Entry {
                state: t.before,
                label: t.label,
            });
            self.trim();
        }
    }

    /// Abandons the outermost transaction and restores the state before it.
    pub fn rollback(&mut self) {
        if let Some(t) = self.txn.take() {
            self.set_state(t.before);
        }
    }

    pub fn in_transaction(&self) -> bool {
        self.txn.is_some()
    }

    pub fn can_undo(&self) -> bool {
        self.txn.is_none() && !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        self.txn.is_none() && !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_ref())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_ref())
    }

    pub fn undo(&mut self) -> bool {
        if !self.can_undo() {
            return false;
        }
        let entry = self.undo.pop().expect("checked");
        let prev = std::mem::replace(&mut self.current, entry.state.clone());
        self.publish();
        self.redo.push(Entry {
            state: prev,
            label: entry.label,
        });
        self.merge = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        if !self.can_redo() {
            return false;
        }
        let entry = self.redo.pop().expect("checked");
        let prev = std::mem::replace(&mut self.current, entry.state.clone());
        self.publish();
        self.undo.push(Entry {
            state: prev,
            label: entry.label,
        });
        self.merge = None;
        true
    }

    /// Records the current state as the saved one.
    pub fn mark_saved(&mut self) {
        self.saved_revision = Some(self.current.revision);
    }

    /// Whether the document differs from the last saved state.
    pub fn is_dirty(&self) -> bool {
        self.saved_revision != Some(self.current.revision)
    }

    /// Replaces the whole document (e.g. after opening a file) and clears the
    /// history.
    pub fn reset(&mut self, doc: Document) {
        // Revisions stay monotonic across resets so observers that cache by
        // revision notice the new document.
        let revision = self.next_revision;
        *self = History {
            max_steps: self.max_steps,
            shared: self.shared.clone(),
            next_revision: revision + 1,
            saved_revision: Some(revision),
            ..History::new(doc)
        };
        self.current.revision = revision;
        self.publish();
    }

    fn replace_current(&mut self, doc: Document) -> State {
        let revision = self.next_revision;
        self.next_revision += 1;
        let prev = std::mem::replace(
            &mut self.current,
            State {
                doc: Arc::new(doc),
                revision,
            },
        );
        self.publish();
        prev
    }

    fn set_state(&mut self, state: State) {
        self.current = state;
        self.publish();
    }

    fn publish(&self) {
        self.shared.store(self.current.doc.clone());
    }

    fn trim(&mut self) {
        if self.undo.len() > self.max_steps {
            let excess = self.undo.len() - self.max_steps;
            self.undo.drain(..excess);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{AddSolidLayer, RemoveLayers, RenameLayer, SetProperty};
    use crate::document::CompSettings;
    use crate::{LinearRgba, Value};

    fn history() -> History {
        History::new(Document::new("Test", CompSettings::default()))
    }

    fn add_solid(h: &mut History) {
        let comp = h.current().root_comp;
        h.execute(AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        })
        .unwrap();
    }

    fn top_layer(h: &History) -> crate::LayerId {
        h.current().root().layers[0].id
    }

    #[test]
    fn undo_redo_restore_exact_snapshots() {
        let mut h = history();
        let s0 = h.current().clone();
        add_solid(&mut h);
        let s1 = h.current().clone();
        assert_eq!(h.undo_label(), Some("Add Solid"));
        assert!(h.undo());
        assert!(Arc::ptr_eq(h.current(), &s0));
        assert!(h.redo());
        assert!(Arc::ptr_eq(h.current(), &s1));
        assert!(!h.redo());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut h = history();
        add_solid(&mut h);
        h.undo();
        assert!(h.can_redo());
        add_solid(&mut h);
        assert!(!h.can_redo());
    }

    #[test]
    fn failed_command_changes_nothing() {
        let mut h = history();
        let rev = h.revision();
        let err = h.execute(RemoveLayers {
            layers: vec![crate::LayerId(999)],
        });
        assert!(err.is_err());
        assert_eq!(h.revision(), rev);
        assert!(!h.can_undo());
    }

    #[test]
    fn mergeable_edits_collapse_until_interaction_ends() {
        let mut h = history();
        add_solid(&mut h);
        let opacity = h.current().root().layers[0].transform.opacity.id;
        for v in [0.9, 0.8, 0.7] {
            h.execute(SetProperty {
                prop: opacity,
                value: Value::Scalar(v),
            })
            .unwrap();
        }
        h.end_interaction();
        h.execute(SetProperty {
            prop: opacity,
            value: Value::Scalar(0.1),
        })
        .unwrap();
        // Add Solid, drag (3 merged), second drag.
        h.undo();
        assert_eq!(
            h.current()
                .property(opacity)
                .unwrap()
                .value_at(crate::Time::ZERO),
            &Value::Scalar(0.7)
        );
        h.undo();
        assert_eq!(
            h.current()
                .property(opacity)
                .unwrap()
                .value_at(crate::Time::ZERO),
            &Value::Scalar(1.0)
        );
        h.undo();
        assert!(h.current().root().layers.is_empty());
        assert!(!h.can_undo());
    }

    #[test]
    fn transactions_group_and_roll_back() {
        let mut h = history();
        h.begin("Add Two");
        add_solid(&mut h);
        h.begin("nested");
        add_solid(&mut h);
        h.commit();
        assert!(h.in_transaction());
        h.commit();
        assert_eq!(h.current().root().layers.len(), 2);
        assert_eq!(h.undo_label(), Some("Add Two"));
        h.undo();
        assert!(h.current().root().layers.is_empty());

        let before = h.current().clone();
        h.begin("Abandoned");
        add_solid(&mut h);
        h.rollback();
        assert!(Arc::ptr_eq(h.current(), &before));
    }

    #[test]
    fn no_op_edits_record_nothing() {
        let mut h = history();
        add_solid(&mut h);
        let layer = top_layer(&h);
        let name = h.current().root().layers[0].name.clone();
        let rev = h.revision();
        h.execute(RenameLayer { layer, name }).unwrap();
        assert_eq!(h.revision(), rev);
    }

    #[test]
    fn dirty_tracking_follows_undo() {
        let mut h = history();
        assert!(!h.is_dirty());
        add_solid(&mut h);
        assert!(h.is_dirty());
        h.mark_saved();
        assert!(!h.is_dirty());
        h.undo();
        assert!(h.is_dirty());
        h.redo();
        assert!(!h.is_dirty());
    }

    #[test]
    fn shared_handle_sees_every_change() {
        let mut h = history();
        let shared = h.shared();
        add_solid(&mut h);
        assert!(Arc::ptr_eq(&shared.load_full(), h.current()));
        h.undo();
        assert!(Arc::ptr_eq(&shared.load_full(), h.current()));
    }

    #[test]
    fn step_limit_drops_oldest() {
        let mut h = history();
        h.set_max_steps(2);
        for _ in 0..4 {
            add_solid(&mut h);
        }
        assert!(h.undo());
        assert!(h.undo());
        assert!(!h.undo());
        assert_eq!(h.current().root().layers.len(), 2);
    }
}
