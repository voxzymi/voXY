//! The OpenFX conformance suite (design §9): for every fixture plugin,
//! Describe and the parameter mapping, rendering against goldens, identity
//! detection, and determinism, all through the real sandbox.
//!
//! Fixtures are our own test plugins (the blur, transform and keyer stand
//! in for the third-party ones the owner ruled out, P-0 Q2). Goldens are
//! PNGs in `tests/golden/ofx/`, the output clamped to 0..1 and stored as
//! 8 bits; `VOXY_UPDATE_GOLDEN=1` rewrites them. Each case renders a fixed
//! 32×24 pattern; a layer input sees a second pattern.

#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use voxy_core::external::{CLIP_PARAM_PREFIX, CpuFrame, ExternalEffects};
use voxy_core::glam::DVec2;
use voxy_core::registry::{EffectDef, EffectRegistry};
use voxy_core::{EffectId, EvaluatedEffect, LinearRgba, Value, WorkingSpace};
use voxy_ofx_host::client::Timeouts;
use voxy_ofx_host::runtime::{OfxRuntime, plugin_type};

const EXE: &str = env!("CARGO_BIN_EXE_voxy-ofx-sandbox");
const W: u32 = 32;
const H: u32 = 24;
/// The largest difference allowed per 8-bit channel.
const TOLERANCE: u8 = 1;

/// One render of one plugin.
struct Case {
    plugin: &'static str,
    name: &'static str,
    /// Values other than the defaults.
    params: Vec<(&'static str, Value)>,
    /// The plugin must report an identity: the output is the input.
    identity: bool,
}

fn case(plugin: &'static str, name: &'static str, params: Vec<(&'static str, Value)>) -> Case {
    Case {
        plugin,
        name,
        params,
        identity: false,
    }
}

fn identity(plugin: &'static str, params: Vec<(&'static str, Value)>) -> Case {
    Case {
        identity: true,
        ..case(plugin, "identity", params)
    }
}

fn cases() -> Vec<Case> {
    let s = Value::Scalar;
    let v2 = |x, y| Value::Vec2(DVec2::new(x, y));
    let rgb = |r, g, b| Value::Color(LinearRgba::new(r, g, b, 1.0));
    vec![
        case("org.voxy.test.Gain", "double", vec![("gain", s(2.0))]),
        identity("org.voxy.test.Gain", vec![("gain", s(1.0))]),
        case("org.voxy.test.Solid", "default", vec![]),
        case("org.voxy.test.Blur", "radius-2", vec![("radius", s(2.0))]),
        identity("org.voxy.test.Blur", vec![("radius", s(0.0))]),
        case(
            "org.voxy.test.Transform",
            "move-and-scale",
            vec![("offset", v2(4.0, -3.0)), ("scale", s(0.5))],
        ),
        identity(
            "org.voxy.test.Transform",
            vec![("offset", v2(0.0, 0.0)), ("scale", s(1.0))],
        ),
        case(
            "org.voxy.test.Keyer",
            "key-green",
            vec![("key", rgb(0.0, 1.0, 0.0)), ("tolerance", s(0.3))],
        ),
        identity("org.voxy.test.Keyer", vec![("tolerance", s(0.0))]),
        case(
            "org.voxy.test.Fade",
            "half",
            vec![("Transition", s(0.5)), ("voxy.auto", Value::Bool(false))],
        ),
        case("org.voxy.test.Matte", "matte", vec![]),
    ]
}

/// The input pattern: colour ramps, an opaque green square (for the
/// keyer), a half-transparent band, and a transparent corner; rows
/// top-down, premultiplied. `second` gives the other layer's pattern.
fn pattern(second: bool) -> Vec<f32> {
    let mut px = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let (fx, fy) = (x as f32 / (W - 1) as f32, y as f32 / (H - 1) as f32);
            let (rgb, a) = if second {
                ([1.0 - fx, fy * 0.5, 0.75], if x < 16 { 1.0 } else { 0.25 })
            } else if (8..16).contains(&x) && (6..14).contains(&y) {
                ([0.0, 1.0, 0.0], 1.0)
            } else if y >= 20 {
                ([fx, 0.2, 1.0 - fx], 0.5)
            } else if x >= 28 && y < 4 {
                ([0.0; 3], 0.0)
            } else {
                ([fx, fy, f32::midpoint(fx, fy)], 1.0)
            };
            px.extend_from_slice(&[rgb[0] * a, rgb[1] * a, rgb[2] * a, a]);
        }
    }
    px
}

fn frame(def: &EffectDef) -> CpuFrame {
    let layers = def
        .params
        .iter()
        .filter(|p| p.name.starts_with(CLIP_PARAM_PREFIX))
        .map(|p| (Arc::clone(&p.name), pattern(true)))
        .collect();
    CpuFrame {
        width: W,
        height: H,
        origin: DVec2::ZERO,
        texel_scale: 1.0,
        layer_size: DVec2::new(f64::from(W), f64::from(H)),
        space: WorkingSpace::Linear,
        pixels: pattern(false),
        layers,
    }
}

