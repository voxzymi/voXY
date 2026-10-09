//! Reading and writing `.voxy` project files.
//!
//! A `.voxy` file is UTF-8 JSON with a small envelope around the document;
//! see `docs/voxy-format.md`. Media lives next to it in `<name>.assets/`.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::document::{Document, ValidationError};

/// File extension of project files.
pub const PROJECT_EXTENSION: &str = "voxy";
/// File extension of packaged (zipped) projects. Packaging arrives later.
pub const PACKAGE_EXTENSION: &str = "voxyz";
/// Value of the envelope's `format` field.
pub const FORMAT_ID: &str = "voxy-project";
/// The format version this build writes, and the newest it reads.
pub const FORMAT_VERSION: Version = Version::new(0, 5, 0);
/// Version of the `ui_state` section this build writes. The section is
/// opaque to this crate; see [`to_json_with_ui`].
pub const UI_STATE_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("could not read project: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a voXY project file")]
    NotAProject,
    #[error("invalid format version `{0}`")]
    BadVersion(String),
    #[error(
        "this project was written by a newer voXY (format {found}); this build reads up to {supported}"
    )]
    TooNew { found: Version, supported: Version },
    #[error("malformed project: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid project: {0}")]
    Invalid(#[from] ValidationError),
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("could not write project: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not encode project: {0}")]
    Json(#[from] serde_json::Error),
    #[error("refusing to save an invalid document: {0}")]
    Invalid(#[from] ValidationError),
}

#[derive(Serialize)]
struct EnvelopeOut<'a> {
    format: &'static str,
    format_version: String,
    generator: String,
    project: &'a Document,
    #[serde(skip_serializing_if = "Option::is_none")]
    ui_state: Option<&'a serde_json::Value>,
}

#[derive(Deserialize)]
struct Header {
    format: String,
    format_version: String,
}

#[derive(Deserialize)]
struct EnvelopeIn {
    project: serde_json::Value,
    #[serde(default)]
    ui_state: Option<serde_json::Value>,
}

/// Encodes a document as pretty-printed project JSON.
pub fn to_json(doc: &Document) -> Result<String, SaveError> {
    to_json_with_ui(doc, None)
}

/// Encodes a document plus the editor's `ui_state` section (open tabs,
/// playheads, panel state). The section sits beside `project`, never inside
/// it: rendering and the document never read it, and loaders ignore it if
/// they do not understand it.
pub fn to_json_with_ui(
    doc: &Document,
    ui_state: Option<&serde_json::Value>,
) -> Result<String, SaveError> {
    doc.validate()?;
    let env = EnvelopeOut {
        format: FORMAT_ID,
        format_version: FORMAT_VERSION.to_string(),
        generator: format!("voXY {}", env!("CARGO_PKG_VERSION")),
        project: doc,
        ui_state,
    };
    let mut s = serde_json::to_string_pretty(&env)?;
    s.push('\n');
    Ok(s)
}

/// Decodes project JSON, checking the format version and the document's
/// invariants.
pub fn from_json(json: &str) -> Result<Document, LoadError> {
    from_json_with_ui(json).map(|(doc, _)| doc)
}

/// [`from_json`], also returning the `ui_state` section if the file has
/// one. It is returned unparsed: the editor validates it and drops what it
/// cannot use.
pub fn from_json_with_ui(json: &str) -> Result<(Document, Option<serde_json::Value>), LoadError> {
    let header: Header = serde_json::from_str(json).map_err(|_| LoadError::NotAProject)?;
    if header.format != FORMAT_ID {
        return Err(LoadError::NotAProject);
    }
    let found = Version::parse(&header.format_version)
        .map_err(|_| LoadError::BadVersion(header.format_version.clone()))?;
    // Compare without pre-release/build metadata: same numbers, same format.
    let found_core = Version::new(found.major, found.minor, found.patch);
    if found_core > FORMAT_VERSION {
        return Err(LoadError::TooNew {
            found,
            supported: FORMAT_VERSION,
        });
    }
    // Older versions are migrated here, one step at a time.
    // - 0.1.0 → 0.2.0: 0.2.0 only adds keyframed properties; every 0.1.0 file
    //   is already a valid 0.2.0 document, so no rewrite is needed.
    // - 0.2.0 → 0.3.0: 0.3.0 only adds video assets and layers (and the
    //   optional asset fields `duration` and `frame_rate`); likewise no
    //   rewrite.
    // - 0.3.0 → 0.4.0: `root_comp` becomes the optional `main_comp`; every
    //   other addition has a default (no folders, square pixels, no work
    //   area, default project settings).
    // - 0.4.0 → 0.5.0: only adds the optional composition fields
    //   `start_offset`, `drop_frame` and `preview_resolution` (and the same
    //   in the project's new-composition defaults); no rewrite.
    let env: EnvelopeIn = serde_json::from_str(json)?;
    let mut project = env.project;
    if found_core < Version::new(0, 4, 0) {
        migrate_0_3_to_0_4(&mut project);
    }
    let doc: Document = serde_json::from_value(project)?;
    doc.validate()?;
    Ok((doc, env.ui_state))
}

