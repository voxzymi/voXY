//! Parameters of the parametric easings as the editor presents them: one
//! table drives the numeric fields, their tooltips, and the two on-graph
//! handles.
//!
//! # Provisional handle mapping
//!
//! **Which parameter each handle controls ([`handle_layout`]) is a
//! proposal, not a confirmed design.** It is based
//! on the project owner's reading of a promotional image (yellow *main*
//! handle, white *secondary* handle) and has not been confirmed by anything
//! else. Everything that depends on the mapping lives in [`handle_layout`]
//! alone; changing the mapping means editing that one function (and the
//! table in `docs/design-graphs.md` §4). [`HANDLE_MAPPING_PROVISIONAL`] is
//! shown in the UI tooltip while it is unconfirmed.

use voxy_core::shapes::{Range, STEP_COUNT_MAX, STEP_COUNT_MIN};
use voxy_core::{
    BounceParams, Interp, InterpKind, OscillationParams, SpringParams, SteppedRampParams,
    StepsParams,
};

/// `true` while the handle mapping is an unconfirmed design choice that may
/// still change.
pub const HANDLE_MAPPING_PROVISIONAL: bool = true;

/// One editable number of an easing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamId {
    SpringFrequency,
    SpringDamping,
    SpringOvershoot,
    BounceCount,
    BounceFirstHeight,
    BounceRestitution,
    OscCycles,
    OscAmplitude,
    OscPhase,
    OscDecay,
    StepsCount,
    StepsHold,
    RampCount,
    RampSlope,
}

/// How a parameter is shown and edited.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamDesc {
    pub id: ParamId,
    pub label: &'static str,
    pub tooltip: &'static str,
    pub min: f64,
    pub max: f64,
    /// Whole numbers only (counts).
    pub integer: bool,
    /// Handle positions follow `ln` of the value (wide, decay-like ranges).
    pub log: bool,
    /// Shown value = stored value × `scale`.
    pub scale: f64,
    pub suffix: &'static str,
    pub decimals: usize,
}

fn range(r: Range) -> (f64, f64) {
    (r.min, r.max)
}

/// Descriptor of `id`. Ranges come straight from `voxy-core`, so the editor
/// can never offer a value the model rejects.
#[allow(clippy::too_many_lines)] // one table
pub fn desc(id: ParamId) -> ParamDesc {
    let d = |label, tooltip, (min, max): (f64, f64)| ParamDesc {
        id,
        label,
        tooltip,
        min,
        max,
        integer: false,
        log: false,
        scale: 1.0,
        suffix: "",
        decimals: 2,
    };
    let count = |label, tooltip| ParamDesc {
        integer: true,
        decimals: 0,
        ..d(
            label,
            tooltip,
            (f64::from(STEP_COUNT_MIN), f64::from(STEP_COUNT_MAX)),
        )
    };
    let percent = |p: ParamDesc| ParamDesc {
        scale: 100.0,
        suffix: "%",
        decimals: 0,
        ..p
    };
    match id {
        ParamId::SpringFrequency => d(
            "Frequency",
            "How many full oscillations the spring makes across the segment.",
            range(SpringParams::FREQUENCY),
        ),
        ParamId::SpringDamping => ParamDesc {
            log: true,
            ..d(
                "Damping",
                "How fast the oscillation dies out. Higher settles sooner; the swings shrink by e^(−damping) over the segment.",
                range(SpringParams::DAMPING),
            )
        },
        ParamId::SpringOvershoot => d(
            "Overshoot (launch energy)",
            "Extra energy the spring is launched with. 0 releases it from rest; higher values hit the target faster and swing further past it. The peak overshoot this produces is shown below.",
            range(SpringParams::OVERSHOOT),
        ),
        ParamId::BounceCount => ParamDesc {
            integer: true,
            decimals: 0,
            ..d(
                "Bounces",
                "How many times the ball rebounds after first landing.",
                (
                    f64::from(BounceParams::COUNT_MIN),
                    f64::from(BounceParams::COUNT_MAX),
                ),
            )
        },
        ParamId::BounceFirstHeight => percent(d(
            "First bounce height",
            "How high the first rebound goes, as a share of the whole drop.",
            range(BounceParams::FIRST_HEIGHT),
        )),
        ParamId::BounceRestitution => percent(d(
            "Bounciness",
            "How much speed the ball keeps at each bounce. Higher means the bounces last longer; each one is this share as tall as the one before, squared.",
            range(BounceParams::RESTITUTION),
        )),
        ParamId::OscCycles => d(
            "Cycles",
            "How many full waves fit in the segment. Half-integers (0.5, 1.5, …) swing exactly between the two values.",
            range(OscillationParams::CYCLES),
        ),
        ParamId::OscAmplitude => percent(d(
            "Amplitude",
            "Height of the wave. 100% swings between the two values; 0% is a straight line.",
            range(OscillationParams::AMPLITUDE),
        )),
        ParamId::OscPhase => d(
            "Phase",
            "Where in the wave the segment starts, in cycles (0.25 is a quarter turn). The segment still begins and ends exactly on its two values.",
            range(OscillationParams::PHASE),
        ),
        ParamId::OscDecay => d(
            "Decay",
            "How quickly the wave fades toward a straight line. 0 keeps the full amplitude.",
            range(OscillationParams::DECAY),
        ),
        ParamId::StepsCount => count("Steps", "Number of equal steps between the two values."),
        ParamId::StepsHold => percent(d(
            "Hold last value",
            "Share of the segment, at its end, that holds the final value. The steps are squeezed into the rest.",
            range(StepsParams::HOLD),
        )),
        ParamId::RampCount => count("Steps", "Number of steps the staircase has."),
        ParamId::RampSlope => percent(d(
            "Ramp",
            "0% is pure steps, 100% is a straight line. In between, every plateau climbs a little.",
            range(SteppedRampParams::RAMP),
        )),
    }
}

