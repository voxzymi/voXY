//! The project pool: folders, usage queries and composition nesting rules
//! (design note `docs/design-media-comps.md` §1).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::id::{AssetId, CompId, FolderId, LayerId};
use crate::layer::SourceRef;

/// A folder (bin) in the project panel. Items inside are sorted by the
/// panel, so no order is stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
    /// `None` is the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<FolderId>,
}

/// Anything the project panel lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PoolItem {
    Asset(AssetId),
    Comp(CompId),
    Folder(FolderId),
}

impl From<SourceRef> for PoolItem {
    fn from(s: SourceRef) -> Self {
        match s {
            SourceRef::Asset(a) => PoolItem::Asset(a),
            SourceRef::Comp(c) => PoolItem::Comp(c),
        }
    }
}

/// Where sources are used: per composition, the layers showing them.
///
/// Compositions are ordered by name (case-insensitive), then ID; layers are
/// in stack order (top first).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Usage {
    pub per_comp: Vec<(CompId, Vec<LayerId>)>,
}

impl Usage {
    /// Number of layers.
    pub fn total(&self) -> usize {
        self.per_comp.iter().map(|(_, l)| l.len()).sum()
    }

    pub fn comp_count(&self) -> usize {
        self.per_comp.len()
    }

    pub fn is_empty(&self) -> bool {
        self.per_comp.is_empty()
    }

    /// The composition with the most layers; ties go to the first in
    /// name order. The keyboard "Reveal in Composition" target (§6.2).
    pub fn primary(&self) -> Option<CompId> {
        self.per_comp
            .iter()
            .enumerate()
            .max_by_key(|(i, (_, l))| (l.len(), std::cmp::Reverse(*i)))
            .map(|(_, (c, _))| *c)
    }
}

/// Every folder, asset and composition a set of pool items stands for:
/// folders include everything inside them, recursively.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExpandedItems {
    pub assets: BTreeSet<AssetId>,
    pub comps: BTreeSet<CompId>,
    pub folders: BTreeSet<FolderId>,
}

impl ExpandedItems {
    pub fn contains_source(&self, s: SourceRef) -> bool {
        match s {
            SourceRef::Asset(a) => self.assets.contains(&a),
            SourceRef::Comp(c) => self.comps.contains(&c),
        }
    }
}

impl Document {
    pub fn folder(&self, id: FolderId) -> Option<&Folder> {
        self.folders.get(&id)
    }

    /// Display name of a source, if it exists.
    pub fn source_name(&self, s: SourceRef) -> Option<&str> {
        match s {
            SourceRef::Asset(a) => self.assets.get(a).map(|a| a.name.as_str()),
            SourceRef::Comp(c) => self.comp(c).map(|c| c.name.as_str()),
        }
    }

    /// The folder an item is in (`None` at the top level or if missing).
    pub fn item_folder(&self, item: PoolItem) -> Option<FolderId> {
        match item {
            PoolItem::Asset(a) => self.assets.get(a).and_then(|a| a.folder),
            PoolItem::Comp(c) => self.comp(c).and_then(|c| c.folder),
            PoolItem::Folder(f) => self.folder(f).and_then(|f| f.parent),
        }
    }

    /// Whether `folder` is `ancestor` or inside it.
    pub fn folder_within(&self, folder: FolderId, ancestor: FolderId) -> bool {
        let mut cur = Some(folder);
        let mut steps = 0;
        while let Some(f) = cur {
            if f == ancestor {
                return true;
            }
            steps += 1;
            if steps > self.folders.len() {
                return false;
            }
            cur = self.folder(f).and_then(|f| f.parent);
        }
        false
    }

