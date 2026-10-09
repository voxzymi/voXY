//! The document: the root of the model.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::asset::{Asset, AssetTable};
use crate::color::{LinearRgba, WorkingSpace};
use crate::comp::Composition;
use crate::id::{AssetId, CompId, LayerId, PropId};
use crate::layer::{ForeignData, Layer, LayerKind};
use crate::property::{Property, keys};
use crate::time::{FrameRate, Time};
use crate::value::Value;

/// Project-wide settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    #[serde(default)]
    pub working_space: WorkingSpace,
}

/// Settings for a new composition.
#[derive(Debug, Clone, PartialEq)]
pub struct CompSettings {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub duration: Time,
    pub background: LinearRgba,
}

impl Default for CompSettings {
    fn default() -> Self {
        CompSettings {
            name: "Main".into(),
            width: 1920,
            height: 1080,
            frame_rate: FrameRate::FPS_30,
            duration: Time::from_secs(10),
            background: LinearRgba::BLACK,
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
    pub root_comp: CompId,
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
    #[error("root composition {0} does not exist")]
    MissingRoot(CompId),
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
    #[error("property {prop} has key `{found}` where `{expected}` is required")]
    WrongProperty {
        prop: PropId,
        expected: &'static str,
        found: String,
    },
}

impl Document {
    /// A new document with one empty root composition.
    pub fn new(name: impl Into<String>, settings: CompSettings) -> Self {
        let mut doc = Document {
            meta: ProjectMeta {
                name: name.into(),
                working_space: WorkingSpace::default(),
            },
            assets: Arc::default(),
            comps: Arc::default(),
            root_comp: CompId(0),
            foreign: ForeignData::default(),
            next_id: 1,
        };
        let id = doc.add_comp(settings);
        doc.root_comp = id;
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
            frame_rate: s.frame_rate,
            duration: s.duration,
            background,
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

    pub fn root(&self) -> &Composition {
        self.comp(self.root_comp)
            .expect("validated documents always have their root composition")
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
        if !self.comps.contains_key(&self.root_comp) {
            return Err(ValidationError::MissingRoot(self.root_comp));
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
        for asset in self.assets.iter() {
            claim(asset.id.raw())?;
        }
        for comp in self.comps.values() {
            claim(comp.id.raw())?;
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
                validate_layer_props(layer)?;
                if layer.out_point < layer.in_point {
                    return Err(ValidationError::InvertedLayer(layer.id));
                }
                if let LayerKind::Image { asset } = layer.kind
                    && self.assets.get(asset).is_none()
                {
                    return Err(ValidationError::MissingAsset {
                        layer: layer.id,
                        asset,
                    });
                }
            }
            validate_parenting(comp)?;
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

fn validate_layer_props(layer: &Layer) -> Result<(), ValidationError> {
    let t = &layer.transform;
    expect_key(&t.anchor, keys::ANCHOR)?;
    expect_key(&t.position, keys::POSITION)?;
    expect_key(&t.scale, keys::SCALE)?;
    expect_key(&t.rotation, keys::ROTATION)?;
    expect_key(&t.opacity, keys::OPACITY)?;
    if let LayerKind::Solid { color, size } = &layer.kind {
        expect_key(color, keys::SOLID_COLOR)?;
        expect_key(size, keys::SOLID_SIZE)?;
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
    use crate::id::{AssetId, CompId};

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
        let doc = Document::new("Test", CompSettings::default());
        doc.validate().unwrap();
        assert_eq!(doc.root().width, 1920);
        assert_eq!(
            doc.locate_property(doc.root().background.id),
            Some(PropertyOwner::CompBackground(doc.root_comp))
        );
    }

    #[test]
    fn clones_share_until_written() {
        let mut a = Document::new("Test", CompSettings::default());
        let b = a.clone();
        assert!(Arc::ptr_eq(&a.comps, &b.comps));
        let root = a.root_comp;
        a.comp_mut(root).unwrap().name = "Changed".into();
        assert_eq!(b.root().name, "Main");
        assert_eq!(a.root().name, "Changed");
    }
}