/// The fields shown for a kind, in order.
pub fn fields(kind: InterpKind) -> &'static [ParamId] {
    match kind {
        InterpKind::Spring => &[
            ParamId::SpringFrequency,
            ParamId::SpringDamping,
            ParamId::SpringOvershoot,
        ],
        InterpKind::Bounce => &[
            ParamId::BounceCount,
            ParamId::BounceFirstHeight,
            ParamId::BounceRestitution,
        ],
        InterpKind::Oscillation => &[
            ParamId::OscCycles,
            ParamId::OscAmplitude,
            ParamId::OscPhase,
            ParamId::OscDecay,
        ],
        InterpKind::Steps => &[ParamId::StepsCount, ParamId::StepsHold],
        InterpKind::SteppedRamp => &[ParamId::RampCount, ParamId::RampSlope],
        InterpKind::Hold | InterpKind::Linear | InterpKind::Bezier => &[],
    }
}

/// The value of `id` in `interp`, or `None` if `interp` has no such
/// parameter.
pub fn get(interp: &Interp, id: ParamId) -> Option<f64> {
    Some(match (interp, id) {
        (Interp::Spring(p), ParamId::SpringFrequency) => p.frequency,
        (Interp::Spring(p), ParamId::SpringDamping) => p.damping,
        (Interp::Spring(p), ParamId::SpringOvershoot) => p.overshoot,
        (Interp::Bounce(p), ParamId::BounceCount) => f64::from(p.count),
        (Interp::Bounce(p), ParamId::BounceFirstHeight) => p.first_height,
        (Interp::Bounce(p), ParamId::BounceRestitution) => p.restitution,
        (Interp::Oscillation(p), ParamId::OscCycles) => p.cycles,
        (Interp::Oscillation(p), ParamId::OscAmplitude) => p.amplitude,
        (Interp::Oscillation(p), ParamId::OscPhase) => p.phase,
        (Interp::Oscillation(p), ParamId::OscDecay) => p.decay,
        (Interp::Steps(p), ParamId::StepsCount) => f64::from(p.count),
        (Interp::Steps(p), ParamId::StepsHold) => p.hold,
        (Interp::SteppedRamp(p), ParamId::RampCount) => f64::from(p.count),
        (Interp::SteppedRamp(p), ParamId::RampSlope) => p.ramp,
        _ => return None,
    })
}

