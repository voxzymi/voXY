//! Layers and their transform.

use std::collections::BTreeMap;

use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::effect::EffectInstance;
use crate::id::{AssetId, CompId, EffectId, LayerId, PropId};
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

/// What a layer shows when its source is a project item: a media file or a
/// composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum SourceRef {
    Asset(AssetId),
    Comp(CompId),
}

/// What kind of source a placeholder layer stands in for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceholderKind {
    Image,
    Video,
    Comp,
}

/// What a placeholder layer remembers about its removed source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceholderInfo {
    pub kind: PlaceholderKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_size: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Time>,
}

fn one() -> f64 {
    1.0
}

/// What a layer draws.
///
/// Solids, still images, video, nested compositions and placeholders so
/// far; the remaining layer types from the design proposal arrive with their
/// milestones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerKind {
    /// A rectangle of flat color, `size` pixels, with its origin top-left.
    Solid { color: Property, size: Property },
    /// A still image asset at its native pixel size.
    Image { asset: AssetId },
    /// A video asset at its native pixel size, showing source time
    /// [`Layer::source_time`].
    Video { asset: AssetId },
    /// Another composition (a precomp), showing child time
    /// [`Layer::child_time`] (format 0.4.0).
    Precomp {
        comp: CompId,
        /// Playback rate: 1.0 is normal, 2.0 twice as fast. Always > 0.
        #[serde(default = "one")]
        speed: f64,
        /// Repeat the child composition after its end.
        #[serde(default)]
        looping: bool,
        /// Child time in seconds as a function of composition time
        /// (`precomp.time_remap`). Overrides `speed` and `start_offset`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_remap: Option<Property>,
        /// Collapse transformations (2D): render the child's layers directly
        /// into this composition instead of through an offscreen canvas.
        #[serde(default)]
        collapse: bool,
    },
    /// Stands in for a source that was removed from the project (format
    /// 0.4.0). Draws a neutral placeholder at the remembered size.
    Placeholder { was: PlaceholderInfo },
}

impl LayerKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            LayerKind::Solid { .. } => "Solid",
            LayerKind::Image { .. } => "Image",
            LayerKind::Video { .. } => "Video",
            LayerKind::Precomp { .. } => "Precomp",
            LayerKind::Placeholder { .. } => "Placeholder",
        }
    }

    /// The asset this layer draws, if any.
    pub fn asset(&self) -> Option<AssetId> {
        match self {
            LayerKind::Image { asset } | LayerKind::Video { asset } => Some(*asset),
            _ => None,
        }
    }

    /// The project item this layer shows, if any.
    pub fn source(&self) -> Option<SourceRef> {
        match self {
            LayerKind::Image { asset } | LayerKind::Video { asset } => {
                Some(SourceRef::Asset(*asset))
            }
            LayerKind::Precomp { comp, .. } => Some(SourceRef::Comp(*comp)),
            LayerKind::Solid { .. } | LayerKind::Placeholder { .. } => None,
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
    /// The effect stack: index 0 is applied first (format 0.7.0).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectInstance>,
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

    /// The source (media) time shown at composition time `t`: the layer
    /// starts showing its source at `start_offset` when it comes in.
    pub fn source_time(&self, t: Time) -> Time {
        t - self.in_point + self.start_offset
    }

    /// The child composition time a precomp layer shows at composition
    /// time `t` (design note §7.2), or `None` for other layer kinds.
    ///
    /// - With time remap, the remap value (seconds), rounded to a flick.
    /// - Otherwise `start_offset + (t − in_point) × speed`; speed 1 is exact
    ///   integer arithmetic.
    /// - Looping wraps the result into `[0, child_duration)`.
    ///
    /// The result is not clamped: times outside the child show nothing.
    pub fn child_time(&self, t: Time, child_duration: Time) -> Option<Time> {
        let LayerKind::Precomp {
            speed,
            looping,
            time_remap,
            ..
        } = &self.kind
        else {
            return None;
        };
        let c = if let Some(remap) = time_remap {
            Time::from_secs_f64(remap.value_at(t).as_scalar().unwrap_or(0.0))
        } else if *speed == 1.0 {
            self.start_offset + (t - self.in_point)
        } else {
            let local = (t - self.in_point).flicks() as f64 * speed;
            self.start_offset + Time::from_flicks(local.round() as i64)
        };
        Some(if *looping && child_duration > Time::ZERO {
            Time::from_flicks(c.flicks().rem_euclid(child_duration.flicks()))
        } else {
            c
        })
    }

    /// Every editable property of the layer: transform first, then the
    /// layer kind's, then each effect's parameters in stack order.
    /// Placeholder effects' parameters are not included (they are
    /// read-only).
    pub fn properties(&self) -> Vec<&Property> {
        let mut out: Vec<&Property> = self.transform.properties().to_vec();
        match &self.kind {
            LayerKind::Solid { color, size } => {
                out.push(color);
                out.push(size);
            }
            LayerKind::Precomp {
                time_remap: Some(remap),
                ..
            } => out.push(remap),
            _ => {}
        }
        for fx in &self.effects {
            out.extend(fx.editable_params());
        }
        out
    }

    /// [`Layer::properties`], mutably.
    pub fn properties_mut(&mut self) -> Vec<&mut Property> {
        let mut out: Vec<&mut Property> = self.transform.properties_mut().into_iter().collect();
        match &mut self.kind {
            LayerKind::Solid { color, size } => {
                out.push(color);
                out.push(size);
            }
            LayerKind::Precomp {
                time_remap: Some(remap),
                ..
            } => out.push(remap),
            _ => {}
        }
        for fx in &mut self.effects {
            out.extend(fx.editable_params_mut());
        }
        out
    }

    pub fn effect(&self, id: EffectId) -> Option<&EffectInstance> {
        self.effects.iter().find(|e| e.id == id)
    }

    pub fn effect_mut(&mut self, id: EffectId) -> Option<&mut EffectInstance> {
        self.effects.iter_mut().find(|e| e.id == id)
    }

    pub fn property_mut(&mut self, id: PropId) -> Option<&mut Property> {
        self.properties_mut().into_iter().find(|p| p.id == id)
    }
}
