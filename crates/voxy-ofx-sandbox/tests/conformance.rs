//! The OpenFX conformance suite (design §9): for every fixture plugin,
//! Describe and the parameter mapping, rendering against goldens, identity
//! detection, and determinism, all through the real sandbox.
//!
//! Fixtures are our own test plugins (the blur, transform and keyer stand
//! in for the third-party ones the owner ruled out, P-0 Q2) and, when
//! `VOXY_OFX_FIXTURES` names the folder `cargo xtask ofx-fixtures` built,
//! the approved OpenFX samples (P-0 Q1). CI sets it, with
//! `VOXY_REQUIRE_OFX_FIXTURES=1` so a missing folder fails. Goldens are
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

/// What a case checks, beyond rendering without an error twice with the
/// same result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    /// Matches its golden and changes the input.
    Golden,
    /// The output is exactly the input (an identity, or a pass-through).
    Unchanged,
    /// Changes the input; no golden (the result depends on the platform's
    /// C++ library, such as random numbers).
    Changes,
    /// Only renders (a diagnostic plugin).
    Renders,
}

/// One render of one plugin.
struct Case {
    plugin: &'static str,
    name: &'static str,
    /// Values other than the defaults.
    params: Vec<(&'static str, Value)>,
    check: Check,
}

fn case(plugin: &'static str, name: &'static str, params: Vec<(&'static str, Value)>) -> Case {
    Case {
        plugin,
        name,
        params,
        check: Check::Golden,
    }
}

fn identity(plugin: &'static str, params: Vec<(&'static str, Value)>) -> Case {
    Case {
        check: Check::Unchanged,
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

/// The approved OpenFX samples (docs/ofx-plugins.md #1 to #11), by the
/// bundle `cargo xtask ofx-fixtures` builds them into.
fn sample_cases() -> Vec<(&'static str, Case)> {
    let s = Value::Scalar;
    let off = Value::Bool(false);
    let with = |c: Case, check| Case { check, ..c };
    vec![
        (
            "OfxExampleInvert",
            case("uk.co.thefoundry.OfxInvertExample", "default", vec![]),
        ),
        (
            "OfxExampleBasic",
            case(
                "uk.co.thefoundry.BasicGainPlugin",
                "scale-2",
                vec![("scale", s(2.0))],
            ),
        ),
        (
            "OfxExampleBasic",
            identity("uk.co.thefoundry.BasicGainPlugin", vec![]),
        ),
        (
            "OfxExampleRectangle",
            case(
                "uk.co.thefoundry.GeneratorExample",
                "red",
                vec![("colour", Value::Color(LinearRgba::new(1.0, 0.0, 0.0, 1.0)))],
            ),
        ),
        (
            "OfxExampleChoiceParams",
            case("org.openfx.ChoiceParamPlugin", "default", vec![]),
        ),
        // voXY has no multiple clip depths, so every depth is the input's
        // and the converter passes the image through.
        (
            "OfxExampleDepthConverter",
            with(
                case(
                    "uk.co.thefoundry.DepthConverterExample",
                    "same-depth",
                    vec![],
                ),
                Check::Unchanged,
            ),
        ),
        (
            "OfxExampleTest",
            with(
                case("net.sf.openfx:PropertyTestPlugin", "renders", vec![]),
                Check::Renders,
            ),
        ),
        (
            "OfxSupportInvert",
            case("net.sf.openfx.invertPlugin", "default", vec![]),
        ),
        (
            "OfxSupportNoise",
            with(
                case("net.sf.openfx.noisePlugin", "default", vec![]),
                Check::Changes,
            ),
        ),
        (
            "OfxSupportCrossFade",
            case(
                "net.sf.openfx.crossFade",
                "half",
                vec![("Transition", s(0.5)), ("voxy.auto", off.clone())],
            ),
        ),
        (
            "OfxSupportCrossFade",
            identity(
                "net.sf.openfx.crossFade",
                vec![("Transition", s(0.0)), ("voxy.auto", off)],
            ),
        ),
        (
            "OfxSupportTester",
            case("net.sf.openfx.GenericTestPlugin", "default", vec![]),
        ),
        (
            "OfxSupportMultiBundle",
            case("net.sf.openfx.dotexample", "default", vec![]),
        ),
        (
            "OfxSupportMultiBundle",
            case(
                "net.sf.openfx.gammaexample",
                "scale-2",
                vec![("scale", s(2.0))],
            ),
        ),
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
    let short = if plugin.starts_with("org.voxy.test.") {
        plugin.rsplit('.').next().unwrap().to_lowercase()
    } else {
        format!("sample-{}", plugin.replace([':', '.'], "-").to_lowercase())
    };
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

/// Runs `cases` against `rt` (whose plugins are registered in `reg`) and
/// returns the failures.
fn run_cases<'a>(
    rt: &OfxRuntime,
    reg: &EffectRegistry,
    cases: impl IntoIterator<Item = &'a Case>,
) -> Vec<String> {
    let mut failures = Vec::new();
    for (i, c) in cases.into_iter().enumerate() {
        let Some(def) = reg.latest(&plugin_type(c.plugin)) else {
            failures.push(format!("{}: not installed", c.plugin));
            continue;
        };
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
        let unchanged = f.pixels == pattern(false);
        match c.check {
            Check::Unchanged if !unchanged => {
                failures.push(format!("{} {}: changed the input", c.plugin, c.name));
            }
            Check::Golden | Check::Changes if unchanged => {
                failures.push(format!("{} {}: left the input as it was", c.plugin, c.name));
            }
            Check::Golden => failures.extend(check_golden(c.plugin, c.name, &f.pixels)),
            _ => {}
        }
    }
    failures
}

/// Scans `bundles`: nothing may fail to describe or map, and every
/// definition passes the registry's validation.
fn scan_all(rt: &OfxRuntime, bundles: &[PathBuf]) -> EffectRegistry {
    let scan = rt.scan(bundles);
    assert_eq!(scan.problems, vec![], "nothing failed to describe or map");
    let mut reg = EffectRegistry::with_builtins();
    for def in &scan.definitions {
        assert!(def.type_id.is_ofx());
        reg.register(def.clone())
            .unwrap_or_else(|e| panic!("{}: {e}", def.name));
    }
    reg
}

#[test]
fn every_fixture_plugin_conforms() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = common::bundle(tmp.path());
    let rt = OfxRuntime::new(Path::new(EXE), tmp.path(), Timeouts::default()).unwrap();
    let reg = scan_all(&rt, std::slice::from_ref(&bundle));
    let failures = run_cases(&rt, &reg, &cases());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The approved OpenFX samples, built by `cargo xtask ofx-fixtures`. A
/// bundle the build skipped on this platform (its `report.txt` says so)
/// has its cases skipped too.
#[test]
fn the_approved_samples_conform() {
    let Some(dir) = std::env::var_os("VOXY_OFX_FIXTURES").map(PathBuf::from) else {
        assert!(
            std::env::var_os("VOXY_REQUIRE_OFX_FIXTURES").is_none(),
            "VOXY_OFX_FIXTURES is not set"
        );
        eprintln!("skipped: set VOXY_OFX_FIXTURES to a `cargo xtask ofx-fixtures` folder");
        return;
    };
    let report = std::fs::read_to_string(dir.join("report.txt")).unwrap();
    let built: Vec<&str> = report
        .lines()
        .filter_map(|l| l.strip_suffix(": built"))
        .collect();
    let bundles: Vec<PathBuf> = built
        .iter()
        .map(|b| dir.join("bundles").join(format!("{b}.ofx.bundle")))
        .collect();
    let tmp = tempfile::tempdir().unwrap();
    let rt = OfxRuntime::new(Path::new(EXE), tmp.path(), Timeouts::default()).unwrap();
    let reg = scan_all(&rt, &bundles);
    let cases: Vec<Case> = sample_cases()
        .into_iter()
        .filter(|(b, _)| built.contains(b))
        .map(|(_, c)| c)
        .collect();
    let mut failures = run_cases(&rt, &reg, &cases);

    // The tester's push buttons reach the plugin.
    if built.contains(&"OfxSupportTester") {
        let def = reg
            .latest(&plugin_type("net.sf.openfx.GenericTestPlugin"))
            .unwrap();
        let buttons: Vec<&str> = def
            .params
            .iter()
            .filter(|p| p.flags.visibility == voxy_core::registry::ParamVisibility::Button)
            .map(|p| &*p.name)
            .collect();
        assert!(!buttons.is_empty(), "the tester has push buttons");
        let fx = effect(def, 5000, &[]);
        for b in buttons {
            if let Err(e) = rt.instance_changed(&fx, DVec2::new(f64::from(W), f64::from(H)), b) {
                failures.push(format!("tester button {b}: {e}"));
            }
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
