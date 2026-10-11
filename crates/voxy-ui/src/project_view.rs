//! Project panel logic: the item tree, sorting, filtering, search,
//! selection and the "missing file" cache. No egui drawing lives here, so
//! every rule is unit tested.
//!
//! The panel lists footage, compositions and folders in one tree
//! (`design-project-panel.md` §2). With a search or a type filter active the
//! tree flattens into a list of matches, as in After Effects.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use voxy_core::{
    AssetId, AssetKind, CompId, Document, FolderId, FrameRate, PoolItem, SourceRef, Time,
};

/// What an item is, for icons, the Type column and filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemKind {
    Folder,
    Composition,
    Image,
    Video,
    /// A colour lookup table for the LUT effect.
    Lut,
}

impl ItemKind {
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Folder => "Folder",
            ItemKind::Composition => "Composition",
            ItemKind::Image => "Image",
            ItemKind::Video => "Video",
            ItemKind::Lut => "LUT",
        }
    }

    /// Footage is media: images and videos.
    pub fn is_footage(self) -> bool {
        matches!(self, ItemKind::Image | ItemKind::Video)
    }

    /// Backed by a file that can go missing: footage and LUTs.
    pub fn is_file(self) -> bool {
        self.is_footage() || self == ItemKind::Lut
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    #[default]
    Name,
    /// Folders, then compositions, then footage.
    Type,
    Size,
    FrameRate,
    Duration,
    Used,
    /// The item's ID: IDs only increase, so this is creation order.
    DateAdded,
}

impl SortKey {
    pub const ALL: [SortKey; 7] = [
        SortKey::Name,
        SortKey::Type,
        SortKey::Size,
        SortKey::FrameRate,
        SortKey::Duration,
        SortKey::Used,
        SortKey::DateAdded,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Type => "Type",
            SortKey::Size => "Size",
            SortKey::FrameRate => "Frame Rate",
            SortKey::Duration => "Duration",
            SortKey::Used => "Used",
            SortKey::DateAdded => "Date Added",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeFilter {
    #[default]
    All,
    Compositions,
    Footage,
    Folders,
    /// Footage whose file cannot be found.
    Missing,
}

impl TypeFilter {
    pub const ALL: [TypeFilter; 5] = [
        TypeFilter::All,
        TypeFilter::Compositions,
        TypeFilter::Footage,
        TypeFilter::Folders,
        TypeFilter::Missing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TypeFilter::All => "All items",
            TypeFilter::Compositions => "Compositions",
            TypeFilter::Footage => "Footage",
            TypeFilter::Folders => "Folders",
            TypeFilter::Missing => "Missing",
        }
    }

    fn accepts(self, kind: ItemKind, missing: bool) -> bool {
        match self {
            TypeFilter::All => true,
            TypeFilter::Compositions => kind == ItemKind::Composition,
            TypeFilter::Footage => kind.is_footage(),
            TypeFilter::Folders => kind == ItemKind::Folder,
            TypeFilter::Missing => kind.is_file() && missing,
        }
    }
}

/// A togglable column. The Name column is always shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    Type,
    Size,
    FrameRate,
    Duration,
    Used,
    DateAdded,
}

impl Column {
    pub const ALL: [Column; 6] = [
        Column::Type,
        Column::Size,
        Column::FrameRate,
        Column::Duration,
        Column::Used,
        Column::DateAdded,
    ];

    pub fn label(self) -> &'static str {
        self.sort_key().label()
    }

    pub fn sort_key(self) -> SortKey {
        match self {
            Column::Type => SortKey::Type,
            Column::Size => SortKey::Size,
            Column::FrameRate => SortKey::FrameRate,
            Column::Duration => SortKey::Duration,
            Column::Used => SortKey::Used,
            Column::DateAdded => SortKey::DateAdded,
        }
    }

    /// Width in points.
    pub fn width(self) -> f32 {
        match self {
            Column::Type => 84.0,
            Column::Size | Column::Duration => 92.0,
            Column::FrameRate => 66.0,
            Column::Used => 44.0,
            Column::DateAdded => 62.0,
        }
    }
}

/// Smallest width the Name column keeps; columns that do not fit next to it
/// are left out, last first.
pub const NAME_MIN_WIDTH: f32 = 140.0;

/// The columns that fit in `width`, in order.
pub fn fitting_columns(wanted: &BTreeSet<Column>, width: f32) -> Vec<Column> {
    let mut room = width - NAME_MIN_WIDTH;
    let mut out = Vec::new();
    for c in Column::ALL.iter().filter(|c| wanted.contains(c)) {
        if c.width() > room {
            break;
        }
        room -= c.width();
        out.push(*c);
    }
    out
}

/// View settings: what `ui_state` stores for the panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewSettings {
    pub sort: SortKey,
    pub descending: bool,
    pub filter: TypeFilter,
    pub search: String,
    pub columns: BTreeSet<Column>,
    pub expanded: BTreeSet<FolderId>,
}

