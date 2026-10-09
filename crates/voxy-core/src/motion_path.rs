//! Spatial interpolation along cubic motion paths, parameterized by arc
//! length.
//!
//! A segment of a position path is the cubic Bézier
//! `P0 = v0, P1 = v0 + out_tangent(k0), P2 = v1 + in_tangent(k1), P3 = v1`.
//! Temporal easing yields progress `p`, which is interpreted as the fraction
//! of the path's *length* travelled. Speed along the path therefore follows
//! the ease curve exactly, however unevenly the Bézier parameter is spread
//! along the curve.

use glam::DVec3;

/// Sub-intervals of the Bézier parameter used for the arc-length table.
const SPANS: usize = 32;

/// 5-point Gauss–Legendre nodes and weights on `[-1, 1]`.
const GL_NODES: [f64; 5] = [
    0.0,
    -0.538_469_310_105_683_1,
    0.538_469_310_105_683_1,
    -0.906_179_845_938_664,
    0.906_179_845_938_664,
];
const GL_WEIGHTS: [f64; 5] = [
    0.568_888_888_888_888_9,
    0.478_628_670_499_366_5,
    0.478_628_670_499_366_5,
    0.236_926_885_056_189_1,
    0.236_926_885_056_189_1,
];

/// One segment of a motion path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicPath {
    pub p: [DVec3; 4],
}

impl CubicPath {
    pub fn new(v0: DVec3, out_tangent: DVec3, in_tangent: DVec3, v1: DVec3) -> Self {
        CubicPath {
            p: [v0, v0 + out_tangent, v1 + in_tangent, v1],
        }
    }

    /// `true` when both tangents are zero: the path is the straight line.
    pub fn is_straight(&self) -> bool {
        self.p[1] == self.p[0] && self.p[2] == self.p[3]
    }

    /// Point at Bézier parameter `s`.
    pub fn point(&self, s: f64) -> DVec3 {
        let t = 1.0 - s;
        let [p0, p1, p2, p3] = self.p;
        p0 * (t * t * t) + p1 * (3.0 * t * t * s) + p2 * (3.0 * t * s * s) + p3 * (s * s * s)
    }

    /// Derivative with respect to `s`.
    pub fn derivative(&self, s: f64) -> DVec3 {
        let t = 1.0 - s;
        let [p0, p1, p2, p3] = self.p;
        (p1 - p0) * (3.0 * t * t) + (p2 - p1) * (6.0 * t * s) + (p3 - p2) * (3.0 * s * s)
    }

    /// Arc length between parameters `a` and `b` (5-point Gauss–Legendre).
    fn length_between(&self, a: f64, b: f64) -> f64 {
        let half = 0.5 * (b - a);
        let mid = f64::midpoint(a, b);
        GL_NODES
            .iter()
            .zip(GL_WEIGHTS)
            .map(|(&n, w)| w * self.derivative(mid + half * n).length())
            .sum::<f64>()
            * half
    }

    /// Cumulative arc length at each span boundary; `table[SPANS]` is the
    /// total length.
    fn length_table(&self) -> [f64; SPANS + 1] {
        let mut table = [0.0; SPANS + 1];
        for i in 0..SPANS {
            let a = i as f64 / SPANS as f64;
            let b = (i + 1) as f64 / SPANS as f64;
            table[i + 1] = table[i] + self.length_between(a, b);
        }
        table
    }

    /// Total arc length.
    pub fn length(&self) -> f64 {
        if self.is_straight() {
            return self.p[0].distance(self.p[3]);
        }
        self.length_table()[SPANS]
    }

