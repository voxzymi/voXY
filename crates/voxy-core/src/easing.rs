//! Temporal easing: normalized cubic-bezier curves, like CSS `cubic-bezier()`.
//!
//! A segment between two keyframes maps normalized time `u ∈ [0, 1]` to
//! progress `p` through the value change. The curve runs from `(0, 0)` to
//! `(1, 1)` with control points `P1` (the left key's outgoing handle) and `P2`
//! (the right key's incoming handle). Handle `x` is restricted to `[0, 1]` so
//! the curve is a function of time; `y` may leave `[0, 1]` to overshoot.

use serde::{Deserialize, Serialize};

/// One control point of a segment's ease curve, in normalized segment space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[f64; 2]", into = "[f64; 2]")]
pub struct EaseHandle {
    x: f64,
    y: f64,
}

impl EaseHandle {
    /// Outgoing handle that makes a segment exactly linear.
    pub const LINEAR_OUT: EaseHandle = EaseHandle {
        x: 1.0 / 3.0,
        y: 1.0 / 3.0,
    };
    /// Incoming handle that makes a segment exactly linear.
    pub const LINEAR_IN: EaseHandle = EaseHandle {
        x: 2.0 / 3.0,
        y: 2.0 / 3.0,
    };

    /// `None` unless `x ∈ [0, 1]` and both coordinates are finite.
    pub fn new(x: f64, y: f64) -> Option<Self> {
        ((0.0..=1.0).contains(&x) && y.is_finite()).then_some(EaseHandle { x, y })
    }

    pub fn x(self) -> f64 {
        self.x
    }

    pub fn y(self) -> f64 {
        self.y
    }
}

impl TryFrom<[f64; 2]> for EaseHandle {
    type Error = String;
    fn try_from([x, y]: [f64; 2]) -> Result<Self, String> {
        EaseHandle::new(x, y)
            .ok_or_else(|| format!("invalid ease handle [{x}, {y}]: x must be in 0..=1"))
    }
}

impl From<EaseHandle> for [f64; 2] {
    fn from(h: EaseHandle) -> Self {
        [h.x, h.y]
    }
}

/// A segment's ease curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicEase {
    pub p1: EaseHandle,
    pub p2: EaseHandle,
}

impl CubicEase {
    pub const LINEAR: CubicEase = CubicEase {
        p1: EaseHandle::LINEAR_OUT,
        p2: EaseHandle::LINEAR_IN,
    };