impl Default for ViewSettings {
    fn default() -> Self {
        ViewSettings {
            sort: SortKey::Name,
            descending: false,
            filter: TypeFilter::All,
            search: String::new(),
            columns: [
                Column::Type,
                Column::Size,
                Column::FrameRate,
                Column::Duration,
                Column::Used,
            ]
            .into(),
            expanded: BTreeSet::new(),
        }
    }
}

impl ViewSettings {
    /// Whether the tree is flattened into a list of matches.
    pub fn is_flat(&self) -> bool {
        !self.search.trim().is_empty() || self.filter != TypeFilter::All
    }
}

/// One visible line of the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub item: PoolItem,
    pub depth: usize,
    pub name: String,
    pub kind: ItemKind,
    pub size: Option<[u32; 2]>,
    pub rate: Option<FrameRate>,
    pub duration: Option<Time>,
    /// Layers showing this item; `None` for folders.
    pub used: Option<usize>,
    pub missing: bool,
    pub expandable: bool,
    pub expanded: bool,
}

/// Raw ID of any item, the "date added" order.
pub fn item_raw(item: PoolItem) -> u64 {
    match item {
        PoolItem::Asset(a) => a.raw(),
        PoolItem::Comp(c) => c.raw(),
        PoolItem::Folder(f) => f.raw(),
    }
}

/// Layers per source, one pass over the document.
pub fn usage_counts(doc: &Document) -> HashMap<SourceRef, usize> {
    let mut counts = HashMap::new();
    for comp in doc.comps.values() {
        for layer in &comp.layers {
            if let Some(s) = layer.kind.source() {
                *counts.entry(s).or_insert(0) += 1;
            }
        }
    }
    counts
}

pub(crate) fn make_row(
    doc: &Document,
    item: PoolItem,
    depth: usize,
    usage: &HashMap<SourceRef, usize>,
    missing: &HashSet<AssetId>,
) -> Option<Row> {
    let used = |s: SourceRef| usage.get(&s).copied().unwrap_or(0);
    Some(match item {
        PoolItem::Asset(id) => {
            let a = doc.assets.get(id)?;
            Row {
                item,
                depth,
                name: a.name.clone(),
                kind: match a.kind {
                    AssetKind::Image => ItemKind::Image,
                    AssetKind::Video => ItemKind::Video,
                    AssetKind::Lut => ItemKind::Lut,
                },
                size: a.pixel_size,
                rate: a.frame_rate,
                duration: a.duration,
                used: Some(used(SourceRef::Asset(id))),
                missing: missing.contains(&id),
                expandable: false,
                expanded: false,
            }
        }
        PoolItem::Comp(id) => {
            let c = doc.comp(id)?;
            Row {
                item,
                depth,
                name: c.name.clone(),
                kind: ItemKind::Composition,
                size: Some([c.width, c.height]),
                rate: Some(c.frame_rate),
                duration: Some(c.duration),
                used: Some(used(SourceRef::Comp(id))),
                missing: false,
                expandable: false,
                expanded: false,
            }
        }
        PoolItem::Folder(id) => {
            let f = doc.folder(id)?;
            Row {
                item,
                depth,
                name: f.name.clone(),
                kind: ItemKind::Folder,
                size: None,
                rate: None,
                duration: None,
                used: None,
                missing: false,
                expandable: false,
                expanded: false,
            }
        }
    })
}

fn all_items(doc: &Document) -> impl Iterator<Item = PoolItem> + '_ {
    doc.folders
        .keys()
        .map(|&f| PoolItem::Folder(f))
        .chain(doc.comps.keys().map(|&c| PoolItem::Comp(c)))
        .chain(doc.assets.iter().map(|a| PoolItem::Asset(a.id)))
}

fn type_rank(kind: ItemKind) -> u8 {
    match kind {
        ItemKind::Folder => 0,
        ItemKind::Composition => 1,
        ItemKind::Image => 2,
        ItemKind::Video => 3,
        ItemKind::Lut => 4,
    }
}

/// Orders rows by `key`. The tie-break (name, then ID) is always ascending,
/// so the order is deterministic and stable.
fn compare(a: &Row, b: &Row, key: SortKey, descending: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let pixels = |r: &Row| r.size.map_or(0, |[w, h]| u64::from(w) * u64::from(h));
    let rate = |r: &Row| r.rate.map(FrameRate::as_f64);
    let primary = match key {
        SortKey::Name => Ordering::Equal,
        SortKey::Type => type_rank(a.kind).cmp(&type_rank(b.kind)),
        SortKey::Size => pixels(a).cmp(&pixels(b)),
        SortKey::FrameRate => rate(a).partial_cmp(&rate(b)).unwrap_or(Ordering::Equal),
        SortKey::Duration => a.duration.cmp(&b.duration),
        SortKey::Used => a.used.cmp(&b.used),
        SortKey::DateAdded => item_raw(a.item).cmp(&item_raw(b.item)),
    };
    let primary = if descending {
        primary.reverse()
    } else {
        primary
    };
    primary
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| item_raw(a.item).cmp(&item_raw(b.item)))
}

