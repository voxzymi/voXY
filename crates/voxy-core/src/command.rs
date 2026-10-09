//! Commands: every edit to a document is one.

use std::borrow::Cow;
use std::fmt::Debug;

use glam::DVec2;

use crate::asset::{Asset, AssetKind};
use crate::color::{LinearRgba, WorkingSpace};
use crate::document::{Document, ValidationError};
use crate::easing::{EaseHandle, EasePreset};
use crate::id::{CompId, LayerId, PropId};
use crate::keyframe::{Interp, Keyframe, SpatialTangents};
use crate::layer::{BlendMode, ForeignData, Layer, LayerFlags, LayerKind, Transform};
use crate::property::{Property, PropertyError, keys};
use crate::time::Time;
use crate::value::Value;

/// Identifies edits that may collapse into one undo step while an
/// interaction (a drag, a slider scrub) is in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MergeKey {
    pub kind: &'static str,
    pub target: u64,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CommandError {
    #[error("{0} not found")]
    NotFound(String),
    #[error("invalid edit: {0}")]
    Invalid(String),
    #[error(transparent)]
    Property(#[from] PropertyError),
    #[error("edit would make the document invalid: {0}")]
    Validation(#[from] ValidationError),
}

/// An edit. [`crate::History`] applies it to a copy of the current document,
/// so `apply` only has to move forward: undo restores the previous snapshot
/// and no inverse operation is needed.
///
/// `apply` must either succeed or return an error; on error the copy is
/// discarded, so edits are atomic.
pub trait Command: Send + Debug + 'static {
    /// Shown in the Edit menu, e.g. "Undo Add Solid".
    fn label(&self) -> Cow<'static, str>;
    fn apply(&self, doc: &mut Document) -> Result<(), CommandError>;
    /// Commands with equal keys merge while an interaction is open.
    fn merge_key(&self) -> Option<MergeKey> {
        None
    }
}

fn comp_mut(doc: &mut Document, id: CompId) -> Result<&mut crate::Composition, CommandError> {
    doc.comp_mut(id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))
}

fn insert_layer(
    doc: &mut Document,
    comp: CompId,
    index: usize,
    layer: Layer,
) -> Result<(), CommandError> {
    let c = comp_mut(doc, comp)?;
    let index = index.min(c.layers.len());
    c.layers.insert(index, layer.into());
    Ok(())
}

/// Adds a solid that covers the composition. `index` 0 puts it on top.
#[derive(Debug, Clone)]
pub struct AddSolidLayer {
    pub comp: CompId,
    pub name: Option<String>,
    pub color: LinearRgba,
    pub index: usize,
}

impl Command for AddSolidLayer {
    fn label(&self) -> Cow<'static, str> {
        "Add Solid".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let c = doc
            .comp(self.comp)
            .ok_or_else(|| CommandError::NotFound(self.comp.to_string()))?;
        let size = DVec2::new(f64::from(c.width), f64::from(c.height));
        let duration = c.duration;
        let n = c
            .layers
            .iter()
            .filter(|l| matches!(l.kind, LayerKind::Solid { .. }))
            .count();
        let name = self
            .name
            .clone()
            .unwrap_or_else(|| format!("Solid {}", n + 1));
        let id = doc.alloc_id();
        let color = Property::builtin(doc.alloc_id(), keys::SOLID_COLOR, Value::Color(self.color));
        let size_prop = Property::builtin(doc.alloc_id(), keys::SOLID_SIZE, Value::Vec2(size));
        let transform = Transform::new(&mut || doc.alloc_id(), size / 2.0, size / 2.0);
        let layer = new_layer(
            id,
            name,
            LayerKind::Solid {
                color,
                size: size_prop,
            },
            duration,
            transform,
        );
        insert_layer(doc, self.comp, self.index, layer)
    }
}

/// Registers an image file as an asset and adds a layer showing it,
/// centered in the composition.
#[derive(Debug, Clone)]
pub struct AddImageLayer {
    pub comp: CompId,
    pub path: String,
    pub pixel_size: [u32; 2],
    pub index: usize,
}