/// `interp` with `id` set to `value`, clamped into the documented range
/// (counts are rounded). Unchanged if `interp` has no such parameter.
#[must_use]
pub fn with(interp: Interp, id: ParamId, value: f64) -> Interp {
    let d = desc(id);
    let v = if value.is_finite() {
        value.clamp(d.min, d.max)
    } else {
        return interp;
    };
    let v = if d.integer { v.round() } else { v };
    let count = v as u32;
    let mut out = interp;
    match (&mut out, id) {
        (Interp::Spring(p), ParamId::SpringFrequency) => p.frequency = v,
        (Interp::Spring(p), ParamId::SpringDamping) => p.damping = v,
        (Interp::Spring(p), ParamId::SpringOvershoot) => p.overshoot = v,
        (Interp::Bounce(p), ParamId::BounceCount) => p.count = count,
        (Interp::Bounce(p), ParamId::BounceFirstHeight) => p.first_height = v,
        (Interp::Bounce(p), ParamId::BounceRestitution) => p.restitution = v,
        (Interp::Oscillation(p), ParamId::OscCycles) => p.cycles = v,
        (Interp::Oscillation(p), ParamId::OscAmplitude) => p.amplitude = v,
        (Interp::Oscillation(p), ParamId::OscPhase) => p.phase = v,
        (Interp::Oscillation(p), ParamId::OscDecay) => p.decay = v,
        (Interp::Steps(p), ParamId::StepsCount) => p.count = count,
        (Interp::Steps(p), ParamId::StepsHold) => p.hold = v,
        (Interp::SteppedRamp(p), ParamId::RampCount) => p.count = count,
        (Interp::SteppedRamp(p), ParamId::RampSlope) => p.ramp = v,
        _ => {}
    }
    out.sanitized()
}

/// `value` as a position in `0..=1` along its range.
pub fn to_unit(d: &ParamDesc, value: f64) -> f64 {
    let v = value.clamp(d.min, d.max);
    let n = if d.log {
        (v / d.min).ln() / (d.max / d.min).ln()
    } else {
        (v - d.min) / (d.max - d.min)
    };
    n.clamp(0.0, 1.0)
}

/// The inverse of [`to_unit`] (before rounding counts).
pub fn from_unit(d: &ParamDesc, n: f64) -> f64 {
    let n = n.clamp(0.0, 1.0);
    if d.log {
        d.min * (d.max / d.min).powf(n)
    } else {
        d.min + n * (d.max - d.min)
    }
}

// ───────────────────────── on-graph handles ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleRole {
    /// The yellow handle: the principal parameters.
    Main,
    /// The white handle: the secondary parameter.
    Secondary,
}

/// What one handle controls. A handle sits in the graph's unit square
/// (`u` across, progress up). A mapped axis follows its parameter over the
/// parameter's whole range; an unmapped axis rests at a fixed place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HandleSpec {
    pub role: HandleRole,
    /// Parameter driven by dragging left and right.
    pub x: Option<ParamId>,
    /// Parameter driven by dragging up and down.
    pub y: Option<ParamId>,
    /// Where an unmapped axis rests, as `(u, progress)`.
    pub rest: (f64, f64),
}

/// Rest position for a handle that only moves horizontally: main handles
/// ride the top edge of the square, secondary ones just below the baseline.
const MAIN_X_ONLY_Y: f64 = 1.0;
const SECONDARY_X_ONLY_Y: f64 = -0.075;
/// Rest position for a handle that only moves vertically.
const Y_ONLY_X: f64 = 1.0;

/// **PROVISIONAL.** The handles of each parametric kind. See the module
/// documentation; this is the only place that decides the mapping.
///
/// | Kind | Main (yellow) | Secondary (white) |
/// |---|---|---|
/// | Spring | x: damping, y: overshoot | x: frequency |
/// | Bounce | x: bounciness, y: first height | x: bounces |
/// | Oscillation | x: decay, y: amplitude | x: cycles |
/// | Steps | x: steps | x: hold |
/// | Stepped ramp | x: steps | y: ramp |
pub fn handle_layout(kind: InterpKind) -> Vec<HandleSpec> {
    let main_2d = |x, y| HandleSpec {
        role: HandleRole::Main,
        x: Some(x),
        y: Some(y),
        rest: (0.0, 0.0),
    };
    let main_x = |x| HandleSpec {
        role: HandleRole::Main,
        x: Some(x),
        y: None,
        rest: (0.0, MAIN_X_ONLY_Y),
    };
    let second_x = |x| HandleSpec {
        role: HandleRole::Secondary,
        x: Some(x),
        y: None,
        rest: (0.0, SECONDARY_X_ONLY_Y),
    };
    let second_y = |y| HandleSpec {
        role: HandleRole::Secondary,
        x: None,
        y: Some(y),
        rest: (Y_ONLY_X, 0.0),
    };
    match kind {
        InterpKind::Spring => vec![
            main_2d(ParamId::SpringDamping, ParamId::SpringOvershoot),
            second_x(ParamId::SpringFrequency),
        ],
        InterpKind::Bounce => vec![
            main_2d(ParamId::BounceRestitution, ParamId::BounceFirstHeight),
            second_x(ParamId::BounceCount),
        ],
        InterpKind::Oscillation => vec![
            main_2d(ParamId::OscDecay, ParamId::OscAmplitude),
            second_x(ParamId::OscCycles),
        ],
        InterpKind::Steps => vec![main_x(ParamId::StepsCount), second_x(ParamId::StepsHold)],
        InterpKind::SteppedRamp => vec![main_x(ParamId::RampCount), second_y(ParamId::RampSlope)],
        InterpKind::Hold | InterpKind::Linear | InterpKind::Bezier => Vec::new(),
    }
}