    /// Layers showing any of `sources`, grouped by composition (§1.9).
    /// Computed by scanning: there is no stored index to go stale.
    pub fn usages_of(&self, sources: &[SourceRef]) -> Usage {
        let mut per_comp: Vec<(CompId, Vec<LayerId>)> = self
            .comps
            .values()
            .filter_map(|c| {
                let layers: Vec<LayerId> = c
                    .layers
                    .iter()
                    .filter(|l| l.kind.source().is_some_and(|s| sources.contains(&s)))
                    .map(|l| l.id)
                    .collect();
                (!layers.is_empty()).then_some((c.id, layers))
            })
            .collect();
        per_comp.sort_by_cached_key(|(c, _)| {
            let name = self
                .comp(*c)
                .map_or(String::new(), |c| c.name.to_lowercase());
            (name, *c)
        });
        Usage { per_comp }
    }

    pub fn usages(&self, source: SourceRef) -> Usage {
        self.usages_of(&[source])
    }

    /// Expands folders into everything they contain.
    pub fn expand_items(&self, items: &[PoolItem]) -> ExpandedItems {
        let mut out = ExpandedItems::default();
        for &item in items {
            match item {
                PoolItem::Asset(a) => {
                    out.assets.insert(a);
                }
                PoolItem::Comp(c) => {
                    out.comps.insert(c);
                }
                PoolItem::Folder(f) => {
                    out.folders
                        .extend(self.folders.keys().filter(|&&g| self.folder_within(g, f)));
                }
            }
        }
        let inside = |f: Option<FolderId>| f.is_some_and(|f| out.folders.contains(&f));
        let assets: Vec<AssetId> = self
            .assets
            .iter()
            .filter(|a| inside(a.folder))
            .map(|a| a.id)
            .collect();
        let comps: Vec<CompId> = self
            .comps
            .values()
            .filter(|c| inside(c.folder))
            .map(|c| c.id)
            .collect();
        out.assets.extend(assets);
        out.comps.extend(comps);
        out
    }

    /// Layers that removing `items` affects: layers in compositions that
    /// stay, showing a source that goes. This is what the removal
    /// confirmation lists.
    pub fn removal_impact(&self, items: &[PoolItem]) -> Usage {
        let ex = self.expand_items(items);
        let sources: Vec<SourceRef> = ex
            .assets
            .iter()
            .map(|&a| SourceRef::Asset(a))
            .chain(ex.comps.iter().map(|&c| SourceRef::Comp(c)))
            .collect();
        let mut usage = self.usages_of(&sources);
        usage.per_comp.retain(|(c, _)| !ex.comps.contains(c));
        usage
    }

    /// Whether `outer` shows `inner` through precomp layers, at any depth.
    /// `comp_contains(c, c)` is true only for a cycle.
    pub fn comp_contains(&self, outer: CompId, inner: CompId) -> bool {
        self.precomp_path(outer, inner).is_some()
    }

    /// A chain of compositions `outer → … → inner` through precomp layers,
    /// excluding `outer` itself, if one exists.
    fn precomp_path(&self, outer: CompId, inner: CompId) -> Option<Vec<CompId>> {
        let mut visited = BTreeSet::new();
        let mut stack: Vec<(CompId, Vec<CompId>)> = vec![(outer, Vec::new())];
        while let Some((c, path)) = stack.pop() {
            let Some(comp) = self.comp(c) else { continue };
            for layer in &comp.layers {
                if let Some(SourceRef::Comp(child)) = layer.kind.source() {
                    let mut p = path.clone();
                    p.push(child);
                    if child == inner {
                        return Some(p);
                    }
                    if visited.insert(child) {
                        stack.push((child, p));
                    }
                }
            }
        }
        None
    }

    /// Why placing `inserted` inside `target` would make a composition
    /// contain itself, or `None` if it is allowed.
    pub fn nesting_error(&self, target: CompId, inserted: CompId) -> Option<String> {
        let name = |c: CompId| {
            self.comp(c)
                .map_or_else(|| c.to_string(), |c| format!("“{}”", c.name))
        };
        if target == inserted {
            return Some(format!("{} can't be placed inside itself", name(target)));
        }
        let path = self.precomp_path(inserted, target)?;
        let chain: Vec<String> = std::iter::once(inserted).chain(path).map(name).collect();
        Some(format!(
            "{} can't be placed in {}: {} already contains it ({})",
            name(inserted),
            name(target),
            name(inserted),
            chain.join(" › ")
        ))
    }
}
