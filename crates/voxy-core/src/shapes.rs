//! Parametric easings: spring, bounce, oscillation, steps and stepped ramp.
//!
//! Each is an *analytic* normalized function `f(u)` of segment time
//! `u ∈ [0, 1]` with `f(0) = 0` and `f(1) = 1` for every parameter value. The
//! segment's values are `mix(v0, v1, f(u))`, exactly like a cubic-bézier
//! ease; `f` may leave `[0, 1]` (springs, oscillations) and may be
//! discontinuous (steps). Nothing here is approximated by a bézier.
//!
//! The formulas, parameter ranges and discontinuity rules are specified in
//! `docs/design-graphs.md`; this file is their reference implementation.
//!
//! # Numeric contract
//!
//! - Every function is total: any parameter bit pattern (including NaN or
//!   infinity) is sanitized first, and any `u` (including NaN) is accepted;
//!   the result is always finite.
//! - Only `+ − × ÷`, `sqrt`, `floor` and the pure-Rust [`libm`] `sin`, `cos`
//!   and `exp` are used, with no fused multiply-add and no fast-math, so the
//!   result is bit-identical on every platform. `std`'s `sin`/`cos`/`exp`
//!   defer to the platform's math library and are deliberately not used.
//! - `f(u)` for `u ≤ 0` (or NaN) is exactly `0`, and for `u ≥ 1` exactly `1`.

use std::f64::consts::{PI, TAU};

use serde::{Deserialize, Serialize};

/// `true` for `u ≤ 0` and for NaN: times at or before the segment start.
fn at_or_before_start(u: f64) -> bool {
    u.is_nan() || u <= 0.0
}

/// `u` forced into `[0, 1]`; NaN becomes `0`. (`f64::clamp` passes NaN on.)
fn unit(u: f64) -> f64 {
    if u > 0.0 { u.min(1.0) } else { 0.0 }
}

fn sin(x: f64) -> f64 {
    libm::sin(x)
}

fn cos(x: f64) -> f64 {
    libm::cos(x)
}

fn exp(x: f64) -> f64 {
    libm::exp(x)
}

/// An inclusive parameter range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

impl Range {
    const fn new(min: f64, default: f64, max: f64) -> Self {
        Range { min, max, default }
    }

    pub fn contains(self, v: f64) -> bool {
        v.is_finite() && v >= self.min && v <= self.max
    }

    /// `v` clamped into the range; non-finite values become the default.
    pub fn clamp(self, v: f64) -> f64 {
        if v.is_finite() {
            v.clamp(self.min, self.max)
        } else {
            self.default
        }
    }

    fn check(self, name: &str, v: f64) -> Result<(), String> {
        if self.contains(v) {
            Ok(())
        } else {
            Err(format!(
                "{name} = {v} is outside {}..={}",
                self.min, self.max
            ))
        }
    }
}

/// A discontinuity of an easing: at normalized segment time `at` the
/// progress jumps up by `height` (always positive). `at` is `0` for a jump
/// that happens the instant after the segment's left key, and `1` for one
/// that happens at its right key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jump {
    pub at: f64,
    pub height: f64,
}

// ───────────────────────────── Spring ─────────────────────────────

/// Damped oscillator released toward the target.
///
/// With `a = damping`, `ω = 2π·frequency` and `B = (a/ω)(1 − overshoot)`:
///
/// ```text
/// D(u) = e^(−a·u) · (cos ωu + B·sin ωu)
/// R    = D(1)
/// f(u) = 1 − D(u) + R·h(u),     h(u) = u²(3 − 2u)
/// ```
///
/// `1 − D(u)` is the closed-form response of a damped spring; with
/// `overshoot = 0` it is released from rest (`f′(0) = 0`) and its first peak
/// is `1 + e^(−aπ/ω)`. `overshoot` raises the launch speed to
/// `f′(0) = a·overshoot` (more energy, a taller first peak). The `R·h(u)`
/// term is the *window*: it removes the unsettled residual `R` so that
/// `f(1) = 1` exactly, with `|R| ≤ e^(−a)·√(1 + B²)` (under 0.1 % at the
/// default damping).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpringParams {
    /// Oscillations (full cycles) across the segment.
    pub frequency: f64,
    /// Decay rate: the envelope shrinks by `e^(−damping)` over the segment.
    pub damping: f64,
    /// Extra launch energy; `0` releases the spring from rest.
    pub overshoot: f64,
}

impl SpringParams {
    pub const FREQUENCY: Range = Range::new(0.5, 2.5, 12.0);
    pub const DAMPING: Range = Range::new(1.0, 7.0, 24.0);
    pub const OVERSHOOT: Range = Range::new(0.0, 0.0, 2.0);

