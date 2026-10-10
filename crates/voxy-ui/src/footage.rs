//! New Composition From Footage (`docs/design-project-panel.md` §4.3).
//!
//! The footage comes from the Project panel's selection (folders expand to
//! their assets) or, with nothing selected, from a file picker. One item (or
//! several that agree on size, rate and duration) makes one composition with
//! that item's settings. Items that differ ask which one to take the settings
//! from and whether to make one composition or one per item.
//!
//! Importing the files, creating the composition(s) and placing the layers
//! are one transaction, so one undo removes all of it.

use std::fmt::Write as _;
use std::path::PathBuf;

use egui::{Align, ComboBox, Context, Id, Key, Layout, Modal, RadioButton, Ui};
use voxy_core::command::{DropItem, DropItems, DropTarget, Placement, Plan, resolve_drop};
use voxy_core::{Asset, AssetId, FrameRate, PoolItem, Time};

use crate::state::EditorState;

/// What a composition takes from a footage item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub size: [u32; 2],
    pub rate: Option<FrameRate>,
    pub duration: Option<Time>,
}

impl Spec {
    pub fn of(asset: &Asset) -> Spec {
        Spec {
            size: asset.pixel_size.unwrap_or([0, 0]),
            rate: asset.frame_rate,
            duration: asset.duration.filter(|d| *d > Time::ZERO),
        }
    }

    /// "1920×1080, 30 fps, 5 s" (a still has no rate or duration of its own).
    pub fn describe(self) -> String {
        let mut text = format!("{}×{}", self.size[0], self.size[1]);
        if let Some(rate) = self.rate {
            let _ = write!(text, ", {rate}");
        }
        if let Some(d) = self.duration {
            let _ = write!(text, ", {:.2} s", d.as_secs_f64());
        }
        text
    }
}

/// Whether one composition can serve every item as it is.
pub fn all_agree(assets: &[Asset]) -> bool {
    assets
        .windows(2)
        .all(|w| Spec::of(&w[0]) == Spec::of(&w[1]))
}

/// One input: an asset already in the project, or a probed file that the
/// transaction imports.
#[derive(Debug, Clone)]
pub enum Entry {
    Existing(AssetId, Asset),
    New(Asset),
}

impl Entry {
    fn asset(&self) -> &Asset {
        match self {
            Entry::Existing(_, a) | Entry::New(a) => a,
        }
    }
}

/// The question asked when the items differ.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub entries: Vec<Entry>,
    /// Index of the item whose settings the single composition takes.
    pub source: usize,
    pub per_item: bool,
}

impl EditorState {
    /// The assets among `items` (folders expanded, compositions ignored) in
    /// the project's own order.
    fn footage_of(&self, items: &[PoolItem]) -> Vec<Entry> {
        let doc = self.history.current();
        let expanded = doc.expand_items(items);
        doc.assets
            .iter()
            .filter(|a| expanded.assets.contains(&a.id))
            .map(|a| Entry::Existing(a.id, a.clone()))
            .collect()
    }

    /// The assets selected in the Project panel.
    fn selected_footage(&self) -> Vec<Entry> {
        let items: Vec<PoolItem> = self.project.selection.items.iter().copied().collect();
        self.footage_of(&items)
    }

    /// Footage dropped on the empty timeline: a composition from it.
    pub fn new_comp_from_pool_items(&mut self, items: &[PoolItem]) {
        let entries = self.footage_of(items);
        if entries.is_empty() {
            self.error("Drop footage here to make a composition from it.");
            return;
        }
        self.begin_from_footage(entries);
    }

    /// File > New > Composition From Footage. Uses the selected footage, else
    /// asks for files. Cancelling does nothing at all.
    pub fn new_comp_from_footage(&mut self) {
        let selected = self.selected_footage();
        if !selected.is_empty() {
            self.begin_from_footage(selected);
            return;
        }
        let paths = self.dialogs.pick_media();
        if !paths.is_empty() {
            self.new_comp_from_files(&paths);
        }
    }

