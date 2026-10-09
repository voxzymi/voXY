//! Speed-aware motion paths, end to end through `Property` and `evaluate`.

#![allow(clippy::unwrap_used)]

use voxy_core::command::{
    AddKeyframe, AddSolidLayer, ApplyEasePreset, Command, SetInterpolation, SetProperty,
    SetSpatialTangents,
};
use voxy_core::glam::{DVec2, DVec3};
use voxy_core::motion_path::CubicPath;
use voxy_core::{
    CompSettings, Document, EasePreset, Interp, LinearRgba, PropId, SpatialTangents, Time, Value,
    evaluate,
};

const START: DVec2 = DVec2::new(100.0, 500.0);
const END: DVec2 = DVec2::new(900.0, 500.0);
const OUT_TANGENT: DVec3 = DVec3::new(50.0, -600.0, 0.0);
const IN_TANGENT: DVec3 = DVec3::new(300.0, -100.0, 0.0);

/// A solid whose position animates over 2 s along a strongly curved,
/// unevenly parameterized arc.
fn animated_doc() -> (Document, PropId) {
    let mut doc = Document::with_comp("Motion", CompSettings::default());
    let comp = doc.main_comp.unwrap();
    AddSolidLayer {
        comp,
        name: None,
        color: LinearRgba::WHITE,
        index: 0,
    }
    .apply(&mut doc)
    .unwrap();
    let pos = doc.main().unwrap().layers[0].transform.position.id;
    let t0 = Time::ZERO;
    let t1 = Time::from_secs(2);
    SetProperty {
        prop: pos,
        time: t0,
        value: Value::Vec2(START),
    }
    .apply(&mut doc)
    .unwrap();
    AddKeyframe {
        prop: pos,
        time: t0,
    }
    .apply(&mut doc)
    .unwrap();
    SetProperty {
        prop: pos,
        time: t1,
        value: Value::Vec2(END),
    }
    .apply(&mut doc)
    .unwrap();
    let tangents = |incoming, outgoing| Some(SpatialTangents { incoming, outgoing });
    SetSpatialTangents {
        prop: pos,
        time: t0,
        tangents: tangents(DVec3::ZERO, OUT_TANGENT),
    }
    .apply(&mut doc)
    .unwrap();
    SetSpatialTangents {
        prop: pos,
        time: t1,
        tangents: tangents(IN_TANGENT, DVec3::ZERO),
    }
    .apply(&mut doc)
    .unwrap();
    (doc, pos)
}

fn path() -> CubicPath {
    CubicPath::new(START.extend(0.0), OUT_TANGENT, IN_TANGENT, END.extend(0.0))
}

/// Arc length from the start of the path to the point nearest `p`, found by
/// a fine polyline walk.
fn arc_to(p: DVec2) -> f64 {
    let path = path();
    let n = 20_000;
    let pts: Vec<DVec2> = (0..=n)
        .map(|i| path.point(f64::from(i) / f64::from(n)).truncate())
        .collect();
    let nearest = (0..=n as usize)
        .min_by(|&a, &b| pts[a].distance(p).total_cmp(&pts[b].distance(p)))
        .unwrap();
    pts.windows(2)
        .take(nearest)
        .map(|w| w[0].distance(w[1]))
        .sum()
}

fn position_at(doc: &Document, pos: PropId, time: Time) -> DVec2 {
    doc.property(pos).unwrap().value_at(time).as_vec2().unwrap()
}

#[test]
fn linear_timing_moves_at_constant_speed_along_the_curve() {
    let (doc, pos) = animated_doc();
    let total = path().length();
    for i in 0..=20 {
        let time = Time::from_millis(i * 100);
        let travelled = arc_to(position_at(&doc, pos, time));
        let expected = total * (i as f64 / 20.0);
        // Polyline resolution limits accuracy to well under a pixel.
        assert!(
            (travelled - expected).abs() < 0.2,
            "t={time}: {travelled} vs {expected}"
        );
    }
}

#[test]
fn eased_timing_follows_the_ease_curve_along_the_path() {
    let (mut doc, pos) = animated_doc();
    ApplyEasePreset {
        prop: pos,
        times: vec![Time::ZERO],
        preset: EasePreset::EasyEase,
    }
    .apply(&mut doc)
    .unwrap();
    let total = path().length();
    let curve = EasePreset::EasyEase.curve();
    for i in 0..=20 {
        let time = Time::from_millis(i * 100);
        let travelled = arc_to(position_at(&doc, pos, time));
        let expected = total * curve.progress(i as f64 / 20.0);
        assert!(
            (travelled - expected).abs() < 0.2,
            "t={time}: {travelled} vs {expected}"
        );
    }
}

#[test]
fn keys_are_hit_exactly_and_hold_works() {
    let (mut doc, pos) = animated_doc();
    assert_eq!(position_at(&doc, pos, Time::ZERO), START);
    assert_eq!(position_at(&doc, pos, Time::from_secs(2)), END);
    assert_eq!(position_at(&doc, pos, Time::from_secs(5)), END);
    SetInterpolation {
        prop: pos,
        times: vec![Time::ZERO],
        interp: Interp::Hold,
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(position_at(&doc, pos, Time::from_millis(1999)), START);
}

#[test]
fn renderer_input_follows_the_animation() {
    // The evaluated world matrix moves the layer's anchor along the path.
    let (doc, pos) = animated_doc();
    let layer = &doc.main().unwrap().layers[0];
    let anchor = layer
        .transform
        .anchor
        .value_at(Time::ZERO)
        .as_vec2()
        .unwrap();
    for ms in [0, 500, 1000, 1500, 2000] {
        let t = Time::from_millis(ms);
        let eval = evaluate(&doc, doc.main_comp.unwrap(), t).unwrap();
        let on_screen = eval.layers[0].world.transform_point2(anchor);
        assert!((on_screen - position_at(&doc, pos, t)).length() < 1e-9);
    }
}

#[test]
fn evaluation_is_deterministic_across_snapshots() {
    let (doc, pos) = animated_doc();
    let copy = voxy_core::persist::from_json(&voxy_core::persist::to_json(&doc).unwrap()).unwrap();
    for ms in (0..=2000).step_by(37) {
        let t = Time::from_millis(ms);
        // Bit-identical, not just close.
        assert_eq!(
            position_at(&doc, pos, t).to_array(),
            position_at(&copy, pos, t).to_array()
        );
    }
}
