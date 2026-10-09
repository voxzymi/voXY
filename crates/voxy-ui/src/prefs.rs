//! Per-user preferences, kept in the operating system's config directory
//! and never in a project file.
//!
//! The file is TOML. A missing, unreadable or corrupt file gives the
//! defaults, and saving problems are logged, never fatal.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use voxy_core::{FrameRate, PixelAspect, Time};

use crate::reveal::Os;

/// A composition preset the user saved. Built-in presets live in code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPreset {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: PixelAspect,
    pub frame_rate: FrameRate,
    pub duration: Time,
}

/// What the Composition Settings dialog remembers between uses.
#[allow(clippy::struct_excessive_bools)] // independent checkboxes of the dialog
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DialogMemory {
    pub lock_aspect: bool,
    /// Size changes keep layers (true) or scale them to fit (false).
    pub keep_layers: bool,
    /// The 3×3 anchor grid cell, `0..3` each.
    pub anchor: (u8, u8),
    pub snap_keys: bool,
    pub live_preview: bool,
}

impl Default for DialogMemory {
    fn default() -> Self {
        DialogMemory {
            lock_aspect: false,
            keep_layers: true,
            anchor: (1, 1),
            snap_keys: false,
            live_preview: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub user_presets: Vec<UserPreset>,
    pub comp_dialog: DialogMemory,
}

/// The per-user config directory for voXY on `os`, from environment
/// lookups, or `None` when the needed variable is missing.
pub fn config_dir_with(os: Os, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let base = match os {
        Os::Windows => absolute(env("APPDATA")),
        Os::Mac => absolute(env("HOME")).map(|h| h.join("Library").join("Application Support")),
        Os::Linux => absolute(env("XDG_CONFIG_HOME"))
            .or_else(|| absolute(env("HOME")).map(|h| h.join(".config"))),
    }?;
    Some(base.join("voXY"))
}

/// Where the preferences file lives on this machine.
pub fn default_path() -> Option<PathBuf> {
    config_dir_with(Os::current(), &|k| std::env::var_os(k)).map(|d| d.join("preferences.toml"))
}

impl Preferences {
    /// Reads `path`; anything wrong gives the defaults.
    pub fn load_from(path: &Path) -> Preferences {
        match std::fs::read_to_string(path) {
            Ok(text) => Preferences::parse(&text),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("could not read {}: {e}", path.display());
                }
                Preferences::default()
            }
        }
    }

    pub fn parse(text: &str) -> Preferences {
        toml::from_str(text).unwrap_or_else(|e| {
            tracing::warn!("ignoring corrupt preferences: {e}");
            Preferences::default()
        })
    }