    /// The same from files (picked or dropped on the tile).
    pub fn new_comp_from_files(&mut self, paths: &[PathBuf]) {
        let root = self.asset_root();
        let mut entries = Vec::new();
        let mut failures = Vec::new();
        for path in paths {
            match crate::state::probe_asset(path, root.as_deref()) {
                Ok(asset) => entries.push(Entry::New(asset)),
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        if entries.is_empty() {
            if !failures.is_empty() {
                self.error(format!("Could not read {}", failures.join("; ")));
            }
            return;
        }
        self.begin_from_footage(entries);
        if !failures.is_empty() {
            // After the success line: say what was skipped.
            let text = self.status.as_ref().map(|s| s.text.clone());
            self.error(format!(
                "{} Skipped {}",
                text.unwrap_or_default(),
                failures.join("; ")
            ));
        }
    }

    /// Makes the composition now, or asks first when the items differ.
    fn begin_from_footage(&mut self, entries: Vec<Entry>) {
        let assets: Vec<Asset> = entries.iter().map(|e| e.asset().clone()).collect();
        if all_agree(&assets) {
            self.finish_from_footage(&entries, 0, false);
        } else {
            self.footage_prompt = Some(Prompt {
                entries,
                source: 0,
                per_item: false,
            });
        }
    }

    pub fn cancel_footage_prompt(&mut self) {
        self.footage_prompt = None;
    }

    pub fn confirm_footage_prompt(&mut self) {
        if let Some(p) = self.footage_prompt.take() {
            self.finish_from_footage(&p.entries, p.source, p.per_item);
        }
    }

    /// Import, create and place as one undo step (`DropItems` onto "nowhere").
    /// Nothing changes if the drop is blocked or anything fails.
    fn finish_from_footage(&mut self, entries: &[Entry], source: usize, per_item: bool) {
        let items: Vec<DropItem> = entries
            .iter()
            .map(|e| match e {
                Entry::Existing(id, _) => DropItem::Asset(*id),
                Entry::New(a) => DropItem::File(a.clone()),
            })
            .collect();
        let target = DropTarget::NewComp { per_item, source };
        if let Plan::Blocked(why) = resolve_drop(
            self.history.current(),
            &target,
            &items,
            Placement::default(),
        ) {
            self.error(why);
            return;
        }
        let folder = self.target_folder();
        let Some(report) = self.execute_reported(DropItems {
            target,
            items,
            placement: Placement::default(),
            folder,
        }) else {
            return;
        };
        self.project.missing.invalidate();
        self.show_created(&report);
        let count = report.created_comps.len();
        if let Some(&first) = report.created_comps.first() {
            self.open_comp(first);
        }
        self.info(if count == 1 {
            "Created a composition from the footage".to_owned()
        } else {
            format!("Created {count} compositions from the footage")
        });
    }
}

/// The question dialog, when one is pending.
pub fn show_prompt(ctx: &Context, state: &mut EditorState) {
    let Some(mut p) = state.footage_prompt.take() else {
        return;
    };
    let mut ok = false;
    let mut cancel = false;
    let modal = Modal::new(Id::new("voxy-footage-prompt")).show(ctx, |ui| {
        ui.set_width(420.0_f32.min((ctx.content_rect().width() - 32.0).max(260.0)));
        ui.heading("Composition From Footage");
        ui.label("These items differ in size, frame rate or duration.");
        ui.add_space(6.0);
        ui.add_enabled_ui(!p.per_item, |ui| {
            ui.label("Use settings from:");
            let current = &p.entries[p.source];
            ComboBox::from_id_salt("footage-source")
                .selected_text(label(current))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for (i, e) in p.entries.iter().enumerate() {
                        ui.selectable_value(&mut p.source, i, label(e));
                    }
                });
        });
        ui.add_space(6.0);
        if ui
            .add(RadioButton::new(!p.per_item, "Create a single composition"))
            .clicked()
        {
            p.per_item = false;
        }
        if ui
            .add(RadioButton::new(
                p.per_item,
                "Create one composition per item",
            ))
            .on_hover_text("Each composition takes its own item's settings")
            .clicked()
        {
            p.per_item = true;
        }
        ui.add_space(8.0);
        buttons(ui, &mut ok, &mut cancel);
    });
    if modal.should_close() || ctx.input(|i| i.key_pressed(Key::Escape)) {
        cancel = true;
    }
    if ctx.input(|i| i.key_pressed(Key::Enter)) {
        ok = true;
    }
    if cancel {
        state.cancel_footage_prompt();
    } else if ok {
        state.footage_prompt = Some(p);
        state.confirm_footage_prompt();
    } else {
        state.footage_prompt = Some(p);
    }
}

fn label(e: &Entry) -> String {
    let a = e.asset();
    format!("{} ({})", a.name, Spec::of(a).describe())
}

