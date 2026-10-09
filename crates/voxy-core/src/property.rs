//! Properties: typed, described, animatable values.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::color::LinearRgba;
use crate::easing::{EaseHandle, EasePreset};
use crate::id::PropId;
use crate::keyframe::{Interp, Keyframe, KeyframeTrack, SpatialTangents, TrackError};
use crate::time::Time;
use crate::value::{Value, ValueKind};

/// Display and editing unit of a property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    None,
    Pixels,
    Degrees,
    /// A multiplier stored as a factor (1.0) and shown as a percentage (100%).
    Factor,
    /// `0..=1` stored, shown as a percentage.
    Normalized,
    Seconds,
}

/// Static description of a property type, shared between all instances.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyDesc {
    /// Stable key, e.g. `"transform.position"` or, later, `"ofx:<plugin>/<param>"`.
    pub key: Arc<str>,
    pub label: Arc<str>,
    pub kind: ValueKind,
    pub default: Value,
    /// Hard range; scalar values are clamped into it, including interpolated
    /// values that overshoot.
    pub range: Option<(f64, f64)>,
    /// Suggested UI range for sliders.
    pub soft_range: Option<(f64, f64)>,
    pub unit: Unit,
    pub animatable: bool,
    /// Vector values move along motion paths with spatial tangents
    /// (position, anchor point).
    pub spatial: bool,
}

impl PropertyDesc {
    /// A descriptor for a key voXY does not know (for example one written by
    /// a newer version). The data is preserved as-is.
    fn unknown(key: &str, value: &Value, animated: bool) -> Self {
        PropertyDesc {
            key: key.into(),
            label: key.into(),
            kind: value.kind(),
            default: value.clone(),
            range: None,
            soft_range: None,
            unit: Unit::None,
            animatable: animated,
            spatial: false,
        }
    }
}

/// How a property's value varies over time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Animation {
    Static(Value),
    Keyframed(KeyframeTrack),
}

/// Errors from editing a property.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PropertyError {
    #[error("property `{key}` expects a {expected:?} value, got {got:?}")]
    TypeMismatch {
        key: Arc<str>,
        expected: ValueKind,
        got: ValueKind,
    },
    #[error("property `{key}` rejects non-finite values")]
    NonFinite { key: Arc<str> },
    #[error("property `{key}` cannot be animated")]
    NotAnimatable { key: Arc<str> },
    #[error("property `{key}` has no keyframes")]
    NotAnimated { key: Arc<str> },
    #[error("property `{key}` has no motion path")]
    NotSpatial { key: Arc<str> },
    #[error("property `{key}`: {source}")]
    Track {
        key: Arc<str>,
        #[source]
        source: TrackError,
    },
}

/// A property instance on a layer or composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PropertyRepr", into = "PropertyRepr")]
pub struct Property {
    pub id: PropId,
    pub desc: Arc<PropertyDesc>,
    anim: Animation,
}

impl Property {
    /// A new property with a validated static value.
    pub fn new(id: PropId, desc: Arc<PropertyDesc>, value: Value) -> Result<Self, PropertyError> {
        let value = validate(&desc, value)?;
        Ok(Property {
            id,
            desc,
            anim: Animation::Static(value),
        })
    }

    /// A new property from a built-in descriptor key.
    ///
    /// # Panics
    /// If `key` is not a built-in property or `value` does not fit it; both
    /// are programming errors.
    pub fn builtin(id: PropId, key: &str, value: Value) -> Self {
        let desc = builtin_desc(key).unwrap_or_else(|| panic!("unknown built-in property {key}"));
        Property::new(id, desc, value).unwrap_or_else(|e| panic!("{e}"))
    }

    pub fn key(&self) -> &str {
        &self.desc.key
    }

    pub fn animation(&self) -> &Animation {
        &self.anim
    }

    pub fn is_animated(&self) -> bool {
        matches!(self.anim, Animation::Keyframed(_))
    }