    /// The Bézier parameter at which `fraction` of the length has been
    /// travelled (`fraction` clamped to `[0, 1]`).
    pub fn param_at_length_fraction(&self, fraction: f64) -> f64 {
        let fraction = fraction.clamp(0.0, 1.0);
        if self.is_straight() {
            return fraction;
        }
        let table = self.length_table();
        let total = table[SPANS];
        if total <= 0.0 {
            return fraction;
        }
        let target = fraction * total;
        // Span containing the target length.
        let span = table[1..].partition_point(|&l| l < target).min(SPANS - 1);
        let span_start = span as f64 / SPANS as f64;
        let (mut lo, mut hi) = (span_start, (span + 1) as f64 / SPANS as f64);
        let base = table[span];
        let span_len = (table[span + 1] - base).max(f64::MIN_POSITIVE);
        // Safeguarded Newton on L(s) − target, where L'(s) = |B'(s)|. Every
        // iterate stays inside [lo, hi] ⊆ this span, so L(s) = base + ∫ span_start..s.
        let mut s = lo + (hi - lo) * ((target - base) / span_len).clamp(0.0, 1.0);
        for _ in 0..24 {
            let err = base + self.length_between(span_start, s) - target;
            if err.abs() <= 1e-12 * total.max(1.0) {
                break;
            }
            if err > 0.0 {
                hi = s;
            } else {
                lo = s;
            }
            let speed = self.derivative(s).length();
            let newton = s - err / speed;
            s = if speed > 1e-12 && newton > lo && newton < hi {
                newton
            } else {
                f64::midpoint(lo, hi)
            };
        }
        s
    }

    /// The point `fraction` of the way along the path by arc length.
    ///
    /// Fractions outside `[0, 1]` (overshooting eases) continue in a straight
    /// line along the tangent at the nearer end, at the same speed.
    pub fn point_at_length_fraction(&self, fraction: f64) -> DVec3 {
        if self.is_straight() {
            // Exact at both ends, and extrapolates naturally.
            return self.p[0] * (1.0 - fraction) + self.p[3] * fraction;
        }
        if (0.0..=1.0).contains(&fraction) {
            return self.point(self.param_at_length_fraction(fraction));
        }
        let length = self.length();
        if fraction < 0.0 {
            self.p[0] + self.end_direction(false) * (fraction * length)
        } else {
            self.p[3] + self.end_direction(true) * ((fraction - 1.0) * length)
        }
    }

    /// Unit direction of travel at the start or end. Falls back through the
    /// control points when a tangent is zero.
    fn end_direction(&self, at_end: bool) -> DVec3 {
        let [p0, p1, p2, p3] = self.p;
        let candidates = if at_end {
            [p3 - p2, p3 - p1, p3 - p0]
        } else {
            [p1 - p0, p2 - p0, p3 - p0]
        };
        candidates
            .into_iter()
            .find(|d| d.length_squared() > 0.0)
            .map_or(DVec3::ZERO, DVec3::normalize)
    }
}

