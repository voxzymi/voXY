//! Dynamically typed property values.

use std::sync::Arc;

use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::color::LinearRgba;
use crate::id::{AssetId, LayerId};

/// A property value.
///
/// Properties are dynamically typed, so built-in, OpenFX and Alight Motion
/// parameters all share one keyframe, undo and serialization path (design
/// proposal §3.2). Path and gradient values arrive with shapes in Milestone 4.
///
/// [`Value::Layer`], [`Value::Curve`] and [`Value::Asset`] are effect
/// parameters (format 0.7.0). They never interpolate: a keyframed one holds
/// its value until the next key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Bool(bool),
    Int(i64),
    Scalar(f64),
    Vec2(DVec2),
    Vec3(DVec3),
    Color(LinearRgba),
    Enum(u32),
    String(Arc<str>),
    /// A reference to another layer in the same composition, or none.
    Layer(Option<LayerId>),
    /// A tone curve: control points of a monotone-x mapping of `0..=1`.
    Curve(Arc<Curve>),
    /// A reference to an asset (for example a LUT file), or none.
    Asset(Option<AssetId>),
}

/// The type of a [`Value`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Bool,
    Int,
    Scalar,
    Vec2,
    Vec3,
    Color,
    Enum,
    String,
    Layer,
    Curve,
    Asset,
}

