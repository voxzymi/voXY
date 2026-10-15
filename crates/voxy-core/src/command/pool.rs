//! Project pool commands: names, folders, removal, compositions, sources
//! (design note §2).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use glam::DVec2;

use super::settings::validate_comp_settings;
use super::{Command, CommandError, CommandReport, comp_mut, insert_layer, new_layer};
use crate::asset::{AssetKind, AudioInfo, ContentHash};
use crate::document::{CompSettings, Document};
use crate::id::{AssetId, CompId, FolderId, LayerId};
use crate::layer::{Layer, LayerKind, PlaceholderInfo, PlaceholderKind, SourceRef, Transform};
use crate::pool::{Folder, PoolItem};
use crate::property::Property;
use crate::time::{FrameRate, Time};

/// Longest accepted item name, in characters.
pub const MAX_NAME_CHARS: usize = 255;

/// Checks a user-entered item name: trimmed, not empty, no control
/// characters, at most [`MAX_NAME_CHARS`]. Duplicate names are allowed.
pub fn validate_name(name: &str) -> Result<String, CommandError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CommandError::Invalid("a name cannot be empty".into()));
    }
    if name.chars().any(char::is_control) {
        return Err(CommandError::Invalid(
            "a name cannot contain control characters".into(),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(CommandError::Invalid(format!(
            "a name can be at most {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(name.to_owned())
}

/// `"<base> N"` with the smallest `N ≥ 2` not taken. A trailing number on
/// `name` is replaced, so "Comp 1" gives "Comp 2".
pub fn numbered_name(name: &str, taken: &impl Fn(&str) -> bool) -> String {
    let base = match name.rsplit_once(' ') {
        Some((b, n)) if !b.is_empty() && !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) => {
            b
        }
        _ => name,
    };
    let mut n = 2u64;
    loop {
        let candidate = format!("{base} {n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[allow(clippy::needless_pass_by_value)] // IDs are Copy; callers pass them by value.
fn not_found(what: impl ToString) -> CommandError {
    CommandError::NotFound(what.to_string())
}

fn check_folder(doc: &Document, folder: Option<FolderId>) -> Result<(), CommandError> {
    match folder {
        Some(f) if doc.folder(f).is_none() => Err(not_found(f)),
        _ => Ok(()),
    }
}

/// A copy of `layer` with a fresh layer ID and fresh property IDs.
/// Parent links are left as they are; see [`remap_parents`].
pub(crate) fn renumber_layer(doc: &mut Document, layer: &Layer) -> Layer {
    let mut copy = layer.clone();
    copy.id = doc.alloc_id();
    let mut effects = std::mem::take(&mut copy.effects);
    for p in copy.properties_mut() {
        p.id = doc.alloc_id();
    }
    for fx in &mut effects {
        fx.renumber(&mut || doc.alloc_id());
    }
    copy.effects = effects;
    copy
}

/// Points parent links at the new IDs in `map`; links to layers outside
/// `map` are dropped.
pub(crate) fn remap_parents(layers: &mut [Layer], map: &HashMap<LayerId, LayerId>) {
    for l in layers {
        l.parent = l.parent.and_then(|p| map.get(&p).copied());
    }
}

/// Native pixel size of a source, when known.
pub(crate) fn source_size(doc: &Document, s: SourceRef) -> Option<[u32; 2]> {
    match s {
        SourceRef::Asset(a) => doc.assets.get(a).and_then(|a| a.pixel_size),
        SourceRef::Comp(c) => doc.comp(c).map(|c| [c.width, c.height]),
    }
}

/// Length of a time-based source; `None` for stills and unknown lengths.
pub(crate) fn source_duration(doc: &Document, s: SourceRef) -> Option<Time> {
    match s {
        SourceRef::Asset(a) => doc
            .assets
            .get(a)
            .filter(|a| a.kind == AssetKind::Video)
            .and_then(|a| a.duration)
            .filter(|d| *d > Time::ZERO),
        SourceRef::Comp(c) => doc.comp(c).map(|c| c.duration),
    }
}

/// Fails unless `s` exists and can be shown by a layer (a LUT cannot).
pub(crate) fn source_exists(doc: &Document, s: SourceRef) -> Result<(), CommandError> {
    let found = match s {
        SourceRef::Asset(a) => match doc.assets.get(a) {
            Some(asset) if asset.kind == AssetKind::Lut => {
                return Err(CommandError::Invalid(format!(
                    "“{}” is a LUT: apply it with the LUT effect instead",
                    asset.name
                )));
            }
            found => found.is_some(),
        },
        SourceRef::Comp(c) => doc.comp(c).is_some(),
    };
    if found {
        Ok(())
    } else {
        Err(not_found(display(s)))
    }
}

fn display(s: SourceRef) -> String {
    match s {
        SourceRef::Asset(a) => a.to_string(),
        SourceRef::Comp(c) => c.to_string(),
    }
}

/// Fails if placing `s` inside `target` would nest a composition in itself.
pub(crate) fn check_nesting(
    doc: &Document,
    target: CompId,
    s: SourceRef,
) -> Result<(), CommandError> {
    if let SourceRef::Comp(c) = s
        && let Some(msg) = doc.nesting_error(target, c)
    {
        return Err(CommandError::Invalid(msg));
    }
    Ok(())
}

/// The layer kind that shows `s`, keeping precomp timing options from
/// `previous` when both are precomps.
pub(crate) fn kind_for_source(
    doc: &Document,
    s: SourceRef,
    previous: Option<&LayerKind>,
) -> LayerKind {
    match s {
        SourceRef::Asset(a) => match doc.assets.get(a).map(|a| a.kind) {
            Some(AssetKind::Video) => LayerKind::Video { asset: a },
            _ => LayerKind::Image { asset: a },
        },
        SourceRef::Comp(comp) => match previous {
            Some(LayerKind::Precomp {
                speed,
                looping,
                time_remap,
                collapse,
                ..
            }) => LayerKind::Precomp {
                comp,
                speed: *speed,
                looping: *looping,
                time_remap: time_remap.clone(),
                collapse: *collapse,
            },
            _ => LayerKind::Precomp {
                comp,
                speed: 1.0,
                looping: false,
                time_remap: None,
                collapse: false,
            },
        },
    }
}

fn placeholder_for(doc: &Document, s: SourceRef) -> PlaceholderInfo {
    let kind = match s {
        SourceRef::Asset(a) => match doc.assets.get(a).map(|a| a.kind) {
            Some(AssetKind::Video) => PlaceholderKind::Video,
            _ => PlaceholderKind::Image,
        },
        SourceRef::Comp(_) => PlaceholderKind::Comp,
    };
    PlaceholderInfo {
        kind,
        name: doc.source_name(s).unwrap_or("Missing").to_owned(),
        pixel_size: source_size(doc, s),
        duration: source_duration(doc, s),
    }
}

/// Removes layer `id` from `comp`'s stack data and unparents its children.
fn unparent_children(layers: &mut [Arc<Layer>], removed: &[LayerId]) {
    for l in layers {
        if l.parent.is_some_and(|p| removed.contains(&p)) {
            Arc::make_mut(l).parent = None;
        }
    }
}

// ---------------------------------------------------------------- renaming

#[derive(Debug, Clone)]
pub struct RenameMedia {
    pub asset: AssetId,
    pub name: String,
}

impl Command for RenameMedia {
    fn label(&self) -> Cow<'static, str> {
        "Rename Media".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let name = validate_name(&self.name)?;
        let a = Arc::make_mut(&mut doc.assets)
            .0
            .get_mut(&self.asset)
            .ok_or_else(|| not_found(self.asset))?;
        a.name = name;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RenameComposition {
    pub comp: CompId,
    pub name: String,
}

impl Command for RenameComposition {
    fn label(&self) -> Cow<'static, str> {
        "Rename Composition".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let name = validate_name(&self.name)?;
        comp_mut(doc, self.comp)?.name = name;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RenameFolder {
    pub folder: FolderId,
    pub name: String,
}

impl Command for RenameFolder {
    fn label(&self) -> Cow<'static, str> {
        "Rename Folder".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let name = validate_name(&self.name)?;
        let f = Arc::make_mut(&mut doc.folders)
            .get_mut(&self.folder)
            .ok_or_else(|| not_found(self.folder))?;
        f.name = name;
        Ok(())
    }
}

// ----------------------------------------------------------------- folders

#[derive(Debug, Clone)]
pub struct CreateFolder {
    pub name: String,
    pub parent: Option<FolderId>,
}

impl Command for CreateFolder {
    fn label(&self) -> Cow<'static, str> {
        "New Folder".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        let name = validate_name(&self.name)?;
        check_folder(doc, self.parent)?;
        let id: FolderId = doc.alloc_id();
        Arc::make_mut(&mut doc.folders).insert(
            id,
            Folder {
                id,
                name,
                parent: self.parent,
            },
        );
        report.created_folders.push(id);
        Ok(())
    }
}

/// Name of the root folder pre-composed compositions go in.
pub const PRECOMPS_FOLDER: &str = "Pre-comps";

/// The root folder named [`PRECOMPS_FOLDER`], created if there is none.
/// It is found by exact name, so renaming it makes the next pre-compose
/// create a new one.
pub(crate) fn precomps_folder(doc: &mut Document, report: &mut CommandReport) -> FolderId {
    if let Some(f) = doc
        .folders
        .values()
        .find(|f| f.parent.is_none() && f.name == PRECOMPS_FOLDER)
    {
        return f.id;
    }
    let id: FolderId = doc.alloc_id();
    Arc::make_mut(&mut doc.folders).insert(
        id,
        Folder {
            id,
            name: PRECOMPS_FOLDER.to_owned(),
            parent: None,
        },
    );
    report.created_folders.push(id);
    id
}

/// Adds media to the project without placing it in any composition.
/// Importing never creates a composition. The assets' IDs are assigned
/// here and returned in the report; the `id` and `folder` fields of the
/// given assets are ignored.
#[derive(Debug, Clone)]
pub struct ImportAssets {
    pub assets: Vec<crate::Asset>,
    pub folder: Option<FolderId>,
}

impl Command for ImportAssets {
    fn label(&self) -> Cow<'static, str> {
        "Import".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        if self.assets.is_empty() {
            return Err(CommandError::Invalid("nothing to import".into()));
        }
        check_folder(doc, self.folder)?;
        for asset in &self.assets {
            if asset.path.trim().is_empty() {
                return Err(CommandError::Invalid("a media path cannot be empty".into()));
            }
            // Reuse an item already in the project with the same content,
            // so importing the same file twice does not duplicate it.
            if let Some(hash) = &asset.content_hash
                && let Some(existing) = doc.asset_by_content_hash(hash)
            {
                report.reused_assets.push(existing);
                continue;
            }
            let mut asset = asset.clone();
            asset.name = validate_name(&asset.name)?;
            asset.folder = self.folder;
            report.created_assets.push(doc.add_asset(asset));
        }
        Ok(())
    }
}

/// Moves items into `folder` (`None`: the top level). A folder cannot move
/// into itself or its own subfolders.
#[derive(Debug, Clone)]
pub struct MoveToFolder {
    pub items: Vec<PoolItem>,
    pub folder: Option<FolderId>,
}

impl Command for MoveToFolder {
    fn label(&self) -> Cow<'static, str> {
        "Move to Folder".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        check_folder(doc, self.folder)?;
        for &item in &self.items {
            match item {
                PoolItem::Asset(a) => {
                    Arc::make_mut(&mut doc.assets)
                        .0
                        .get_mut(&a)
                        .ok_or_else(|| not_found(a))?
                        .folder = self.folder;
                }
                PoolItem::Comp(c) => comp_mut(doc, c)?.folder = self.folder,
                PoolItem::Folder(f) => {
                    if doc.folder(f).is_none() {
                        return Err(not_found(f));
                    }
                    if self.folder.is_some_and(|dest| doc.folder_within(dest, f)) {
                        return Err(CommandError::Invalid(
                            "a folder cannot move into itself".into(),
                        ));
                    }
                    Arc::make_mut(&mut doc.folders)
                        .get_mut(&f)
                        .expect("checked above")
                        .parent = self.folder;
                }
            }
        }
        Ok(())
    }
}

// ----------------------------------------------------------------- removal

/// What happens to layers whose source is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveMode {
    /// Keep them as [`LayerKind::Placeholder`] layers.
    PlaceholderLayers,
    /// Delete them (their children are unparented).
    DeleteLayers,
}

/// Removes media, compositions and folders (with everything inside) from
/// the project. Files on disk are never touched. Layers in remaining
/// compositions that showed a removed source are handled per `mode`.
/// Removing the main composition makes the one with the lowest ID main.
#[derive(Debug, Clone)]
pub struct RemoveItems {
    pub items: Vec<PoolItem>,
    pub mode: RemoveMode,
}

impl Command for RemoveItems {
    fn label(&self) -> Cow<'static, str> {
        "Remove from Project".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        for &item in &self.items {
            let found = match item {
                PoolItem::Asset(a) => doc.assets.get(a).is_some(),
                PoolItem::Comp(c) => doc.comp(c).is_some(),
                PoolItem::Folder(f) => doc.folder(f).is_some(),
            };
            if !found {
                return Err(not_found(format!("{item:?}")));
            }
        }
        let ex = doc.expand_items(&self.items);
        // Placeholders describe sources before they disappear.
        let placeholders: HashMap<SourceRef, PlaceholderInfo> = ex
            .assets
            .iter()
            .map(|&a| SourceRef::Asset(a))
            .chain(ex.comps.iter().map(|&c| SourceRef::Comp(c)))
            .map(|s| (s, placeholder_for(doc, s)))
            .collect();
        {
            let assets = Arc::make_mut(&mut doc.assets);
            for a in &ex.assets {
                assets.0.remove(a);
            }
            let comps = Arc::make_mut(&mut doc.comps);
            for c in &ex.comps {
                comps.remove(c);
            }
            let folders = Arc::make_mut(&mut doc.folders);
            for f in &ex.folders {
                folders.remove(f);
            }
        }
        let affected: Vec<CompId> = doc
            .comps
            .values()
            .filter(|c| {
                c.layers
                    .iter()
                    .any(|l| l.kind.source().is_some_and(|s| ex.contains_source(s)))
            })
            .map(|c| c.id)
            .collect();
        for cid in affected {
            let c = comp_mut(doc, cid)?;
            match self.mode {
                RemoveMode::PlaceholderLayers => {
                    for l in &mut c.layers {
                        if let Some(s) = l.kind.source()
                            && let Some(info) = placeholders.get(&s)
                        {
                            Arc::make_mut(l).kind = LayerKind::Placeholder { was: info.clone() };
                        }
                    }
                }
                RemoveMode::DeleteLayers => {
                    let removed: Vec<LayerId> = c
                        .layers
                        .iter()
                        .filter(|l| l.kind.source().is_some_and(|s| ex.contains_source(s)))
                        .map(|l| l.id)
                        .collect();
                    c.layers.retain(|l| !removed.contains(&l.id));
                    unparent_children(&mut c.layers, &removed);
                    super::effects::clear_layer_refs(&mut c.layers, &removed);
                }
            }
        }
        let gone: Vec<AssetId> = ex.assets.iter().copied().collect();
        super::effects::clear_asset_refs(doc, &gone);
        if doc.main_comp.is_some_and(|m| ex.comps.contains(&m)) {
            doc.main_comp = doc.comps.keys().next().copied();
        }
        Ok(())
    }
}

// ------------------------------------------------------------ compositions

#[derive(Debug, Clone)]
pub struct CreateComposition {
    pub settings: CompSettings,
    pub folder: Option<FolderId>,
}

impl Command for CreateComposition {
    fn label(&self) -> Cow<'static, str> {
        "New Composition".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        let mut settings = self.settings.clone();
        settings.name = validate_name(&settings.name)?;
        validate_comp_settings(&settings)?;
        check_folder(doc, self.folder)?;
        let id = doc.add_comp(settings);
        comp_mut(doc, id)?.folder = self.folder;
        if doc.main_comp.is_none() {
            doc.main_comp = Some(id);
        }
        report.created_comps.push(id);
        Ok(())
    }
}

/// Copies a composition with new IDs for it, its layers and their
/// properties. Precomp layers inside still show the same compositions.
#[derive(Debug, Clone)]
pub struct DuplicateComposition {
    pub comp: CompId,
}

impl Command for DuplicateComposition {
    fn label(&self) -> Cow<'static, str> {
        "Duplicate Composition".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        let src = doc
            .comp(self.comp)
            .ok_or_else(|| not_found(self.comp))?
            .clone();
        let name = numbered_name(&src.name, &|n| doc.comps.values().any(|c| c.name == n));
        let mut copy = src.clone();
        copy.id = doc.alloc_id();
        copy.name = name;
        copy.background.id = doc.alloc_id();
        let mut map = HashMap::new();
        let mut layers: Vec<Layer> = src
            .layers
            .iter()
            .map(|l| {
                let n = renumber_layer(doc, l);
                map.insert(l.id, n.id);
                n
            })
            .collect();
        remap_parents(&mut layers, &map);
        super::effects::remap_layer_refs(&mut layers, &map);
        copy.layers = layers.into_iter().map(Arc::new).collect();
        let id = copy.id;
        Arc::make_mut(&mut doc.comps).insert(id, Arc::new(copy));
        report.created_comps.push(id);
        Ok(())
    }
}

// ----------------------------------------------------------------- sources

/// Relinks a media item to another file. Every layer keeps showing the same
/// item. Warns when the new file's size or frame rate differ.
#[derive(Debug, Clone)]
pub struct ReplaceMedia {
    pub asset: AssetId,
    pub path: String,
    pub pixel_size: Option<[u32; 2]>,
    pub duration: Option<Time>,
    pub frame_rate: Option<FrameRate>,
    pub audio: Option<AudioInfo>,
    pub content_hash: Option<ContentHash>,
}

impl Command for ReplaceMedia {
    fn label(&self) -> Cow<'static, str> {
        "Replace Media".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        let a = Arc::make_mut(&mut doc.assets)
            .0
            .get_mut(&self.asset)
            .ok_or_else(|| not_found(self.asset))?;
        if let (Some([ow, oh]), Some([nw, nh])) = (a.pixel_size, self.pixel_size)
            && (ow, oh) != (nw, nh)
        {
            report.warnings.push(format!(
                "The new file is {nw}×{nh}; “{}” was {ow}×{oh}.",
                a.name
            ));
        }
        if let (Some(old), Some(new)) = (a.frame_rate, self.frame_rate)
            && old != new
        {
            report.warnings.push(format!(
                "The new file runs at {new} fps; “{}” ran at {old} fps.",
                a.name
            ));
        }
        a.path.clone_from(&self.path);
        a.pixel_size = self.pixel_size.or(a.pixel_size);
        a.duration = self.duration;
        a.frame_rate = self.frame_rate;
        a.audio = self.audio;
        a.content_hash.clone_from(&self.content_hash);
        Ok(())
    }
}