impl Command for AddImageLayer {
    fn label(&self) -> Cow<'static, str> {
        "Import Image".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let c = doc
            .comp(self.comp)
            .ok_or_else(|| CommandError::NotFound(self.comp.to_string()))?;
        let center = DVec2::new(f64::from(c.width), f64::from(c.height)) / 2.0;
        let duration = c.duration;
        let name = self
            .path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&self.path)
            .to_owned();
        let asset = doc.add_asset(Asset {
            id: 0.into(),
            name: name.clone(),
            kind: AssetKind::Image,
            path: self.path.clone(),
            pixel_size: Some(self.pixel_size),
        });
        let id = doc.alloc_id();
        let image_center =
            DVec2::new(f64::from(self.pixel_size[0]), f64::from(self.pixel_size[1])) / 2.0;
        let transform = Transform::new(&mut || doc.alloc_id(), image_center, center);
        let layer = new_layer(id, name, LayerKind::Image { asset }, duration, transform);
        insert_layer(doc, self.comp, self.index, layer)
    }
}

fn new_layer(
    id: LayerId,
    name: String,
    kind: LayerKind,
    duration: Time,
    transform: Transform,
) -> Layer {
    Layer {
        id,
        name,
        kind,
        in_point: Time::ZERO,
        out_point: duration,
        start_offset: Time::ZERO,
        parent: None,
        transform,
        blend_mode: BlendMode::Normal,
        flags: LayerFlags::default(),
        foreign: ForeignData::default(),
    }
}

/// Removes layers. Children of a removed layer are unparented.
#[derive(Debug, Clone)]
pub struct RemoveLayers {
    pub layers: Vec<LayerId>,
}

impl Command for RemoveLayers {
    fn label(&self) -> Cow<'static, str> {
        if self.layers.len() == 1 {
            "Delete Layer"
        } else {
            "Delete Layers"
        }
        .into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        for &id in &self.layers {
            let (comp, _) = doc
                .find_layer(id)
                .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
            let c = comp_mut(doc, comp)?;
            c.layers.retain(|l| l.id != id);
            let children: Vec<LayerId> = c
                .layers
                .iter()
                .filter(|l| l.parent == Some(id))
                .map(|l| l.id)
                .collect();
            for child in children {
                if let Some(l) = c.layer_mut(child) {
                    l.parent = None;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RenameLayer {
    pub layer: LayerId,
    pub name: String,
}

impl Command for RenameLayer {
    fn label(&self) -> Cow<'static, str> {
        "Rename Layer".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let l = doc
            .layer_mut(self.layer)
            .ok_or_else(|| CommandError::NotFound(self.layer.to_string()))?;
        l.name.clone_from(&self.name);
        Ok(())
    }

    fn merge_key(&self) -> Option<MergeKey> {
        Some(MergeKey {
            kind: "rename-layer",
            target: self.layer.raw(),
        })
    }
}

/// Moves a layer to `to_index` in its stack (0 = top).
#[derive(Debug, Clone)]
pub struct MoveLayer {
    pub layer: LayerId,
    pub to_index: usize,
}

impl Command for MoveLayer {
    fn label(&self) -> Cow<'static, str> {
        "Reorder Layer".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let (comp, _) = doc
            .find_layer(self.layer)
            .ok_or_else(|| CommandError::NotFound(self.layer.to_string()))?;
        let c = comp_mut(doc, comp)?;
        let from = c.layer_index(self.layer).expect("found above");
        let layer = c.layers.remove(from);
        let to = self.to_index.min(c.layers.len());
        c.layers.insert(to, layer);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SetLayerVisible {
    pub layer: LayerId,
    pub visible: bool,
}

impl Command for SetLayerVisible {
    fn label(&self) -> Cow<'static, str> {
        if self.visible {
            "Show Layer"
        } else {
            "Hide Layer"
        }
        .into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let l = doc
            .layer_mut(self.layer)
            .ok_or_else(|| CommandError::NotFound(self.layer.to_string()))?;
        l.flags.visible = self.visible;
        Ok(())
    }
}

fn prop_mut(doc: &mut Document, id: PropId) -> Result<&mut Property, CommandError> {
    doc.property_mut(id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))
}

/// Sets a property's value as a user edit at `time`: static properties
/// change their value, animated ones get a keyframe at `time`.
#[derive(Debug, Clone)]
pub struct SetProperty {
    pub prop: PropId,
    pub time: Time,
    pub value: Value,
}

impl Command for SetProperty {
    fn label(&self) -> Cow<'static, str> {
        "Change Property".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        prop_mut(doc, self.prop)?.set_value_at(self.time, self.value.clone())?;
        Ok(())
    }

    fn merge_key(&self) -> Option<MergeKey> {
        Some(MergeKey {
            kind: "set-property",
            target: self.prop.raw(),
        })
    }
}

/// Turns a property's animation on (one key at `time`) or off (static at the
/// value it has at `time`).
#[derive(Debug, Clone)]
pub struct SetAnimated {
    pub prop: PropId,
    pub time: Time,
    pub animated: bool,
}

impl Command for SetAnimated {
    fn label(&self) -> Cow<'static, str> {
        if self.animated {
            "Enable Keyframes"
        } else {
            "Disable Keyframes"
        }
        .into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        prop_mut(doc, self.prop)?.set_animated(self.time, self.animated)?;
        Ok(())
    }
}