    #[must_use]
    pub fn sanitized(self) -> Self {
        SpringParams {
            frequency: Self::FREQUENCY.clamp(self.frequency),
            damping: Self::DAMPING.clamp(self.damping),
            overshoot: Self::OVERSHOOT.clamp(self.overshoot),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        Self::FREQUENCY.check("spring frequency", self.frequency)?;
        Self::DAMPING.check("spring damping", self.damping)?;
        Self::OVERSHOOT.check("spring overshoot", self.overshoot)
    }

    /// `(a, ω, B)` of the sanitized parameters: decay rate, angular
    /// frequency and the sine coefficient.
    fn terms(self) -> (f64, f64, f64) {
        let p = self.sanitized();
        let decay = p.damping;
        let omega = TAU * p.frequency;
        (decay, omega, (decay / omega) * (1.0 - p.overshoot))
    }

    pub fn progress(self, u: f64) -> f64 {
        if at_or_before_start(u) {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        let (decay, omega, skew) = self.terms();
        let deviation = |x: f64| exp(-decay * x) * (cos(omega * x) + skew * sin(omega * x));
        1.0 - deviation(u) + deviation(1.0) * (u * u * (3.0 - 2.0 * u))
    }

    pub fn slope(self, u: f64) -> f64 {
        let u = unit(u);
        let (decay, omega, skew) = self.terms();
        let envelope = exp(-decay * u);
        let d_deviation = envelope
            * ((skew * omega - decay) * cos(omega * u) - (decay * skew + omega) * sin(omega * u));
        let residual = exp(-decay) * (cos(omega) + skew * sin(omega));
        -d_deviation + residual * 6.0 * u * (1.0 - u)
    }
}

impl SpringParams {
    /// How far the first peak rises past the target, as a fraction of the
    /// change (`0.25` = 25 % overshoot), found by sampling the curve. A
    /// display aid for the editor: it is the result of the three parameters,
    /// not a fourth one. Deterministic, like everything in this module.
    pub fn peak_overshoot(self) -> f64 {
        const SAMPLES: u32 = 2048;
        let mut peak = 1.0f64;
        for i in 1..SAMPLES {
            peak = peak.max(self.progress(f64::from(i) / f64::from(SAMPLES)));
        }
        peak - 1.0
    }
}

impl Default for SpringParams {
    fn default() -> Self {
        SpringParams {
            frequency: Self::FREQUENCY.default,
            damping: Self::DAMPING.default,
            overshoot: Self::OVERSHOOT.default,
        }
    }
}

// ───────────────────────────── Bounce ─────────────────────────────

/// A ball dropped onto the target and bouncing to rest.
///
/// Gravity is constant, so everything follows from the first arc. Let
/// `h = first_height` (the first rebound's height as a fraction of the whole
/// drop), `r = restitution` (speed kept per bounce) and `n = count`
/// (rebound arcs). Arc `k = 1..n` has height `H_k = h·r^(2(k−1))` and
/// duration `d_k = 2·t₀·√h·r^(k−1)`, where `t₀` is the fall time. Total time
/// is normalized to 1:
///
/// ```text
/// t₀ = 1 / (1 + 2√h · Σ_{k=1..n} r^(k−1))
/// fall    (0 ≤ u ≤ t₀):    f = (u / t₀)²
/// arc k   (q = (u − c_{k−1}) / d_k):   f = 1 − 4·H_k·q·(1 − q)
/// c₀ = t₀,  c_k = c_{k−1} + d_k,  c_n ≡ 1
/// ```
///
/// Contacts (`f = 1`, speed reverses: a corner, not a jump) are at `c₀..c_n`.
/// `f` stays within `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BounceParams {
    /// Rebound arcs after the first landing.
    pub count: u32,
    /// Height of the first rebound, as a fraction of the drop.
    pub first_height: f64,
    /// Speed kept at each bounce (arc heights shrink by `restitution²`).
    pub restitution: f64,
}

impl BounceParams {
    pub const COUNT_MIN: u32 = 1;
    pub const COUNT_MAX: u32 = 12;
    pub const COUNT_DEFAULT: u32 = 3;
    pub const FIRST_HEIGHT: Range = Range::new(0.02, 0.25, 1.0);
    pub const RESTITUTION: Range = Range::new(0.1, 0.5, 0.9);

