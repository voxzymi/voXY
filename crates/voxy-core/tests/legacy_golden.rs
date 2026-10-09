//! Golden proof that the legacy interpolation modes (hold, linear, cubic
//! Bézier, spatial motion paths) evaluate bit-for-bit as they did before the
//! parametric easings were added.
//!
//! `fixtures/legacy_interp.txt` was generated from the code *before* that
//! change. Regenerate it only for an intentional, documented change to legacy
//! behavior: `VOXY_BLESS=1 cargo test -p voxy-core --test legacy_golden`.

#![allow(clippy::unwrap_used)]

use std::fmt::Write as _;

use voxy_core::glam::{DVec2, DVec3};
use voxy_core::{
    EaseHandle, EasePreset, Interp, Keyframe, KeyframeTrack, LinearRgba, SpatialTangents, Time,
    Value,
};

const FIXTURE: &str = "tests/fixtures/legacy_interp.txt";
const STEPS_PER_SEC: i64 = 48;

fn key(secs: i64, value: Value, interp: Interp) -> Keyframe {
    let mut k = Keyframe::new(Time::from_secs(secs), value);
    k.interp = interp;
    k
}

fn bits(v: &Value) -> String {
    match v {
        Value::Scalar(x) => format!("{:016x}", x.to_bits()),
        Value::Vec2(v) => format!("{:016x},{:016x}", v.x.to_bits(), v.y.to_bits()),
        Value::Vec3(v) => format!(
            "{:016x},{:016x},{:016x}",
            v.x.to_bits(),
            v.y.to_bits(),
            v.z.to_bits()
        ),
        Value::Color(c) => format!(
            "{:08x},{:08x},{:08x},{:08x}",
            c.r.to_bits(),
            c.g.to_bits(),
            c.b.to_bits(),
            c.a.to_bits()
        ),
        Value::Int(i) => format!("{i}"),
        other => format!("{other:?}"),
    }
}

fn dump(name: &str, track: &KeyframeTrack, spatial: bool, out: &mut String) {
    let end = track.last().time.0;
    let step = voxy_core::FLICKS_PER_SECOND / STEPS_PER_SEC;
    let mut t = -step;
    while t <= end + step {
        let v = track.evaluate(Time(t), spatial);
        writeln!(out, "{name} {t} {}", bits(&v)).unwrap();
        t += step;
    }
}

fn scenarios() -> String {
    let mut out = String::new();

    // Hold / linear / each preset, on a four-key scalar track.
    let mut presets: Vec<(String, EasePreset)> = EasePreset::ALL
        .iter()
        .map(|p| (format!("{p:?}"), *p))
        .collect();
    presets.retain(|(_, p)| *p != EasePreset::Linear);
    let scalar = |interp: Interp, preset: Option<EasePreset>| {
        let mut keys = vec![
            key(0, Value::Scalar(0.0), interp),
            key(1, Value::Scalar(100.0), interp),
            key(3, Value::Scalar(-20.0), interp),
            key(4, Value::Scalar(50.0), Interp::Linear),
        ];
        if let Some(p) = preset {
            let c = p.curve();
            for i in 0..3 {
                keys[i].ease_out = c.p1;
                keys[i + 1].ease_in = c.p2;
            }
        }
        KeyframeTrack::try_from(keys).unwrap()
    };
    dump("hold", &scalar(Interp::Hold, None), false, &mut out);
    dump("linear", &scalar(Interp::Linear, None), false, &mut out);
    for (name, p) in &presets {
        dump(
            &format!("bezier-{name}"),
            &scalar(Interp::Bezier, Some(*p)),
            false,
            &mut out,
        );
    }
    // A hand-made asymmetric curve with y handles outside 0..1.
    let mut keys = vec![
        key(0, Value::Scalar(1.0), Interp::Bezier),
        key(2, Value::Scalar(-7.5), Interp::Linear),
    ];
    keys[0].ease_out = EaseHandle::new(0.07, 1.9).unwrap();
    keys[1].ease_in = EaseHandle::new(0.81, -0.6).unwrap();
    dump(
        "bezier-custom",
        &KeyframeTrack::try_from(keys).unwrap(),
        false,
        &mut out,
    );

    // Other value kinds.
    let color = KeyframeTrack::try_from(vec![
        key(0, Value::Color(LinearRgba::BLACK), Interp::Linear),
        key(
            2,
            Value::Color(LinearRgba::new(0.9, 0.2, 0.4, 0.5)),
            Interp::Linear,
        ),
    ])
    .unwrap();
    dump("color", &color, false, &mut out);
    let int = KeyframeTrack::try_from(vec![
        key(0, Value::Int(-3), Interp::Bezier),
        key(2, Value::Int(11), Interp::Linear),
    ])
    .unwrap();
    dump("int", &int, false, &mut out);

    // Spatial path with tangents under an overshooting ease.
    let mut a = key(0, Value::Vec2(DVec2::new(100.0, 500.0)), Interp::Bezier);
    a.spatial = Some(SpatialTangents {
        incoming: DVec3::ZERO,
        outgoing: DVec3::new(50.0, -600.0, 0.0),
    });
    a.ease_out = EasePreset::BackOut.curve().p1;
    let mut b = key(2, Value::Vec2(DVec2::new(900.0, 500.0)), Interp::Linear);
    b.spatial = Some(SpatialTangents {
        incoming: DVec3::new(300.0, -100.0, 0.0),
        outgoing: DVec3::ZERO,
    });
    b.ease_in = EasePreset::BackOut.curve().p2;
    let spatial = KeyframeTrack::try_from(vec![a, b]).unwrap();
    dump("spatial", &spatial, true, &mut out);
    dump("spatial-straight", &spatial, false, &mut out);
    out
}

#[test]
fn legacy_interpolation_is_bit_exact() {
    let actual = scenarios();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    if std::env::var_os("VOXY_BLESS").is_some() {
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    if actual != expected {
        let first = actual
            .lines()
            .zip(expected.lines())
            .position(|(a, e)| a != e);
        panic!("legacy interpolation changed; first differing line: {first:?}");
    }
}