/// Makes layers show another source, keeping their name, timing,
/// transform, keyframes and flags. Placeholder layers become real layers
/// again. Precomp timing options carry over between precomps.
#[derive(Debug, Clone)]
pub struct ReplaceLayerSource {
    pub layers: Vec<LayerId>,
    pub source: SourceRef,
}

impl Command for ReplaceLayerSource {
    fn label(&self) -> Cow<'static, str> {
        "Replace Layer Source".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        source_exists(doc, self.source)?;
        for &id in &self.layers {
            replace_layer_source(doc, id, self.source)?;
        }
        Ok(())
    }
}

/// Makes layer `id` show `source`, keeping its name, timing, transform,
/// keyframes and flags (a placeholder becomes a real layer again, precomp
/// timing options carry over between precomps). Checks nesting. The one
/// place this rule lives: the menu command and a drop onto a layer both call
/// it.
pub(crate) fn replace_layer_source(
    doc: &mut Document,
    id: LayerId,
    source: SourceRef,
) -> Result<(), CommandError> {
    let (comp, layer) = doc.find_layer(id).ok_or_else(|| not_found(id))?;
    check_nesting(doc, comp, source)?;
    let kind = kind_for_source(doc, source, Some(&layer.kind));
    comp_mut(doc, comp)?
        .layer_mut(id)
        .expect("found above")
        .kind = kind;
    Ok(())
}