    #[must_use]
    pub fn sanitized(self) -> Self {
        BounceParams {
            count: self.count.clamp(Self::COUNT_MIN, Self::COUNT_MAX),
            first_height: Self::FIRST_HEIGHT.clamp(self.first_height),
            restitution: Self::RESTITUTION.clamp(self.restitution),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(Self::COUNT_MIN..=Self::COUNT_MAX).contains(&self.count) {
            return Err(format!(
                "bounce count = {} is outside {}..={}",
                self.count,
                Self::COUNT_MIN,
                Self::COUNT_MAX
            ));
        }
        Self::FIRST_HEIGHT.check("bounce first height", self.first_height)?;
        Self::RESTITUTION.check("bounce restitution", self.restitution)
    }

    /// Fall time `t₀` and the arc durations `d_1..d_n`.
    fn timing(self) -> (f64, Vec<f64>) {
        let p = self.sanitized();
        let root = sqrt(p.first_height);
        let mut sum = 0.0;
        let mut scale = 1.0;
        for _ in 0..p.count {
            sum += scale;
            scale *= p.restitution;
        }
        let t0 = 1.0 / (1.0 + 2.0 * root * sum);
        let mut durations = Vec::with_capacity(p.count as usize);
        let mut scale = 1.0;
        for _ in 0..p.count {
            durations.push(2.0 * t0 * root * scale);
            scale *= p.restitution;
        }
        (t0, durations)
    }

    /// Times of the `count + 1` contacts with the target, ascending; the
    /// last is exactly `1`.
    pub fn contact_times(self) -> Vec<f64> {
        let (t0, durations) = self.timing();
        let mut times = vec![t0];
        let mut c = t0;
        for d in &durations {
            c += d;
            times.push(c);
        }
        if let Some(last) = times.last_mut() {
            *last = 1.0;
        }
        times
    }

    pub fn progress(self, u: f64) -> f64 {
        if at_or_before_start(u) {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        let p = self.sanitized();
        let contacts = p.contact_times();
        let t0 = contacts[0];
        if u < t0 {
            let q = u / t0;
            return (q * q).clamp(0.0, 1.0);
        }
        let mut height = p.first_height;
        for k in 1..contacts.len() {
            let (start, end) = (contacts[k - 1], contacts[k]);
            if u < end {
                let q = (u - start) / (end - start);
                return (1.0 - 4.0 * height * q * (1.0 - q)).clamp(0.0, 1.0);
            }
            height *= p.restitution * p.restitution;
        }
        1.0
    }

    pub fn slope(self, u: f64) -> f64 {
        let u = unit(u);
        let p = self.sanitized();
        let contacts = p.contact_times();
        let t0 = contacts[0];
        if u < t0 {
            return 2.0 * u / (t0 * t0);
        }
        let mut height = p.first_height;
        for k in 1..contacts.len() {
            let (start, end) = (contacts[k - 1], contacts[k]);
            if u < end {
                let d = end - start;
                let q = (u - start) / d;
                return -4.0 * height * (1.0 - 2.0 * q) / d;
            }
            height *= p.restitution * p.restitution;
        }
        0.0
    }
}

impl Default for BounceParams {
    fn default() -> Self {
        BounceParams {
            count: Self::COUNT_DEFAULT,
            first_height: Self::FIRST_HEIGHT.default,
            restitution: Self::RESTITUTION.default,
        }
    }
}

fn sqrt(x: f64) -> f64 {
    // IEEE-754 square root is correctly rounded, hence identical everywhere.
    x.sqrt()
}

// ─────────────────────────── Oscillation ───────────────────────────

/// A wave between the two levels, pinned to the segment's ends.
///
/// With `c = cycles`, `φ = phase` (in cycles), `A = amplitude` and
/// `δ = decay`:
///
/// ```text
/// w(u) = ½ − ½·cos(2π(c·u + φ))             the raw wave, between 0 and 1
/// g(u) = w(u) − (1 − u)·w(0) − u·w(1)        w minus its end-to-end ramp
/// f(u) = u + A·e^(−δu)·g(u)
/// ```
///
/// `g(0) = g(1) = 0`, so **`f(0) = 0` and `f(1) = 1` hold exactly for every
/// parameter value**: the wave rides on a linear ramp and cannot miss the
/// target. With `A = 1`, `δ = 0`, `φ = 0` and a half-integer `c` (0.5, 1.5,
/// …) `f` *is* the pure wave `w` between the two levels; `A = 0` is linear.
/// `δ > 0` damps the wave toward the ramp.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OscillationParams {
    /// Wave cycles across the segment.
    pub cycles: f64,
    /// Wave amplitude; `1` swings between the two levels.
    pub amplitude: f64,
    /// Start phase, in cycles (`0.25` = a quarter turn).
    pub phase: f64,
    /// Decay envelope rate; `0` keeps the full amplitude.
    pub decay: f64,
}

impl OscillationParams {
    pub const CYCLES: Range = Range::new(0.25, 1.5, 20.0);
    pub const AMPLITUDE: Range = Range::new(0.0, 1.0, 2.0);
    pub const PHASE: Range = Range::new(0.0, 0.0, 1.0);
    pub const DECAY: Range = Range::new(0.0, 0.0, 12.0);

    #[must_use]
    pub fn sanitized(self) -> Self {
        OscillationParams {
            cycles: Self::CYCLES.clamp(self.cycles),
            amplitude: Self::AMPLITUDE.clamp(self.amplitude),
            phase: Self::PHASE.clamp(self.phase),
            decay: Self::DECAY.clamp(self.decay),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        Self::CYCLES.check("oscillation cycles", self.cycles)?;
        Self::AMPLITUDE.check("oscillation amplitude", self.amplitude)?;
        Self::PHASE.check("oscillation phase", self.phase)?;
        Self::DECAY.check("oscillation decay", self.decay)
    }

    pub fn progress(self, u: f64) -> f64 {
        if at_or_before_start(u) {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        let params = self.sanitized();
        let (wave_start, wave_end) = (params.wave(0.0), params.wave(1.0));
        let detrended = params.wave(u) - (1.0 - u) * wave_start - u * wave_end;
        u + params.amplitude * exp(-params.decay * u) * detrended
    }

    pub fn slope(self, u: f64) -> f64 {
        let u = unit(u);
        let params = self.sanitized();
        let (wave_start, wave_end) = (params.wave(0.0), params.wave(1.0));
        let detrended = params.wave(u) - (1.0 - u) * wave_start - u * wave_end;
        let angle = TAU * (params.cycles * u + params.phase);
        let d_detrended = PI * params.cycles * sin(angle) + wave_start - wave_end;
        let envelope = exp(-params.decay * u);
        1.0 + params.amplitude * envelope * (d_detrended - params.decay * detrended)
    }

    /// The raw wave `w(x)`, between 0 and 1. Expects sanitized parameters.
    fn wave(&self, x: f64) -> f64 {
        0.5 - 0.5 * cos(TAU * (self.cycles * x + self.phase))
    }
}

impl Default for OscillationParams {
    fn default() -> Self {
        OscillationParams {
            cycles: Self::CYCLES.default,
            amplitude: Self::AMPLITUDE.default,
            phase: Self::PHASE.default,
            decay: Self::DECAY.default,
        }
    }
}

// ────────────────────────── Steps & ramp ──────────────────────────

/// Where a staircase's jumps fall, as in CSS `steps()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JumpMode {
    /// `jump-start`: each interval begins with its jump. The first jump
    /// happens the instant after the left key.
    Start,
    /// `jump-end`: each interval ends with its jump. The last jump happens
    /// at the right key.
    #[default]
    End,
    /// `jump-both`: jumps at both ends; `count + 1` jumps and levels.
    Both,
    /// `jump-none`: no jump at either end; `count − 1` jumps (`count ≥ 2`).
    None,
}

impl JumpMode {
    pub const ALL: [JumpMode; 4] = [
        JumpMode::Start,
        JumpMode::End,
        JumpMode::Both,
        JumpMode::None,
    ];