impl Value {
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Bool(_) => ValueKind::Bool,
            Value::Int(_) => ValueKind::Int,
            Value::Scalar(_) => ValueKind::Scalar,
            Value::Vec2(_) => ValueKind::Vec2,
            Value::Vec3(_) => ValueKind::Vec3,
            Value::Color(_) => ValueKind::Color,
            Value::Enum(_) => ValueKind::Enum,
            Value::String(_) => ValueKind::String,
            Value::Layer(_) => ValueKind::Layer,
            Value::Curve(_) => ValueKind::Curve,
            Value::Asset(_) => ValueKind::Asset,
        }
    }

    /// `false` if any float component is NaN or infinite. Non-finite values
    /// are rejected everywhere: they break determinism and cannot be
    /// represented in JSON.
    pub fn is_finite(&self) -> bool {
        match self {
            Value::Scalar(v) => v.is_finite(),
            Value::Vec2(v) => v.is_finite(),
            Value::Vec3(v) => v.is_finite(),
            Value::Color(c) => c.is_finite(),
            // A curve's points are validated finite when it is built.
            Value::Bool(_)
            | Value::Int(_)
            | Value::Enum(_)
            | Value::String(_)
            | Value::Layer(_)
            | Value::Curve(_)
            | Value::Asset(_) => true,
        }
    }

    pub fn as_scalar(&self) -> Option<f64> {
        match *self {
            Value::Scalar(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_vec2(&self) -> Option<DVec2> {
        match *self {
            Value::Vec2(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_color(&self) -> Option<LinearRgba> {
        match *self {
            Value::Color(c) => Some(c),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }
}
/// A tone curve for the Curves effect: input `x` to output `y`, both in
/// `0..=1`, through [`Curve::MIN_POINTS`]..=[`Curve::MAX_POINTS`] control
/// points with strictly increasing `x`.
///
/// Every `Curve` is valid: the only constructor is [`Curve::new`], and
/// deserializing runs the same checks, so a damaged file is rejected
/// instead of reaching the renderer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CurveRepr", into = "CurveRepr")]
pub struct Curve {
    points: Vec<DVec2>,
}

#[derive(Serialize, Deserialize)]
struct CurveRepr {
    points: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CurveError {
    #[error("a curve needs {min} to {max} points, not {got}", min = Curve::MIN_POINTS, max = Curve::MAX_POINTS)]
    PointCount { got: usize },
    #[error("curve point {index} is outside 0..=1 or not finite")]
    OutOfRange { index: usize },
    #[error("curve point {index} does not have a larger x than the point before it")]
    NotIncreasing { index: usize },
}

impl Curve {
    pub const MIN_POINTS: usize = 2;
    pub const MAX_POINTS: usize = 64;

    pub fn new(points: Vec<DVec2>) -> Result<Self, CurveError> {
        if !(Self::MIN_POINTS..=Self::MAX_POINTS).contains(&points.len()) {
            return Err(CurveError::PointCount { got: points.len() });
        }
        for (index, p) in points.iter().enumerate() {
            let unit = |v: f64| (0.0..=1.0).contains(&v);
            if !unit(p.x) || !unit(p.y) {
                return Err(CurveError::OutOfRange { index });
            }
            if index > 0 && p.x <= points[index - 1].x {
                return Err(CurveError::NotIncreasing { index });
            }
        }
        Ok(Curve { points })
    }

    /// The identity curve: `(0, 0)` to `(1, 1)`.
    pub fn identity() -> Self {
        Curve {
            points: vec![DVec2::ZERO, DVec2::ONE],
        }
    }

    pub fn points(&self) -> &[DVec2] {
        &self.points
    }

    /// Whether this is the identity: exactly `(0, 0)` and `(1, 1)`.
    pub fn is_identity(&self) -> bool {
        self.points == [DVec2::ZERO, DVec2::ONE]
    }

    /// The curve at `x`: a monotone cubic through the points (Fritsch and
    /// Carlson, so it never overshoots and stays in `0..=1`), flat before
    /// the first point and after the last. Pure `f64`, the same on every
    /// platform. A non-finite `x` reads as 0.
    #[allow(clippy::many_single_char_names)] // The Hermite form's own names.
    pub fn eval(&self, x: f64) -> f64 {
        let p = &self.points;
        let x = if x.is_finite() { x } else { 0.0 };
        let last = p.len() - 1;
        if x <= p[0].x {
            return p[0].y;
        }
        if x >= p[last].x {
            return p[last].y;
        }
        // The segment holding x: p[k].x < x < p[k + 1].x (or equal to p[k].x).
        let k = p
            .partition_point(|q| q.x <= x)
            .saturating_sub(1)
            .min(last - 1);
        let m = self.tangents();
        let h = p[k + 1].x - p[k].x;
        let t = (x - p[k].x) / h;
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * p[k].y
            + (t3 - 2.0 * t2 + t) * h * m[k]
            + (-2.0 * t3 + 3.0 * t2) * p[k + 1].y
            + (t3 - t2) * h * m[k + 1];
        y.clamp(0.0, 1.0)
    }

    /// Fritsch–Carlson tangents: the mean of the neighbouring slopes, zero
    /// at local extremes, then limited so each segment stays monotone.
    #[allow(clippy::many_single_char_names)] // The paper's names: d, m, a, b.
    fn tangents(&self) -> Vec<f64> {
        let p = &self.points;
        let n = p.len();
        let d: Vec<f64> = p
            .windows(2)
            .map(|w| (w[1].y - w[0].y) / (w[1].x - w[0].x))
            .collect();
        let mut m = vec![0.0; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for k in 1..n - 1 {
            m[k] = if d[k - 1] * d[k] <= 0.0 {
                0.0
            } else {
                f64::midpoint(d[k - 1], d[k])
            };
        }
        for k in 0..n - 1 {
            if d[k] == 0.0 {
                m[k] = 0.0;
                m[k + 1] = 0.0;
                continue;
            }
            let (a, b) = (m[k] / d[k], m[k + 1] / d[k]);
            let r = a * a + b * b;
            if r > 9.0 {
                let s = 3.0 / r.sqrt();
                m[k] = s * a * d[k];
                m[k + 1] = s * b * d[k];
            }
        }
        m
    }
}

impl TryFrom<CurveRepr> for Curve {
    type Error = CurveError;

    fn try_from(repr: CurveRepr) -> Result<Self, CurveError> {
        Curve::new(repr.points.into_iter().map(DVec2::from).collect())
    }
}

impl From<Curve> for CurveRepr {
    fn from(curve: Curve) -> Self {
        CurveRepr {
            points: curve.points.into_iter().map(|p| p.to_array()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_pass_through_their_points_and_never_overshoot() {
        let id = Curve::identity();
        assert!(id.is_identity());
        for i in 0..=20 {
            let x = f64::from(i) / 20.0;
            assert!((id.eval(x) - x).abs() < 1e-12);
        }
        let s = Curve::new(vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(0.25, 0.1),
            DVec2::new(0.5, 0.5),
            DVec2::new(0.75, 0.9),
            DVec2::new(1.0, 1.0),
        ])
        .unwrap();
        for p in s.points() {
            assert!((s.eval(p.x) - p.y).abs() < 1e-12, "{p}");
        }
        // Monotone input points give a monotone curve.
        let mut prev = -1.0;
        for i in 0..=1000 {
            let y = s.eval(f64::from(i) / 1000.0);
            assert!(y >= prev - 1e-12 && (0.0..=1.0).contains(&y));
            prev = y;
        }
        // A peak stays at its point's height, and the ends hold flat.
        let peak = Curve::new(vec![
            DVec2::new(0.2, 0.0),
            DVec2::new(0.5, 1.0),
            DVec2::new(0.8, 0.0),
        ])
        .unwrap();
        for i in 0..=1000 {
            let y = peak.eval(f64::from(i) / 1000.0);
            assert!((0.0..=1.0).contains(&y));
        }
        assert_eq!(peak.eval(0.1), 0.0);
        assert_eq!(peak.eval(0.95), 0.0);
        assert_eq!(peak.eval(f64::NAN), 0.0);
    }

    fn pts(p: &[(f64, f64)]) -> Vec<DVec2> {
        p.iter().map(|&(x, y)| DVec2::new(x, y)).collect()
    }

    #[test]
    fn curve_validation() {
        assert!(Curve::new(pts(&[(0.0, 0.0), (0.5, 0.7), (1.0, 1.0)])).is_ok());
        assert_eq!(
            Curve::new(pts(&[(0.0, 0.0)])),
            Err(CurveError::PointCount { got: 1 })
        );
        let many = (0..=Curve::MAX_POINTS)
            .map(|i| DVec2::new(i as f64 / Curve::MAX_POINTS as f64, 0.5))
            .collect();
        assert!(matches!(
            Curve::new(many),
            Err(CurveError::PointCount { .. })
        ));
        assert_eq!(
            Curve::new(pts(&[(0.0, 0.0), (0.0, 1.0)])),
            Err(CurveError::NotIncreasing { index: 1 })
        );
        assert_eq!(
            Curve::new(pts(&[(0.0, 0.0), (1.0, 1.5)])),
            Err(CurveError::OutOfRange { index: 1 })
        );
        assert_eq!(
            Curve::new(pts(&[(0.0, f64::NAN), (1.0, 1.0)])),
            Err(CurveError::OutOfRange { index: 0 })
        );
    }

    #[test]
    fn new_values_round_trip_through_json() {
        let values = [
            Value::Layer(Some(LayerId(7))),
            Value::Layer(None),
            Value::Asset(Some(AssetId(3))),
            Value::Asset(None),
            Value::Curve(Arc::new(
                Curve::new(pts(&[(0.0, 0.1), (0.4, 0.6), (1.0, 0.9)])).unwrap(),
            )),
        ];
        for v in values {
            let json = serde_json::to_string(&v).unwrap();
            let back: Value = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v, "{json}");
            assert!(back.is_finite());
        }
        assert_eq!(
            serde_json::to_string(&Value::Layer(Some(LayerId(7)))).unwrap(),
            r#"{"type":"layer","value":7}"#
        );
    }

    #[test]
    fn damaged_curves_are_rejected_on_load() {
        for bad in [
            r#"{"type":"curve","value":{"points":[[0.0,0.0]]}}"#,
            r#"{"type":"curve","value":{"points":[[0.5,0.0],[0.2,1.0]]}}"#,
            r#"{"type":"curve","value":{"points":[[0.0,0.0],[1.0,2.0]]}}"#,
            r#"{"type":"curve","value":{"points":"nonsense"}}"#,
        ] {
            assert!(serde_json::from_str::<Value>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn references_and_curves_hold_between_keys() {
        let a = Value::Layer(Some(LayerId(1)));
        let b = Value::Layer(Some(LayerId(2)));
        assert_eq!(crate::keyframe::interpolate(&a, &b, 0.99), a);
        let c0 = Value::Curve(Arc::new(Curve::identity()));
        let c1 = Value::Curve(Arc::new(
            Curve::new(pts(&[(0.0, 1.0), (1.0, 0.0)])).unwrap(),
        ));
        assert_eq!(crate::keyframe::interpolate(&c0, &c1, 0.5), c0);
    }
}