fn matches_search(doc: &Document, row: &Row, needle: &str) -> bool {
    if needle.is_empty() || row.name.to_lowercase().contains(needle) {
        return true;
    }
    match row.item {
        PoolItem::Asset(id) => doc
            .assets
            .get(id)
            .is_some_and(|a| a.path.to_lowercase().contains(needle)),
        _ => false,
    }
}

/// The visible rows, top to bottom.
///
/// - Tree mode: folders first, then items by the sort key; a folder's
///   children appear only while it is expanded.
/// - Flat mode (search or filter active): every matching item, sorted by the
///   key, with no folder grouping.
pub fn build_rows(
    doc: &Document,
    view: &ViewSettings,
    usage: &HashMap<SourceRef, usize>,
    missing: &HashSet<AssetId>,
) -> Vec<Row> {
    if view.is_flat() {
        let needle = view.search.trim().to_lowercase();
        let mut rows: Vec<Row> = all_items(doc)
            .filter_map(|i| make_row(doc, i, 0, usage, missing))
            .filter(|r| view.filter.accepts(r.kind, r.missing) && matches_search(doc, r, &needle))
            .collect();
        rows.sort_by(|a, b| compare(a, b, view.sort, view.descending));
        return rows;
    }
    let mut out = Vec::new();
    push_children(doc, view, usage, missing, None, 0, &mut out);
    out
}

fn push_children(
    doc: &Document,
    view: &ViewSettings,
    usage: &HashMap<SourceRef, usize>,
    missing: &HashSet<AssetId>,
    parent: Option<FolderId>,
    depth: usize,
    out: &mut Vec<Row>,
) {
    let mut rows: Vec<Row> = all_items(doc)
        .filter(|&i| doc.item_folder(i) == parent)
        .filter_map(|i| make_row(doc, i, depth, usage, missing))
        .collect();
    rows.sort_by(|a, b| {
        (a.kind != ItemKind::Folder)
            .cmp(&(b.kind != ItemKind::Folder))
            .then_with(|| compare(a, b, view.sort, view.descending))
    });
    for mut row in rows {
        if let PoolItem::Folder(f) = row.item {
            let has_children = all_items(doc).any(|i| doc.item_folder(i) == Some(f));
            row.expandable = has_children;
            row.expanded = has_children && view.expanded.contains(&f);
            let expanded = row.expanded;
            out.push(row);
            if expanded {
                push_children(doc, view, usage, missing, Some(f), depth + 1, out);
            }
        } else {
            out.push(row);
        }
    }
}

/// Items directly inside `folder`.
pub fn folder_len(doc: &Document, folder: FolderId) -> usize {
    all_items(doc)
        .filter(|&i| doc.item_folder(i) == Some(folder))
        .count()
}

/// Folders above `item`, nearest first.
pub fn ancestors(doc: &Document, item: PoolItem) -> Vec<FolderId> {
    let mut out = Vec::new();
    let mut cur = doc.item_folder(item);
    while let Some(f) = cur {
        if out.contains(&f) {
            break; // a cycle; validation forbids it
        }
        out.push(f);
        cur = doc.folder(f).and_then(|f| f.parent);
    }
    out
}

// ------------------------------------------------------------ selection

/// The selected items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub items: BTreeSet<PoolItem>,
    /// Where Shift+click ranges start.
    pub anchor: Option<PoolItem>,
}

impl Selection {
    pub fn clear(&mut self) {
        self.items.clear();
        self.anchor = None;
    }

    pub fn set_one(&mut self, item: PoolItem) {
        self.items.clear();
        self.items.insert(item);
        self.anchor = Some(item);
    }

    /// A click on row `index`: plain selects only it, `toggle` (Ctrl/Cmd)
    /// flips it, `range` (Shift) selects from the anchor.
    pub fn click(&mut self, rows: &[Row], index: usize, toggle: bool, range: bool) {
        let Some(row) = rows.get(index) else { return };
        let item = row.item;
        if range
            && let Some(a) = self
                .anchor
                .and_then(|a| rows.iter().position(|r| r.item == a))
        {
            let (lo, hi) = (a.min(index), a.max(index));
            if !toggle {
                self.items.clear();
            }
            self.items.extend(rows[lo..=hi].iter().map(|r| r.item));
        } else if toggle {
            if !self.items.remove(&item) {
                self.items.insert(item);
            }
            self.anchor = Some(item);
        } else {
            self.set_one(item);
        }
    }

    pub fn select_all(&mut self, rows: &[Row]) {
        self.items = rows.iter().map(|r| r.item).collect();
        self.anchor = rows.first().map(|r| r.item);
    }