/// Adds a layer showing a project item: centered, starting at `in_point`.
///
/// Length: video uses its full duration and compositions theirs (neither is
/// trimmed at the composition end); stills use the project's default still
/// duration or last until the composition ends; unknown lengths last until
/// the composition ends. Never shorter than one frame.
#[derive(Debug, Clone)]
pub struct AddSourceLayer {
    pub comp: CompId,
    pub source: SourceRef,
    /// 0 is the top of the stack.
    pub index: usize,
    pub in_point: Time,
}

impl Command for AddSourceLayer {
    fn label(&self) -> Cow<'static, str> {
        "Add Layer".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        source_exists(doc, self.source)?;
        check_nesting(doc, self.comp, self.source)?;
        let c = doc.comp(self.comp).ok_or_else(|| not_found(self.comp))?;
        let length = source_layer_length(
            source_duration(doc, self.source),
            matches!(self.source, SourceRef::Asset(_)),
            doc.meta.settings.default_still_duration,
            c.duration - self.in_point,
            c.frame_rate.frame_to_time(1),
        );
        let id = insert_source_layer(
            doc,
            self.comp,
            self.source,
            self.index,
            self.in_point,
            length,
            None,
        )?;
        report.created_layers.push(id);
        Ok(())
    }
}

/// How long a new layer for a source lasts. A time-based source uses its own
/// duration (not trimmed at the composition end); a still uses the project's
/// default still duration, or lasts until the composition ends; an unknown
/// length lasts until the end. Never shorter than one frame.
///
/// `until_end` is the time left in the composition from the layer's in-point.
/// This one function serves the command and the drop preview, so they agree.
#[must_use]
pub fn source_layer_length(
    source_duration: Option<Time>,
    is_asset: bool,
    default_still: Option<Time>,
    until_end: Time,
    frame: Time,
) -> Time {
    match (source_duration, is_asset) {
        (Some(d), _) => d,
        (None, true) => default_still.unwrap_or(until_end),
        (None, false) => until_end,
    }
    .max(frame)
}