fn migrate_0_3_to_0_4(project: &mut serde_json::Value) {
    if let Some(obj) = project.as_object_mut()
        && let Some(root) = obj.remove("root_comp")
    {
        obj.insert("main_comp".into(), root);
    }
}

/// Writes a project atomically: to a temporary file in the same directory,
/// flushed to disk, then renamed over the target.
pub fn save(doc: &Document, path: &Path) -> Result<(), SaveError> {
    save_with_ui(doc, None, path)
}

/// [`save`] with a `ui_state` section.
pub fn save_with_ui(
    doc: &Document,
    ui_state: Option<&serde_json::Value>,
    path: &Path,
) -> Result<(), SaveError> {
    let json = to_json_with_ui(doc, ui_state)?;
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .map_or_else(|| "project".into(), |n| n.to_string_lossy());
    let tmp = dir.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

pub fn load(path: &Path) -> Result<Document, LoadError> {
    from_json(&fs::read_to_string(path)?)
}

/// [`load`], also returning the `ui_state` section.
pub fn load_with_ui(path: &Path) -> Result<(Document, Option<serde_json::Value>), LoadError> {
    from_json_with_ui(&fs::read_to_string(path)?)
}

/// The assets folder that belongs to a project file: `<name>.assets/` next to
/// `<name>.voxy`.
pub fn assets_dir(project_path: &Path) -> PathBuf {
    let stem = project_path
        .file_stem()
        .map_or_else(|| "project".into(), |s| s.to_string_lossy());
    project_path.with_file_name(format!("{stem}.assets"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{AddImageLayer, AddSolidLayer, Command, SetProperty};
    use crate::document::CompSettings;
    use crate::{LinearRgba, Value};

    fn sample() -> Document {
        let mut doc = Document::with_comp("Sample", CompSettings::default());
        let comp = doc.main_comp.unwrap();
        AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::new(0.1, 0.2, 0.3, 1.0),
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        AddImageLayer {
            comp,
            path: "Sample.assets/logo.png".into(),
            pixel_size: [64, 32],
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let rot = doc.main().unwrap().layers[0].transform.rotation.id;
        SetProperty {
            time: crate::Time::ZERO,
            prop: rot,
            value: Value::Scalar(0.1 + 0.2),
        }
        .apply(&mut doc)
        .unwrap();
        doc
    }

    #[test]
    fn json_round_trip_is_exact() {
        let doc = sample();
        let json = to_json(&doc).unwrap();
        let back = from_json(&json).unwrap();
        assert_eq!(back, doc);
        // Re-encoding is byte-identical.
        assert_eq!(to_json(&back).unwrap(), json);
    }

    #[test]
    fn file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Sample.voxy");
        let doc = sample();
        save(&doc, &path).unwrap();
        save(&doc, &path).unwrap(); // overwrite in place
        assert_eq!(load(&path).unwrap(), doc);
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "temporary file left behind");
        assert_eq!(assets_dir(&path), dir.path().join("Sample.assets"));
    }

    #[test]
    fn newer_versions_are_refused() {
        let current = format!("\"{FORMAT_VERSION}\"");
        let json = to_json(&sample()).unwrap().replace(&current, "\"0.6.0\"");
        assert!(matches!(from_json(&json), Err(LoadError::TooNew { .. })));
        let json = to_json(&sample()).unwrap().replace(&current, "\"banana\"");
        assert!(matches!(from_json(&json), Err(LoadError::BadVersion(_))));
    }

    #[test]
    fn older_versions_still_load() {
        // A 0.1.0 file (static properties only) or 0.2.0 file (no video) is
        // valid 0.3.0 content. (Real 0.3.0 files: `tests/pool.rs`.)
        let doc = sample();
        let current = format!("\"{FORMAT_VERSION}\"");
        for old in ["0.1.0", "0.2.0"] {
            let json = to_json(&doc)
                .unwrap()
                .replace(&current, &format!("\"{old}\""));
            assert_eq!(from_json(&json).unwrap(), doc, "{old}");
        }
    }

    #[test]
    fn video_layers_round_trip() {
        use crate::command::AddVideoLayer;
        use crate::{FrameRate, LayerKind, Time};
        let mut doc = sample();
        let comp = doc.main_comp.unwrap();
        AddVideoLayer {
            comp,
            path: "clip.mp4".into(),
            pixel_size: [1920, 1080],
            duration: Some(Time::from_secs(2)),
            frame_rate: Some(FrameRate::FPS_29_97),
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let layer = &doc.main().unwrap().layers[0];
        assert!(matches!(layer.kind, LayerKind::Video { .. }));
        assert_eq!(layer.out_point, Time::from_secs(2), "ends with the video");
        let json = to_json(&doc).unwrap();
        assert!(json.contains("\"type\": \"video\""), "{json}");
        assert_eq!(from_json(&json).unwrap(), doc);
    }

    #[test]
    fn keyframed_documents_round_trip() {
        use crate::command::{AddKeyframe, ApplyEasePreset, SetSpatialTangents};
        use crate::{EasePreset, SpatialTangents, Time};
        let mut doc = sample();
        let pos = doc.main().unwrap().layers[0].transform.position.id;
        AddKeyframe {
            prop: pos,
            time: Time::ZERO,
        }
        .apply(&mut doc)
        .unwrap();
        SetProperty {
            prop: pos,
            time: Time::from_secs(2),
            value: Value::Vec2(crate::glam::DVec2::new(10.0, 20.0)),
        }
        .apply(&mut doc)
        .unwrap();
        ApplyEasePreset {
            prop: pos,
            times: vec![Time::ZERO],
            preset: EasePreset::EasyEase,
        }
        .apply(&mut doc)
        .unwrap();
        SetSpatialTangents {
            prop: pos,
            time: Time::ZERO,
            tangents: Some(SpatialTangents {
                incoming: crate::glam::DVec3::ZERO,
                outgoing: crate::glam::DVec3::new(0.0, 300.0, 0.0),
            }),
        }
        .apply(&mut doc)
        .unwrap();
        let json = to_json(&doc).unwrap();
        let back = from_json(&json).unwrap();
        assert_eq!(back, doc);
        assert_eq!(to_json(&back).unwrap(), json);
    }

    #[test]
    fn non_projects_are_refused() {
        assert!(matches!(from_json("{}"), Err(LoadError::NotAProject)));
        assert!(matches!(from_json("[1,2]"), Err(LoadError::NotAProject)));
        assert!(matches!(
            from_json(r#"{"format":"other","format_version":"0.1.0"}"#),
            Err(LoadError::NotAProject)
        ));
    }

    #[test]
    fn invalid_documents_are_refused() {
        let json = to_json(&sample()).unwrap();
        // Point the root at a composition that does not exist.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let mut v2 = v.clone();
        v2["project"]["main_comp"] = 4242.into();
        assert!(matches!(
            from_json(&v2.to_string()),
            Err(LoadError::Invalid(_))
        ));
        // Duplicate an ID.
        let mut v3 = v;
        let first_id = v3["project"]["comps"][0]["id"].clone();
        v3["project"]["comps"][0]["layers"][0]["id"] = first_id;
        assert!(matches!(
            from_json(&v3.to_string()),
            Err(LoadError::Invalid(_))
        ));
    }

    #[test]
    fn unknown_foreign_data_survives() {
        let json = to_json(&sample()).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["project"]["comps"][0]["layers"][0]["foreign"] =
            serde_json::json!({"alight_motion": {"raw": "<x a=\"1\"/>"}});
        let doc = from_json(&v.to_string()).unwrap();
        let again: serde_json::Value = serde_json::from_str(&to_json(&doc).unwrap()).unwrap();
        assert_eq!(
            again["project"]["comps"][0]["layers"][0]["foreign"],
            v["project"]["comps"][0]["layers"][0]["foreign"]
        );
    }
}
