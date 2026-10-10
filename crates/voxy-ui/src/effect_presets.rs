//! User effect presets (`docs/design-effects-ofx.md` §4.3).
//!
//! Kept in `effect_presets.toml` in the per-user config directory, apart
//! from the preferences because presets can be large. Each preset is a name
//! and the effects it holds, as the JSON a project file uses (TOML cannot
//! hold the `null` of an empty layer reference). The loader is tolerant
//! like the easing presets: an unreadable preset is dropped alone and the
//! rest are kept.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use voxy_core::EffectInstance;
use voxy_core::registry::{EffectRegistry, resolve_effect};

use crate::reveal::Os;

/// One saved preset: a name and its effects, in stack order.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectPreset {
    pub name: String,
    pub effects: Vec<EffectInstance>,
}

#[derive(Serialize, Deserialize)]
struct PresetRepr {
    name: String,
    /// The effects as project-file JSON.
    effects: String,
}

#[derive(Serialize, Deserialize, Default)]
struct FileRepr {
    #[serde(default)]
    preset: Vec<toml::Value>,
}

/// Where the presets file lives on this machine.
pub fn default_path() -> Option<PathBuf> {
    crate::prefs::config_dir_with(Os::current(), &|k| std::env::var_os(k))
        .map(|d| d.join("effect_presets.toml"))
}

/// Reads presets from `text`, dropping the ones that do not parse.
pub fn parse(text: &str) -> Vec<EffectPreset> {
    let file: FileRepr = toml::from_str(text).unwrap_or_else(|e| {
        tracing::warn!("ignoring a corrupt effect presets file: {e}");
        FileRepr::default()
    });
    file.preset
        .into_iter()
        .filter_map(|v| {
            let repr: PresetRepr = v
                .try_into()
                .map_err(|e| tracing::warn!("ignoring an unreadable effect preset: {e}"))
                .ok()?;
            let effects: Vec<EffectInstance> = serde_json::from_str(&repr.effects)
                .map_err(|e| tracing::warn!("ignoring effect preset {:?}: {e}", repr.name))
                .ok()?;
            let name = repr.name.trim().to_owned();
            (!name.is_empty() && !effects.is_empty()).then_some(EffectPreset { name, effects })
        })
        .collect()
}

pub fn to_text(presets: &[EffectPreset]) -> Result<String, String> {
    let file = FileRepr {
        preset: presets
            .iter()
            .map(|p| {
                let effects = serde_json::to_string(&p.effects).map_err(|e| e.to_string())?;
                toml::Value::try_from(PresetRepr {
                    name: p.name.clone(),
                    effects,
                })
                .map_err(|e| e.to_string())
            })
            .collect::<Result<_, _>>()?,
    };
    toml::to_string_pretty(&file).map_err(|e| e.to_string())
}

/// Reads `path`; a missing or unreadable file gives no presets.
pub fn load_from(path: &Path) -> Vec<EffectPreset> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("could not read {}: {e}", path.display());
            }
            Vec::new()
        }
    }
}

/// Writes the file atomically, creating the directory.
pub fn save_to(presets: &[EffectPreset], path: &Path) -> Result<(), String> {
    let text = to_text(presets)?;
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

/// Adds or replaces the preset called `name` (trimmed). Returns `false` for
/// an empty name or no effects.
pub fn upsert(presets: &mut Vec<EffectPreset>, name: &str, effects: Vec<EffectInstance>) -> bool {
    let name = name.trim();
    if name.is_empty() || effects.is_empty() {
        return false;
    }
    match presets.iter_mut().find(|p| p.name == name) {
        Some(p) => p.effects = effects,
        None => presets.push(EffectPreset {
            name: name.to_owned(),
            effects,
        }),
    }
    true
}

/// The preset's effects with their definitions from `registry`, ready to
/// paste, and warnings for the ones this session cannot use (left out).
pub fn resolved(
    preset: &EffectPreset,
    registry: &EffectRegistry,
) -> (Vec<EffectInstance>, Vec<String>) {
    let mut next = u64::MAX / 2;
    let mut alloc = || {
        next += 1;
        next
    };
    let mut warnings = Vec::new();
    let mut out = Vec::new();
    for fx in &preset.effects {
        let mut fx = fx.clone();
        warnings.extend(resolve_effect(&mut fx, registry, &mut alloc));
        if fx.is_placeholder() {
            warnings.push(format!("“{}” is not installed and was left out.", fx.name));
        } else {
            out.push(fx);
        }
    }
    (out, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxy_core::registry::{builtins, ids};
    use voxy_core::{CompSettings, Document, EffectTypeId, Value};

    fn blur(radius: f64) -> EffectInstance {
        let mut doc = Document::with_comp("P", CompSettings::default());
        let mut fx = builtins()
            .latest(&EffectTypeId::new(ids::GAUSSIAN_BLUR))
            .expect("built-in")
            .instantiate(&mut doc);
        fx.param_mut("radius")
            .expect("radius")
            .set_static(Value::Scalar(radius))
            .expect("in range");
        fx
    }

    #[test]
    fn presets_round_trip_and_resolve_to_real_definitions() {
        let mut presets = Vec::new();
        assert!(upsert(&mut presets, "  Soft  ", vec![blur(4.0), blur(9.0)]));
        assert!(!upsert(&mut presets, "   ", vec![blur(1.0)]), "empty name");
        assert!(upsert(&mut presets, "Soft", vec![blur(2.0)]), "replaces");
        assert_eq!(presets.len(), 1);
        let text = to_text(&presets).unwrap();
        let back = parse(&text);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "Soft");
        let (effects, warnings) = resolved(&back[0], builtins());
        assert!(warnings.is_empty(), "{warnings:?}");
        let radius = effects[0].param("radius").unwrap();
        assert_eq!(radius.value_at(voxy_core::Time::ZERO), Value::Scalar(2.0));
        assert_eq!(&*radius.desc.label, "Blurriness", "the built-in descriptor");
    }

    #[test]
    fn one_bad_preset_does_not_cost_the_rest() {
        let mut presets = Vec::new();
        upsert(&mut presets, "Good", vec![blur(3.0)]);
        let mut text = to_text(&presets).unwrap();
        text.push_str("\n[[preset]]\nname = \"Broken\"\neffects = \"not json\"\n");
        text.push_str("\n[[preset]]\nnope = 1\n");
        let back = parse(&text);
        assert_eq!(
            back.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Good"]
        );
        assert_eq!(parse("this is not toml [[").len(), 0);
    }

    #[test]
    fn effects_this_session_cannot_use_are_left_out_with_a_warning() {
        let mut fx = blur(5.0);
        fx.type_id = EffectTypeId::new("ofx:com.example.Missing");
        let preset = EffectPreset {
            name: "Plugin".into(),
            effects: vec![blur(1.0), fx],
        };
        let text = to_text(std::slice::from_ref(&preset)).unwrap();
        let (effects, warnings) = resolved(&parse(&text)[0], builtins());
        assert_eq!(effects.len(), 1);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn files_save_atomically_and_missing_files_are_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("effect_presets.toml");
        assert_eq!(load_from(&path).len(), 0);
        let mut presets = Vec::new();
        upsert(&mut presets, "A", vec![blur(1.0)]);
        save_to(&presets, &path).unwrap();
        assert_eq!(load_from(&path).len(), 1);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
}