/// Adds a layer for `source`, centered (or with its center at `position`),
/// from `in_point` for `length`. Does not check nesting; callers do.
pub(crate) fn insert_source_layer(
    doc: &mut Document,
    comp: CompId,
    source: SourceRef,
    index: usize,
    in_point: Time,
    length: Time,
    position: Option<DVec2>,
) -> Result<LayerId, CommandError> {
    let c = doc.comp(comp).ok_or_else(|| not_found(comp))?;
    let center =
        position.unwrap_or_else(|| DVec2::new(f64::from(c.width), f64::from(c.height)) / 2.0);
    let [w, h] = source_size(doc, source).unwrap_or([0, 0]);
    let name = doc.source_name(source).unwrap_or_default().to_owned();
    let kind = kind_for_source(doc, source, None);
    let id: LayerId = doc.alloc_id();
    let local_center = DVec2::new(f64::from(w), f64::from(h)) / 2.0;
    let transform = Transform::new(&mut || doc.alloc_id(), local_center, center);
    let mut layer = new_layer(id, name, kind, length, transform);
    layer.in_point = in_point;
    layer.out_point = in_point + length;
    insert_layer(doc, comp, index, layer)?;
    Ok(id)
}

/// A precomp layer's time options. Turning `time_remap` on adds keys at the
/// in- and out-points mapping to the child times shown there now; turning it
/// off removes the remap property.
#[derive(Debug, Clone)]
pub struct SetPrecompTiming {
    pub layer: LayerId,
    pub speed: f64,
    pub looping: bool,
    pub time_remap: bool,
}

