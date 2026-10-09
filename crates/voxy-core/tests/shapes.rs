//! Parametric easings through the public API: properties, serialization,
//! tracks, spatial paths, and bit-exact determinism.
//!
//! `fixtures/shapes_golden.txt` pins the exact bits of every easing on a grid.
//! It was generated on Linux x86-64; CI runs the same comparison on Windows
//! and both macOS architectures, which is what proves the evaluators are
//! platform independent. Regenerate only for an intentional, documented
//! change: `VOXY_BLESS=1 cargo test -p voxy-core --test shapes`.

#![allow(clippy::unwrap_used)]

use std::fmt::Write as _;

use proptest::prelude::*;
use voxy_core::glam::{DVec2, DVec3};
use voxy_core::shapes::Range;
use voxy_core::{
    BounceParams, Interp, JumpMode, Keyframe, KeyframeTrack, OscillationParams, SpatialTangents,
    SpringParams, SteppedRampParams, StepsParams, Time, TrackError, Value,
};

// ───────────────────────────── helpers ─────────────────────────────

fn key(secs: i64, v: f64, interp: Interp) -> Keyframe {
    let mut k = Keyframe::new(Time::from_secs(secs), Value::Scalar(v));
    k.interp = interp;
    k
}

fn two_keys(interp: Interp) -> KeyframeTrack {
    KeyframeTrack::try_from(vec![key(0, 10.0, interp), key(2, 110.0, Interp::Linear)]).unwrap()
}

fn scalar_at(track: &KeyframeTrack, secs: f64) -> f64 {
    track
        .evaluate(Time::from_secs_f64(secs), false)
        .as_scalar()
        .unwrap()
}

/// A representative easing of each parametric type.
fn samples() -> Vec<Interp> {
    vec![
        Interp::Spring(SpringParams::default()),
        Interp::Spring(SpringParams {
            frequency: 6.0,
            damping: 3.5,
            overshoot: 0.7,
        }),
        Interp::Bounce(BounceParams::default()),
        Interp::Bounce(BounceParams {
            count: 7,
            first_height: 0.6,
            restitution: 0.8,
        }),
        Interp::Oscillation(OscillationParams::default()),
        Interp::Oscillation(OscillationParams {
            cycles: 4.3,
            amplitude: 0.35,
            phase: 0.15,
            decay: 3.0,
        }),
        Interp::Steps(StepsParams::default()),
        Interp::Steps(StepsParams {
            count: 7,
            jump: JumpMode::Both,
            hold: 0.2,
        }),
        Interp::SteppedRamp(SteppedRampParams::default()),
        Interp::SteppedRamp(SteppedRampParams {
            count: 5,
            ramp: 0.25,
            jump: JumpMode::Start,
        }),
    ]
}

// ───────────────────────── track evaluation ─────────────────────────

#[test]
fn values_are_the_mix_of_the_keys_through_the_easing() {
    for interp in samples() {
        let track = two_keys(interp);
        // Exactly the keys' values at the keys (bit for bit).
        assert_eq!(scalar_at(&track, 0.0), 10.0, "{interp:?}");
        assert_eq!(scalar_at(&track, 2.0), 110.0, "{interp:?}");
        for i in 1..200 {
            let u = f64::from(i) / 200.0;
            let p = interp.shape_progress(u).unwrap();
            let expected = 10.0 * (1.0 - p) + 110.0 * p;
            let got = scalar_at(&track, 2.0 * u);
            // Time goes through integer flicks, so u is quantized slightly.
            let (lo, hi) = (
                interp.shape_progress(u - 1e-6).unwrap(),
                interp.shape_progress(u + 1e-6).unwrap(),
            );
            let within = |p: f64| 10.0 * (1.0 - p) + 110.0 * p;
            let (a, b) = (within(lo).min(within(hi)), within(lo).max(within(hi)));
            assert!(
                (got - expected).abs() < 1e-3 || (a - 1e-9..=b + 1e-9).contains(&got),
                "{interp:?} at u={u}: {got} vs {expected}"
            );
        }
    }
}

#[test]
fn spring_overshoots_the_target_value() {
    let track = two_keys(Interp::Spring(SpringParams::default()));
    let max = (0..=400)
        .map(|i| scalar_at(&track, 2.0 * f64::from(i) / 400.0))
        .fold(f64::MIN, f64::max);
    assert!(max > 110.0 + 10.0, "{max}");
    assert_eq!(scalar_at(&track, 99.0), 110.0, "after the last key");
}

