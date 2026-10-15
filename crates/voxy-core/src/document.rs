//! The document: the root of the model.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::asset::{Asset, AssetTable};
use crate::color::{LinearRgba, WorkingSpace};
use crate::comp::{Composition, PixelAspect, PreviewResolution};
use crate::effect::EffectInstance;
use crate::id::{AssetId, CompId, EffectId, FolderId, LayerId, PropId};
use crate::layer::{ForeignData, Layer, LayerKind};
use crate::pool::Folder;
use crate::property::{Property, keys};
use crate::time::{FrameRate, Time};
use crate::value::Value;

/// Project-wide settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    #[serde(default)]
    pub working_space: WorkingSpace,
    /// Defaults for new items (format 0.4.0).
    #[serde(default)]
    pub settings: ProjectSettings,
}

/// Defaults the project applies to new compositions and layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    /// Settings of a new composition; its name is the base of new names.
    pub new_comp: CompSettings,
    /// Length of a still image layer; `None` lasts until the composition
    /// ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_still_duration: Option<Time>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        ProjectSettings {
            new_comp: CompSettings {
                name: "Comp".into(),
                ..CompSettings::default()
            },
            default_still_duration: None,
        }
    }
}

/// Settings of a composition: for a new one, or edited in the composition
/// settings dialog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompSettings {
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub pixel_aspect: PixelAspect,
    pub frame_rate: FrameRate,
    pub duration: Time,
    pub background: LinearRgba,
    /// Timecode of frame 0 (format 0.5.0).
    #[serde(default)]
    pub start_offset: Time,
    /// Drop-frame timecode display (format 0.5.0).
    #[serde(default)]
    pub drop_frame: bool,
    /// Viewport render scale (format 0.5.0).
    #[serde(default)]
    pub preview_resolution: PreviewResolution,
}

impl Default for CompSettings {
    fn default() -> Self {
        CompSettings {
            name: "Main".into(),
            width: 1920,
            height: 1080,
            pixel_aspect: PixelAspect::SQUARE,
            frame_rate: FrameRate::FPS_30,
            duration: Time::from_secs(10),
            background: LinearRgba::BLACK,
            start_offset: Time::ZERO,
            drop_frame: false,
            preview_resolution: PreviewResolution::Full,
        }
    }
}

impl CompSettings {
    /// The settings `comp` has now. The background is its value at time 0.
    pub fn of(comp: &Composition) -> Self {
        CompSettings {
            name: comp.name.clone(),
            width: comp.width,
            height: comp.height,
            pixel_aspect: comp.pixel_aspect,
            frame_rate: comp.frame_rate,
            duration: comp.duration,
            background: comp
                .background
                .value_at(Time::ZERO)
                .as_color()
                .unwrap_or(LinearRgba::BLACK),
            start_offset: comp.start_offset,
            drop_frame: comp.drop_frame,
            preview_resolution: comp.preview_resolution,
        }
    }
}

/// A voXY document.
///
/// Compositions and layers sit behind [`Arc`]s and are edited copy-on-write,
/// so cloning a `Document` is cheap and every clone is an independent
/// snapshot. [`crate::History`] relies on this for undo, and the render
/// thread reads published snapshots without locking.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub meta: ProjectMeta,
    #[serde(with = "id_map")]
    pub assets: Arc<AssetTable>,
    #[serde(with = "id_map")]
    pub comps: Arc<BTreeMap<CompId, Arc<Composition>>>,
    /// Project panel folders (format 0.4.0).
    #[serde(default, with = "id_map")]
    pub folders: Arc<BTreeMap<FolderId, Folder>>,
    /// The composition shown when no other is open, and the default for
    /// export. `None` when the project has no compositions.
    pub main_comp: Option<CompId>,
    #[serde(default, skip_serializing_if = "ForeignData::is_empty")]
    pub foreign: ForeignData,
    /// Next unallocated ID. All IDs share this one counter.
    next_id: u64,
}

/// Where a property lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyOwner {
    CompBackground(CompId),
    Layer(CompId, LayerId),
}

