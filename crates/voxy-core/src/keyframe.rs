//! Keyframes and their interpolation.
//!
//! A [`KeyframeTrack`] is a non-empty list of keyframes with strictly
//! increasing [`Time`]s (integer flicks), all holding values of one kind. The
//! segment from key `k0` to the next key `k1` interpolates as follows:
//!
//! 1. **Timing.** Normalized time `u = (t − t0) / (t1 − t0)` is computed from
//!    the integer times. `k0.interp` picks the progress curve:
//!    - [`Interp::Hold`] keeps `k0`'s value until `t1`;
//!    - [`Interp::Linear`] uses `p = u`;
//!    - [`Interp::Bezier`] uses the [`CubicEase`] with `P1 = k0.ease_out` and
//!      `P2 = k1.ease_in`;
//!    - [`Interp::Spring`], [`Interp::Bounce`], [`Interp::Oscillation`],
//!      [`Interp::Steps`] and [`Interp::SteppedRamp`] use the analytic
//!      functions of [`crate::shapes`], which carry their parameters and
//!      ignore the keys' ease handles. They may overshoot `[0, 1]` (spring,
//!      oscillation) or jump (steps).
//! 2. **Value.** Numbers, vectors and colors move from `v0` to `v1` by `p`.
//!    For *spatial* properties (position, anchor point) with tangents, the
//!    value travels along the cubic motion path and `p` is the fraction of the
//!    path's arc length, so speed along the path follows the ease curve.
//!
//! Before the first key the value is the first key's; from the last key on it
//! is the last key's. Evaluating exactly at a key's time returns that key's
//! value bit-for-bit.

use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::color::LinearRgba;
use crate::easing::{CubicEase, EaseHandle, EasePreset};
use crate::motion_path::CubicPath;
use crate::shapes::{
    BounceParams, Jump, OscillationParams, SpringParams, SteppedRampParams, StepsParams,
};
use crate::time::Time;
use crate::value::Value;

/// How the segment leaving a keyframe is timed.
///
/// The parametric variants carry their parameters. They are validated
/// whenever they are read (deserializing an `Interp` rejects out-of-range
/// values), when a track is built ([`TrackError::InvalidEase`]) and when an
/// edit is applied.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", remote = "Self")]
pub enum Interp {
    Hold,
    #[default]
    Linear,
    Bezier,
    Spring(SpringParams),
    Bounce(BounceParams),
    Oscillation(OscillationParams),
    Steps(StepsParams),
    SteppedRamp(SteppedRampParams),
}

impl Serialize for Interp {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Self::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for Interp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let interp = Self::deserialize(deserializer)?;
        interp.validate().map_err(D::Error::custom)?;
        Ok(interp)
    }
}

impl Interp {
    /// The five parametric types with default parameters, in menu order.
    pub fn parametric_defaults() -> [Interp; 5] {
        [
            Interp::Spring(SpringParams::default()),
            Interp::Bounce(BounceParams::default()),
            Interp::Oscillation(OscillationParams::default()),
            Interp::Steps(StepsParams::default()),
            Interp::SteppedRamp(SteppedRampParams::default()),
        ]
    }