/// Where `spec`'s handle sits for `interp`, as `(u, progress)`.
pub fn handle_position(interp: &Interp, spec: &HandleSpec) -> (f64, f64) {
    let axis = |id: Option<ParamId>, rest: f64| {
        id.and_then(|id| get(interp, id).map(|v| to_unit(&desc(id), v)))
            .unwrap_or(rest)
    };
    (axis(spec.x, spec.rest.0), axis(spec.y, spec.rest.1))
}

/// `interp` after dragging `spec`'s handle to `(u, progress)`. Parameters
/// are clamped to their ranges, so the result is always valid.
#[must_use]
pub fn drag_handle(interp: Interp, spec: &HandleSpec, (u, p): (f64, f64)) -> Interp {
    let mut out = interp;
    for (id, n) in [(spec.x, u), (spec.y, p)] {
        if let Some(id) = id {
            let d = desc(id);
            out = with(out, id, from_unit(&d, n));
        }
    }
    out
}

/// Tooltip for a handle: what it controls, and the provisional caveat.
pub fn handle_tooltip(spec: &HandleSpec) -> String {
    let name = |id: Option<ParamId>| id.map(|id| desc(id).label);
    let who = match spec.role {
        HandleRole::Main => "Main handle (yellow)",
        HandleRole::Secondary => "Secondary handle (white)",
    };
    let what = match (name(spec.x), name(spec.y)) {
        (Some(x), Some(y)) => format!("left/right: {x}; up/down: {y}"),
        (Some(x), None) => format!("left/right: {x}"),
        (None, Some(y)) => format!("up/down: {y}"),
        (None, None) => String::new(),
    };
    let mut text = format!("{who}. Drag — {what}. Double-click to reset it.");
    if HANDLE_MAPPING_PROVISIONAL {
        text.push_str("\nWhich parameter each handle controls is provisional and may change.");
    }
    text
}

/// `value` as the field shows it: scaled, with its decimals and suffix.
pub fn format_value(d: &ParamDesc, value: f64) -> String {
    format!("{:.*}{}", d.decimals, value * d.scale, d.suffix)
}