/// A structural problem found by [`Document::validate`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("main composition {0} does not exist")]
    MissingMain(CompId),
    #[error("ID {0} is used more than once")]
    DuplicateId(u64),
    #[error("ID {id} is not below the document's next ID {next}")]
    IdOutOfRange { id: u64, next: u64 },
    #[error("{0} has a zero width or height")]
    EmptyComp(CompId),
    #[error("{0} has a negative duration")]
    NegativeDuration(CompId),
    #[error("{0} ends before it starts")]
    InvertedLayer(LayerId),
    #[error("{layer} is parented to {parent}, which is not in the same composition")]
    MissingParent { layer: LayerId, parent: LayerId },
    #[error("{0} is part of a parenting cycle")]
    ParentCycle(LayerId),
    #[error("{layer} references missing {asset}")]
    MissingAsset { layer: LayerId, asset: AssetId },
    #[error("{layer} references missing {comp}")]
    MissingComp { layer: LayerId, comp: CompId },
    #[error("{0} contains itself, directly or through other compositions")]
    CompCycle(CompId),
    #[error("item {item} is in missing {folder}")]
    MissingFolder { item: u64, folder: FolderId },
    #[error("{0} is inside itself")]
    FolderCycle(FolderId),
    #[error("{0} has an invalid work area")]
    BadWorkArea(CompId),
    #[error("{0} has an invalid pixel aspect ratio")]
    BadPixelAspect(CompId),
    #[error("{0} has an invalid speed")]
    BadSpeed(LayerId),
    #[error("{effect} on {layer} has a missing, duplicate or misnamed parameter `{key}`")]
    BadEffectParam {
        layer: LayerId,
        effect: EffectId,
        key: String,
    },
    #[error("property {prop} has key `{found}` where `{expected}` is required")]
    WrongProperty {
        prop: PropId,
        expected: &'static str,
        found: String,
    },
}

impl Document {
    /// A project with no compositions, media or folders: what a new project
    /// starts as. Nothing creates a composition behind the user's back.
    pub fn empty(name: impl Into<String>) -> Self {
        Document {
            meta: ProjectMeta {
                name: name.into(),
                working_space: WorkingSpace::default(),
                settings: ProjectSettings::default(),
            },
            assets: Arc::default(),
            comps: Arc::default(),
            folders: Arc::default(),
            main_comp: None,
            foreign: ForeignData::default(),
            next_id: 1,
        }
    }

    /// An empty project plus one composition, which becomes the main one.
    /// Scaffolding for tests and fixtures; the application starts from
    /// [`Document::empty`].
    pub fn with_comp(name: impl Into<String>, settings: CompSettings) -> Self {
        let mut doc = Document::empty(name);
        let id = doc.add_comp(settings);
        doc.main_comp = Some(id);
        doc
    }