    /// Writes the file atomically, creating the directory.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })
    }

    /// Adds or replaces the user preset called `preset.name`.
    pub fn upsert_preset(&mut self, preset: UserPreset) {
        match self.user_presets.iter_mut().find(|p| p.name == preset.name) {
            Some(p) => *p = preset,
            None => self.user_presets.push(preset),
        }
    }

    pub fn remove_preset(&mut self, name: &str) -> bool {
        let before = self.user_presets.len();
        self.user_presets.retain(|p| p.name != name);
        self.user_presets.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute path on the host running the test: `C:\a\b` on Windows,
    /// `/a/b` elsewhere. `config_dir_with` checks `Path::is_absolute`, which
    /// is a property of the host, so the test inputs must be built that way
    /// (`/home/me` is not absolute on Windows).
    fn abs(parts: &[&str]) -> PathBuf {
        let root = if cfg!(windows) { r"C:\" } else { "/" };
        parts
            .iter()
            .fold(PathBuf::from(root), |p, part| p.join(part))
    }

    /// Lookup closure over `(name, absolute path)` pairs.
    fn vars(pairs: &[(&str, PathBuf)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone().into_os_string()))
            .collect();
        move |k| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn config_directories_per_os() {
        let home = abs(&["home", "me"]);
        let linux = config_dir_with(Os::Linux, &vars(&[("HOME", home.clone())]));
        assert_eq!(linux, Some(home.join(".config").join("voXY")));

        let cfg = abs(&["cfg"]);
        let xdg = config_dir_with(
            Os::Linux,
            &vars(&[("HOME", home.clone()), ("XDG_CONFIG_HOME", cfg.clone())]),
        );
        assert_eq!(xdg, Some(cfg.join("voXY")));

        // A relative XDG value is ignored, as the spec says.
        let rel = config_dir_with(
            Os::Linux,
            &vars(&[
                ("HOME", home.clone()),
                ("XDG_CONFIG_HOME", PathBuf::from("cfg")),
            ]),
        );
        assert_eq!(rel, Some(home.join(".config").join("voXY")));

        let mac_home = abs(&["Users", "me"]);
        let mac = config_dir_with(Os::Mac, &vars(&[("HOME", mac_home.clone())]));
        assert_eq!(
            mac,
            Some(
                mac_home
                    .join("Library")
                    .join("Application Support")
                    .join("voXY")
            )
        );

        // %APPDATA% on Windows (checkable on every host with a host-absolute
        // path); a relative one is ignored.
        let roaming = abs(&["Users", "me", "AppData", "Roaming"]);
        let win = config_dir_with(Os::Windows, &vars(&[("APPDATA", roaming.clone())]));
        assert_eq!(win, Some(roaming.join("voXY")));
        let win_rel = config_dir_with(Os::Windows, &vars(&[("APPDATA", PathBuf::from("rel"))]));
        assert_eq!(win_rel, None);

        // Without the variables there is no directory.
        assert_eq!(config_dir_with(Os::Windows, &vars(&[])), None);
        assert_eq!(config_dir_with(Os::Linux, &vars(&[])), None);
        assert_eq!(config_dir_with(Os::Mac, &vars(&[])), None);
    }

    #[test]
    fn missing_and_corrupt_files_give_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Preferences::load_from(&dir.path().join("none.toml")),
            Preferences::default()
        );
        let bad = dir.path().join("bad.toml");
        std::fs::write(&bad, "this is [not toml").unwrap();
        assert_eq!(Preferences::load_from(&bad), Preferences::default());
        std::fs::write(&bad, "user_presets = 5").unwrap();
        assert_eq!(
            Preferences::load_from(&bad),
            Preferences::default(),
            "wrong types too"
        );
    }

    #[test]
    fn presets_round_trip_and_unknown_keys_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("preferences.toml");
        let mut p = Preferences::default();
        p.upsert_preset(UserPreset {
            name: "Mine".into(),
            width: 1080,
            height: 1350,
            pixel_aspect: PixelAspect::SQUARE,
            frame_rate: FrameRate::FPS_29_97,
            duration: Time::from_secs(15),
        });
        p.comp_dialog.keep_layers = false;
        p.save_to(&path).unwrap();
        assert_eq!(Preferences::load_from(&path), p);
        let text = std::fs::read_to_string(&path).unwrap() + "\nfuture = 1\n";
        assert_eq!(Preferences::parse(&text), p);
        // Replacing by name keeps one entry.
        let mut q = p.clone();
        q.upsert_preset(UserPreset {
            width: 720,
            ..q.user_presets[0].clone()
        });
        assert_eq!(q.user_presets.len(), 1);
        assert_eq!(q.user_presets[0].width, 720);
        assert!(q.remove_preset("Mine") && !q.remove_preset("Mine"));
    }

    #[test]
    fn partial_files_fill_in_defaults() {
        let p = Preferences::parse("[comp_dialog]\nlock_aspect = true\n");
        assert!(p.comp_dialog.lock_aspect);
        assert!(p.comp_dialog.keep_layers && p.comp_dialog.live_preview);
        assert_eq!(p.user_presets.len(), 0);
    }
}