fn buttons(ui: &mut Ui, ok: &mut bool, cancel: &mut bool) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.button("OK").clicked() {
            *ok = true;
        }
        if ui.button("Cancel").clicked() {
            *cancel = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use voxy_core::AssetKind;

    fn video(name: &str, w: u32, h: u32, fps: u32, secs: i64) -> Asset {
        Asset {
            pixel_size: Some([w, h]),
            duration: Some(Time::from_secs(secs)),
            frame_rate: Some(FrameRate::new(fps, 1).unwrap()),
            ..Asset::new(AssetKind::Video, name.to_owned(), name.to_owned())
        }
    }

    fn still(name: &str, w: u32, h: u32) -> Asset {
        Asset {
            pixel_size: Some([w, h]),
            ..Asset::new(AssetKind::Image, name.to_owned(), name.to_owned())
        }
    }

    #[test]
    fn items_agree_only_when_size_rate_and_duration_match() {
        let a = video("a.mov", 1920, 1080, 30, 5);
        assert!(all_agree(std::slice::from_ref(&a)));
        assert!(all_agree(&[a.clone(), video("b.mov", 1920, 1080, 30, 5)]));
        assert!(!all_agree(&[a.clone(), video("b.mov", 1920, 1080, 24, 5)]));
        assert!(!all_agree(&[a.clone(), video("b.mov", 1280, 720, 30, 5)]));
        assert!(!all_agree(&[a.clone(), video("b.mov", 1920, 1080, 30, 6)]));
        assert!(!all_agree(&[a, still("c.png", 1920, 1080)]));
        assert!(all_agree(&[still("a.png", 8, 8), still("b.png", 8, 8)]));
    }

    #[test]
    fn the_description_names_what_the_item_has() {
        assert_eq!(
            Spec::of(&video("a.mov", 1920, 1080, 30, 5)).describe(),
            "1920×1080, 30 fps, 5.00 s"
        );
        assert_eq!(Spec::of(&still("a.png", 4, 3)).describe(), "4×3");
    }

    use crate::dialogs::{Dialogs, NoDialogs};
    use voxy_core::{CompSettings, Document, PoolItem};

    /// A picker that returns fixed files.
    #[derive(Debug)]
    struct Picks(Vec<PathBuf>);

    impl Dialogs for Picks {
        fn pick_media(&self) -> Vec<PathBuf> {
            self.0.clone()
        }
        fn pick_image(&self) -> Option<PathBuf> {
            None
        }
        fn pick_video(&self) -> Option<PathBuf> {
            None
        }
        fn pick_relink(&self) -> Option<PathBuf> {
            None
        }
        fn open_project(&self) -> Option<PathBuf> {
            None
        }
        fn save_project(&self, _file_name: &str) -> Option<PathBuf> {
            None
        }
    }

    fn png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let path = dir.join(name);
        image::RgbaImage::new(w, h).save(&path).unwrap();
        path
    }

    fn empty(picks: Vec<PathBuf>) -> EditorState {
        let mut s = EditorState::new(Document::empty("t"));
        s.dialogs = Box::new(Picks(picks));
        s
    }

    #[test]
    fn one_file_makes_one_composition_in_one_undo_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = empty(vec![png(dir.path(), "pic.png", 64, 48)]);
        s.new_comp_from_footage();
        let doc = s.history.current().clone();
        assert_eq!(doc.comps.len(), 1);
        let comp = doc.comps.values().next().unwrap();
        assert_eq!((comp.width, comp.height), (64, 48));
        assert_eq!(comp.name, "pic");
        assert_eq!(comp.layers.len(), 1);
        assert_eq!(doc.assets.iter().count(), 1);
        assert_eq!(s.comp_id(), Some(comp.id), "the composition is opened");
        assert!(!s.status.as_ref().unwrap().is_error);
        s.history.undo();
        let doc = s.history.current();
        assert!(doc.comps.is_empty() && doc.assets.iter().next().is_none());
        assert!(doc.main_comp.is_none());
        assert!(
            !s.history.can_undo(),
            "import, create and place were one step"
        );
    }

    #[test]
    fn cancelling_the_picker_changes_nothing() {
        let mut s = empty(vec![]);
        let rev = s.history.revision();
        s.new_comp_from_footage();
        assert_eq!(s.history.revision(), rev);
        assert!(s.status.is_none() && s.footage_prompt.is_none());
    }

    #[test]
    fn matching_files_stack_with_the_first_on_top() {
        let dir = tempfile::tempdir().unwrap();
        let a = png(dir.path(), "a.png", 32, 32);
        let b = png(dir.path(), "b.png", 32, 32);
        let mut s = empty(vec![a, b]);
        s.new_comp_from_footage();
        assert!(s.footage_prompt.is_none(), "they agree, so no question");
        let doc = s.history.current();
        assert_eq!(doc.comps.len(), 1);
        let layers = &doc.comps.values().next().unwrap().layers;
        let names: Vec<_> = layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["a.png", "b.png"]);
    }

    #[test]
    fn differing_files_ask_and_nothing_changes_until_answered() {
        let dir = tempfile::tempdir().unwrap();
        let a = png(dir.path(), "a.png", 32, 32);
        let b = png(dir.path(), "b.png", 48, 40);
        let mut s = empty(vec![a, b]);
        let rev = s.history.revision();
        s.new_comp_from_footage();
        assert!(s.footage_prompt.is_some());
        assert_eq!(s.history.revision(), rev);
        // Cancel: still nothing.
        s.cancel_footage_prompt();
        assert_eq!(s.history.revision(), rev);
        // Single composition from the second item.
        s.new_comp_from_footage();
        s.footage_prompt.as_mut().unwrap().source = 1;
        s.confirm_footage_prompt();
        let doc = s.history.current();
        assert_eq!(doc.comps.len(), 1);
        let comp = doc.comps.values().next().unwrap();
        assert_eq!((comp.width, comp.height), (48, 40));
        assert_eq!(comp.name, "b");
        assert_eq!(comp.layers.len(), 2);
    }

    #[test]
    fn one_composition_per_item_uses_each_items_own_settings() {
        let dir = tempfile::tempdir().unwrap();
        let a = png(dir.path(), "a.png", 32, 32);
        let b = png(dir.path(), "b.png", 48, 40);
        let mut s = empty(vec![a, b]);
        s.new_comp_from_footage();
        s.footage_prompt.as_mut().unwrap().per_item = true;
        s.confirm_footage_prompt();
        let doc = s.history.current().clone();
        let mut sizes: Vec<_> = doc.comps.values().map(|c| (c.width, c.height)).collect();
        sizes.sort_unstable();
        assert_eq!(sizes, [(32, 32), (48, 40)]);
        assert!(doc.comps.values().all(|c| c.layers.len() == 1));
        s.history.undo();
        assert!(s.history.current().comps.is_empty(), "one undo for both");
    }

    #[test]
    fn selected_footage_is_used_without_a_picker() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = empty(vec![]);
        s.import_paths(&[png(dir.path(), "a.png", 20, 18)]);
        // A picker that would add a second file is never asked.
        s.dialogs = Box::new(Picks(vec![png(dir.path(), "z.png", 16, 16)]));
        assert_eq!(s.project.selection.items.len(), 1, "the import is selected");
        s.new_comp_from_footage();
        let doc = s.history.current();
        assert_eq!(doc.assets.iter().count(), 1, "nothing was imported again");
        let comp = doc.comps.values().next().unwrap();
        assert_eq!((comp.width, comp.height), (20, 18));
    }

    #[test]
    fn selected_compositions_are_ignored_so_the_picker_is_asked() {
        let mut s = empty(vec![]);
        s.dialogs = Box::new(NoDialogs);
        let settings = CompSettings {
            name: "Main".into(),
            ..CompSettings::default()
        };
        let c = s.create_composition(settings, None).unwrap();
        s.project.selection.items.insert(PoolItem::Comp(c));
        let rev = s.history.revision();
        s.new_comp_from_footage();
        assert_eq!(s.history.revision(), rev, "cancelled picker, no change");
    }

    #[test]
    fn unreadable_files_are_named_and_change_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("notes.txt");
        let mut s = empty(vec![bad]);
        let rev = s.history.revision();
        s.new_comp_from_footage();
        assert_eq!(s.history.revision(), rev);
        let status = s.status.as_ref().unwrap();
        assert!(
            status.is_error && status.text.contains("notes.txt"),
            "{status:?}"
        );
    }

    #[test]
    fn a_readable_file_still_works_when_another_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let good = png(dir.path(), "a.png", 16, 16);
        let mut s = empty(vec![good, dir.path().join("notes.txt")]);
        s.new_comp_from_footage();
        assert_eq!(s.history.current().comps.len(), 1);
        let status = s.status.as_ref().unwrap();
        assert!(status.text.contains("notes.txt"), "{status:?}");
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = empty(vec![png(dir.path(), "pic.png", 16, 16)]);
        s.new_comp_from_footage();
        s.new_comp_from_files(&[png(dir.path(), "pic.png", 16, 16)]);
        let mut names: Vec<_> = s
            .history
            .current()
            .comps
            .values()
            .map(|c| c.name.clone())
            .collect();
        names.sort();
        assert_eq!(names, ["pic", "pic 2"]);
    }
}