    /// Allocates a fresh ID.
    pub fn alloc_id<T: From<u64>>(&mut self) -> T {
        let id = self.next_id;
        self.next_id += 1;
        T::from(id)
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Raises the ID counter to at least `next`, so IDs handed out before an
    /// undo are never handed out again.
    pub fn reserve_ids(&mut self, next: u64) {
        self.next_id = self.next_id.max(next);
    }

    /// Adds a composition and returns its ID.
    pub fn add_comp(&mut self, s: CompSettings) -> CompId {
        let id: CompId = self.alloc_id();
        let background = Property::builtin(
            self.alloc_id(),
            keys::COMP_BACKGROUND,
            Value::Color(s.background),
        );
        let comp = Composition {
            id,
            name: s.name,
            width: s.width,
            height: s.height,
            pixel_aspect: s.pixel_aspect,
            frame_rate: s.frame_rate,
            duration: s.duration,
            work_area: None,
            background,
            folder: None,
            start_offset: s.start_offset,
            drop_frame: s.drop_frame,
            preview_resolution: s.preview_resolution,
            layers: Vec::new(),
            foreign: ForeignData::default(),
        };
        Arc::make_mut(&mut self.comps).insert(id, Arc::new(comp));
        id
    }

    pub fn add_asset(&mut self, mut asset: Asset) -> AssetId {
        let id: AssetId = self.alloc_id();
        asset.id = id;
        Arc::make_mut(&mut self.assets).0.insert(id, asset);
        id
    }

    /// The id of an existing asset whose content matches `hash`, if any.
    /// Used so that importing the same file twice reuses the first item
    /// instead of adding a duplicate. The lowest id wins, so the result is
    /// the same whatever order the assets were added in.
    pub fn asset_by_content_hash(&self, hash: &crate::ContentHash) -> Option<AssetId> {
        self.assets
            .0
            .iter()
            .find(|(_, a)| a.content_hash.as_ref() == Some(hash))
            .map(|(&id, _)| id)
    }

    /// The main composition, if the project has one.
    pub fn main(&self) -> Option<&Composition> {
        self.main_comp.and_then(|id| self.comp(id))
    }

    pub fn comp(&self, id: CompId) -> Option<&Composition> {
        self.comps.get(&id).map(AsRef::as_ref)
    }

    /// Mutable access, copying the composition if a snapshot shares it.
    pub fn comp_mut(&mut self, id: CompId) -> Option<&mut Composition> {
        if !self.comps.contains_key(&id) {
            return None;
        }
        Arc::make_mut(&mut self.comps)
            .get_mut(&id)
            .map(Arc::make_mut)
    }

    /// Finds a layer in any composition.
    pub fn find_layer(&self, id: LayerId) -> Option<(CompId, &Layer)> {
        self.comps
            .values()
            .find_map(|c| c.layer(id).map(|l| (c.id, l)))
    }

    /// Size of a layer's content in layer pixels, when known: a solid's
    /// size at time zero, an asset's recorded size, a precomp's (or, for an
    /// adjustment layer, its own) composition size.
    pub fn content_size(&self, id: LayerId) -> Option<glam::DVec2> {
        let (comp, layer) = self.find_layer(id)?;
        let px = |s: [u32; 2]| glam::DVec2::new(f64::from(s[0]), f64::from(s[1]));
        let comp_size = |c: CompId| self.comp(c).map(|c| px([c.width, c.height]));
        match &layer.kind {
            LayerKind::Solid { size, .. } => match size.value_at(Time::ZERO) {
                Value::Vec2(v) if v.is_finite() => Some(v),
                _ => None,
            },
            LayerKind::Image { asset } | LayerKind::Video { asset } => {
                self.assets.get(*asset).and_then(|a| a.pixel_size).map(px)
            }
            LayerKind::Precomp { comp: child, .. } => comp_size(*child),
            LayerKind::Adjustment => comp_size(comp),
            LayerKind::Placeholder { was } => was.pixel_size.map(px),
        }
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        let (comp, _) = self.find_layer(id)?;
        self.comp_mut(comp)?.layer_mut(id)
    }

    /// Finds which node owns a property, without copying anything.
    pub fn locate_property(&self, id: PropId) -> Option<PropertyOwner> {
        for comp in self.comps.values() {
            if comp.background.id == id {
                return Some(PropertyOwner::CompBackground(comp.id));
            }
            for layer in &comp.layers {
                if layer.properties().iter().any(|p| p.id == id) {
                    return Some(PropertyOwner::Layer(comp.id, layer.id));
                }
            }
        }
        None
    }

    pub fn property(&self, id: PropId) -> Option<&Property> {
        match self.locate_property(id)? {
            PropertyOwner::CompBackground(c) => Some(&self.comp(c)?.background),
            PropertyOwner::Layer(c, l) => self
                .comp(c)?
                .layer(l)?
                .properties()
                .into_iter()
                .find(|p| p.id == id),
        }
    }

    /// Mutable access to a property; only its owner is copied.
    pub fn property_mut(&mut self, id: PropId) -> Option<&mut Property> {
        match self.locate_property(id)? {
            PropertyOwner::CompBackground(c) => Some(&mut self.comp_mut(c)?.background),
            PropertyOwner::Layer(c, l) => self.comp_mut(c)?.layer_mut(l)?.property_mut(id),
        }
    }

    /// Checks the structural invariants every loaded or edited document must
    /// satisfy.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(main) = self.main_comp
            && !self.comps.contains_key(&main)
        {
            return Err(ValidationError::MissingMain(main));
        }
        let mut seen = HashSet::new();
        let mut claim = |id: u64| {
            if id >= self.next_id {
                return Err(ValidationError::IdOutOfRange {
                    id,
                    next: self.next_id,
                });
            }
            if seen.insert(id) {
                Ok(())
            } else {
                Err(ValidationError::DuplicateId(id))
            }
        };
        for folder in self.folders.values() {
            claim(folder.id.raw())?;
        }
        self.validate_folders()?;
        let in_folder = |item: u64, folder: Option<FolderId>| match folder {
            Some(f) if !self.folders.contains_key(&f) => {
                Err(ValidationError::MissingFolder { item, folder: f })
            }
            _ => Ok(()),
        };
        for asset in self.assets.iter() {
            claim(asset.id.raw())?;
            in_folder(asset.id.raw(), asset.folder)?;
        }
        for comp in self.comps.values() {
            claim(comp.id.raw())?;
            in_folder(comp.id.raw(), comp.folder)?;
            let pa = comp.pixel_aspect;
            if pa.num == 0 || pa.den == 0 {
                return Err(ValidationError::BadPixelAspect(comp.id));
            }
            if let Some(w) = comp.work_area
                && !(Time::ZERO <= w.start && w.start < w.end && w.end <= comp.duration)
            {
                return Err(ValidationError::BadWorkArea(comp.id));
            }
            claim(comp.background.id.raw())?;
            expect_key(&comp.background, keys::COMP_BACKGROUND)?;
            if comp.width == 0 || comp.height == 0 {
                return Err(ValidationError::EmptyComp(comp.id));
            }
            if comp.duration < Time::ZERO {
                return Err(ValidationError::NegativeDuration(comp.id));
            }
            for layer in &comp.layers {
                claim(layer.id.raw())?;
                for p in layer.properties() {
                    claim(p.id.raw())?;
                }
                for fx in &layer.effects {
                    claim(fx.id.raw())?;
                    // Editable parameters were claimed above.
                    if let Some(ph) = &fx.placeholder {
                        for id in fx
                            .params
                            .iter()
                            .map(|p| p.id)
                            .chain(ph.reserved.iter().copied())
                        {
                            claim(id.raw())?;
                        }
                    }
                    validate_effect(layer.id, fx)?;
                }
                validate_layer_props(layer)?;
                if layer.out_point < layer.in_point {
                    return Err(ValidationError::InvertedLayer(layer.id));
                }
                if let Some(asset) = layer.kind.asset()
                    && self.assets.get(asset).is_none()
                {
                    return Err(ValidationError::MissingAsset {
                        layer: layer.id,
                        asset,
                    });
                }
                if let LayerKind::Precomp {
                    comp: child, speed, ..
                } = &layer.kind
                {
                    if !self.comps.contains_key(child) {
                        return Err(ValidationError::MissingComp {
                            layer: layer.id,
                            comp: *child,
                        });
                    }
                    if !(speed.is_finite() && *speed > 0.0) {
                        return Err(ValidationError::BadSpeed(layer.id));
                    }
                }
            }
            validate_parenting(comp)?;
        }
        for &id in self.comps.keys() {
            if self.comp_contains(id, id) {
                return Err(ValidationError::CompCycle(id));
            }
        }
        Ok(())
    }

