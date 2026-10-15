//! The discovery cache, `ofx-cache.json` in the config folder (design
//! §6.2): each bundle's description, keyed by its binary's path,
//! modification time and size. Start-up re-describes only bundles whose
//! key changed, so unchanged plugins are not loaded at all until used.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::protocol::BundleDesc;

/// Bumped when the stored form changes; an older cache is ignored.
const FORMAT: u32 = 1;

/// What identifies one version of a binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryKey {
    pub path: PathBuf,
    /// Modification time in nanoseconds since 1970.
    pub mtime_ns: u128,
    pub size: u64,
}

impl BinaryKey {
    /// The key of the binary at `path` now, if it can be read.
    pub fn of(path: &Path) -> Option<BinaryKey> {
        let m = std::fs::metadata(path).ok()?;
        let mtime_ns = m
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(BinaryKey {
            path: path.to_path_buf(),
            mtime_ns,
            size: m.len(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    key: BinaryKey,
    bundle: PathBuf,
    desc: BundleDesc,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DescribeCache {
    format: u32,
    entries: Vec<Entry>,
}

impl DescribeCache {
    /// Reads `path`; a missing, unreadable or old cache is empty.
    pub fn load(path: &Path) -> DescribeCache {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<DescribeCache>(&b).ok())
            .filter(|c| c.format == FORMAT)
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_vec_pretty(&DescribeCache {
            format: FORMAT,
            entries: self.entries.clone(),
        })
        .map_err(std::io::Error::other)?;
        // Write then rename, so a crash never leaves half a cache.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    /// The description stored for `key`, if it is still current.
    pub fn get(&self, key: &BinaryKey) -> Option<&BundleDesc> {
        self.entries.iter().find(|e| &e.key == key).map(|e| &e.desc)
    }

    /// Stores `desc` for `key`, replacing older entries of the same binary.
    pub fn put(&mut self, bundle: &Path, key: BinaryKey, desc: BundleDesc) {
        self.entries.retain(|e| e.key.path != key.path);
        self.entries.push(Entry {
            key,
            bundle: bundle.to_path_buf(),
            desc,
        });
    }

    /// Drops entries whose binary is not in `keep` (uninstalled plugins).
    pub fn retain_binaries(&mut self, keep: &[PathBuf]) {
        self.entries.retain(|e| keep.contains(&e.key.path));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