#[test]
fn steps_hold_plateaus_and_integers_and_colors_follow() {
    let steps = Interp::Steps(StepsParams {
        count: 4,
        jump: JumpMode::End,
        hold: 0.0,
    });
    let track = two_keys(steps);
    assert_eq!(scalar_at(&track, 0.49), 10.0);
    assert_eq!(scalar_at(&track, 0.51), 35.0);
    assert_eq!(scalar_at(&track, 1.49), 60.0);
    assert_eq!(scalar_at(&track, 1.99), 85.0);

    let mut a = Keyframe::new(Time::ZERO, Value::Int(0));
    a.interp = steps;
    let ints = KeyframeTrack::try_from(vec![a, Keyframe::new(Time::from_secs(2), Value::Int(100))])
        .unwrap();
    assert_eq!(
        ints.evaluate(Time::from_secs_f64(0.51), false),
        Value::Int(25)
    );
}

#[test]
fn spatial_paths_hop_along_the_same_curve_for_steps() {
    let mut a = Keyframe::new(Time::ZERO, Value::Vec2(DVec2::new(0.0, 0.0)));
    a.interp = Interp::Steps(StepsParams {
        count: 4,
        jump: JumpMode::End,
        hold: 0.0,
    });
    a.spatial = Some(SpatialTangents {
        incoming: DVec3::ZERO,
        outgoing: DVec3::new(0.0, 400.0, 0.0),
    });
    let mut b = Keyframe::new(Time::from_secs(1), Value::Vec2(DVec2::new(400.0, 0.0)));
    b.spatial = Some(SpatialTangents {
        incoming: DVec3::new(0.0, 400.0, 0.0),
        outgoing: DVec3::ZERO,
    });
    let track = KeyframeTrack::try_from(vec![a, b]).unwrap();
    let path = track.segment_path(0).unwrap();
    // Every plateau sits exactly on the motion path at its arc-length
    // fraction; between jumps the position does not move.
    for (secs, fraction) in [(0.1, 0.0), (0.3, 0.25), (0.6, 0.5), (0.9, 0.75)] {
        let at = track.evaluate(Time::from_secs_f64(secs), true);
        let expected = path.point_at_length_fraction(fraction);
        let Value::Vec2(v) = at else { panic!() };
        assert!((v.extend(0.0) - expected).length() < 1e-9, "{secs}: {v}");
    }
    assert_eq!(
        track.evaluate(Time::from_secs_f64(0.30), true),
        track.evaluate(Time::from_secs_f64(0.45), true)
    );
}

#[test]
fn segment_queries_expose_speed_and_jumps() {
    let steps = two_keys(Interp::SteppedRamp(SteppedRampParams {
        count: 4,
        ramp: 0.5,
        jump: JumpMode::End,
    }));
    assert_eq!(steps.segment_slope(0, 0.3), Some(0.5));
    let jumps = steps.segment_jumps(0);
    assert_eq!(jumps.len(), 4);
    assert!((jumps[0].height - 0.125).abs() < 1e-15);
    // Legacy modes: unchanged answers, no jumps.
    let linear = two_keys(Interp::Linear);
    assert_eq!(linear.segment_slope(0, 0.3), Some(1.0));
    assert!(linear.segment_jumps(0).is_empty());
    assert_eq!(two_keys(Interp::Hold).segment_slope(0, 0.3), None);
}

#[test]
fn evaluating_the_same_time_twice_is_bit_identical() {
    for interp in samples() {
        let track = two_keys(interp);
        for i in 0..=300 {
            let t = Time(i * 2_352_000);
            let (x, y) = (track.evaluate(t, false), track.evaluate(t, false));
            assert_eq!(x, y);
            let (Value::Scalar(x), Value::Scalar(y)) = (x, y) else {
                panic!()
            };
            assert_eq!(x.to_bits(), y.to_bits());
        }
    }
}

// ───────────────────────── serialization ─────────────────────────