    /// Display name, also used in undo labels.
    pub fn label(&self) -> &'static str {
        match self {
            Interp::Hold => "Hold",
            Interp::Linear => "Linear",
            Interp::Bezier => "Bézier",
            Interp::Spring(_) => "Spring",
            Interp::Bounce(_) => "Bounce",
            Interp::Oscillation(_) => "Oscillation",
            Interp::Steps(_) => "Steps",
            Interp::SteppedRamp(_) => "Stepped Ramp",
        }
    }

    /// `true` for the five analytic easings that carry parameters.
    pub fn is_parametric(&self) -> bool {
        !matches!(self, Interp::Hold | Interp::Linear | Interp::Bezier)
    }

    /// Checks parameter ranges. Always `Ok` for the legacy modes.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Interp::Hold | Interp::Linear | Interp::Bezier => Ok(()),
            Interp::Spring(p) => p.validate(),
            Interp::Bounce(p) => p.validate(),
            Interp::Oscillation(p) => p.validate(),
            Interp::Steps(p) => p.validate(),
            Interp::SteppedRamp(p) => p.validate(),
        }
    }

    /// The same easing with every parameter forced into its documented
    /// range (non-finite values become the default).
    #[must_use]
    pub fn sanitized(self) -> Self {
        match self {
            Interp::Hold | Interp::Linear | Interp::Bezier => self,
            Interp::Spring(p) => Interp::Spring(p.sanitized()),
            Interp::Bounce(p) => Interp::Bounce(p.sanitized()),
            Interp::Oscillation(p) => Interp::Oscillation(p.sanitized()),
            Interp::Steps(p) => Interp::Steps(p.sanitized()),
            Interp::SteppedRamp(p) => Interp::SteppedRamp(p.sanitized()),
        }
    }

    /// Progress of a parametric easing at `u`; `None` for the legacy modes,
    /// whose curves depend on the keys.
    pub fn shape_progress(&self, u: f64) -> Option<f64> {
        Some(match self {
            Interp::Hold | Interp::Linear | Interp::Bezier => return None,
            Interp::Spring(p) => p.progress(u),
            Interp::Bounce(p) => p.progress(u),
            Interp::Oscillation(p) => p.progress(u),
            Interp::Steps(p) => p.progress(u),
            Interp::SteppedRamp(p) => p.progress(u),
        })
    }

    /// d(progress)/du of a parametric easing where it is defined (flat
    /// plateaus report their slope; jumps are in [`Self::shape_jumps`]).
    pub fn shape_slope(&self, u: f64) -> Option<f64> {
        Some(match self {
            Interp::Hold | Interp::Linear | Interp::Bezier => return None,
            Interp::Spring(p) => p.slope(u),
            Interp::Bounce(p) => p.slope(u),
            Interp::Oscillation(p) => p.slope(u),
            Interp::Steps(p) => p.slope(u),
            Interp::SteppedRamp(p) => p.slope(u),
        })
    }

    /// Discontinuities of a parametric easing, ascending. Empty for
    /// everything but steps and stepped ramps.
    pub fn shape_jumps(&self) -> Vec<Jump> {
        match self {
            Interp::Steps(p) => p.jumps(),
            Interp::SteppedRamp(p) => p.jumps(),
            _ => Vec::new(),
        }
    }
}

/// Motion-path tangents of a spatial keyframe, relative to its value.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct SpatialTangents {
    /// Handle of the path arriving at this key.
    #[serde(rename = "in")]
    pub incoming: DVec3,
    /// Handle of the path leaving this key.
    #[serde(rename = "out")]
    pub outgoing: DVec3,
}

impl SpatialTangents {
    pub fn is_finite(&self) -> bool {
        self.incoming.is_finite() && self.outgoing.is_finite()
    }
}

/// One keyframe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub time: Time,
    pub value: Value,
    /// Timing of the segment that starts at this key.
    #[serde(default)]
    pub interp: Interp,
    /// `P2` of the incoming segment's ease curve (used when the previous key
    /// is [`Interp::Bezier`]).
    #[serde(default = "linear_in")]
    pub ease_in: EaseHandle,
    /// `P1` of the outgoing segment's ease curve (used when this key is
    /// [`Interp::Bezier`]).
    #[serde(default = "linear_out")]
    pub ease_out: EaseHandle,
    /// Motion-path tangents; only meaningful on spatial properties.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial: Option<SpatialTangents>,
}

fn linear_in() -> EaseHandle {
    EaseHandle::LINEAR_IN
}

fn linear_out() -> EaseHandle {
    EaseHandle::LINEAR_OUT
}

impl Keyframe {
    /// A linear keyframe with neutral ease handles and no tangents.
    pub fn new(time: Time, value: Value) -> Self {
        Keyframe {
            time,
            value,
            interp: Interp::Linear,
            ease_in: EaseHandle::LINEAR_IN,
            ease_out: EaseHandle::LINEAR_OUT,
            spatial: None,
        }
    }
}

/// Structural problems with a track.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrackError {
    #[error("a keyframe track needs at least one keyframe")]
    Empty,
    #[error("two keyframes at {0}")]
    DuplicateTime(Time),
    #[error("keyframes are not in time order at {0}")]
    Unordered(Time),
    #[error("no keyframe at {0}")]
    NoKeyframe(Time),
    #[error("keyframes would collide at {0}")]
    Collision(Time),
    #[error("invalid easing parameters at {time}: {reason}")]
    InvalidEase { time: Time, reason: String },
}

/// A property's keyframes. See the module documentation for the
/// interpolation rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<Keyframe>", into = "Vec<Keyframe>")]
pub struct KeyframeTrack {
    keys: Vec<Keyframe>,
}