    pub fn label(self) -> &'static str {
        match self {
            JumpMode::Start => "Jump at start",
            JumpMode::End => "Jump at end",
            JumpMode::Both => "Jump at both",
            JumpMode::None => "No end jumps",
        }
    }
}

pub const STEP_COUNT_MIN: u32 = 1;
pub const STEP_COUNT_MAX: u32 = 64;
pub const STEP_COUNT_DEFAULT: u32 = 4;

/// Intervals actually used: `jump-none` needs at least two.
fn effective_count(count: u32, jump: JumpMode) -> u32 {
    let n = count.clamp(STEP_COUNT_MIN, STEP_COUNT_MAX);
    if jump == JumpMode::None { n.max(2) } else { n }
}

/// Index `k` of the interval containing `u ∈ (0, 1)`: the number of
/// `j ∈ 1..=n` with `j/n ≤ u`, where `j/n` is the correctly rounded `f64`
/// quotient. Defining it by comparison (not by `floor(n·u)`) keeps the jump
/// instants identical everywhere and consistent with [`staircase_jumps`].
fn interval_index(u: f64, n: u32) -> u32 {
    let nf = f64::from(n);
    let mut k = (u * nf).floor().clamp(0.0, nf - 1.0) as u32;
    if k + 1 < n && f64::from(k + 1) / nf <= u {
        k += 1;
    }
    if k > 0 && f64::from(k) / nf > u {
        k -= 1;
    }
    k
}

/// The unit staircase, for `u ∈ (0, 1)`.
fn staircase(u: f64, n: u32, jump: JumpMode) -> f64 {
    let k = f64::from(interval_index(u, n));
    let nf = f64::from(n);
    match jump {
        JumpMode::End => k / nf,
        JumpMode::Start => (k + 1.0) / nf,
        JumpMode::Both => (k + 1.0) / (nf + 1.0),
        JumpMode::None => k / (nf - 1.0),
    }
}

/// Jumps of the unit staircase, ascending, positions in `[0, 1]`.
fn staircase_jumps(n: u32, jump: JumpMode) -> Vec<Jump> {
    let nf = f64::from(n);
    let at = |j: u32| f64::from(j) / nf;
    match jump {
        JumpMode::End => (1..=n)
            .map(|j| Jump {
                at: at(j),
                height: 1.0 / nf,
            })
            .collect(),
        JumpMode::Start => (0..n)
            .map(|j| Jump {
                at: at(j),
                height: 1.0 / nf,
            })
            .collect(),
        JumpMode::Both => (0..=n)
            .map(|j| Jump {
                at: at(j),
                height: 1.0 / (nf + 1.0),
            })
            .collect(),
        JumpMode::None => (1..n)
            .map(|j| Jump {
                at: at(j),
                height: 1.0 / (nf - 1.0),
            })
            .collect(),
    }
}

/// A staircase of equal jumps.
///
/// With `n = count` and `k(u)` the interval index (see [`JumpMode`]):
///
/// ```text
/// end:   f = k/n          start: f = (k+1)/n
/// both:  f = (k+1)/(n+1)  none:  f = k/(n−1)      for 0 < u < 1
/// ```
///
/// `f(0) = 0` and `f(1) = 1` always. `hold` is the fraction of the segment
/// at its end for which the final value is held: the staircase is squeezed
/// into `u ∈ [0, 1 − hold]` and `f = 1` after it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StepsParams {
    pub count: u32,
    pub jump: JumpMode,
    /// Fraction of the segment spent holding the last value.
    pub hold: f64,
}

impl StepsParams {
    pub const HOLD: Range = Range::new(0.0, 0.0, 0.9);

    #[must_use]
    pub fn sanitized(self) -> Self {
        StepsParams {
            count: effective_count(self.count, self.jump),
            jump: self.jump,
            hold: Self::HOLD.clamp(self.hold),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(STEP_COUNT_MIN..=STEP_COUNT_MAX).contains(&self.count) {
            return Err(format!(
                "step count = {} is outside {STEP_COUNT_MIN}..={STEP_COUNT_MAX}",
                self.count
            ));
        }
        Self::HOLD.check("steps hold", self.hold)
    }

    pub fn progress(self, u: f64) -> f64 {
        if at_or_before_start(u) {
            return 0.0;
        }
        let p = self.sanitized();
        let v = u / (1.0 - p.hold);
        if v >= 1.0 {
            return 1.0;
        }
        staircase(v, p.count, p.jump)
    }

    /// Always `0`: plateaus are flat and the jumps are in [`Self::jumps`].
    pub fn slope(self, _u: f64) -> f64 {
        0.0
    }

    pub fn jumps(self) -> Vec<Jump> {
        let p = self.sanitized();
        let squeeze = 1.0 - p.hold;
        staircase_jumps(p.count, p.jump)
            .into_iter()
            .map(|j| Jump {
                at: j.at * squeeze,
                height: j.height,
            })
            .collect()
    }
}

impl Default for StepsParams {
    fn default() -> Self {
        StepsParams {
            count: STEP_COUNT_DEFAULT,
            jump: JumpMode::End,
            hold: Self::HOLD.default,
        }
    }
}

/// A staircase on a slope: `f = ramp·u + (1 − ramp)·staircase(u)`.
///
/// `ramp = 0` is the pure staircase of [`StepsParams`] (without `hold`),
/// `ramp = 1` is exactly linear. In between each plateau climbs with slope
/// `ramp` and each jump has height `(1 − ramp)·(unit jump)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteppedRampParams {
    pub count: u32,
    /// `0` = pure steps, `1` = linear.
    pub ramp: f64,
    pub jump: JumpMode,
}

impl SteppedRampParams {
    pub const RAMP: Range = Range::new(0.0, 0.5, 1.0);