impl Command for SetPrecompTiming {
    fn label(&self) -> Cow<'static, str> {
        "Edit Precomp Timing".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        if !(self.speed.is_finite() && self.speed > 0.0 && self.speed <= 100.0) {
            return Err(CommandError::Invalid(
                "speed must be above 0 and at most 100×".into(),
            ));
        }
        let (comp, layer) = doc
            .find_layer(self.layer)
            .ok_or_else(|| not_found(self.layer))?;
        let LayerKind::Precomp {
            comp: child,
            time_remap,
            ..
        } = &layer.kind
        else {
            return Err(CommandError::Invalid("not a precomp layer".into()));
        };
        let child = *child;
        let duration = doc.comp(child).map_or(Time::ZERO, |c| c.duration);
        let needs_remap = self.time_remap && time_remap.is_none();
        // Child times at the edges with the *new* speed and looping.
        let mut probe = layer.clone();
        if let LayerKind::Precomp { speed, looping, .. } = &mut probe.kind {
            *speed = self.speed;
            *looping = self.looping;
        }
        let at_in = probe
            .child_time(layer.in_point, duration)
            .unwrap_or(Time::ZERO);
        let at_out = probe
            .child_time(layer.out_point, duration)
            .unwrap_or(Time::ZERO);
        let (in_point, out_point) = (layer.in_point, layer.out_point);
        let remap = if needs_remap {
            let mut p = Property::builtin(
                doc.alloc_id(),
                crate::property::keys::TIME_REMAP,
                crate::Value::Scalar(at_in.as_secs_f64()),
            );
            p.set_animated(in_point, true)?;
            if out_point > in_point {
                p.set_value_at(out_point, crate::Value::Scalar(at_out.as_secs_f64()))?;
            }
            Some(p)
        } else {
            None
        };
        let l = comp_mut(doc, comp)?
            .layer_mut(self.layer)
            .expect("found above");
        if let LayerKind::Precomp {
            speed,
            looping,
            time_remap,
            ..
        } = &mut l.kind
        {
            *speed = self.speed;
            *looping = self.looping;
            if !self.time_remap {
                *time_remap = None;
            } else if remap.is_some() {
                *time_remap = remap;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SetCollapse {
    pub layer: LayerId,
    pub collapse: bool,
}

impl Command for SetCollapse {
    fn label(&self) -> Cow<'static, str> {
        "Collapse Transformations".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let l = doc
            .layer_mut(self.layer)
            .ok_or_else(|| not_found(self.layer))?;
        match &mut l.kind {
            LayerKind::Precomp { collapse, .. } => {
                *collapse = self.collapse;
                Ok(())
            }
            _ => Err(CommandError::Invalid("not a precomp layer".into())),
        }
    }
}
