//! Media assets referenced by layers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::id::{AssetId, FolderId};
use crate::time::{FrameRate, Time};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    /// A video file (format 0.3.0). Its audio is not used yet.
    Video,
    /// A 3D colour lookup table (`.cube`) for the LUT effect (format 0.7.0).
    /// Never a layer source.
    Lut,
}

impl AssetKind {
    pub fn label(self) -> &'static str {
        match self {
            AssetKind::Image => "Image",
            AssetKind::Video => "Video",
            AssetKind::Lut => "LUT",
        }
    }
}

/// A cheap fingerprint of a file's content, used to suggest relink
/// candidates (format 0.4.0). voXY computes it outside this crate (it needs
/// file I/O); here it is opaque data. It is never used as an identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentHash {
    /// Algorithm name, e.g. `"fnv1a64-sampled"`.
    pub algo: String,
    /// File size in bytes.
    pub size: u64,
    /// Hex digest.
    pub digest: String,
}

/// Audio stream facts of a media file, when known (format 0.4.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudioInfo {
    pub channels: u16,
    pub sample_rate: u32,
}

/// A file a project uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    /// Display name in the project panel; independent of the file name.
    pub name: String,
    pub kind: AssetKind,
    /// Path with `/` separators. Relative paths resolve against the directory
    /// that contains the `.voxy` file; absolute paths are used as-is.
    pub path: String,
    /// Native pixel size, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_size: Option<[u32; 2]>,
    /// Length of a video, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Time>,
    /// Nominal frame rate of a video, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_rate: Option<FrameRate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<ContentHash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioInfo>,
    /// Project panel folder; `None` is the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<FolderId>,
}

impl Asset {
    /// An asset with no metadata yet. The ID is assigned by
    /// [`crate::Document::add_asset`].
    pub fn new(kind: AssetKind, name: impl Into<String>, path: impl Into<String>) -> Self {
        Asset {
            id: AssetId(0),
            name: name.into(),
            kind,
            path: path.into(),
            pixel_size: None,
            duration: None,
            frame_rate: None,
            content_hash: None,
            audio: None,
            folder: None,
        }
    }

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