#[test]
fn json_shape_and_legacy_compatibility() {
    // Legacy modes serialize exactly as before.
    for (interp, text) in [
        (Interp::Hold, "\"hold\""),
        (Interp::Linear, "\"linear\""),
        (Interp::Bezier, "\"bezier\""),
    ] {
        assert_eq!(serde_json::to_string(&interp).unwrap(), text);
        assert_eq!(serde_json::from_str::<Interp>(text).unwrap(), interp);
    }
    // Parametric modes are externally tagged objects.
    for interp in samples() {
        let json = serde_json::to_string(&interp).unwrap();
        assert_eq!(
            serde_json::from_str::<Interp>(&json).unwrap(),
            interp,
            "{json}"
        );
    }
    assert_eq!(
        serde_json::to_string(&Interp::Steps(StepsParams::default())).unwrap(),
        r#"{"steps":{"count":4,"jump":"end","hold":0.0}}"#
    );
    // Missing fields take their defaults; unknown easings are an error.
    assert_eq!(
        serde_json::from_str::<Interp>(r#"{"spring":{"damping":10.0}}"#).unwrap(),
        Interp::Spring(SpringParams {
            damping: 10.0,
            ..Default::default()
        })
    );
    assert!(serde_json::from_str::<Interp>(r#"{"wobble":{}}"#).is_err());
    // An `Interp` read on its own (a preset file, say) is validated too.
    for bad in [
        r#"{"steps":{"hold":1.5631816003011821}}"#,
        r#"{"spring":{"frequency":0.0}}"#,
        r#"{"bounce":{"count":0}}"#,
        r#"{"stepped_ramp":{"ramp":2.0}}"#,
    ] {
        assert!(serde_json::from_str::<Interp>(bad).is_err(), "{bad}");
    }
    assert!(serde_json::from_str::<Interp>(r#"{"steps":{"jump":"middle"}}"#).is_err());
}

#[test]
fn out_of_range_parameters_are_rejected_when_a_track_loads() {
    let mut k = key(0, 0.0, Interp::Spring(SpringParams::default()));
    let good = vec![k.clone(), key(1, 1.0, Interp::Linear)];
    assert!(KeyframeTrack::try_from(good).is_ok());
    k.interp = Interp::Bounce(BounceParams {
        count: 99,
        ..Default::default()
    });
    let err = KeyframeTrack::try_from(vec![k, key(1, 1.0, Interp::Linear)]).unwrap_err();
    assert!(matches!(err, TrackError::InvalidEase { .. }), "{err}");
    assert!(err.to_string().contains("bounce count"), "{err}");

    // Through the JSON of a whole track, with a non-finite number spelled
    // out (JSON has no NaN; a too-large number is the closest thing).
    let json = r#"[{"time":0,"value":{"scalar":0.0},"interp":{"spring":{"frequency":1e400}}},
                   {"time":705600000,"value":{"scalar":1.0}}]"#;
    assert!(serde_json::from_str::<KeyframeTrack>(json).is_err());
}

// ───────────────────────── property tests ─────────────────────────

fn any_param(r: Range) -> impl Strategy<Value = f64> {
    prop_oneof![
        8 => r.min..=r.max,
        1 => Just(r.min),
        1 => Just(r.max),
        // Out of range and non-finite values must still be safe.
        1 => prop::num::f64::ANY,
    ]
}

fn any_jump() -> impl Strategy<Value = JumpMode> {
    prop::sample::select(JumpMode::ALL.to_vec())
}

fn any_shape() -> impl Strategy<Value = Interp> {
    prop_oneof![
        (
            any_param(SpringParams::FREQUENCY),
            any_param(SpringParams::DAMPING),
            any_param(SpringParams::OVERSHOOT)
        )
            .prop_map(
                |(frequency, damping, overshoot)| Interp::Spring(SpringParams {
                    frequency,
                    damping,
                    overshoot
                })
            ),
        (
            any::<u32>(),
            any_param(BounceParams::FIRST_HEIGHT),
            any_param(BounceParams::RESTITUTION)
        )
            .prop_map(
                |(count, first_height, restitution)| Interp::Bounce(BounceParams {
                    count,
                    first_height,
                    restitution
                })
            ),
        (
            any_param(OscillationParams::CYCLES),
            any_param(OscillationParams::AMPLITUDE),
            any_param(OscillationParams::PHASE),
            any_param(OscillationParams::DECAY)
        )
            .prop_map(|(cycles, amplitude, phase, decay)| Interp::Oscillation(
                OscillationParams {
                    cycles,
                    amplitude,
                    phase,
                    decay
                }
            )),
        (any::<u32>(), any_jump(), any_param(StepsParams::HOLD))
            .prop_map(|(count, jump, hold)| Interp::Steps(StepsParams { count, jump, hold })),
        (any::<u32>(), any_param(SteppedRampParams::RAMP), any_jump()).prop_map(
            |(count, ramp, jump)| Interp::SteppedRamp(SteppedRampParams { count, ramp, jump })
        ),
    ]
}

proptest! {
    /// For any parameters at all, the endpoints are exact and every value is
    /// finite, at any time.
    #[test]
    fn endpoints_and_finiteness(shape in any_shape(), u in prop::num::f64::ANY) {
        prop_assert_eq!(shape.shape_progress(0.0), Some(0.0));
        prop_assert_eq!(shape.shape_progress(1.0), Some(1.0));
        prop_assert!(shape.shape_progress(u).unwrap().is_finite());
        prop_assert!(shape.shape_slope(u).unwrap().is_finite());
        let clean = shape.sanitized();
        prop_assert!(clean.validate().is_ok());
        // The sanitized easing is what is evaluated.
        prop_assert_eq!(
            clean.shape_progress(0.37).unwrap().to_bits(),
            shape.shape_progress(0.37).unwrap().to_bits()
        );
    }

    /// Time stays monotonic through a segment: the normalized time of a later
    /// instant is never earlier, and a given instant always maps to the same
    /// progress. Staircases never decrease; nothing runs backwards in time.
    #[test]
    fn time_is_monotonic_and_stairs_never_decrease(
        shape in any_shape(),
        a in 0i64..1_000_000,
        b in 0i64..1_000_000,
    ) {
        let track = KeyframeTrack::try_from(vec![
            Keyframe { interp: shape.sanitized(), ..Keyframe::new(Time(0), Value::Scalar(0.0)) },
            Keyframe::new(Time(1_000_000), Value::Scalar(1.0)),
        ]).unwrap();
        let (lo, hi) = (a.min(b), a.max(b));
        let u = |t: i64| track.segment_at(Time(t)).map_or(if t <= 0 { 0.0 } else { 1.0 }, |(_, u)| u);
        prop_assert!(u(lo) <= u(hi));
        let (plo, phi) = (
            shape.shape_progress(u(lo)).unwrap(),
            shape.shape_progress(u(hi)).unwrap(),
        );
        if matches!(shape, Interp::Steps(_) | Interp::SteppedRamp(_)) {
            prop_assert!(plo <= phi, "{plo} > {phi}");
        }
        if matches!(shape, Interp::Bounce(_)) {
            prop_assert!((0.0..=1.0).contains(&plo));
        }
        prop_assert_eq!(
            track.evaluate(Time(lo), false),
            track.evaluate(Time(lo), false)
        );
    }

    /// Jumps are ascending, positive and inside the segment.
    #[test]
    fn jumps_are_well_formed(shape in any_shape()) {
        let jumps = shape.sanitized().shape_jumps();
        prop_assert!(jumps.windows(2).all(|w| w[0].at <= w[1].at));
        for j in jumps {
            prop_assert!(j.height > 0.0 && (0.0..=1.0).contains(&j.at));
        }
    }

    /// Parameters survive a JSON round trip exactly.
    #[test]
    fn json_round_trips(shape in any_shape()) {
        let clean = shape.sanitized();
        let json = serde_json::to_string(&clean).unwrap();
        prop_assert_eq!(serde_json::from_str::<Interp>(&json).unwrap(), clean);
    }
}

const ALPHABET: &[u8] = b" 0123456789.-eE{}[]\":,truefalsnul";

/// The stable-Rust twin of `fuzz/fuzz_targets/ease_json.rs`: damaged easing
/// JSON is rejected or, when accepted, satisfies every guarantee.
fn damaged_json() -> impl Strategy<Value = String> {
    (
        any_shape(),
        prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..4),
    )
        .prop_map(|(shape, edits)| {
            let mut bytes = serde_json::to_string(&shape.sanitized())
                .unwrap()
                .into_bytes();
            for (at, byte) in edits {
                let i = at.index(bytes.len());
                // Keep it printable so edits are more often *almost* valid.
                bytes[i] = ALPHABET[usize::from(byte) % ALPHABET.len()];
            }
            String::from_utf8(bytes).unwrap()
        })
}

proptest! {
    #[test]
    fn damaged_easing_json_never_breaks_the_guarantees(text in damaged_json()) {
        if let Ok(interp) = serde_json::from_str::<Interp>(&text) {
            prop_assert!(interp.validate().is_ok(), "{text}");
            if interp.is_parametric() {
                prop_assert_eq!(interp.shape_progress(0.0), Some(0.0));
                prop_assert_eq!(interp.shape_progress(1.0), Some(1.0));
                for i in 0..=32 {
                    let u = f64::from(i) / 32.0;
                    prop_assert!(interp.shape_progress(u).unwrap().is_finite());
                }
            }
        }
    }
}

// ───────────────────────── determinism golden ─────────────────────────

const FIXTURE: &str = "tests/fixtures/shapes_golden.txt";

fn golden_table() -> String {
    let mut out = String::new();
    for (n, interp) in samples().into_iter().enumerate() {
        for i in 0..=512 {
            let u = f64::from(i) / 512.0;
            writeln!(
                out,
                "{n} {} {:016x} {:016x}",
                i,
                interp.shape_progress(u).unwrap().to_bits(),
                interp.shape_slope(u).unwrap().to_bits()
            )
            .unwrap();
        }
    }
    out
}

#[test]
fn every_easing_is_bit_identical_to_the_pinned_table() {
    let actual = golden_table();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    if std::env::var_os("VOXY_BLESS").is_some() {
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    if actual != expected {
        let first = actual.lines().zip(expected.lines()).find(|(a, e)| a != e);
        panic!("easing output changed; first difference: {first:?}");
    }
}