    /// Drops items that no longer exist (after remove or undo).
    pub fn retain_existing(&mut self, doc: &Document) {
        let exists = |i: &PoolItem| match *i {
            PoolItem::Asset(a) => doc.assets.get(a).is_some(),
            PoolItem::Comp(c) => doc.comp(c).is_some(),
            PoolItem::Folder(f) => doc.folder(f).is_some(),
        };
        self.items.retain(exists);
        if self.anchor.is_some_and(|a| !exists(&a)) {
            self.anchor = self.items.iter().next().copied();
        }
    }

    /// Selected items in the panel's display order; ones not currently
    /// visible follow, by ID. This is the order Reveal and drops use, so
    /// results never depend on click order.
    pub fn in_display_order(&self, rows: &[Row]) -> Vec<PoolItem> {
        let mut out: Vec<PoolItem> = rows
            .iter()
            .map(|r| r.item)
            .filter(|i| self.items.contains(i))
            .collect();
        let rest: Vec<PoolItem> = self
            .items
            .iter()
            .filter(|i| !out.contains(i))
            .copied()
            .collect();
        out.extend(rest);
        out
    }
}

// ------------------------------------------------------------- missing

/// Which footage files cannot be found. This is runtime state: it is never
/// saved and never changes the document.
#[derive(Debug, Default)]
pub struct MissingCache {
    checked: Option<(Instant, u64, Option<PathBuf>)>,
    pub missing: HashSet<AssetId>,
}

/// How long a result is trusted before the files are checked again.
pub const MISSING_RECHECK: Duration = Duration::from_secs(2);

impl MissingCache {
    /// Re-checks the files when the document changed, the project moved, or
    /// the last check is old.
    pub fn refresh(
        &mut self,
        doc: &Document,
        revision: u64,
        root: Option<&Path>,
        now: Instant,
        exists: impl Fn(&Path) -> bool,
    ) {
        if let Some((at, rev, r)) = &self.checked
            && *rev == revision
            && r.as_deref() == root
            && now.duration_since(*at) < MISSING_RECHECK
        {
            return;
        }
        self.missing = doc
            .assets
            .iter()
            .filter(|a| !exists(&a.resolve(root)))
            .map(|a| a.id)
            .collect();
        self.checked = Some((now, revision, root.map(Path::to_path_buf)));
    }

    /// Forces the next [`MissingCache::refresh`] to look at the disk.
    pub fn invalidate(&mut self) {
        self.checked = None;
    }
}

// ------------------------------------------------------------ summaries

/// `1920 x 1080`.
pub fn size_text(size: Option<[u32; 2]>) -> String {
    size.map_or_else(|| "—".to_owned(), |[w, h]| format!("{w} x {h}"))
}

/// `30 fps`, or a dash.
pub fn rate_text(rate: Option<FrameRate>) -> String {
    rate.map_or_else(|| "—".to_owned(), |r| r.to_string())
}

/// `HH:MM:SS:FF` at `rate`, or a dash when unknown. Items without a rate
/// (stills never have a duration) use 30 fps.
pub fn duration_text(duration: Option<Time>, rate: Option<FrameRate>) -> String {
    duration.map_or_else(
        || "—".to_owned(),
        |d| crate::panels::timecode(d, rate.unwrap_or(FrameRate::FPS_30)),
    )
}

/// `1920 x 1080, 30 fps`: the settings summary of a row.
pub fn settings_summary(row: &Row) -> String {
    match (row.size, row.rate) {
        (Some(_), Some(_)) => format!("{}, {}", size_text(row.size), rate_text(row.rate)),
        (Some(_), None) => size_text(row.size),
        _ => String::new(),
    }
}

/// The id of a composition row, if it is one.
pub fn comp_of(item: PoolItem) -> Option<CompId> {
    match item {
        PoolItem::Comp(c) => Some(c),
        _ => None,
    }
}

// -------------------------------------------------------------- state

/// An inline rename in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub item: PoolItem,
    pub text: String,
    /// The text field has not taken focus yet.
    pub fresh: bool,
}

/// A removal waiting for the user's choice about the layers that use the
/// items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRemove {
    pub items: Vec<PoolItem>,
    pub layers: usize,
    pub comps: usize,
    /// Footage and compositions that go with the removed folders.
    pub contained: usize,
}

/// Everything the Project panel remembers between frames.
#[derive(Debug, Default)]
pub struct ProjectState {
    pub view: ViewSettings,
    pub selection: Selection,
    /// Scroll this item into view on the next frame.
    pub scroll_to: Option<PoolItem>,
    pub rename: Option<Rename>,
    pub missing: MissingCache,
    pub pending_remove: Option<PendingRemove>,
    /// The panel has keyboard focus (decides what Delete and Ctrl+A mean).
    pub focused: bool,
    usage: Option<(u64, HashMap<SourceRef, usize>)>,
}

/// Shown when a search or filter was cleared to reveal an item.
pub const SEARCH_CLEARED: &str = "Search cleared to show the item";

