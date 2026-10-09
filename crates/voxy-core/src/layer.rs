//! Layers and their transform.

use std::collections::BTreeMap;

use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::id::{AssetId, LayerId, PropId};
use crate::property::{Property, keys};
use crate::time::Time;
use crate::value::Value;

/// Data voXY does not model, kept verbatim so that import → save never drops
/// it. Keys name the origin (for example `"alight_motion"` from Compat 2a).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ForeignData(pub BTreeMap<String, serde_json::Value>);

impl ForeignData {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The 2D transform every layer has.
///
/// Coordinates are composition pixels with the origin at the top-left and y
/// pointing down. Positive rotation is clockwise on screen. The layer-to-parent
/// matrix is `T(position) · R(rotation) · S(scale) · T(−anchor)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub anchor: Property,
    pub position: Property,
    pub scale: Property,
    pub rotation: Property,
    pub opacity: Property,
}

impl Transform {
    /// A transform placing the layer's `anchor` at `position`, unscaled,
    /// unrotated and opaque. `next_id` supplies fresh property IDs.
    pub fn new(next_id: &mut impl FnMut() -> PropId, anchor: DVec2, position: DVec2) -> Self {
        Transform {
            anchor: Property::builtin(next_id(), keys::ANCHOR, Value::Vec2(anchor)),
            position: Property::builtin(next_id(), keys::POSITION, Value::Vec2(position)),
            scale: Property::builtin(next_id(), keys::SCALE, Value::Vec2(DVec2::ONE)),
            rotation: Property::builtin(next_id(), keys::ROTATION, Value::Scalar(0.0)),
            opacity: Property::builtin(next_id(), keys::OPACITY, Value::Scalar(1.0)),
        }
    }

    pub fn properties(&self) -> [&Property; 5] {
        [
            &self.anchor,
            &self.position,
            &self.scale,
            &self.rotation,
            &self.opacity,
        ]
    }

    pub fn properties_mut(&mut self) -> [&mut Property; 5] {
        [
            &mut self.anchor,
            &mut self.position,
            &mut self.scale,
            &mut self.rotation,
            &mut self.opacity,
        ]
    }
}

/// What a layer draws.
///
/// Milestone 1 has solids and still images; the remaining layer types from the
/// design proposal arrive with their milestones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerKind {
    /// A rectangle of flat color, `size` pixels, with its origin top-left.
    Solid { color: Property, size: Property },
    /// A still image asset at its native pixel size.
    Image { asset: AssetId },
}

impl LayerKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            LayerKind::Solid { .. } => "Solid",
            LayerKind::Image { .. } => "Image",
        }
    }
}

/// How a layer combines with what is below it. More modes arrive in
/// Milestone 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LayerFlags {
    pub visible: bool,
    pub locked: bool,
}

impl Default for LayerFlags {
    fn default() -> Self {
        LayerFlags {
            visible: true,
            locked: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub kind: LayerKind,
    /// First composition time at which the layer is active.
    pub in_point: Time,
    /// First composition time at which the layer is no longer active.
    pub out_point: Time,
    /// Source time shown at `in_point` (used by trimming and splitting).
    #[serde(default)]
    pub start_offset: Time,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<LayerId>,
    pub transform: Transform,
    #[serde(default)]
    pub blend_mode: BlendMode,
    #[serde(default)]
    pub flags: LayerFlags,
    #[serde(default, skip_serializing_if = "ForeignData::is_empty")]
    pub foreign: ForeignData,
}

impl Layer {
    /// Whether the layer renders at composition time `t`.
    pub fn is_active_at(&self, t: Time) -> bool {
        self.flags.visible && self.in_point <= t && t < self.out_point
    }

    /// Every property of the layer, transform first.
    pub fn properties(&self) -> Vec<&Property> {
        let mut out: Vec<&Property> = self.transform.properties().to_vec();
        if let LayerKind::Solid { color, size } = &self.kind {
            out.push(color);
            out.push(size);
        }
        out
    }

    pub fn property_mut(&mut self, id: PropId) -> Option<&mut Property> {
        if let Some(p) = self
            .transform
            .properties_mut()
            .into_iter()
            .find(|p| p.id == id)
        {
            return Some(p);
        }
        match &mut self.kind {
            LayerKind::Solid { color, size } => [color, size].into_iter().find(|p| p.id == id),
            LayerKind::Image { .. } => None,
        }
    }
}