    pub fn track(&self) -> Option<&KeyframeTrack> {
        match &self.anim {
            Animation::Keyframed(t) => Some(t),
            Animation::Static(_) => None,
        }
    }

    /// The value at `time`: interpolated between keyframes, then clamped to
    /// the descriptor's hard range.
    pub fn value_at(&self, time: Time) -> Value {
        match &self.anim {
            Animation::Static(v) => v.clone(),
            Animation::Keyframed(t) => clamp(&self.desc, t.evaluate(time, self.desc.spatial)),
        }
    }

    /// Replaces the property with a static value (validated, scalars clamped).
    pub fn set_static(&mut self, value: Value) -> Result<(), PropertyError> {
        self.anim = Animation::Static(validate(&self.desc, value)?);
        Ok(())
    }

    /// Sets the value as a user edit at `time` would: a static property
    /// changes its value; an animated one gets a keyframe at `time` (an
    /// existing key there keeps its timing and tangents).
    pub fn set_value_at(&mut self, time: Time, value: Value) -> Result<(), PropertyError> {
        let value = validate(&self.desc, value)?;
        match &mut self.anim {
            Animation::Static(v) => *v = value,
            Animation::Keyframed(t) => t.set_value(time, value),
        }
        Ok(())
    }

    /// Turns animation on (one keyframe at `time` holding the current value)
    /// or off (static, keeping the value at `time`).
    pub fn set_animated(&mut self, time: Time, animated: bool) -> Result<(), PropertyError> {
        if animated == self.is_animated() {
            return Ok(());
        }
        if animated {
            if !self.desc.animatable {
                return Err(PropertyError::NotAnimatable {
                    key: self.desc.key.clone(),
                });
            }
            let value = self.value_at(time);
            self.anim = Animation::Keyframed(KeyframeTrack::single(Keyframe::new(time, value)));
        } else {
            self.anim = Animation::Static(self.value_at(time));
        }
        Ok(())
    }

    /// Adds a keyframe at `time` holding the current value there, turning
    /// animation on if needed. A no-op if a key already exists at `time`.
    pub fn add_keyframe(&mut self, time: Time) -> Result<(), PropertyError> {
        if !self.is_animated() {
            return self.set_animated(time, true);
        }
        let value = self.value_at(time);
        let track = self.track_mut()?;
        if track.key_at(time).is_none() {
            track.set_value(time, value);
        }
        Ok(())
    }

    /// Inserts a fully specified keyframe (paste, import), replacing any key
    /// at the same time and turning animation on if needed.
    pub fn insert_keyframe(&mut self, mut key: Keyframe) -> Result<(), PropertyError> {
        key.value = validate(&self.desc, key.value)?;
        if let Err(reason) = key.interp.validate() {
            return Err(PropertyError::Track {
                key: self.desc.key.clone(),
                source: TrackError::InvalidEase {
                    time: key.time,
                    reason,
                },
            });
        }
        if key.spatial.is_some_and(|s| !s.is_finite()) {
            return Err(PropertyError::NonFinite {
                key: self.desc.key.clone(),
            });
        }
        match &mut self.anim {
            Animation::Keyframed(t) => t.insert(key),
            Animation::Static(_) => {
                if !self.desc.animatable {
                    return Err(PropertyError::NotAnimatable {
                        key: self.desc.key.clone(),
                    });
                }
                self.anim = Animation::Keyframed(KeyframeTrack::single(key));
            }
        }
        Ok(())
    }

    /// Removes the keyframes at `times`. Removing every keyframe makes the
    /// property static with the value it had at the first removed time.
    pub fn remove_keyframes(&mut self, times: &[Time]) -> Result<(), PropertyError> {
        let key = self.desc.key.clone();
        let Some(&first) = times.first() else {
            return Ok(());
        };
        let fallback = self.value_at(first);
        match self.track_mut()?.remove(times) {
            Ok(()) => Ok(()),
            Err(TrackError::Empty) => {
                self.anim = Animation::Static(fallback);
                Ok(())
            }
            Err(source) => Err(PropertyError::Track { key, source }),
        }
    }