/// `def`'s parameters at their defaults, with `overrides`.
fn effect(def: &EffectDef, id: u64, overrides: &[(&str, Value)]) -> EvaluatedEffect {
    for (name, _) in overrides {
        assert!(
            def.param(name).is_some(),
            "{}: no parameter {name}",
            def.name
        );
    }
    EvaluatedEffect {
        id: EffectId(id),
        type_id: def.type_id.clone(),
        version: def.version,
        params: def
            .params
            .iter()
            .map(|p| {
                let v = overrides
                    .iter()
                    .find(|(n, _)| *n == &*p.name)
                    .map_or_else(|| p.desc.default.clone(), |(_, v)| v.clone());
                (Arc::clone(&p.name), v)
            })
            .collect(),
        matte: None,
        frame: 0,
        layer_progress: 0.0,
        timeline: None,
    }
}

fn golden_path(plugin: &str, name: &str) -> PathBuf {
    let short = plugin.rsplit('.').next().unwrap().to_lowercase();
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/ofx")
        .join(format!("{short}-{name}.png"))
}

fn to_png_bytes(pixels: &[f32]) -> Vec<u8> {
    pixels
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

/// Compares against the golden, or rewrites it. Returns a failure line.
fn check_golden(plugin: &str, name: &str, pixels: &[f32]) -> Option<String> {
    let path = golden_path(plugin, name);
    let actual = to_png_bytes(pixels);
    if std::env::var_os("VOXY_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbaImage::from_raw(W, H, actual)
            .unwrap()
            .save(&path)
            .unwrap();
        return None;
    }
    let expected = match image::open(&path) {
        Ok(i) => i.into_rgba8(),
        Err(e) => {
            return Some(format!(
                "{plugin} {name}: missing golden {} ({e}); run with VOXY_UPDATE_GOLDEN=1",
                path.display()
            ));
        }
    };
    let worst = expected
        .as_raw()
        .iter()
        .zip(&actual)
        .map(|(e, a)| e.abs_diff(*a))
        .max()
        .unwrap_or(0);
    (worst > TOLERANCE).then(|| format!("{plugin} {name}: differs from its golden by {worst}"))
}

#[test]
fn every_fixture_plugin_conforms() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = common::bundle(tmp.path());
    let rt = OfxRuntime::new(Path::new(EXE), tmp.path(), Timeouts::default()).unwrap();
    let scan = rt.scan(std::slice::from_ref(&bundle));

    // Describe and mapping: every plugin maps, and its definition passes
    // the registry's validation (defaults fit kinds, ranges, keys).
    assert_eq!(scan.problems, vec![], "nothing failed to describe or map");
    let mut reg = EffectRegistry::with_builtins();
    for def in &scan.definitions {
        assert!(def.type_id.is_ofx());
        reg.register(def.clone())
            .unwrap_or_else(|e| panic!("{}: {e}", def.name));
    }

    let mut failures = Vec::new();
    for (i, c) in cases().iter().enumerate() {
        let def = reg
            .latest(&plugin_type(c.plugin))
            .unwrap_or_else(|| panic!("{} is not installed", c.plugin));
        let fx = effect(def, i as u64 + 1, &c.params);
        let mut f = frame(def);
        if let Err(e) = rt.render(&fx, &mut f) {
            failures.push(format!("{} {}: {e}", c.plugin, c.name));
            continue;
        }
        // Determinism: a second render, from a fresh instance too.
        let mut again = frame(def);
        rt.render(&effect(def, 1000 + i as u64, &c.params), &mut again)
            .unwrap();
        if again.pixels != f.pixels {
            failures.push(format!("{} {}: not deterministic", c.plugin, c.name));
        }
        if c.identity {
            if f.pixels != pattern(false) {
                failures.push(format!("{} {}: not an identity", c.plugin, c.name));
            }
        } else {
            if f.pixels == pattern(false) {
                failures.push(format!("{} {}: left the input as it was", c.plugin, c.name));
            }
            failures.extend(check_golden(c.plugin, c.name, &f.pixels));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every mapped parameter value survives the trip to the plugin and back:
/// a guard that the cases above set what they mean to.
#[test]
fn case_values_fit_their_parameters() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = common::bundle(tmp.path());
    let rt = OfxRuntime::new(Path::new(EXE), tmp.path(), Timeouts::default()).unwrap();
    let scan = rt.scan(std::slice::from_ref(&bundle));
    for c in cases() {
        let def = scan
            .definitions
            .iter()
            .find(|d| d.type_id == plugin_type(c.plugin))
            .unwrap();
        for (name, v) in &c.params {
            let p = def.param(name).unwrap();
            assert_eq!(p.desc.kind, v.kind(), "{} {name}", c.plugin);
            if let (Some((lo, hi)), Value::Scalar(x)) = (p.desc.range, v) {
                assert!((lo..=hi).contains(x), "{} {name} out of range", c.plugin);
            }
        }
    }
}