    /// Progress at normalized time `u`. Clamps `u` to `[0, 1]`; the result
    /// is exactly 0 at `u = 0` and exactly 1 at `u = 1`.
    pub fn progress(&self, u: f64) -> f64 {
        if u <= 0.0 {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        let s = solve_x(self.p1.x, self.p2.x, u);
        bezier(self.p1.y, self.p2.y, s)
    }

    /// d(progress)/du at normalized time `u`; the graph editor draws this as
    /// the speed curve.
    pub fn slope(&self, u: f64) -> f64 {
        let s = solve_x(self.p1.x, self.p2.x, u.clamp(0.0, 1.0));
        let dx = bezier_deriv(self.p1.x, self.p2.x, s);
        let dy = bezier_deriv(self.p1.y, self.p2.y, s);
        if dx.abs() < 1e-12 {
            // Vertical tangent (only possible at the ends with x handles at 0
            // or 1): report a large, sign-correct slope.
            return dy.signum() * 1e12;
        }
        dy / dx
    }
}

/// One coordinate of the cubic with endpoints 0 and 1.
fn bezier(c1: f64, c2: f64, s: f64) -> f64 {
    let t = 1.0 - s;
    3.0 * t * t * s * c1 + 3.0 * t * s * s * c2 + s * s * s
}

fn bezier_deriv(c1: f64, c2: f64, s: f64) -> f64 {
    let t = 1.0 - s;
    3.0 * t * t * c1 + 6.0 * t * s * (c2 - c1) + 3.0 * s * s * (1.0 - c2)
}

/// Finds `s ∈ [0, 1]` with `x(s) = u`. `x` is monotone because both x handles
/// lie in `[0, 1]`. Newton's method converges in a few steps for typical
/// curves; a fixed-length bisection backs it up. Both use fixed iteration
/// limits, so the result is deterministic.
fn solve_x(x1: f64, x2: f64, u: f64) -> f64 {
    const EPS: f64 = 1e-14;
    let mut s = u;
    for _ in 0..8 {
        let err = bezier(x1, x2, s) - u;
        if err.abs() < EPS {
            return s;
        }
        let d = bezier_deriv(x1, x2, s);
        if d.abs() < 1e-9 {
            break;
        }
        let next = s - err / d;
        if !(0.0..=1.0).contains(&next) {
            break;
        }
        s = next;
    }
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    let mut mid = u;
    for _ in 0..64 {
        mid = f64::midpoint(lo, hi);
        let x = bezier(x1, x2, mid);
        if (x - u).abs() < EPS {
            break;
        }
        if x < u {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    mid
}

/// Named ease curves. Applying a preset to a segment sets the left key's
/// outgoing handle and the right key's incoming handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EasePreset {
    Linear,
    /// CSS `ease`.
    Ease,
    /// CSS `ease-in`: slow start.
    EaseIn,
    /// CSS `ease-out`: slow end.
    EaseOut,
    /// CSS `ease-in-out`.
    EaseInOut,
    /// Zero speed at both ends with one-third influence (a classic motion
    /// design "easy ease").
    EasyEase,
    /// Accelerates then overshoots past the target and settles.
    BackOut,
    /// Pulls back below the start before accelerating.
    BackIn,
}

impl EasePreset {
    pub const ALL: [EasePreset; 8] = [
        EasePreset::Linear,
        EasePreset::Ease,
        EasePreset::EaseIn,
        EasePreset::EaseOut,
        EasePreset::EaseInOut,
        EasePreset::EasyEase,
        EasePreset::BackOut,
        EasePreset::BackIn,
    ];

    pub fn curve(self) -> CubicEase {
        let c = |x1, y1, x2, y2| CubicEase {
            p1: EaseHandle { x: x1, y: y1 },
            p2: EaseHandle { x: x2, y: y2 },
        };
        match self {
            EasePreset::Linear => CubicEase::LINEAR,
            EasePreset::Ease => c(0.25, 0.1, 0.25, 1.0),
            EasePreset::EaseIn => c(0.42, 0.0, 1.0, 1.0),
            EasePreset::EaseOut => c(0.0, 0.0, 0.58, 1.0),
            EasePreset::EaseInOut => c(0.42, 0.0, 0.58, 1.0),
            EasePreset::EasyEase => c(1.0 / 3.0, 0.0, 2.0 / 3.0, 1.0),
            EasePreset::BackOut => c(0.34, 1.56, 0.64, 1.0),
            EasePreset::BackIn => c(0.36, 0.0, 0.66, -0.56),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EasePreset::Linear => "Linear",
            EasePreset::Ease => "Ease",
            EasePreset::EaseIn => "Ease In",
            EasePreset::EaseOut => "Ease Out",
            EasePreset::EaseInOut => "Ease In Out",
            EasePreset::EasyEase => "Easy Ease",
            EasePreset::BackOut => "Back Out (overshoot)",
            EasePreset::BackIn => "Back In (anticipation)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn linear_handles_are_exactly_linear() {
        for i in 0..=1000 {
            let u = f64::from(i) / 1000.0;
            assert!((CubicEase::LINEAR.progress(u) - u).abs() < 1e-12, "{u}");
        }
    }

    #[test]
    fn endpoints_are_exact() {
        for preset in EasePreset::ALL {
            let c = preset.curve();
            assert_eq!(c.progress(0.0), 0.0, "{preset:?}");
            assert_eq!(c.progress(1.0), 1.0, "{preset:?}");
            assert_eq!(c.progress(-3.0), 0.0);
            assert_eq!(c.progress(7.0), 1.0);
        }
    }

    #[test]
    fn known_values() {
        // Symmetric curves pass through the midpoint.
        assert!((EasePreset::EaseInOut.curve().progress(0.5) - 0.5).abs() < 1e-12);
        assert!((EasePreset::EasyEase.curve().progress(0.5) - 0.5).abs() < 1e-12);
        // Ease-in starts slow, ease-out starts fast.
        assert!(EasePreset::EaseIn.curve().progress(0.25) < 0.25);
        assert!(EasePreset::EaseOut.curve().progress(0.25) > 0.25);
        // Easy ease has zero speed at both ends.
        assert!(EasePreset::EasyEase.curve().slope(0.0).abs() < 1e-9);
        assert!(EasePreset::EasyEase.curve().slope(1.0).abs() < 1e-9);
        // Back out overshoots; back in anticipates.
        let max = (0..=100)
            .map(|i| EasePreset::BackOut.curve().progress(f64::from(i) / 100.0))
            .fold(0.0, f64::max);
        assert!(max > 1.05, "{max}");
        let min = (0..=100)
            .map(|i| EasePreset::BackIn.curve().progress(f64::from(i) / 100.0))
            .fold(1.0, f64::min);
        assert!(min < -0.05, "{min}");
    }

    #[test]
    fn handles_validate() {
        assert!(EaseHandle::new(1.2, 0.0).is_none());
        assert!(EaseHandle::new(0.5, f64::NAN).is_none());
        assert!(EaseHandle::new(0.5, -3.0).is_some());
        assert!(serde_json::from_str::<EaseHandle>("[1.5, 0.0]").is_err());
        assert_eq!(
            serde_json::to_string(&EaseHandle::new(0.25, 1.5).unwrap()).unwrap(),
            "[0.25,1.5]"
        );
    }

    proptest! {
        /// The solver inverts x(s) accurately for any valid curve.
        #[test]
        fn solver_inverts_x(x1 in 0.0f64..=1.0, x2 in 0.0f64..=1.0, u in 0.0f64..=1.0) {
            let s = solve_x(x1, x2, u);
            prop_assert!((0.0..=1.0).contains(&s));
            prop_assert!((bezier(x1, x2, s) - u).abs() < 1e-9);
        }

        /// For y handles inside [0, 1], progress is monotone in time.
        #[test]
        fn progress_is_monotone(x1 in 0.0f64..=1.0, y1 in 0.0f64..=1.0, x2 in 0.0f64..=1.0, y2 in 0.0f64..=1.0) {
            let c = CubicEase { p1: EaseHandle::new(x1, y1).unwrap(), p2: EaseHandle::new(x2, y2).unwrap() };
            let mut prev = 0.0;
            for i in 0..=200 {
                let p = c.progress(f64::from(i) / 200.0);
                prop_assert!(p >= prev - 1e-9, "{p} < {prev}");
                prev = p;
            }
        }
    }
}