impl TryFrom<Vec<Keyframe>> for KeyframeTrack {
    type Error = TrackError;
    /// Requires keys already in strictly increasing time order; files are
    /// never silently reordered.
    fn try_from(keys: Vec<Keyframe>) -> Result<Self, TrackError> {
        if keys.is_empty() {
            return Err(TrackError::Empty);
        }
        for w in keys.windows(2) {
            if w[0].time == w[1].time {
                return Err(TrackError::DuplicateTime(w[1].time));
            }
            if w[0].time > w[1].time {
                return Err(TrackError::Unordered(w[1].time));
            }
        }
        for k in &keys {
            k.interp
                .validate()
                .map_err(|reason| TrackError::InvalidEase {
                    time: k.time,
                    reason,
                })?;
        }
        Ok(KeyframeTrack { keys })
    }
}

impl From<KeyframeTrack> for Vec<Keyframe> {
    fn from(t: KeyframeTrack) -> Self {
        t.keys
    }
}

impl KeyframeTrack {
    /// A track with a single key.
    pub fn single(key: Keyframe) -> Self {
        KeyframeTrack { keys: vec![key] }
    }

    /// Builds a track from keys in any order. Duplicate times are an error.
    pub fn from_unsorted(mut keys: Vec<Keyframe>) -> Result<Self, TrackError> {
        keys.sort_by_key(|k| k.time);
        Self::try_from(keys)
    }