    /// Moves the keyframes at `times` by `delta`; fails without changes on a
    /// collision.
    pub fn shift_keyframes(&mut self, times: &[Time], delta: Time) -> Result<(), PropertyError> {
        let key = self.desc.key.clone();
        self.track_mut()?
            .shift(times, delta)
            .map_err(|source| PropertyError::Track { key, source })
    }

    pub fn set_interp(&mut self, times: &[Time], interp: Interp) -> Result<(), PropertyError> {
        if let Err(reason) = interp.validate() {
            return Err(PropertyError::Track {
                key: self.desc.key.clone(),
                source: TrackError::InvalidEase {
                    time: times.first().copied().unwrap_or(Time::ZERO),
                    reason,
                },
            });
        }
        self.edit_keys(times, |k| {
            k.interp = interp;
            Ok(())
        })
    }

    /// Sets a key's incoming and/or outgoing ease handle.
    pub fn set_ease(
        &mut self,
        time: Time,
        ease_in: Option<EaseHandle>,
        ease_out: Option<EaseHandle>,
    ) -> Result<(), PropertyError> {
        self.edit_keys(&[time], |k| {
            if let Some(h) = ease_in {
                k.ease_in = h;
            }
            if let Some(h) = ease_out {
                k.ease_out = h;
            }
            Ok(())
        })
    }

    /// Applies an ease preset to the segments starting at `times`.
    pub fn apply_preset(
        &mut self,
        times: &[Time],
        preset: EasePreset,
    ) -> Result<(), PropertyError> {
        let key = self.desc.key.clone();
        let track = self.track_mut()?;
        let mut next = track.clone();
        for &t in times {
            next.apply_preset(t, preset)
                .map_err(|source| PropertyError::Track {
                    key: key.clone(),
                    source,
                })?;
        }
        *track = next;
        Ok(())
    }

    /// Sets (or clears) a key's motion-path tangents. Only spatial vector
    /// properties have motion paths.
    pub fn set_tangents(
        &mut self,
        time: Time,
        tangents: Option<SpatialTangents>,
    ) -> Result<(), PropertyError> {
        let key = self.desc.key.clone();
        if !self.desc.spatial {
            return Err(PropertyError::NotSpatial { key });
        }
        if tangents.is_some_and(|t| !t.is_finite()) {
            return Err(PropertyError::NonFinite { key });
        }
        self.edit_keys(&[time], |k| {
            k.spatial = tangents;
            Ok(())
        })
    }

    /// Moves every keyframe to `time(old time)`. Keys that land on the same
    /// time merge, the later original winning. Returns how many merged
    /// away. Static properties are unchanged.
    pub fn retime_keys(&mut self, time: impl Fn(Time) -> Time) -> usize {
        let Animation::Keyframed(track) = &mut self.anim else {
            return 0;
        };
        let mut out: std::collections::BTreeMap<Time, Keyframe> = std::collections::BTreeMap::new();
        for k in track.keys() {
            let mut k = k.clone();
            k.time = time(k.time);
            out.insert(k.time, k);
        }
        let merged = track.len() - out.len();
        *track = KeyframeTrack::from_unsorted(out.into_values().collect())
            .expect("deduplicated, non-empty keys");
        merged
    }