/// Adds a keyframe holding the current value at `time`.
#[derive(Debug, Clone)]
pub struct AddKeyframe {
    pub prop: PropId,
    pub time: Time,
}

impl Command for AddKeyframe {
    fn label(&self) -> Cow<'static, str> {
        "Add Keyframe".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        prop_mut(doc, self.prop)?.add_keyframe(self.time)?;
        Ok(())
    }
}

/// Inserts fully specified keyframes (paste, import), replacing keys at the
/// same times.
#[derive(Debug, Clone)]
pub struct InsertKeyframes {
    pub prop: PropId,
    pub keys: Vec<Keyframe>,
}

impl Command for InsertKeyframes {
    fn label(&self) -> Cow<'static, str> {
        "Paste Keyframes".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let p = prop_mut(doc, self.prop)?;
        for k in &self.keys {
            p.insert_keyframe(k.clone())?;
        }
        Ok(())
    }
}

/// Removes keyframes; removing the last one makes the property static.
#[derive(Debug, Clone)]
pub struct RemoveKeyframes {
    pub prop: PropId,
    pub times: Vec<Time>,
}

impl Command for RemoveKeyframes {
    fn label(&self) -> Cow<'static, str> {
        if self.times.len() == 1 {
            "Delete Keyframe"
        } else {
            "Delete Keyframes"
        }
        .into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        prop_mut(doc, self.prop)?.remove_keyframes(&self.times)?;
        Ok(())
    }
}

/// Moves keyframes in time. Fails without changes if keys would collide.
///
/// Keys are identified by time, so a drag that issues several of these must
/// pass the keys' *current* times each step; consecutive steps merge into
/// one undo step.
#[derive(Debug, Clone)]
pub struct MoveKeyframes {
    pub prop: PropId,
    pub times: Vec<Time>,
    pub delta: Time,
}

impl Command for MoveKeyframes {
    fn label(&self) -> Cow<'static, str> {
        "Move Keyframes".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        prop_mut(doc, self.prop)?.shift_keyframes(&self.times, self.delta)?;
        Ok(())
    }

    fn merge_key(&self) -> Option<MergeKey> {
        Some(MergeKey {
            kind: "move-keyframes",
            target: self.prop.raw(),
        })
    }
}

/// Sets how the segments leaving the given keys are timed.
#[derive(Debug, Clone)]
pub struct SetInterpolation {
    pub prop: PropId,
    pub times: Vec<Time>,
    pub interp: Interp,
}

