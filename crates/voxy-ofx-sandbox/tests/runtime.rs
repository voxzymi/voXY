//! The editor's OpenFX runtime against the real sandbox: scanning a
//! bundle, rendering through it, and a crashing plugin being blocked
//! until Retry.

#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use voxy_core::external::{CpuFrame, ExternalEffects};
use voxy_core::glam::DVec2;
use voxy_core::{EffectId, EvaluatedEffect, Value, WorkingSpace};
use voxy_ofx_host::client::Timeouts;
use voxy_ofx_host::runtime::{OfxRuntime, plugin_type};

const EXE: &str = env!("CARGO_BIN_EXE_voxy-ofx-sandbox");

fn effect(id: u64, plugin: &str, gain: f64) -> EvaluatedEffect {
    EvaluatedEffect {
        id: EffectId(id),
        type_id: plugin_type(plugin),
        version: 1,
        params: vec![
            (Arc::from("gain"), Value::Scalar(gain)),
            (Arc::from("straight"), Value::Bool(false)),
        ],
        matte: None,
        frame: 0,
        layer_progress: 0.0,
    }
}

/// A 3×2 frame whose rows differ, to catch a flip.
fn frame() -> CpuFrame {
    let mut pixels = Vec::new();
    for y in 0..2 {
        for x in 0..3 {
            let v = 0.1 * f32::from(u8::try_from(x + 3 * y).unwrap());
            pixels.extend_from_slice(&[v, v / 2.0, 0.05, 1.0]);
        }
    }
    CpuFrame {
        width: 3,
        height: 2,
        origin: DVec2::ZERO,
        texel_scale: 1.0,
        space: WorkingSpace::Linear,
        pixels,
    }
}

#[test]
fn scan_render_block_and_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = common::bundle(tmp.path());
    let timeouts = Timeouts {
        render: Duration::from_millis(500),
        ..Timeouts::default()
    };
    let rt = OfxRuntime::new(Path::new(EXE), tmp.path(), timeouts).unwrap();
    let scan = rt.scan(std::slice::from_ref(&bundle));
    let mut names: Vec<&str> = scan.definitions.iter().map(|d| &*d.name).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["Test Gain", "Test Gain", "Test Gain", "Test Solid"],
        "gain, crash, hang share the description"
    );
    assert_eq!(scan.problems, vec![]);
    assert!(rt.supports(&plugin_type("org.voxy.test.Gain")));

    let mut f = frame();
    let before = f.pixels.clone();
    rt.render(&effect(1, "org.voxy.test.Gain", 2.0), &mut f)
        .unwrap();
    let want: Vec<f32> = before
        .iter()
        .enumerate()
        .map(|(i, v)| if i % 4 == 3 { *v } else { v * 2.0 })
        .collect();
    assert!(
        f.pixels
            .iter()
            .zip(&want)
            .all(|(a, b)| (a - b).abs() < 1e-6),
        "{:?}",
        f.pixels
    );
    // A second render reuses the instance, with new parameters.
    let mut g = frame();
    rt.render(&effect(1, "org.voxy.test.Gain", 0.5), &mut g)
        .unwrap();
    assert!((g.pixels[4] - before[4] * 0.5).abs() < 1e-6);

    // The crash plugin kills its sandbox and is blocked.
    let err = rt
        .render(&effect(2, "org.voxy.test.Crash", 1.0), &mut frame())
        .unwrap_err();
    assert!(err.contains("crashed during Render"), "{err}");
    let blocked = rt.blocked(&plugin_type("org.voxy.test.Crash")).unwrap();
    assert!(!blocked.persistent);
    assert_eq!(
        rt.render(&effect(2, "org.voxy.test.Crash", 1.0), &mut frame())
            .unwrap_err(),
        err,
        "blocked: no new attempt"
    );

    // The gain plugin in the same bundle carries on in a new sandbox.
    let mut h = frame();
    rt.render(&effect(1, "org.voxy.test.Gain", 2.0), &mut h)
        .unwrap();
    assert_eq!(h.pixels, f.pixels);

    // A hang times out and blocks too.
    let err = rt
        .render(&effect(3, "org.voxy.test.Hang", 1.0), &mut frame())
        .unwrap_err();
    assert!(err.contains("did not finish Render"), "{err}");

    // Retry unblocks; the crash plugin crashes again and counts twice.
    rt.retry(&plugin_type("org.voxy.test.Crash"));
    assert_eq!(rt.blocked(&plugin_type("org.voxy.test.Crash")), None);
    assert!(
        rt.render(&effect(2, "org.voxy.test.Crash", 1.0), &mut frame())
            .is_err()
    );
}