/// The current values of the parameters a handle drives, for the live
/// read-out beside it: `"Damping 7.00, Overshoot (launch energy) 0.00"`.
pub fn handle_readout(interp: &Interp, spec: &HandleSpec) -> String {
    [spec.x, spec.y]
        .into_iter()
        .flatten()
        .filter_map(|id| {
            let d = desc(id);
            get(interp, id).map(|v| format!("{} {}", d.label, format_value(&d, v)))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `interp` with the parameters `spec`'s handle drives set back to the
/// defaults of its kind; everything else is untouched.
#[must_use]
pub fn reset_handle(interp: Interp, spec: &HandleSpec) -> Interp {
    let defaults = interp.kind().default_interp();
    [spec.x, spec.y]
        .into_iter()
        .flatten()
        .fold(interp, |acc, id| match get(&defaults, id) {
            Some(v) => with(acc, id, v),
            None => acc,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_ids() -> Vec<ParamId> {
        InterpKind::PARAMETRIC
            .iter()
            .flat_map(|k| fields(*k).iter().copied())
            .collect()
    }

    #[test]
    fn every_parameter_is_in_exactly_one_kinds_field_list() {
        let ids = all_ids();
        assert_eq!(ids.len(), 14);
        let mut sorted = ids.clone();
        sorted.sort_by_key(|i| format!("{i:?}"));
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
        for kind in InterpKind::PARAMETRIC {
            let interp = kind.default_interp();
            for &id in fields(kind) {
                assert!(get(&interp, id).is_some(), "{kind:?} lacks {id:?}");
            }
        }
    }

    #[test]
    fn ranges_match_the_model_and_defaults_are_inside() {
        for kind in InterpKind::PARAMETRIC {
            let interp = kind.default_interp();
            for &id in fields(kind) {
                let d = desc(id);
                let v = get(&interp, id).unwrap();
                assert!((d.min..=d.max).contains(&v), "{id:?}: {v}");
                // The ends of the range are valid for the model.
                for end in [d.min, d.max] {
                    assert_eq!(with(interp, id, end).validate(), Ok(()), "{id:?}={end}");
                    assert_eq!(get(&with(interp, id, end), id), Some(end), "{id:?}");
                }
                assert!(!d.tooltip.is_empty() && !d.label.is_empty());
            }
        }
    }

    #[test]
    fn with_clamps_rounds_and_ignores_garbage() {
        let steps = Interp::Steps(StepsParams::default());
        let Interp::Steps(p) = with(steps, ParamId::StepsCount, 1e9) else {
            panic!()
        };
        assert_eq!(p.count, STEP_COUNT_MAX);
        let Interp::Steps(p) = with(steps, ParamId::StepsCount, 3.6) else {
            panic!()
        };
        assert_eq!(p.count, 4);
        assert_eq!(with(steps, ParamId::StepsCount, f64::NAN), steps);
        // A parameter of another kind changes nothing.
        assert_eq!(with(steps, ParamId::SpringDamping, 3.0), steps);
    }

    #[test]
    fn unit_mapping_round_trips() {
        for id in all_ids() {
            let d = desc(id);
            for i in 0..=10 {
                let n = f64::from(i) / 10.0;
                assert!((to_unit(&d, from_unit(&d, n)) - n).abs() < 1e-12, "{id:?}");
            }
            assert_eq!(from_unit(&d, 0.0), d.min);
            assert!((from_unit(&d, 1.0) - d.max).abs() < 1e-9);
        }
    }

    #[test]
    fn every_parametric_kind_has_a_main_and_a_secondary_handle() {
        for kind in InterpKind::PARAMETRIC {
            let layout = handle_layout(kind);
            assert_eq!(layout.len(), 2, "{kind:?}");
            assert_eq!(layout[0].role, HandleRole::Main);
            assert_eq!(layout[1].role, HandleRole::Secondary);
            // Handles drive parameters of their own kind, and between them
            // each handle drives something.
            for h in &layout {
                assert!(h.x.is_some() || h.y.is_some());
                for id in [h.x, h.y].into_iter().flatten() {
                    assert!(fields(kind).contains(&id), "{kind:?} {id:?}");
                }
            }
            // No parameter is driven by two handles.
            let driven: Vec<_> = layout.iter().flat_map(|h| [h.x, h.y]).flatten().collect();
            for (i, a) in driven.iter().enumerate() {
                assert!(!driven[i + 1..].contains(a), "{kind:?} {a:?}");
            }
        }
        for kind in [InterpKind::Hold, InterpKind::Linear, InterpKind::Bezier] {
            assert_eq!(handle_layout(kind).len(), 0);
        }
    }

    #[test]
    fn dragging_a_handle_lands_where_it_was_dropped() {
        for kind in InterpKind::PARAMETRIC {
            let start = kind.default_interp();
            for spec in handle_layout(kind) {
                for target in [(0.2, 0.3), (0.5, 0.5), (0.9, 0.8), (0.0, 0.0), (1.0, 1.0)] {
                    let moved = drag_handle(start, &spec, target);
                    assert_eq!(moved.validate(), Ok(()));
                    let (u, p) = handle_position(&moved, &spec);
                    // Mapped axes follow the pointer (counts snap).
                    if let Some(id) = spec.x {
                        let tol = if desc(id).integer {
                            0.5 / (desc(id).max - desc(id).min)
                        } else {
                            1e-9
                        };
                        assert!((u - target.0).abs() <= tol + 1e-9, "{kind:?} {id:?}: {u}");
                    } else {
                        assert_eq!(u, spec.rest.0);
                    }
                    if let Some(id) = spec.y {
                        let tol = if desc(id).integer {
                            0.5 / (desc(id).max - desc(id).min)
                        } else {
                            1e-9
                        };
                        assert!((p - target.1).abs() <= tol + 1e-9, "{kind:?} {id:?}: {p}");
                    } else {
                        assert_eq!(p, spec.rest.1);
                    }
                }
                // Far outside the square clamps into range instead of failing.
                let wild = drag_handle(start, &spec, (-40.0, 1e9));
                assert_eq!(wild.validate(), Ok(()));
                assert_eq!(wild.kind(), kind);
            }
        }
    }

    #[test]
    fn dragging_keeps_the_endpoint_guarantees() {
        for kind in InterpKind::PARAMETRIC {
            for spec in handle_layout(kind) {
                for i in 0..=10 {
                    for j in 0..=10 {
                        let t = (f64::from(i) / 10.0, f64::from(j) / 10.0);
                        let moved = drag_handle(kind.default_interp(), &spec, t);
                        assert_eq!(moved.shape_progress(0.0), Some(0.0));
                        assert_eq!(moved.shape_progress(1.0), Some(1.0));
                        assert!(moved.shape_progress(0.5).unwrap().is_finite());
                    }
                }
            }
        }
    }

    #[test]
    fn readout_shows_current_values_as_the_fields_do() {
        let spring = handle_layout(InterpKind::Spring);
        let interp = Interp::Spring(SpringParams {
            frequency: 3.0,
            damping: 9.5,
            overshoot: 0.75,
        });
        assert_eq!(
            handle_readout(&interp, &spring[0]),
            "Damping 9.50, Overshoot (launch energy) 0.75"
        );
        assert_eq!(handle_readout(&interp, &spring[1]), "Frequency 3.00");
        // Percent parameters are scaled like their fields; counts are whole.
        let bounce = Interp::Bounce(BounceParams {
            count: 5,
            first_height: 0.4,
            restitution: 0.65,
        });
        let b = handle_layout(InterpKind::Bounce);
        assert_eq!(
            handle_readout(&bounce, &b[0]),
            "Bounciness 65%, First bounce height 40%"
        );
        assert_eq!(handle_readout(&bounce, &b[1]), "Bounces 5");
        // A handle of another kind reads nothing.
        assert_eq!(handle_readout(&Interp::Linear, &spring[0]), "");
    }

    #[test]
    fn reset_handle_restores_only_what_the_handle_drives() {
        for kind in InterpKind::PARAMETRIC {
            let defaults = kind.default_interp();
            for spec in handle_layout(kind) {
                // Move every parameter away from its default, then reset one handle.
                let mut moved = defaults;
                for &id in fields(kind) {
                    let d = desc(id);
                    let away = if get(&defaults, id) == Some(d.max) {
                        d.min
                    } else {
                        d.max
                    };
                    moved = with(moved, id, away);
                }
                let after = reset_handle(moved, &spec);
                let driven: Vec<ParamId> = [spec.x, spec.y].into_iter().flatten().collect();
                for &id in fields(kind) {
                    let expected = if driven.contains(&id) {
                        &defaults
                    } else {
                        &moved
                    };
                    assert_eq!(get(&after, id), get(expected, id), "{kind:?} {id:?}");
                }
                assert_eq!(after.validate(), Ok(()));
            }
        }
        // Jump mode is not a handle parameter and survives a reset.
        let steps = Interp::Steps(StepsParams {
            count: 9,
            jump: voxy_core::JumpMode::Both,
            hold: 0.5,
        });
        let specs = handle_layout(InterpKind::Steps);
        let Interp::Steps(p) = reset_handle(reset_handle(steps, &specs[0]), &specs[1]) else {
            panic!()
        };
        assert_eq!(
            (p.count, p.jump, p.hold),
            (StepsParams::default().count, voxy_core::JumpMode::Both, 0.0)
        );
    }

    #[test]
    fn tooltips_say_what_each_handle_does_and_that_it_is_provisional() {
        let spring = handle_layout(InterpKind::Spring);
        let main = handle_tooltip(&spring[0]);
        assert!(main.contains("Main handle (yellow)") && main.contains("Damping"));
        assert!(main.contains("Overshoot"));
        assert!(handle_tooltip(&spring[1]).contains("Frequency"));
        assert!(main.contains("provisional") && main.contains("Double-click"));
        assert!(
            !main.contains("Alight"),
            "no stale reference to the cancelled work"
        );
    }
}