    /// Replaces the static value or every keyframe value with `value(old)`,
    /// and every spatial tangent with `tangents(old)`. Atomic.
    pub fn map_values(
        &mut self,
        value: impl Fn(&Value) -> Value,
        tangents: impl Fn(SpatialTangents) -> SpatialTangents,
    ) -> Result<(), PropertyError> {
        match &self.anim {
            Animation::Static(v) => {
                let v = validate(&self.desc, value(v))?;
                self.anim = Animation::Static(v);
            }
            Animation::Keyframed(track) => {
                let mut keys = Vec::with_capacity(track.len());
                for k in track.keys() {
                    let mut k = k.clone();
                    k.value = validate(&self.desc, value(&k.value))?;
                    k.spatial = k.spatial.map(&tangents);
                    if k.spatial.is_some_and(|s| !s.is_finite()) {
                        return Err(PropertyError::NonFinite {
                            key: self.desc.key.clone(),
                        });
                    }
                    keys.push(k);
                }
                self.anim = Animation::Keyframed(
                    KeyframeTrack::try_from(keys).expect("times are unchanged"),
                );
            }
        }
        Ok(())
    }

    /// Replaces the animation with linear keys sampled from `sample` at
    /// `times` (strictly increasing, non-empty). Used to bake a nonlinear
    /// time mapping into keyframes.
    pub fn bake_samples(
        &mut self,
        times: &[Time],
        sample: impl Fn(Time) -> Value,
    ) -> Result<(), PropertyError> {
        let key = self.desc.key.clone();
        let mut keys = Vec::with_capacity(times.len());
        for &t in times {
            keys.push(Keyframe::new(t, validate(&self.desc, sample(t))?));
        }
        let track =
            KeyframeTrack::try_from(keys).map_err(|source| PropertyError::Track { key, source })?;
        self.anim = Animation::Keyframed(track);
        Ok(())
    }

    fn track_mut(&mut self) -> Result<&mut KeyframeTrack, PropertyError> {
        match &mut self.anim {
            Animation::Keyframed(t) => Ok(t),
            Animation::Static(_) => Err(PropertyError::NotAnimated {
                key: self.desc.key.clone(),
            }),
        }
    }

    /// Applies `f` to each key at `times`, atomically.
    fn edit_keys(
        &mut self,
        times: &[Time],
        mut f: impl FnMut(&mut Keyframe) -> Result<(), PropertyError>,
    ) -> Result<(), PropertyError> {
        let key = self.desc.key.clone();
        let track = self.track_mut()?;
        let mut next = track.clone();
        for &t in times {
            let k = next.key_at_mut(t).map_err(|source| PropertyError::Track {
                key: key.clone(),
                source,
            })?;
            f(k)?;
        }
        *track = next;
        Ok(())
    }
}

fn validate(desc: &PropertyDesc, value: Value) -> Result<Value, PropertyError> {
    check_kind(desc, &value)?;
    Ok(clamp(desc, value))
}

fn check_kind(desc: &PropertyDesc, value: &Value) -> Result<(), PropertyError> {
    if value.kind() != desc.kind {
        return Err(PropertyError::TypeMismatch {
            key: desc.key.clone(),
            expected: desc.kind,
            got: value.kind(),
        });
    }
    if !value.is_finite() {
        return Err(PropertyError::NonFinite {
            key: desc.key.clone(),
        });
    }
    Ok(())
}

fn clamp(desc: &PropertyDesc, value: Value) -> Value {
    match (value, desc.range) {
        (Value::Scalar(v), Some((lo, hi))) => Value::Scalar(v.clamp(lo, hi)),
        (Value::Int(v), Some((lo, hi))) => Value::Int(v.clamp(lo as i64, hi as i64)),
        (v, _) => v,
    }
}

/// On-disk form: the descriptor is referenced by key, not duplicated.
#[derive(Serialize, Deserialize)]
struct PropertyRepr {
    id: PropId,
    key: Arc<str>,
    #[serde(flatten)]
    anim: Animation,
}

