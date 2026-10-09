//! Media assets referenced by layers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::id::AssetId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
}

/// A file a project uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    pub name: String,
    pub kind: AssetKind,
    /// Path with `/` separators. Relative paths resolve against the directory
    /// that contains the `.voxy` file; absolute paths are used as-is.
    pub path: String,
    /// Native pixel size, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_size: Option<[u32; 2]>,
}

impl Asset {
    /// The file system path, resolved against `project_dir` if relative.
    pub fn resolve(&self, project_dir: Option<&Path>) -> PathBuf {
        let p = Path::new(&self.path);
        match project_dir {
            Some(dir) if p.is_relative() => dir.join(p),
            _ => p.to_path_buf(),
        }
    }
}

/// All assets of a document, keyed by ID.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetTable(pub BTreeMap<AssetId, Asset>);

impl AssetTable {
    pub fn get(&self, id: AssetId) -> Option<&Asset> {
        self.0.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Asset> {
        self.0.values()
    }
}