#[test]
fn a_bundle_that_does_not_load_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = OfxRuntime::new(Path::new(EXE), tmp.path(), Timeouts::default()).unwrap();
    let empty = tmp.path().join("Empty.ofx.bundle");
    std::fs::create_dir_all(&empty).unwrap();
    let scan = rt.scan(&[empty]);
    assert_eq!(scan.definitions.len(), 0);
    assert_eq!(scan.problems.len(), 1);
    assert!(
        scan.problems[0].1.contains("no binary"),
        "{:?}",
        scan.problems
    );
}

#[test]
fn the_cache_skips_unchanged_bundles_and_redescribes_changed_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = common::bundle(tmp.path());
    let cache = tmp.path().join("config").join("ofx-cache.json");

    let first = OfxRuntime::new(Path::new(EXE), tmp.path(), Timeouts::default()).unwrap();
    let a = first.scan_cached(std::slice::from_ref(&bundle), &cache);
    assert_eq!(first.sandboxes_running(), 1);
    assert!(cache.is_file());
    drop(first);

    let second = OfxRuntime::new(Path::new(EXE), tmp.path(), Timeouts::default()).unwrap();
    let b = second.scan_cached(std::slice::from_ref(&bundle), &cache);
    assert_eq!(second.sandboxes_running(), 0, "described from the cache");
    assert_eq!(a.definitions, b.definitions);
    // A plugin from the cache still renders: its sandbox starts on demand.
    let mut f = frame();
    second
        .render(&effect(9, "org.voxy.test.Gain", 2.0), &mut f)
        .unwrap();
    assert_eq!(second.sandboxes_running(), 1);
    drop(second);

    // A changed binary (here: one byte longer) is described again.
    let bin = std::fs::read_dir(bundle.join("Contents"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("VoxyTest.ofx");
    let mut bytes = std::fs::read(&bin).unwrap();
    bytes.push(0);
    std::fs::write(&bin, bytes).unwrap();
    let third = OfxRuntime::new(Path::new(EXE), tmp.path(), Timeouts::default()).unwrap();
    let c = third.scan_cached(std::slice::from_ref(&bundle), &cache);
    assert_eq!(third.sandboxes_running(), 1, "re-described");
    assert_eq!(c.definitions, a.definitions);
}

#[test]
fn a_generator_replaces_the_input() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = common::bundle(tmp.path());
    let rt = OfxRuntime::new(Path::new(EXE), tmp.path(), Timeouts::default()).unwrap();
    let scan = rt.scan(std::slice::from_ref(&bundle));
    let def = scan
        .definitions
        .iter()
        .find(|d| d.type_id == plugin_type("org.voxy.test.Solid"))
        .expect("the generator-only plugin is mapped");
    assert_eq!(
        def.params.iter().map(|p| &*p.name).collect::<Vec<_>>(),
        ["color"]
    );
    let fx = EvaluatedEffect {
        id: EffectId(1),
        type_id: plugin_type("org.voxy.test.Solid"),
        version: 1,
        params: vec![(
            Arc::from("color"),
            Value::Color(voxy_core::LinearRgba::new(0.5, 0.25, 1.0, 0.5)),
        )],
        matte: None,
        frame: 0,
        layer_progress: 0.0,
    };
    let mut f = frame();
    rt.render(&fx, &mut f).unwrap();
    for px in f.pixels.as_chunks::<4>().0 {
        assert_eq!(*px, [0.25, 0.125, 0.5, 0.5], "premultiplied colour");
    }
}