impl TryFrom<PropertyRepr> for Property {
    type Error = PropertyError;
    fn try_from(r: PropertyRepr) -> Result<Self, PropertyError> {
        let (sample, animated) = match &r.anim {
            Animation::Static(v) => (v, false),
            Animation::Keyframed(t) => (&t.first().value, true),
        };
        let desc = builtin_desc(&r.key)
            .unwrap_or_else(|| Arc::new(PropertyDesc::unknown(&r.key, sample, animated)));
        // Load exactly what was saved: kinds and finiteness are checked, but
        // values are not re-clamped, so a load/save cycle never alters data.
        match &r.anim {
            Animation::Static(v) => check_kind(&desc, v)?,
            Animation::Keyframed(t) => {
                for k in t.keys() {
                    check_kind(&desc, &k.value)?;
                    if k.spatial.is_some_and(|s| !s.is_finite()) {
                        return Err(PropertyError::NonFinite {
                            key: desc.key.clone(),
                        });
                    }
                }
            }
        }
        Ok(Property {
            id: r.id,
            desc,
            anim: r.anim,
        })
    }
}

impl From<Property> for PropertyRepr {
    fn from(p: Property) -> Self {
        PropertyRepr {
            id: p.id,
            key: p.desc.key.clone(),
            anim: p.anim,
        }
    }
}

/// Keys of the built-in properties.
pub mod keys {
    pub const ANCHOR: &str = "transform.anchor";
    pub const POSITION: &str = "transform.position";
    pub const SCALE: &str = "transform.scale";
    pub const ROTATION: &str = "transform.rotation";
    pub const OPACITY: &str = "transform.opacity";
    pub const SOLID_COLOR: &str = "solid.color";
    pub const SOLID_SIZE: &str = "solid.size";
    pub const COMP_BACKGROUND: &str = "comp.background";
    pub const TIME_REMAP: &str = "precomp.time_remap";
}

/// Looks up a built-in property descriptor.
pub fn builtin_desc(key: &str) -> Option<Arc<PropertyDesc>> {
    BUILTINS.get(key).cloned()
}