impl Command for SetInterpolation {
    fn label(&self) -> Cow<'static, str> {
        match self.interp {
            Interp::Hold => "Hold Keyframes",
            Interp::Linear => "Linear Keyframes",
            Interp::Bezier => "Bézier Keyframes",
        }
        .into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        prop_mut(doc, self.prop)?.set_interp(&self.times, self.interp)?;
        Ok(())
    }
}

/// Edits one key's ease handles (graph-editor handle drags).
#[derive(Debug, Clone)]
pub struct SetEase {
    pub prop: PropId,
    pub time: Time,
    pub ease_in: Option<EaseHandle>,
    pub ease_out: Option<EaseHandle>,
}

impl Command for SetEase {
    fn label(&self) -> Cow<'static, str> {
        "Edit Easing".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        prop_mut(doc, self.prop)?.set_ease(self.time, self.ease_in, self.ease_out)?;
        Ok(())
    }

    fn merge_key(&self) -> Option<MergeKey> {
        Some(MergeKey {
            kind: "set-ease",
            target: self.prop.raw(),
        })
    }
}

/// Applies an ease preset to the segments starting at the given keys.
#[derive(Debug, Clone)]
pub struct ApplyEasePreset {
    pub prop: PropId,
    pub times: Vec<Time>,
    pub preset: EasePreset,
}

impl Command for ApplyEasePreset {
    fn label(&self) -> Cow<'static, str> {
        format!("Apply {}", self.preset.label()).into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        prop_mut(doc, self.prop)?.apply_preset(&self.times, self.preset)?;
        Ok(())
    }
}

/// Sets or clears a key's motion-path tangents.
#[derive(Debug, Clone)]
pub struct SetSpatialTangents {
    pub prop: PropId,
    pub time: Time,
    pub tangents: Option<SpatialTangents>,
}

impl Command for SetSpatialTangents {
    fn label(&self) -> Cow<'static, str> {
        "Edit Motion Path".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        prop_mut(doc, self.prop)?.set_tangents(self.time, self.tangents)?;
        Ok(())
    }

    fn merge_key(&self) -> Option<MergeKey> {
        Some(MergeKey {
            kind: "set-tangents",
            target: self.prop.raw(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct SetWorkingSpace(pub WorkingSpace);

impl Command for SetWorkingSpace {
    fn label(&self) -> Cow<'static, str> {
        "Change Working Color Space".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        doc.meta.working_space = self.0;
        Ok(())
    }
}

/// Points an asset at a different file (relinking, or rebasing relative paths
/// when a project is saved elsewhere).
#[derive(Debug, Clone)]
pub struct SetAssetPath {
    pub asset: crate::AssetId,
    pub path: String,
}

impl Command for SetAssetPath {
    fn label(&self) -> Cow<'static, str> {
        "Relink Asset".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let assets = std::sync::Arc::make_mut(&mut doc.assets);
        let a = assets
            .0
            .get_mut(&self.asset)
            .ok_or_else(|| CommandError::NotFound(self.asset.to_string()))?;
        a.path.clone_from(&self.path);
        Ok(())
    }
}

/// Sets a layer's timing: where it starts and ends on the composition
/// timeline, and which source time it shows at its start. Moving a layer
/// shifts `in_point` and `out_point`; trimming the start also shifts
/// `start_offset`, so the source stays put under the cut.
#[derive(Debug, Clone)]
pub struct SetLayerTiming {
    pub layer: LayerId,
    pub in_point: Time,
    pub out_point: Time,
    pub start_offset: Time,
}

impl Command for SetLayerTiming {
    fn label(&self) -> Cow<'static, str> {
        "Edit Layer Timing".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        if self.out_point < self.in_point {
            return Err(CommandError::Invalid(
                "a layer cannot end before it starts".into(),
            ));
        }
        let l = doc
            .layer_mut(self.layer)
            .ok_or_else(|| CommandError::NotFound(self.layer.to_string()))?;
        l.in_point = self.in_point;
        l.out_point = self.out_point;
        l.start_offset = self.start_offset;
        Ok(())
    }

    fn merge_key(&self) -> Option<MergeKey> {
        Some(MergeKey {
            kind: "layer-timing",
            target: self.layer.raw(),
        })
    }
}