    #[must_use]
    pub fn sanitized(self) -> Self {
        SteppedRampParams {
            count: effective_count(self.count, self.jump),
            ramp: Self::RAMP.clamp(self.ramp),
            jump: self.jump,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(STEP_COUNT_MIN..=STEP_COUNT_MAX).contains(&self.count) {
            return Err(format!(
                "step count = {} is outside {STEP_COUNT_MIN}..={STEP_COUNT_MAX}",
                self.count
            ));
        }
        Self::RAMP.check("stepped ramp slope", self.ramp)
    }

    pub fn progress(self, u: f64) -> f64 {
        if at_or_before_start(u) {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        let p = self.sanitized();
        p.ramp * u + (1.0 - p.ramp) * staircase(u, p.count, p.jump)
    }

    /// The plateau slope, `ramp`.
    pub fn slope(self, _u: f64) -> f64 {
        self.sanitized().ramp
    }

    pub fn jumps(self) -> Vec<Jump> {
        let p = self.sanitized();
        staircase_jumps(p.count, p.jump)
            .into_iter()
            .map(|j| Jump {
                at: j.at,
                height: j.height * (1.0 - p.ramp),
            })
            .filter(|j| j.height > 0.0)
            .collect()
    }
}

impl Default for SteppedRampParams {
    fn default() -> Self {
        SteppedRampParams {
            count: STEP_COUNT_DEFAULT,
            ramp: Self::RAMP.default,
            jump: JumpMode::End,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyframe::Interp;

    /// Every parametric type over a dense grid of its documented ranges,
    /// plus out-of-range and non-finite parameters (which are sanitized).
    fn all_shapes() -> Vec<Interp> {
        let mut v = Vec::new();
        let span = |r: Range| {
            let bad = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1e300, 1e300];
            let mut xs: Vec<f64> = (0..=4)
                .map(|i| r.min + (r.max - r.min) * f64::from(i) / 4.0)
                .collect();
            xs.push(r.default);
            xs.extend(bad);
            xs
        };
        for &frequency in &span(SpringParams::FREQUENCY) {
            for &damping in &span(SpringParams::DAMPING) {
                for &overshoot in &span(SpringParams::OVERSHOOT) {
                    v.push(Interp::Spring(SpringParams {
                        frequency,
                        damping,
                        overshoot,
                    }));
                }
            }
        }
        for count in [0, 1, 2, 3, 7, 12, 13, u32::MAX] {
            for &first_height in &span(BounceParams::FIRST_HEIGHT) {
                for &restitution in &span(BounceParams::RESTITUTION) {
                    v.push(Interp::Bounce(BounceParams {
                        count,
                        first_height,
                        restitution,
                    }));
                }
            }
        }
        for &cycles in &span(OscillationParams::CYCLES) {
            for &amplitude in &span(OscillationParams::AMPLITUDE) {
                for &phase in &span(OscillationParams::PHASE) {
                    for &decay in &span(OscillationParams::DECAY) {
                        v.push(Interp::Oscillation(OscillationParams {
                            cycles,
                            amplitude,
                            phase,
                            decay,
                        }));
                    }
                }
            }
        }
        for count in [0, 1, 2, 3, 4, 7, 64, 65, u32::MAX] {
            for jump in JumpMode::ALL {
                for &hold in &span(StepsParams::HOLD) {
                    v.push(Interp::Steps(StepsParams { count, jump, hold }));
                }
                for &ramp in &span(SteppedRampParams::RAMP) {
                    v.push(Interp::SteppedRamp(SteppedRampParams { count, ramp, jump }));
                }
            }
        }
        v
    }

    /// Times that stress edges: ends, their neighbours, subnormals, and a
    /// fine uniform grid.
    fn probe_times() -> Vec<f64> {
        let mut t: Vec<f64> = (0..=200).map(|i| f64::from(i) / 200.0).collect();
        t.extend([
            f64::MIN_POSITIVE,
            5e-324,
            1e-9,
            1.0 - 1e-9,
            f64::from_bits(1.0f64.to_bits() - 1),
            -0.0,
            -1.0,
            2.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ]);
        t
    }

    fn progress(i: &Interp, u: f64) -> f64 {
        i.shape_progress(u).unwrap()
    }

    #[test]
    fn endpoints_are_exact_and_values_finite_for_the_whole_grid() {
        let shapes = all_shapes();
        assert!(shapes.len() > 5000, "{}", shapes.len());
        for shape in &shapes {
            assert_eq!(progress(shape, 0.0), 0.0, "{shape:?}");
            assert_eq!(progress(shape, 1.0), 1.0, "{shape:?}");
            assert_eq!(progress(shape, -5.0), 0.0, "{shape:?}");
            assert_eq!(progress(shape, 5.0), 1.0, "{shape:?}");
            assert_eq!(progress(shape, f64::NAN), 0.0, "{shape:?}");
            for u in probe_times() {
                let p = progress(shape, u);
                assert!(p.is_finite(), "{shape:?} at {u}: {p}");
                let s = shape.shape_slope(u).unwrap();
                assert!(s.is_finite(), "{shape:?} slope at {u}: {s}");
            }
            for j in shape.sanitized().shape_jumps() {
                assert!(
                    (0.0..=1.0).contains(&j.at) && j.height > 0.0,
                    "{shape:?} {j:?}"
                );
            }
        }
    }

    #[test]
    fn sanitizing_is_idempotent_and_yields_valid_parameters() {
        for shape in all_shapes() {
            let once = shape.sanitized();
            assert_eq!(once.validate(), Ok(()), "{shape:?}");
            assert_eq!(once.sanitized(), once);
            // Sanitizing never changes what a valid easing evaluates to.
            if shape.validate().is_ok()
                && !matches!(shape, Interp::Steps(_) | Interp::SteppedRamp(_))
            {
                assert_eq!(shape, once);
            }
        }
    }

    #[test]
    fn out_of_range_parameters_are_rejected_by_validation() {
        let bad = [
            Interp::Spring(SpringParams {
                frequency: 0.0,
                ..Default::default()
            }),
            Interp::Spring(SpringParams {
                damping: f64::NAN,
                ..Default::default()
            }),
            Interp::Bounce(BounceParams {
                count: 0,
                ..Default::default()
            }),
            Interp::Bounce(BounceParams {
                restitution: 1.0,
                ..Default::default()
            }),
            Interp::Oscillation(OscillationParams {
                phase: 1.5,
                ..Default::default()
            }),
            Interp::Steps(StepsParams {
                count: 65,
                ..Default::default()
            }),
            Interp::Steps(StepsParams {
                hold: 0.95,
                ..Default::default()
            }),
            Interp::SteppedRamp(SteppedRampParams {
                ramp: -0.1,
                ..Default::default()
            }),
        ];
        for b in bad {
            assert!(b.validate().is_err(), "{b:?}");
        }
        for d in Interp::parametric_defaults() {
            assert_eq!(d.validate(), Ok(()));
        }
    }

    // ── Spring ──

    #[test]
    fn spring_overshoots_dips_and_settles() {
        let s = Interp::Spring(SpringParams::default());
        let samples: Vec<f64> = (0..=1000)
            .map(|i| progress(&s, f64::from(i) / 1000.0))
            .collect();
        let max = samples.iter().copied().fold(f64::MIN, f64::max);
        let min_after_peak = samples
            .iter()
            .copied()
            .skip_while(|&p| p < max)
            .fold(f64::MAX, f64::min);
        assert!(max > 1.1, "rises past the target: {max}");
        assert!(
            min_after_peak < 1.0,
            "then dips back below it: {min_after_peak}"
        );
        // Settled: the oscillation envelope e^(-damping·u) is under 0.1 % by
        // u = 0.99, so the tail is within a few thousandths of the target.
        for &p in &samples[900..] {
            assert!((p - 1.0).abs() < 0.03, "{p}");
        }
        assert_eq!(samples[1000], 1.0);
    }

    #[test]
    fn spring_first_peak_matches_the_closed_form() {
        // Released from rest: first peak 1 + e^(−aπ/ω) at u = π/ω.
        let p = SpringParams {
            frequency: 2.0,
            damping: 8.0,
            overshoot: 0.0,
        };
        let w = TAU * 2.0;
        let peak_u = PI / w;
        let expected = 1.0 + (-8.0 * PI / w).exp();
        // The window changes the value by at most |R| ≈ e^-8.
        assert!((p.progress(peak_u) - expected).abs() < 1e-3);
        assert!(p.slope(peak_u).abs() < 1e-2, "peak has zero speed");
        assert_eq!(p.slope(0.0), 0.0, "released from rest");
        let kick = SpringParams {
            overshoot: 1.0,
            ..p
        };
        assert!((kick.slope(0.0) - 8.0).abs() < 1e-12, "{}", kick.slope(0.0));
        assert!(kick.progress(peak_u * 0.8) > p.progress(peak_u * 0.8));
    }

    #[test]
    fn peak_overshoot_matches_the_closed_form_and_the_curve() {
        // Released from rest: e^(−aπ/ω), up to the end window, whose size is
        // bounded by e^(−a)·√(1 + B²) (documented in design-graphs.md §2.1).
        for (frequency, damping) in [(2.5, 7.0), (4.0, 10.0), (1.0, 5.0), (8.0, 20.0)] {
            let p = SpringParams {
                frequency,
                damping,
                overshoot: 0.0,
            };
            let expected = (-damping * PI / (TAU * frequency)).exp();
            let b = damping / (TAU * frequency);
            let window = (-damping).exp() * (1.0 + b * b).sqrt();
            assert!(
                (p.peak_overshoot() - expected).abs() < window + 1e-3,
                "{p:?}: {} vs {expected}",
                p.peak_overshoot()
            );
        }
        // More launch energy, taller peak; never negative.
        let calm = SpringParams {
            frequency: 2.5,
            damping: 24.0,
            overshoot: 0.0,
        };
        let kicked = SpringParams {
            overshoot: 2.0,
            ..calm
        };
        assert!(calm.peak_overshoot() >= 0.0);
        assert!(kicked.peak_overshoot() > calm.peak_overshoot());
    }

    // ── Bounce ──

    #[test]
    fn bounce_defaults_are_the_classic_ease_out_bounce() {
        // h = ¼, r = ½: the familiar 1/2.75 landing and 0.75 / 0.9375 rebounds.
        let b = BounceParams::default();
        let c = b.contact_times();
        assert_eq!(c.len(), 4);
        assert!((c[0] - 1.0 / 2.75).abs() < 1e-12);
        assert!((c[1] - 2.0 / 2.75).abs() < 1e-12);
        assert!((c[2] - 2.5 / 2.75).abs() < 1e-12);
        assert_eq!(c[3], 1.0);
        let mid = |a: f64, z: f64| b.progress(f64::midpoint(a, z));
        assert!((mid(c[0], c[1]) - 0.75).abs() < 1e-12);
        assert!((mid(c[1], c[2]) - 0.9375).abs() < 1e-12);
        assert!((mid(c[2], c[3]) - 0.984_375).abs() < 1e-12);
    }

    #[test]
    fn bounce_contacts_reach_the_target_and_arcs_follow_the_rule() {
        for count in 1..=BounceParams::COUNT_MAX {
            for h in [0.02, 0.25, 0.6, 1.0] {
                for r in [0.1, 0.5, 0.9] {
                    let b = BounceParams {
                        count,
                        first_height: h,
                        restitution: r,
                    };
                    let c = b.contact_times();
                    assert_eq!(c.len(), count as usize + 1);
                    assert_eq!(*c.last().unwrap(), 1.0);
                    assert!(c.windows(2).all(|w| w[0] < w[1]), "{b:?} {c:?}");
                    for &t in &c[..c.len() - 1] {
                        assert!((b.progress(t) - 1.0).abs() < 1e-9, "{b:?} contact {t}");
                    }
                    // Durations shrink by r, rebound heights by r².
                    for k in 1..c.len() - 1 {
                        // Contact times are accumulated sums, so compare the
                        // durations with an absolute tolerance.
                        let (d0, d1) = (c[k] - c[k - 1], c[k + 1] - c[k]);
                        assert!((d1 - r * d0).abs() < 1e-12, "{b:?}");
                    }
                    let mut height = h;
                    for k in 1..c.len() {
                        let peak = b.progress(f64::midpoint(c[k - 1], c[k]));
                        assert!((peak - (1.0 - height)).abs() < 1e-9, "{b:?} arc {k}");
                        height *= r * r;
                    }
                    for i in 0..=400 {
                        let p = b.progress(f64::from(i) / 400.0);
                        assert!((0.0..=1.0).contains(&p), "{b:?}: {p}");
                    }
                }
            }
        }
    }

    // ── Oscillation ──

    #[test]
    fn oscillation_special_cases() {
        // Pure wave between the two levels at a half-integer cycle count.
        let wave = OscillationParams {
            cycles: 1.5,
            amplitude: 1.0,
            phase: 0.0,
            decay: 0.0,
        };
        for i in 0..=100 {
            let u = f64::from(i) / 100.0;
            let w = 0.5 - 0.5 * cos(TAU * 1.5 * u);
            assert!((wave.progress(u) - w).abs() < 1e-12, "{u}");
        }
        // Zero amplitude is linear.
        let flat = OscillationParams {
            amplitude: 0.0,
            ..wave
        };
        for i in 0..=100 {
            let u = f64::from(i) / 100.0;
            assert!((flat.progress(u) - u).abs() < 1e-15);
        }
        // Decay pulls the wave onto the ramp.
        let damped = OscillationParams {
            decay: 12.0,
            ..wave
        };
        assert!((damped.progress(0.9) - 0.9).abs() < 1e-3);
        // Phase shifts the wave but never the ends.
        for phase in [0.1, 0.25, 0.5, 0.9] {
            let p = OscillationParams { phase, ..wave };
            assert_eq!(p.progress(0.0), 0.0);
            assert_eq!(p.progress(1.0), 1.0);
            assert!(p.progress(0.5).is_finite());
        }
    }

    // ── Steps & stepped ramp ──

    #[test]
    fn steps_have_the_documented_levels_and_jumps() {
        let levels = |n: u32, jump: JumpMode| {
            let p = StepsParams {
                count: n,
                jump,
                hold: 0.0,
            };
            let mut seen: Vec<f64> = (0..=10_000)
                .map(|i| p.progress(f64::from(i) / 10_000.0))
                .collect();
            seen.dedup();
            seen
        };
        for n in [1u32, 2, 3, 5, 8, 64] {
            let nf = f64::from(n);
            assert_eq!(levels(n, JumpMode::End).len() as u32, n + 1, "end {n}");
            assert_eq!(levels(n, JumpMode::Start).len() as u32, n + 1, "start {n}");
            assert_eq!(levels(n, JumpMode::Both).len() as u32, n + 2, "both {n}");
            // jump-none needs two intervals.
            let none = n.max(2);
            assert_eq!(levels(n, JumpMode::None).len() as u32, none, "none {n}");
            assert_eq!(levels(n, JumpMode::End)[1], 1.0 / nf);
        }
        // Jump counts and heights.
        let count = |jump| {
            StepsParams {
                count: 4,
                jump,
                hold: 0.0,
            }
            .jumps()
        };
        assert_eq!(count(JumpMode::End).len(), 4);
        assert_eq!(count(JumpMode::Start).len(), 4);
        assert_eq!(count(JumpMode::Both).len(), 5);
        assert_eq!(count(JumpMode::None).len(), 3);
        assert_eq!(count(JumpMode::End)[0].at, 0.25);
        assert_eq!(
            count(JumpMode::End)[3].at,
            1.0,
            "last end-jump is at the right key"
        );
        assert_eq!(
            count(JumpMode::Start)[0].at,
            0.0,
            "first start-jump is at the left key"
        );
        // Heights sum to the whole change.
        for jump in JumpMode::ALL {
            let total: f64 = count(jump).iter().map(|j| j.height).sum();
            assert!((total - 1.0).abs() < 1e-12, "{jump:?}");
        }
    }

    #[test]
    fn jumps_describe_exactly_where_progress_jumps() {
        for jump in JumpMode::ALL {
            for hold in [0.0, 0.3] {
                for n in [1u32, 2, 5, 9] {
                    let p = StepsParams {
                        count: n,
                        jump,
                        hold,
                    };
                    // The instant is exact for hold = 0; a squeezed instant
                    // (hold > 0) is accurate to a few ulps.
                    let (below, above): (Nudge, Nudge) = if hold == 0.0 {
                        (|x| x.next_down_ish(), |x| x.next_up_ish())
                    } else {
                        (|x| x - 1e-12, |x| x + 1e-12)
                    };
                    for j in p.jumps() {
                        let before = if j.at == 0.0 {
                            0.0
                        } else {
                            p.progress(below(j.at))
                        };
                        let after = if j.at >= 1.0 {
                            1.0
                        } else {
                            p.progress(above(j.at))
                        };
                        assert!(
                            (after - before - j.height).abs() < 1e-9,
                            "{p:?} jump {j:?}: {before} -> {after}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn steps_hold_keeps_the_last_value() {
        let p = StepsParams {
            count: 4,
            jump: JumpMode::End,
            hold: 0.25,
        };
        assert_eq!(p.progress(0.74), 0.75);
        assert_eq!(
            p.progress(0.75),
            1.0,
            "the last jump lands where the hold starts"
        );
        assert_eq!(p.progress(0.99), 1.0);
        assert_eq!(p.jumps().last().unwrap().at, 0.75);
    }

    #[test]
    fn stepped_ramp_interpolates_between_steps_and_linear() {
        let steps = StepsParams {
            count: 5,
            jump: JumpMode::Both,
            hold: 0.0,
        };
        let none = SteppedRampParams {
            count: 5,
            ramp: 0.0,
            jump: JumpMode::Both,
        };
        let lin = SteppedRampParams {
            count: 5,
            ramp: 1.0,
            jump: JumpMode::Both,
        };
        for i in 0..=1000 {
            let u = f64::from(i) / 1000.0;
            assert_eq!(none.progress(u), steps.progress(u), "{u}");
            assert!((lin.progress(u) - u).abs() < 1e-15, "{u}");
        }
        assert!(lin.jumps().is_empty(), "no jumps when fully linear");
        let half = SteppedRampParams {
            count: 4,
            ramp: 0.5,
            jump: JumpMode::End,
        };
        assert_eq!(half.slope(0.1), 0.5);
        assert!((half.jumps()[0].height - 0.125).abs() < 1e-15);
        // Plateaus climb: strictly increasing between jumps.
        assert!(half.progress(0.2) < half.progress(0.24));
    }

    // ── Speed ──

    #[test]
    fn slope_matches_a_numerical_derivative_away_from_corners() {
        let shapes = [
            Interp::Spring(SpringParams::default()),
            Interp::Spring(SpringParams {
                frequency: 5.0,
                damping: 3.0,
                overshoot: 1.3,
            }),
            Interp::Oscillation(OscillationParams::default()),
            Interp::Oscillation(OscillationParams {
                cycles: 3.3,
                amplitude: 0.4,
                phase: 0.2,
                decay: 2.0,
            }),
            Interp::Bounce(BounceParams::default()),
            Interp::SteppedRamp(SteppedRampParams::default()),
        ];
        let h = 1e-6;
        for shape in shapes {
            let contacts = match shape {
                Interp::Bounce(b) => b.contact_times(),
                _ => Vec::new(),
            };
            let jumps: Vec<f64> = shape.shape_jumps().iter().map(|j| j.at).collect();
            for i in 1..400 {
                let u = f64::from(i) / 400.0 + 0.000_137;
                if contacts
                    .iter()
                    .chain(&jumps)
                    .any(|c| (c - u).abs() < 2.0 * h)
                {
                    continue;
                }
                let numeric = (progress(&shape, u + h) - progress(&shape, u - h)) / (2.0 * h);
                let analytic = shape.shape_slope(u).unwrap();
                assert!(
                    (numeric - analytic).abs() < 1e-4 * (1.0 + analytic.abs()),
                    "{shape:?} at {u}: numeric {numeric}, analytic {analytic}"
                );
            }
        }
    }

    #[test]
    fn steps_are_flat_between_jumps() {
        assert_eq!(
            Interp::Steps(StepsParams::default()).shape_slope(0.3),
            Some(0.0)
        );
    }

    #[test]
    fn staircase_levels_match_css_steps() {
        let level = |u, n, j| staircase(u, n, j);
        // steps(4, jump-end): 0, ¼, ½, ¾
        assert_eq!(level(0.1, 4, JumpMode::End), 0.0);
        assert_eq!(level(0.25, 4, JumpMode::End), 0.25);
        assert_eq!(level(0.99, 4, JumpMode::End), 0.75);
        // steps(4, jump-start): ¼, ½, ¾, 1
        assert_eq!(level(0.1, 4, JumpMode::Start), 0.25);
        assert_eq!(level(0.99, 4, JumpMode::Start), 1.0);
        // steps(4, jump-both): ⅕ … ⅘
        assert_eq!(level(0.1, 4, JumpMode::Both), 0.2);
        assert_eq!(level(0.99, 4, JumpMode::Both), 0.8);
        // steps(4, jump-none): 0, ⅓, ⅔, 1
        assert_eq!(level(0.1, 4, JumpMode::None), 0.0);
        assert_eq!(level(0.99, 4, JumpMode::None), 1.0);
    }

    #[test]
    fn interval_index_agrees_with_the_jump_instants() {
        for n in 1..=STEP_COUNT_MAX {
            for j in 1..n {
                let at = f64::from(j) / f64::from(n);
                assert_eq!(interval_index(at, n), j, "n={n} j={j}");
                assert_eq!(interval_index(at.next_down_ish(), n), j - 1, "n={n} j={j}");
            }
        }
    }

    type Nudge = fn(f64) -> f64;

    trait Neighbours {
        fn next_up_ish(self) -> f64;
        fn next_down_ish(self) -> f64;
    }

    impl Neighbours for f64 {
        fn next_up_ish(self) -> f64 {
            f64::from_bits(self.to_bits() + 1)
        }
        fn next_down_ish(self) -> f64 {
            f64::from_bits(self.to_bits() - 1)
        }
    }
}