static BUILTINS: LazyLock<HashMap<&'static str, Arc<PropertyDesc>>> = LazyLock::new(|| {
    let d = |key: &'static str,
             label: &'static str,
             default: Value,
             range: Option<(f64, f64)>,
             soft_range: Option<(f64, f64)>,
             unit: Unit| {
        (
            key,
            Arc::new(PropertyDesc {
                key: key.into(),
                label: label.into(),
                kind: default.kind(),
                default,
                range,
                soft_range,
                unit,
                animatable: true,
                spatial: key == keys::ANCHOR || key == keys::POSITION,
            }),
        )
    };
    HashMap::from([
        d(
            keys::ANCHOR,
            "Anchor Point",
            Value::Vec2(DVec2::ZERO),
            None,
            None,
            Unit::Pixels,
        ),
        d(
            keys::POSITION,
            "Position",
            Value::Vec2(DVec2::ZERO),
            None,
            None,
            Unit::Pixels,
        ),
        d(
            keys::SCALE,
            "Scale",
            Value::Vec2(DVec2::ONE),
            None,
            Some((0.0, 4.0)),
            Unit::Factor,
        ),
        d(
            keys::ROTATION,
            "Rotation",
            Value::Scalar(0.0),
            None,
            None,
            Unit::Degrees,
        ),
        d(
            keys::OPACITY,
            "Opacity",
            Value::Scalar(1.0),
            Some((0.0, 1.0)),
            None,
            Unit::Normalized,
        ),
        d(
            keys::SOLID_COLOR,
            "Color",
            Value::Color(LinearRgba::WHITE),
            None,
            None,
            Unit::None,
        ),
        d(
            keys::SOLID_SIZE,
            "Size",
            Value::Vec2(DVec2::new(100.0, 100.0)),
            None,
            Some((1.0, 8192.0)),
            Unit::Pixels,
        ),
        d(
            keys::TIME_REMAP,
            "Time Remap",
            Value::Scalar(0.0),
            None,
            None,
            Unit::Seconds,
        ),
        d(
            keys::COMP_BACKGROUND,
            "Background",
            Value::Color(LinearRgba::BLACK),
            None,
            None,
            Unit::None,
        ),
    ])
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_validates_and_clamps() {
        let mut p = Property::builtin(PropId(1), keys::OPACITY, Value::Scalar(0.5));
        p.set_static(Value::Scalar(2.0)).unwrap();
        assert_eq!(p.value_at(Time::ZERO), Value::Scalar(1.0));
        assert!(matches!(
            p.set_static(Value::Bool(true)),
            Err(PropertyError::TypeMismatch { .. })
        ));
        assert!(matches!(
            p.set_static(Value::Scalar(f64::NAN)),
            Err(PropertyError::NonFinite { .. })
        ));
    }

    #[test]
    fn serde_by_key() {
        let p = Property::builtin(PropId(7), keys::ROTATION, Value::Scalar(45.0));
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(
            json,
            r#"{"id":7,"key":"transform.rotation","static":{"type":"scalar","value":45.0}}"#
        );
        let back: Property = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert!(Arc::ptr_eq(
            &back.desc,
            &builtin_desc(keys::ROTATION).unwrap()
        ));
    }

    #[test]
    fn unknown_keys_are_preserved() {
        let json = r#"{"id":3,"key":"future.thing","static":{"type":"int","value":4}}"#;
        let p: Property = serde_json::from_str(json).unwrap();
        assert_eq!(p.key(), "future.thing");
        assert_eq!(serde_json::to_string(&p).unwrap(), json);
    }

    #[test]
    fn wrong_kind_on_load_is_an_error() {
        let json = r#"{"id":3,"key":"transform.rotation","static":{"type":"bool","value":true}}"#;
        assert!(serde_json::from_str::<Property>(json).is_err());
    }

    fn secs(s: i64) -> Time {
        Time::from_secs(s)
    }

    #[test]
    fn editing_an_animated_property_sets_keys() {
        let mut p = Property::builtin(PropId(1), keys::ROTATION, Value::Scalar(10.0));
        p.set_value_at(secs(5), Value::Scalar(20.0)).unwrap();
        assert!(!p.is_animated(), "static edits stay static");
        assert_eq!(p.value_at(secs(0)), Value::Scalar(20.0));

        p.set_animated(secs(0), true).unwrap();
        p.set_value_at(secs(2), Value::Scalar(40.0)).unwrap();
        assert_eq!(p.track().unwrap().len(), 2);
        assert_eq!(p.value_at(secs(1)), Value::Scalar(30.0));
        // Re-editing an existing key replaces its value only.
        p.set_interp(&[secs(0)], Interp::Hold).unwrap();
        p.set_value_at(secs(0), Value::Scalar(0.0)).unwrap();
        assert_eq!(p.track().unwrap().first().interp, Interp::Hold);
        assert_eq!(p.value_at(secs(1)), Value::Scalar(0.0));

        // Turning animation off keeps the value at that time.
        p.set_animated(secs(2), false).unwrap();
        assert_eq!(p.animation(), &Animation::Static(Value::Scalar(40.0)));
    }

    #[test]
    fn interpolated_values_are_clamped_to_range() {
        let mut p = Property::builtin(PropId(1), keys::OPACITY, Value::Scalar(0.0));
        p.set_animated(secs(0), true).unwrap();
        p.set_value_at(secs(1), Value::Scalar(1.0)).unwrap();
        p.apply_preset(&[secs(0)], EasePreset::BackOut).unwrap();
        for ms in 0..=1000 {
            let v = p.value_at(Time::from_millis(ms)).as_scalar().unwrap();
            assert!((0.0..=1.0).contains(&v), "{v}");
        }
    }

    #[test]
    fn removing_all_keys_makes_static() {
        let mut p = Property::builtin(PropId(1), keys::ROTATION, Value::Scalar(0.0));
        p.add_keyframe(secs(0)).unwrap();
        p.set_value_at(secs(1), Value::Scalar(90.0)).unwrap();
        p.remove_keyframes(&[secs(0)]).unwrap();
        assert!(p.is_animated());
        p.remove_keyframes(&[secs(1)]).unwrap();
        assert_eq!(p.animation(), &Animation::Static(Value::Scalar(90.0)));
        assert!(matches!(
            p.remove_keyframes(&[secs(1)]),
            Err(PropertyError::NotAnimated { .. })
        ));
    }

    #[test]
    fn failed_multi_key_edits_change_nothing() {
        let mut p = Property::builtin(PropId(1), keys::ROTATION, Value::Scalar(0.0));
        p.add_keyframe(secs(0)).unwrap();
        p.set_value_at(secs(1), Value::Scalar(1.0)).unwrap();
        let before = p.clone();
        assert!(p.set_interp(&[secs(0), secs(9)], Interp::Hold).is_err());
        assert!(
            p.apply_preset(&[secs(0), secs(9)], EasePreset::Ease)
                .is_err()
        );
        assert_eq!(p, before);
    }

    #[test]
    fn tangents_only_on_spatial_properties() {
        let t = SpatialTangents {
            incoming: glam::DVec3::ZERO,
            outgoing: glam::DVec3::X,
        };
        let mut rot = Property::builtin(PropId(1), keys::ROTATION, Value::Scalar(0.0));
        rot.add_keyframe(secs(0)).unwrap();
        assert!(matches!(
            rot.set_tangents(secs(0), Some(t)),
            Err(PropertyError::NotSpatial { .. })
        ));
        let mut pos = Property::builtin(PropId(2), keys::POSITION, Value::Vec2(DVec2::ZERO));
        pos.add_keyframe(secs(0)).unwrap();
        pos.set_tangents(secs(0), Some(t)).unwrap();
        assert_eq!(pos.track().unwrap().first().spatial, Some(t));
    }

    #[test]
    fn keyframed_serde_round_trip() {
        let mut p = Property::builtin(PropId(4), keys::POSITION, Value::Vec2(DVec2::ZERO));
        p.add_keyframe(secs(0)).unwrap();
        p.set_value_at(secs(1), Value::Vec2(DVec2::new(100.0, 50.0)))
            .unwrap();
        p.apply_preset(&[secs(0)], EasePreset::EaseOut).unwrap();
        p.set_tangents(
            secs(0),
            Some(SpatialTangents {
                incoming: glam::DVec3::ZERO,
                outgoing: glam::DVec3::new(30.0, -20.0, 0.0),
            }),
        )
        .unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains(r#""keyframed":[{"time":0,"#), "{json}");
        let back: Property = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn invalid_tracks_are_rejected_on_load() {
        let key =
            |t: i64, v: &str| format!(r#"{{"time":{t},"value":{{"type":"scalar","value":{v}}}}}"#);
        let prop =
            |keys: String| format!(r#"{{"id":1,"key":"transform.rotation","keyframed":[{keys}]}}"#);
        let ok = prop(format!("{},{}", key(0, "1.0"), key(5, "2.0")));
        let p: Property = serde_json::from_str(&ok).unwrap();
        // Omitted interp/ease fields take their defaults.
        assert_eq!(p.track().unwrap().first().interp, Interp::Linear);
        for bad in [
            prop(String::new()),
            prop(format!("{},{}", key(5, "1.0"), key(0, "2.0"))),
            prop(format!("{},{}", key(5, "1.0"), key(5, "2.0"))),
            prop(r#"{"time":0,"value":{"type":"bool","value":true}}"#.to_owned()),
            prop(
                r#"{"time":0,"value":{"type":"scalar","value":1.0},"ease_out":[1.5,0.0]}"#
                    .to_owned(),
            ),
        ] {
            assert!(serde_json::from_str::<Property>(&bad).is_err(), "{bad}");
        }
    }
}