impl ProjectState {
    /// The visible rows. Usage counts are recomputed only when the document
    /// revision changes.
    pub fn rows(&mut self, doc: &Document, revision: u64) -> Vec<Row> {
        if self.usage.as_ref().is_none_or(|(r, _)| *r != revision) {
            self.usage = Some((revision, usage_counts(doc)));
        }
        let usage = &self.usage.as_ref().expect("set above").1;
        build_rows(doc, &self.view, usage, &self.missing.missing)
    }

    /// The row for one item, whether or not it is currently visible.
    pub fn row_for(&mut self, doc: &Document, revision: u64, item: PoolItem) -> Option<Row> {
        if self.usage.as_ref().is_none_or(|(r, _)| *r != revision) {
            self.usage = Some((revision, usage_counts(doc)));
        }
        let usage = &self.usage.as_ref().expect("set above").1;
        make_row(doc, item, 0, usage, &self.missing.missing)
    }

    /// Selects `items`, expands the folders above them, clears a search or
    /// filter that hides them, and scrolls to the first in display order.
    /// Returns a status message when something had to be cleared.
    pub fn show_items(
        &mut self,
        doc: &Document,
        revision: u64,
        items: &[PoolItem],
    ) -> Option<&'static str> {
        let items: Vec<PoolItem> = items
            .iter()
            .copied()
            .filter(|i| match i {
                PoolItem::Asset(a) => doc.assets.get(*a).is_some(),
                PoolItem::Comp(c) => doc.comp(*c).is_some(),
                PoolItem::Folder(f) => doc.folder(*f).is_some(),
            })
            .collect();
        if items.is_empty() {
            return None;
        }
        for &i in &items {
            self.view.expanded.extend(ancestors(doc, i));
        }
        let mut rows = self.rows(doc, revision);
        let mut message = None;
        if self.view.is_flat() && items.iter().any(|i| !rows.iter().any(|r| r.item == *i)) {
            self.view.search.clear();
            self.view.filter = TypeFilter::All;
            rows = self.rows(doc, revision);
            message = Some(SEARCH_CLEARED);
        }
        self.selection.clear();
        self.selection.items.extend(items.iter().copied());
        self.selection.anchor = items.first().copied();
        self.scroll_to = self.selection.in_display_order(&rows).first().copied();
        message
    }

    /// Starts an inline rename of `item`.
    pub fn begin_rename(&mut self, doc: &Document, item: PoolItem) {
        let name = match item {
            PoolItem::Asset(a) => doc.assets.get(a).map(|a| a.name.clone()),
            PoolItem::Comp(c) => doc.comp(c).map(|c| c.name.clone()),
            PoolItem::Folder(f) => doc.folder(f).map(|f| f.name.clone()),
        };
        if let Some(text) = name {
            self.rename = Some(Rename {
                item,
                text,
                fresh: true,
            });
        }
    }

    /// Forgets selection, rename and pending removals that point at items
    /// that no longer exist (after remove, undo or opening a project).
    pub fn forget_missing(&mut self, doc: &Document) {
        self.selection.retain_existing(doc);
        let keep = |i: &PoolItem| match *i {
            PoolItem::Asset(a) => doc.assets.get(a).is_some(),
            PoolItem::Comp(c) => doc.comp(c).is_some(),
            PoolItem::Folder(f) => doc.folder(f).is_some(),
        };
        if self.rename.as_ref().is_some_and(|r| !keep(&r.item)) {
            self.rename = None;
        }
        if self.scroll_to.as_ref().is_some_and(|i| !keep(i)) {
            self.scroll_to = None;
        }
        if self
            .pending_remove
            .as_ref()
            .is_some_and(|p| p.items.iter().any(|i| !keep(i)))
        {
            self.pending_remove = None;
        }
        self.view.expanded.retain(|f| doc.folder(*f).is_some());
    }

    /// Counts for the footer: items (footage and compositions) and
    /// compositions.
    pub fn counts(doc: &Document) -> (usize, usize) {
        (doc.assets.iter().count() + doc.comps.len(), doc.comps.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxy_core::command::{
        AddSourceLayer, CreateComposition, CreateFolder, ImportAssets, MoveToFolder, RenameFolder,
    };
    use voxy_core::{Asset, CompSettings, History};

    struct Fixture {
        h: History,
        clips: FolderId,
        inner: FolderId,
        main: CompId,
        title: CompId,
        photo: AssetId,
        movie: AssetId,
        nested: AssetId,
    }

    fn comp(h: &mut History, name: &str, w: u32, fps: FrameRate) -> CompId {
        h.execute_reported(CreateComposition {
            settings: CompSettings {
                name: name.into(),
                width: w,
                height: 1080,
                frame_rate: fps,
                ..CompSettings::default()
            },
            folder: None,
        })
        .unwrap()
        .created_comps[0]
    }

    fn fixture() -> Fixture {
        let mut h = History::new(Document::empty("Test"));
        let main = comp(&mut h, "Main", 1920, FrameRate::FPS_30);
        let title = comp(&mut h, "title card", 1280, FrameRate::FPS_24);
        let mut img = Asset::new(AssetKind::Image, "Photo", "pics/photo.png");
        img.pixel_size = Some([4000, 3000]);
        let mut vid = Asset::new(AssetKind::Video, "movie", "clips/Movie.MP4");
        vid.pixel_size = Some([3840, 2160]);
        vid.frame_rate = Some(FrameRate::FPS_60);
        vid.duration = Some(Time::from_secs(5));
        let mut deep = Asset::new(AssetKind::Image, "Deep", "deep.png");
        deep.pixel_size = Some([10, 10]);
        let ids = h
            .execute_reported(ImportAssets {
                assets: vec![img, vid, deep],
                folder: None,
            })
            .unwrap()
            .created_assets;
        let clips = h
            .execute_reported(CreateFolder {
                name: "Clips".into(),
                parent: None,
            })
            .unwrap()
            .created_folders[0];
        let inner = h
            .execute_reported(CreateFolder {
                name: "Inner".into(),
                parent: Some(clips),
            })
            .unwrap()
            .created_folders[0];
        h.execute(MoveToFolder {
            items: vec![PoolItem::Asset(ids[1])],
            folder: Some(clips),
        })
        .unwrap();
        h.execute(MoveToFolder {
            items: vec![PoolItem::Asset(ids[2])],
            folder: Some(inner),
        })
        .unwrap();
        Fixture {
            h,
            clips,
            inner,
            main,
            title,
            photo: ids[0],
            movie: ids[1],
            nested: ids[2],
        }
    }

    fn rows(f: &Fixture, view: &ViewSettings) -> Vec<Row> {
        let doc = f.h.current();
        build_rows(doc, view, &usage_counts(doc), &HashSet::new())
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn tree_lists_folders_first_then_everything_by_name() {
        let f = fixture();
        let r = rows(&f, &ViewSettings::default());
        assert_eq!(names(&r), vec!["Clips", "Main", "Photo", "title card"]);
        assert!(r[0].expandable && !r[0].expanded, "collapsed by default");
        assert_eq!(r[1].kind, ItemKind::Composition);
        assert_eq!(r[2].kind, ItemKind::Image);
    }

    #[test]
    fn expanded_folders_show_nested_items_with_depth() {
        let f = fixture();
        let mut view = ViewSettings::default();
        view.expanded.extend([f.clips, f.inner]);
        let r = rows(&f, &view);
        assert_eq!(
            names(&r),
            vec![
                "Clips",
                "Inner",
                "Deep",
                "movie",
                "Main",
                "Photo",
                "title card"
            ]
        );
        assert_eq!(
            r.iter().map(|r| r.depth).collect::<Vec<_>>(),
            vec![0, 1, 2, 1, 0, 0, 0]
        );
        // A collapsed parent hides an expanded child.
        view.expanded.remove(&f.clips);
        assert_eq!(names(&rows(&f, &view)).len(), 4);
    }

    #[test]
    fn type_sort_groups_compositions_together() {
        let f = fixture();
        let view = ViewSettings {
            sort: SortKey::Type,
            filter: TypeFilter::Compositions,
            ..ViewSettings::default()
        };
        assert_eq!(names(&rows(&f, &view)), vec!["Main", "title card"]);
        let flat = ViewSettings {
            sort: SortKey::Type,
            search: "e".into(),
            ..ViewSettings::default()
        };
        let kinds: Vec<_> = rows(&f, &flat).iter().map(|r| r.kind).collect();
        let mut sorted = kinds.clone();
        sorted.sort();
        assert_eq!(kinds, sorted, "folders, compositions, then footage");
        // In the tree, Type sort keeps folders on top and groups the rest.
        let tree = ViewSettings {
            sort: SortKey::Type,
            ..ViewSettings::default()
        };
        assert_eq!(
            names(&rows(&f, &tree)),
            vec!["Clips", "Main", "title card", "Photo"]
        );
    }

    #[test]
    fn filters_by_type() {
        let f = fixture();
        let with = |filter| ViewSettings {
            filter,
            ..ViewSettings::default()
        };
        assert_eq!(
            names(&rows(&f, &with(TypeFilter::Footage))),
            vec!["Deep", "movie", "Photo"]
        );
        assert_eq!(
            names(&rows(&f, &with(TypeFilter::Folders))),
            vec!["Clips", "Inner"]
        );
        assert_eq!(
            names(&rows(&f, &with(TypeFilter::Compositions))),
            vec!["Main", "title card"]
        );
        assert_eq!(rows(&f, &with(TypeFilter::Missing)).len(), 0);
        let doc = f.h.current();
        let missing: HashSet<_> = [f.photo].into();
        let r = build_rows(
            doc,
            &with(TypeFilter::Missing),
            &usage_counts(doc),
            &missing,
        );
        assert_eq!(names(&r), vec!["Photo"]);
        assert!(r[0].missing);
    }

    #[test]
    fn search_is_case_insensitive_flat_and_matches_file_names() {
        let f = fixture();
        let view = ViewSettings {
            search: "  MOV ".into(),
            ..ViewSettings::default()
        };
        let r = rows(&f, &view);
        assert_eq!(names(&r), vec!["movie"]);
        assert_eq!(r[0].depth, 0, "matches are listed flat");
        let by_path = ViewSettings {
            search: "pics/".into(),
            ..ViewSettings::default()
        };
        assert_eq!(names(&rows(&f, &by_path)), vec!["Photo"]);
        let both = ViewSettings {
            search: "t".into(),
            filter: TypeFilter::Compositions,
            ..ViewSettings::default()
        };
        assert_eq!(names(&rows(&f, &both)), vec!["title card"]);
    }

    #[test]
    fn sorting_by_every_column_is_deterministic_and_can_reverse() {
        let f = fixture();
        let flat = |sort, descending| ViewSettings {
            sort,
            descending,
            filter: TypeFilter::Footage,
            ..ViewSettings::default()
        };
        // Size: Deep (100 px) < Photo (12 MP) < movie (8.3 MP)? 4000x3000 = 12M > 3840x2160 = 8.3M.
        assert_eq!(
            names(&rows(&f, &flat(SortKey::Size, false))),
            vec!["Deep", "movie", "Photo"]
        );
        assert_eq!(
            names(&rows(&f, &flat(SortKey::Size, true))),
            vec!["Photo", "movie", "Deep"]
        );
        // Frame rate: stills have none and sort first.
        assert_eq!(
            names(&rows(&f, &flat(SortKey::FrameRate, false))),
            vec!["Deep", "Photo", "movie"]
        );
        assert_eq!(names(&rows(&f, &flat(SortKey::Duration, true)))[0], "movie");
        // Date added is creation order (IDs): Photo, movie, Deep.
        assert_eq!(
            names(&rows(&f, &flat(SortKey::DateAdded, false))),
            vec!["Photo", "movie", "Deep"]
        );
        // Reversing keeps the tie-break ascending: equal keys stay by name.
        let desc = rows(&f, &flat(SortKey::Used, true));
        assert_eq!(
            names(&desc),
            vec!["Deep", "movie", "Photo"],
            "all unused: by name"
        );
    }

    #[test]
    fn used_counts_layers_and_folders_have_none() {
        let mut f = fixture();
        for _ in 0..2 {
            f.h.execute(AddSourceLayer {
                comp: f.main,
                source: SourceRef::Asset(f.photo),
                index: 0,
                in_point: Time::ZERO,
            })
            .unwrap();
        }
        f.h.execute(AddSourceLayer {
            comp: f.main,
            source: SourceRef::Comp(f.title),
            index: 0,
            in_point: Time::ZERO,
        })
        .unwrap();
        let r = rows(&f, &ViewSettings::default());
        let used = |n: &str| r.iter().find(|r| r.name == n).unwrap().used;
        assert_eq!(used("Photo"), Some(2));
        assert_eq!(used("title card"), Some(1));
        assert_eq!(used("Main"), Some(0));
        assert_eq!(used("Clips"), None);
    }

    #[test]
    fn composition_rows_carry_size_rate_and_duration() {
        let f = fixture();
        let r = rows(&f, &ViewSettings::default());
        let title = r.iter().find(|r| r.name == "title card").unwrap();
        assert_eq!(settings_summary(title), "1280 x 1080, 24 fps");
        assert_eq!(duration_text(title.duration, title.rate), "0:00:10:00");
        assert_eq!(duration_text(None, None), "—");
        assert_eq!(size_text(None), "—");
    }

    #[test]
    fn columns_that_do_not_fit_are_left_out_last_first() {
        let view = ViewSettings::default();
        let all = fitting_columns(&view.columns, 2000.0);
        assert_eq!(
            all,
            vec![
                Column::Type,
                Column::Size,
                Column::FrameRate,
                Column::Duration,
                Column::Used
            ]
        );
        assert_eq!(fitting_columns(&view.columns, 100.0).len(), 0);
        let narrow = fitting_columns(&view.columns, NAME_MIN_WIDTH + 84.0 + 92.0);
        assert_eq!(narrow, vec![Column::Type, Column::Size]);
        let some: BTreeSet<_> = [Column::Used, Column::DateAdded].into();
        assert_eq!(
            fitting_columns(&some, 2000.0),
            vec![Column::Used, Column::DateAdded]
        );
    }

    #[test]
    fn selection_click_toggle_and_range() {
        let f = fixture();
        let r = rows(&f, &ViewSettings::default()); // Clips, Main, Photo, title card
        let mut s = Selection::default();
        s.click(&r, 1, false, false);
        assert_eq!(s.items.len(), 1);
        s.click(&r, 3, true, false);
        assert_eq!(s.items.len(), 2, "ctrl adds");
        s.click(&r, 3, true, false);
        assert_eq!(s.items.len(), 1, "ctrl again removes");
        s.click(&r, 1, false, false);
        s.click(&r, 3, false, true);
        assert_eq!(
            s.items.iter().copied().collect::<BTreeSet<_>>(),
            r[1..=3].iter().map(|r| r.item).collect(),
            "shift selects from the anchor"
        );
        s.click(&r, 0, false, false);
        assert_eq!(s.items.len(), 1);
        s.select_all(&r);
        assert_eq!(s.items.len(), 4);
        s.clear();
        assert!(s.items.is_empty() && s.anchor.is_none());
    }

    #[test]
    fn display_order_ignores_click_order() {
        let f = fixture();
        let r = rows(&f, &ViewSettings::default());
        let mut s = Selection::default();
        s.click(&r, 3, false, false);
        s.click(&r, 1, true, false);
        assert_eq!(
            s.in_display_order(&r),
            vec![r[1].item, r[3].item],
            "panel order, not click order"
        );
        // Hidden selected items follow, by ID.
        s.items.insert(PoolItem::Asset(f.nested));
        s.items.insert(PoolItem::Asset(f.movie));
        let order = s.in_display_order(&r);
        assert_eq!(
            &order[2..],
            &[PoolItem::Asset(f.movie), PoolItem::Asset(f.nested)]
        );
    }

    #[test]
    fn selection_forgets_removed_items() {
        let mut f = fixture();
        let mut s = Selection::default();
        s.items
            .extend([PoolItem::Comp(f.main), PoolItem::Asset(f.photo)]);
        s.anchor = Some(PoolItem::Comp(f.main));
        f.h.execute(voxy_core::command::RemoveItems {
            items: vec![PoolItem::Comp(f.main)],
            mode: voxy_core::command::RemoveMode::PlaceholderLayers,
        })
        .unwrap();
        s.retain_existing(f.h.current());
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.anchor, Some(PoolItem::Asset(f.photo)));
    }

    #[test]
    fn ancestors_are_nearest_first() {
        let f = fixture();
        assert_eq!(
            ancestors(f.h.current(), PoolItem::Asset(f.nested)),
            vec![f.inner, f.clips]
        );
        assert_eq!(ancestors(f.h.current(), PoolItem::Asset(f.photo)).len(), 0);
    }

    #[test]
    fn renaming_a_folder_does_not_change_its_place() {
        let mut f = fixture();
        f.h.execute(RenameFolder {
            folder: f.clips,
            name: "Zebra".into(),
        })
        .unwrap();
        let r = rows(&f, &ViewSettings::default());
        assert_eq!(r[0].name, "Zebra", "folders still first");
    }

    #[test]
    fn missing_files_are_cached_and_rechecked() {
        let f = fixture();
        let doc = f.h.current();
        let mut cache = MissingCache::default();
        let t0 = Instant::now();
        let gone = |p: &Path| !p.ends_with("pics/photo.png");
        cache.refresh(doc, 1, None, t0, gone);
        assert_eq!(cache.missing, [f.photo].into());
        // Same revision, same root, soon after: the disk is not asked again.
        cache.refresh(doc, 1, None, t0 + Duration::from_millis(500), |_| {
            panic!("cached")
        });
        assert_eq!(cache.missing, [f.photo].into());
        // A new revision or an old result checks again.
        cache.refresh(doc, 2, None, t0 + Duration::from_millis(600), |_| true);
        assert!(cache.missing.is_empty());
        cache.refresh(
            doc,
            2,
            None,
            t0 + MISSING_RECHECK + Duration::from_secs(1),
            |_| false,
        );
        assert_eq!(cache.missing.len(), 3);
        // Moving the project changes the root, so paths resolve again.
        let seen = std::cell::RefCell::new(Vec::new());
        cache.refresh(
            doc,
            2,
            Some(Path::new("/proj")),
            t0 + MISSING_RECHECK + Duration::from_secs(1),
            |p| {
                seen.borrow_mut().push(p.to_path_buf());
                true
            },
        );
        assert!(
            seen.borrow()
                .iter()
                .all(|p| p.starts_with("/proj") || p.is_absolute())
        );
        cache.invalidate();
        cache.refresh(doc, 2, Some(Path::new("/proj")), t0, |_| false);
        assert_eq!(cache.missing.len(), 3);
    }

    #[test]
    fn settings_round_trip_as_json() {
        let v = ViewSettings {
            sort: SortKey::DateAdded,
            filter: TypeFilter::Missing,
            search: "x".into(),
            columns: [Column::Type, Column::DateAdded].into(),
            expanded: [FolderId::from(7)].into(),
            ..ViewSettings::default()
        };
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(serde_json::from_value::<ViewSettings>(json).unwrap(), v);
    }
}