    fn validate_folders(&self) -> Result<(), ValidationError> {
        for folder in self.folders.values() {
            let mut cur = folder.parent;
            let mut steps = 0;
            while let Some(p) = cur {
                let Some(parent) = self.folders.get(&p) else {
                    return Err(ValidationError::MissingFolder {
                        item: folder.id.raw(),
                        folder: p,
                    });
                };
                steps += 1;
                if p == folder.id || steps > self.folders.len() {
                    return Err(ValidationError::FolderCycle(folder.id));
                }
                cur = parent.parent;
            }
        }
        Ok(())
    }
}

fn expect_key(p: &Property, expected: &'static str) -> Result<(), ValidationError> {
    if p.key() == expected {
        Ok(())
    } else {
        Err(ValidationError::WrongProperty {
            prop: p.id,
            expected,
            found: p.key().to_owned(),
        })
    }
}

/// An effect's parameters must carry its type's key prefix, each name once.
fn validate_effect(layer: LayerId, fx: &EffectInstance) -> Result<(), ValidationError> {
    let prefix = fx.type_id.param_prefix();
    let mut names = HashSet::new();
    for p in &fx.params {
        let name = p.key().strip_prefix(prefix.as_str());
        if !name.is_some_and(|n| !n.is_empty() && names.insert(n)) {
            return Err(ValidationError::BadEffectParam {
                layer,
                effect: fx.id,
                key: p.key().to_owned(),
            });
        }
    }
    if fx.type_id.as_str().is_empty() {
        return Err(ValidationError::BadEffectParam {
            layer,
            effect: fx.id,
            key: String::new(),
        });
    }
    Ok(())
}

fn validate_layer_props(layer: &Layer) -> Result<(), ValidationError> {
    let t = &layer.transform;
    expect_key(&t.anchor, keys::ANCHOR)?;
    expect_key(&t.position, keys::POSITION)?;
    expect_key(&t.scale, keys::SCALE)?;
    expect_key(&t.rotation, keys::ROTATION)?;
    expect_key(&t.opacity, keys::OPACITY)?;
    match &layer.kind {
        LayerKind::Solid { color, size } => {
            expect_key(color, keys::SOLID_COLOR)?;
            expect_key(size, keys::SOLID_SIZE)?;
        }
        LayerKind::Precomp {
            time_remap: Some(remap),
            ..
        } => expect_key(remap, keys::TIME_REMAP)?,
        _ => {}
    }
    Ok(())
}