/// Smooth ("auto Bézier") tangents for a key between `prev` and `next`, in
/// the Catmull–Rom style: the path passes through the key heading from
/// `prev` towards `next`.
pub fn auto_tangents(prev: DVec3, next: DVec3) -> (DVec3, DVec3) {
    let t = (next - prev) / 6.0;
    (-t, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Quarter circle of radius r approximated by the standard cubic
    /// (k = 0.5523). Its arc length is close to πr/2.
    fn quarter_circle(r: f64) -> CubicPath {
        let k = 0.552_284_749_830_793_4 * r;
        CubicPath::new(
            DVec3::new(r, 0.0, 0.0),
            DVec3::new(0.0, k, 0.0),
            DVec3::new(k, 0.0, 0.0),
            DVec3::new(0.0, r, 0.0),
        )
    }

    fn polyline_length(path: &CubicPath, s_end: f64, n: usize) -> f64 {
        (0..n)
            .map(|i| {
                let a = s_end * i as f64 / n as f64;
                let b = s_end * (i + 1) as f64 / n as f64;
                path.point(a).distance(path.point(b))
            })
            .sum()
    }

    #[test]
    fn straight_path_is_lerp() {
        let p = CubicPath::new(
            DVec3::new(1.0, 2.0, 0.0),
            DVec3::ZERO,
            DVec3::ZERO,
            DVec3::new(11.0, 2.0, 0.0),
        );
        assert!(p.is_straight());
        assert_eq!(p.length(), 10.0);
        assert_eq!(p.point_at_length_fraction(0.25), DVec3::new(3.5, 2.0, 0.0));
        assert_eq!(p.point_at_length_fraction(1.0), DVec3::new(11.0, 2.0, 0.0));
        assert!(
            p.point_at_length_fraction(1.1)
                .distance(DVec3::new(12.0, 2.0, 0.0))
                < 1e-12
        );
    }

    #[test]
    fn quarter_circle_length() {
        let p = quarter_circle(100.0);
        let expected = std::f64::consts::FRAC_PI_2 * 100.0;
        assert!(
            (p.length() - expected).abs() / expected < 3e-4,
            "{}",
            p.length()
        );
        // Agrees with a very fine polyline.
        assert!((p.length() - polyline_length(&p, 1.0, 200_000)).abs() < 1e-6);
    }

    #[test]
    fn length_fraction_is_accurate() {
        let p = CubicPath::new(
            DVec3::ZERO,
            DVec3::new(300.0, 0.0, 0.0),
            DVec3::new(0.0, -10.0, 0.0),
            DVec3::new(100.0, 100.0, 0.0),
        );
        let total = polyline_length(&p, 1.0, 100_000);
        for i in 0..=20 {
            let f = f64::from(i) / 20.0;
            let s = p.param_at_length_fraction(f);
            let travelled = polyline_length(&p, s, 100_000);
            assert!(
                (travelled / total - f).abs() < 1e-6,
                "f={f}: {}",
                travelled / total
            );
        }
    }

    #[test]
    fn uniform_fractions_give_uniform_speed() {
        // Highly uneven parameterization (control points bunched at one end)
        // and a sharp turn near the start.
        let p = CubicPath::new(
            DVec3::ZERO,
            DVec3::new(5.0, 0.0, 0.0),
            DVec3::new(-400.0, 0.0, 0.0),
            DVec3::new(500.0, 200.0, 0.0),
        );
        let n = 200;
        let params: Vec<f64> = (0..=n)
            .map(|i| p.param_at_length_fraction(f64::from(i) / f64::from(n)))
            .collect();
        // Arc travelled per equal step of length fraction, measured along
        // the curve with a fine polyline.
        let steps: Vec<f64> = params
            .windows(2)
            .map(|w| {
                (0..500)
                    .map(|j| {
                        let a = w[0] + (w[1] - w[0]) * f64::from(j) / 500.0;
                        let b = w[0] + (w[1] - w[0]) * f64::from(j + 1) / 500.0;
                        p.point(a).distance(p.point(b))
                    })
                    .sum()
            })
            .collect();
        let expected = p.length() / f64::from(n);
        for d in &steps {
            assert!((d - expected).abs() / expected < 1e-5, "{d} vs {expected}");
        }
    }

    #[test]
    fn extrapolation_is_continuous() {
        let p = quarter_circle(50.0);
        let end = p.point_at_length_fraction(1.0);
        let past = p.point_at_length_fraction(1.0 + 1e-9);
        assert!(end.distance(past) < 1e-6);
        let before = p.point_at_length_fraction(-0.1);
        // Heading at the start is +y; 10% of the length before the start.
        assert!((before - DVec3::new(50.0, -0.1 * p.length(), 0.0)).length() < 1e-9);
    }

    #[test]
    fn degenerate_paths_do_not_misbehave() {
        let a = DVec3::new(3.0, 4.0, 0.0);
        let p = CubicPath::new(a, DVec3::ZERO, DVec3::ZERO, a);
        assert_eq!(p.point_at_length_fraction(0.5), a);
        // Loop that returns to its start: non-zero length, finite results.
        let l = CubicPath::new(
            a,
            DVec3::new(100.0, 0.0, 0.0),
            DVec3::new(100.0, 50.0, 0.0),
            a,
        );
        for i in 0..=10 {
            assert!(l.point_at_length_fraction(f64::from(i) / 10.0).is_finite());
        }
    }

    proptest! {
        #[test]
        fn fraction_to_param_is_monotone(
            c in prop::array::uniform4(-500.0f64..500.0),
            end in prop::array::uniform2(-500.0f64..500.0),
        ) {
            let p = CubicPath::new(
                DVec3::ZERO,
                DVec3::new(c[0], c[1], 0.0),
                DVec3::new(c[2], c[3], 0.0),
                DVec3::new(end[0], end[1], 0.0),
            );
            let mut prev = 0.0;
            for i in 0..=50 {
                let s = p.param_at_length_fraction(f64::from(i) / 50.0);
                prop_assert!((0.0..=1.0).contains(&s));
                prop_assert!(s >= prev - 1e-12);
                prev = s;
            }
            prop_assert_eq!(p.point_at_length_fraction(0.0), p.point(p.param_at_length_fraction(0.0)));
        }
    }
}