    pub fn keys(&self) -> &[Keyframe] {
        &self.keys
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Always `false`: tracks are never empty. Provided for API symmetry.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn first(&self) -> &Keyframe {
        &self.keys[0]
    }

    pub fn last(&self) -> &Keyframe {
        &self.keys[self.keys.len() - 1]
    }

    /// `Ok(index)` of the key at `time`, or `Err(insertion index)`.
    pub fn search(&self, time: Time) -> Result<usize, usize> {
        self.keys.binary_search_by_key(&time, |k| k.time)
    }

    pub fn key_at(&self, time: Time) -> Option<&Keyframe> {
        self.search(time).ok().map(|i| &self.keys[i])
    }

    /// The keys strictly before and after `time` (for "previous/next
    /// keyframe" navigation).
    pub fn neighbors(&self, time: Time) -> (Option<&Keyframe>, Option<&Keyframe>) {
        let before = self.keys.partition_point(|k| k.time < time);
        let after = self.keys.partition_point(|k| k.time <= time);
        (
            before.checked_sub(1).map(|i| &self.keys[i]),
            self.keys.get(after),
        )
    }

    /// Mutable access to the key at `time`. Callers must not change its
    /// time or value kind; property-level methods enforce that.
    pub(crate) fn key_at_mut(&mut self, time: Time) -> Result<&mut Keyframe, TrackError> {
        match self.search(time) {
            Ok(i) => Ok(&mut self.keys[i]),
            Err(_) => Err(TrackError::NoKeyframe(time)),
        }
    }

    /// Sets the value at `time`, replacing an existing key's value (keeping
    /// its timing and tangents) or inserting a new linear key.
    pub(crate) fn set_value(&mut self, time: Time, value: Value) {
        match self.search(time) {
            Ok(i) => self.keys[i].value = value,
            Err(i) => self.keys.insert(i, Keyframe::new(time, value)),
        }
    }

    /// Inserts a fully specified key, replacing any key at the same time.
    pub(crate) fn insert(&mut self, key: Keyframe) {
        match self.search(key.time) {
            Ok(i) => self.keys[i] = key,
            Err(i) => self.keys.insert(i, key),
        }
    }

    /// Removes the keys at `times`. Removing every key is refused (a track is
    /// never empty); the caller converts the property to static instead.
    pub(crate) fn remove(&mut self, times: &[Time]) -> Result<(), TrackError> {
        for &t in times {
            if self.search(t).is_err() {
                return Err(TrackError::NoKeyframe(t));
            }
        }
        let remaining = self
            .keys
            .iter()
            .filter(|k| !times.contains(&k.time))
            .count();
        if remaining == 0 {
            return Err(TrackError::Empty);
        }
        self.keys.retain(|k| !times.contains(&k.time));
        Ok(())
    }

    /// Shifts the keys at `times` by `delta`. Fails without changing anything
    /// if a moved key would land on a key that is not moving (or on another
    /// moved key).
    pub(crate) fn shift(&mut self, times: &[Time], delta: Time) -> Result<(), TrackError> {
        for &t in times {
            if self.search(t).is_err() {
                return Err(TrackError::NoKeyframe(t));
            }
        }
        let mut keys = self.keys.clone();
        for k in &mut keys {
            if times.contains(&k.time) {
                k.time = Time(
                    k.time
                        .0
                        .checked_add(delta.0)
                        .ok_or(TrackError::Collision(k.time))?,
                );
            }
        }
        keys.sort_by_key(|k| k.time);
        if let Some(w) = keys.windows(2).find(|w| w[0].time == w[1].time) {
            return Err(TrackError::Collision(w[0].time));
        }
        self.keys = keys;
        Ok(())
    }

    /// Applies a preset to the segment that starts at the key at `time`: that
    /// key becomes Bézier with the preset's `P1`, and the next key gets the
    /// preset's `P2`. On the last key only the outgoing side is set.
    pub(crate) fn apply_preset(
        &mut self,
        time: Time,
        preset: EasePreset,
    ) -> Result<(), TrackError> {
        let i = self
            .search(time)
            .map_err(|_| TrackError::NoKeyframe(time))?;
        let curve = preset.curve();
        self.keys[i].interp = if preset == EasePreset::Linear {
            Interp::Linear
        } else {
            Interp::Bezier
        };
        self.keys[i].ease_out = curve.p1;
        if let Some(next) = self.keys.get_mut(i + 1) {
            next.ease_in = curve.p2;
        }
        Ok(())
    }

    /// The segment containing `time`: index of its left key and normalized
    /// time within it. `None` before the first or from the last key on.
    pub fn segment_at(&self, time: Time) -> Option<(usize, f64)> {
        let i = self.keys.partition_point(|k| k.time <= time);
        if i == 0 || i == self.keys.len() {
            return None;
        }
        let (t0, t1) = (self.keys[i - 1].time.0, self.keys[i].time.0);
        // i128 keeps the subtraction exact for any pair of i64 times.
        let u =
            (i128::from(time.0) - i128::from(t0)) as f64 / (i128::from(t1) - i128::from(t0)) as f64;
        Some((i - 1, u))
    }

    /// Timing progress through segment `index` at normalized time `u`
    /// (`None` for hold segments).
    pub fn segment_progress(&self, index: usize, u: f64) -> Option<f64> {
        let k0 = &self.keys[index];
        match k0.interp {
            Interp::Hold => None,
            Interp::Linear => Some(u),
            Interp::Spring(_)
            | Interp::Bounce(_)
            | Interp::Oscillation(_)
            | Interp::Steps(_)
            | Interp::SteppedRamp(_) => k0.interp.shape_progress(u),
            Interp::Bezier => {
                let k1 = &self.keys[index + 1];
                Some(
                    CubicEase {
                        p1: k0.ease_out,
                        p2: k1.ease_in,
                    }
                    .progress(u),
                )
            }
        }
    }

    /// d(progress)/du of segment `index` at `u`: the speed curve the graph
    /// editor draws. `None` for hold segments. At a step jump the speed is
    /// infinite and not reported here; see [`Self::segment_jumps`].
    pub fn segment_slope(&self, index: usize, u: f64) -> Option<f64> {
        let k0 = &self.keys[index];
        match k0.interp {
            Interp::Hold => None,
            Interp::Linear => Some(1.0),
            Interp::Bezier => {
                let k1 = &self.keys[index + 1];
                Some(
                    CubicEase {
                        p1: k0.ease_out,
                        p2: k1.ease_in,
                    }
                    .slope(u),
                )
            }
            _ => k0.interp.shape_slope(u),
        }
    }

    /// Discontinuities of segment `index`'s progress (step jumps). A hold
    /// segment's single jump to the next key is not listed.
    pub fn segment_jumps(&self, index: usize) -> Vec<Jump> {
        self.keys[index].interp.shape_jumps()
    }

    /// The motion path of segment `index` if the keys carry tangents.
    pub fn segment_path(&self, index: usize) -> Option<CubicPath> {
        let (k0, k1) = (&self.keys[index], &self.keys[index + 1]);
        let out = k0.spatial.map_or(DVec3::ZERO, |s| s.outgoing);
        let inc = k1.spatial.map_or(DVec3::ZERO, |s| s.incoming);
        let (v0, v1) = (as_point(&k0.value)?, as_point(&k1.value)?);
        Some(CubicPath::new(v0, out, inc, v1))
    }

    /// The value at `time`. `spatial` enables motion-path interpolation for
    /// vector values.
    pub fn evaluate(&self, time: Time, spatial: bool) -> Value {
        let Some((i, u)) = self.segment_at(time) else {
            return if time < self.first().time {
                self.first()
            } else {
                self.last()
            }
            .value
            .clone();
        };
        let (k0, k1) = (&self.keys[i], &self.keys[i + 1]);
        let Some(p) = self.segment_progress(i, u) else {
            return k0.value.clone();
        };
        if spatial
            && let Some(path) = self.segment_path(i)
            && !path.is_straight()
        {
            return from_point(&k0.value, path.point_at_length_fraction(p));
        }
        interpolate(&k0.value, &k1.value, p)
    }
}

fn as_point(v: &Value) -> Option<DVec3> {
    match *v {
        Value::Vec2(v) => Some(v.extend(0.0)),
        Value::Vec3(v) => Some(v),
        _ => None,
    }
}

fn from_point(like: &Value, p: DVec3) -> Value {
    match like {
        Value::Vec2(_) => Value::Vec2(p.truncate()),
        _ => Value::Vec3(p),
    }
}

/// `a·(1−p) + b·p`: exactly `a` at `p = 0` and exactly `b` at `p = 1`.
fn mix(a: f64, b: f64, p: f64) -> f64 {
    a * (1.0 - p) + b * p
}

/// Interpolates two values of the same kind by progress `p` (which may
/// leave `[0, 1]` for overshooting eases).
///
/// - Scalars, vectors: component-wise.
/// - Colors: component-wise in linear light, straight alpha.
/// - Integers: interpolated, then rounded half away from zero.
/// - Booleans, enums, strings: hold the first value until the next key.
pub fn interpolate(from: &Value, to: &Value, progress: f64) -> Value {
    let t = progress;
    match (from, to) {
        (Value::Scalar(lhs), Value::Scalar(rhs)) => Value::Scalar(mix(*lhs, *rhs, t)),
        (Value::Vec2(lhs), Value::Vec2(rhs)) => {
            Value::Vec2(DVec2::new(mix(lhs.x, rhs.x, t), mix(lhs.y, rhs.y, t)))
        }
        (Value::Vec3(lhs), Value::Vec3(rhs)) => Value::Vec3(DVec3::new(
            mix(lhs.x, rhs.x, t),
            mix(lhs.y, rhs.y, t),
            mix(lhs.z, rhs.z, t),
        )),
        (Value::Color(lhs), Value::Color(rhs)) => {
            let channel = |c0: f32, c1: f32| mix(f64::from(c0), f64::from(c1), t) as f32;
            Value::Color(LinearRgba::new(
                channel(lhs.r, rhs.r),
                channel(lhs.g, rhs.g),
                channel(lhs.b, rhs.b),
                channel(lhs.a, rhs.a),
            ))
        }
        (Value::Int(lhs), Value::Int(rhs)) => {
            let v = mix(*lhs as f64, *rhs as f64, t).round();
            // `as` saturates out-of-range floats, which is the clamp we want.
            Value::Int(v as i64)
        }
        _ => from.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: f64) -> Value {
        Value::Scalar(v)
    }

    fn track(keys: &[(i64, f64)]) -> KeyframeTrack {
        KeyframeTrack::from_unsorted(
            keys.iter()
                .map(|&(t, v)| Keyframe::new(Time::from_secs(t), s(v)))
                .collect(),
        )
        .unwrap()
    }

    fn at(track: &KeyframeTrack, secs: f64) -> f64 {
        track
            .evaluate(Time::from_secs_f64(secs), false)
            .as_scalar()
            .unwrap()
    }

    #[test]
    fn linear_and_clamped_ends() {
        let t = track(&[(1, 10.0), (3, 30.0)]);
        assert_eq!(at(&t, 0.0), 10.0);
        assert_eq!(at(&t, 1.0), 10.0);
        assert_eq!(at(&t, 2.0), 20.0);
        assert_eq!(at(&t, 3.0), 30.0);
        assert_eq!(at(&t, 99.0), 30.0);
    }

    #[test]
    fn hold_keeps_value_until_next_key() {
        let mut t = track(&[(0, 1.0), (2, 5.0)]);
        t.key_at_mut(Time::ZERO).unwrap().interp = Interp::Hold;
        assert_eq!(at(&t, 1.999), 1.0);
        assert_eq!(at(&t, 2.0), 5.0);
    }

    #[test]
    fn bezier_uses_handles_from_both_keys() {
        let mut t = track(&[(0, 0.0), (1, 100.0)]);
        t.apply_preset(Time::ZERO, EasePreset::EaseInOut).unwrap();
        let curve = EasePreset::EaseInOut.curve();
        for i in 0..=10 {
            let u = f64::from(i) / 10.0;
            let expected = 100.0 * curve.progress(u);
            assert!((at(&t, u) - expected).abs() < 1e-9);
        }
        assert_eq!(t.keys()[1].ease_in, curve.p2);
    }

    #[test]
    fn exact_at_keys() {
        let t = track(&[(0, 0.1), (1, 0.7), (2, -3.3)]);
        for k in t.keys() {
            assert_eq!(t.evaluate(k.time, false), k.value);
        }
    }

    #[test]
    fn value_kinds() {
        let mid = 0.5;
        assert_eq!(
            interpolate(&Value::Int(0), &Value::Int(3), mid),
            Value::Int(2)
        );
        assert_eq!(
            interpolate(&Value::Int(0), &Value::Int(-3), mid),
            Value::Int(-2)
        );
        assert_eq!(
            interpolate(&Value::Bool(true), &Value::Bool(false), 0.99),
            Value::Bool(true)
        );
        assert_eq!(
            interpolate(
                &Value::Color(LinearRgba::BLACK),
                &Value::Color(LinearRgba::WHITE),
                mid
            ),
            Value::Color(LinearRgba::new(0.5, 0.5, 0.5, 1.0))
        );
    }

    #[test]
    fn spatial_path_with_tangents_curves() {
        let mut a = Keyframe::new(Time::ZERO, Value::Vec2(DVec2::ZERO));
        a.spatial = Some(SpatialTangents {
            incoming: DVec3::ZERO,
            outgoing: DVec3::new(0.0, 100.0, 0.0),
        });
        let mut b = Keyframe::new(Time::from_secs(1), Value::Vec2(DVec2::new(100.0, 0.0)));
        b.spatial = Some(SpatialTangents {
            incoming: DVec3::new(0.0, 100.0, 0.0),
            outgoing: DVec3::ZERO,
        });
        let t = KeyframeTrack::try_from(vec![a, b]).unwrap();
        let mid = t.evaluate(Time::from_millis(500), true).as_vec2().unwrap();
        assert!((mid.x - 50.0).abs() < 1e-9, "{mid}");
        assert!(mid.y > 50.0, "path bulges along the tangents: {mid}");
        // Non-spatial evaluation ignores tangents.
        assert_eq!(
            t.evaluate(Time::from_millis(500), false),
            Value::Vec2(DVec2::new(50.0, 0.0))
        );
    }

    #[test]
    fn track_structure_is_validated() {
        assert_eq!(KeyframeTrack::try_from(vec![]), Err(TrackError::Empty));
        let k = |t| Keyframe::new(Time(t), s(0.0));
        assert_eq!(
            KeyframeTrack::try_from(vec![k(2), k(1)]),
            Err(TrackError::Unordered(Time(1)))
        );
        assert_eq!(
            KeyframeTrack::try_from(vec![k(1), k(1)]),
            Err(TrackError::DuplicateTime(Time(1)))
        );
    }

    #[test]
    fn shifting_and_removing() {
        let mut t = track(&[(0, 0.0), (1, 1.0), (2, 2.0)]);
        assert_eq!(
            t.shift(&[Time::from_secs(1)], Time::from_secs(1)),
            Err(TrackError::Collision(Time::from_secs(2)))
        );
        t.shift(
            &[Time::from_secs(1), Time::from_secs(2)],
            Time::from_secs(1),
        )
        .unwrap();
        assert_eq!(
            t.keys().iter().map(|k| k.time).collect::<Vec<_>>(),
            [0, 2, 3].map(Time::from_secs)
        );
        assert_eq!(
            t.remove(&[Time::ZERO, Time::from_secs(2), Time::from_secs(3)]),
            Err(TrackError::Empty)
        );
        t.remove(&[Time::from_secs(2)]).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(
            t.remove(&[Time::from_secs(7)]),
            Err(TrackError::NoKeyframe(Time::from_secs(7)))
        );
    }

    #[test]
    fn neighbors() {
        let t = track(&[(0, 0.0), (1, 1.0), (2, 2.0)]);
        let (p, n) = t.neighbors(Time::from_secs(1));
        assert_eq!(p.unwrap().time, Time::ZERO);
        assert_eq!(n.unwrap().time, Time::from_secs(2));
        let (p, n) = t.neighbors(Time::from_millis(-1));
        assert!(p.is_none());
        assert_eq!(n.unwrap().time, Time::ZERO);
    }
}