fn validate_parenting(comp: &Composition) -> Result<(), ValidationError> {
    let parents: HashMap<LayerId, Option<LayerId>> =
        comp.layers.iter().map(|l| (l.id, l.parent)).collect();
    for layer in &comp.layers {
        if let Some(parent) = layer.parent
            && !parents.contains_key(&parent)
        {
            return Err(ValidationError::MissingParent {
                layer: layer.id,
                parent,
            });
        }
        // Walk up; more steps than layers means a cycle.
        let mut cur = layer.parent;
        let mut steps = 0;
        while let Some(p) = cur {
            steps += 1;
            if steps > comp.layers.len() {
                return Err(ValidationError::ParentCycle(layer.id));
            }
            cur = parents.get(&p).copied().flatten();
        }
    }
    Ok(())
}

/// Serializes ID-keyed maps as JSON arrays of their values, so each ID is
/// written once (inside the value) and the file reads naturally.
mod id_map {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::asset::{Asset, AssetTable};
    use crate::comp::Composition;
    use crate::id::{AssetId, CompId, FolderId};
    use crate::pool::Folder;

    pub trait IdMap: Sized {
        type Item: Serialize + for<'de> Deserialize<'de>;
        fn items(&self) -> Vec<&Self::Item>;
        fn from_items(items: Vec<Self::Item>) -> Result<Self, String>;
    }

    impl IdMap for Arc<AssetTable> {
        type Item = Asset;
        fn items(&self) -> Vec<&Asset> {
            self.0.values().collect()
        }
        fn from_items(items: Vec<Asset>) -> Result<Self, String> {
            let mut map = BTreeMap::<AssetId, Asset>::new();
            for a in items {
                let id = a.id;
                if map.insert(id, a).is_some() {
                    return Err(format!("duplicate {id}"));
                }
            }
            Ok(Arc::new(AssetTable(map)))
        }
    }

    impl IdMap for Arc<BTreeMap<CompId, Arc<Composition>>> {
        type Item = Arc<Composition>;
        fn items(&self) -> Vec<&Arc<Composition>> {
            self.values().collect()
        }
        fn from_items(items: Vec<Arc<Composition>>) -> Result<Self, String> {
            let mut map = BTreeMap::new();
            for c in items {
                let id = c.id;
                if map.insert(id, c).is_some() {
                    return Err(format!("duplicate {id}"));
                }
            }
            Ok(Arc::new(map))
        }
    }

    impl IdMap for Arc<BTreeMap<FolderId, Folder>> {
        type Item = Folder;
        fn items(&self) -> Vec<&Folder> {
            self.values().collect()
        }
        fn from_items(items: Vec<Folder>) -> Result<Self, String> {
            let mut map = BTreeMap::new();
            for f in items {
                let id = f.id;
                if map.insert(id, f).is_some() {
                    return Err(format!("duplicate {id}"));
                }
            }
            Ok(Arc::new(map))
        }
    }

    pub fn serialize<M: IdMap, S: Serializer>(map: &M, s: S) -> Result<S::Ok, S::Error> {
        map.items().serialize(s)
    }

    pub fn deserialize<'de, M: IdMap, D: Deserializer<'de>>(d: D) -> Result<M, D::Error> {
        let items = Vec::<M::Item>::deserialize(d)?;
        M::from_items(items).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_document_is_valid() {
        let doc = Document::with_comp("Test", CompSettings::default());
        doc.validate().unwrap();
        let main = doc.main().unwrap();
        assert_eq!(main.width, 1920);
        assert_eq!(
            doc.locate_property(main.background.id),
            Some(PropertyOwner::CompBackground(main.id))
        );
    }

    #[test]
    fn clones_share_until_written() {
        let mut a = Document::with_comp("Test", CompSettings::default());
        let b = a.clone();
        assert!(Arc::ptr_eq(&a.comps, &b.comps));
        let main = a.main_comp.unwrap();
        a.comp_mut(main).unwrap().name = "Changed".into();
        assert_eq!(b.main().unwrap().name, "Main");
        assert_eq!(a.main().unwrap().name, "Changed");
    }
}
