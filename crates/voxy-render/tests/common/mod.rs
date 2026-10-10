//! Shared by the golden-image test files.
//!
//! - `VOXY_UPDATE_GOLDEN=1` rewrites the references instead of comparing.
//! - Without a GPU adapter the tests are skipped, unless `VOXY_REQUIRE_GPU=1`
//!   (set in CI), in which case they fail.

#![allow(clippy::unwrap_used, dead_code)]

use std::path::{Path, PathBuf};

use voxy_render::{GpuContext, RgbaImage};

struct Tolerance {
    /// A channel differing by more than this (out of 255) marks the pixel bad.
    channel: u8,
    /// Fraction of pixels allowed to be bad (edge rasterization differences).
    bad_fraction: f64,
    /// Minimum PSNR over all channels, in dB.
    min_psnr: f64,
}

const TOLERANCE: Tolerance = Tolerance {
    channel: 2,
    bad_fraction: 0.01,
    min_psnr: 40.0,
};

pub fn gpu() -> Option<GpuContext> {
    match GpuContext::headless() {
        Ok(gpu) => {
            eprintln!("golden tests on {}", gpu.adapter_summary());
            Some(gpu)
        }
        Err(e) => {
            let required =
                std::env::var("VOXY_REQUIRE_GPU").is_ok_and(|v| !v.is_empty() && v != "0");
            assert!(
                !required,
                "VOXY_REQUIRE_GPU is set but no adapter is available: {e}"
            );
            eprintln!("skipping golden tests: {e}");
            None
        }
    }
}

pub fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/render")
}

pub fn compare(name: &str, actual: &RgbaImage) {
    let path = golden_dir().join(format!("{name}.png"));
    if std::env::var_os("VOXY_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        image::RgbaImage::from_raw(actual.width, actual.height, actual.pixels.clone())
            .unwrap()
            .save(&path)
            .unwrap();
        eprintln!("updated {}", path.display());
        return;
    }
    let expected = image::open(&path)
        .unwrap_or_else(|e| {
            panic!(
                "missing golden {} ({e}); run with VOXY_UPDATE_GOLDEN=1",
                path.display()
            )
        })
        .into_rgba8();
    assert_eq!(
        (expected.width(), expected.height()),
        (actual.width, actual.height),
        "{name}: size"
    );

    let mut bad = 0usize;
    let mut sq_err = 0f64;
    for (a, e) in actual
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_raw().as_chunks::<4>().0)
    {
        let mut pixel_bad = false;
        for (x, y) in a.iter().zip(e) {
            let d = x.abs_diff(*y);
            pixel_bad |= d > TOLERANCE.channel;
            sq_err += f64::from(d) * f64::from(d);
        }
        bad += usize::from(pixel_bad);
    }
    let pixels = (actual.width * actual.height) as usize;
    let mse = sq_err / (pixels * 4) as f64;
    let psnr = if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    };
    let bad_fraction = bad as f64 / pixels as f64;
    eprintln!(
        "{name}: PSNR {psnr:.2} dB, {bad} of {pixels} pixels outside ±{}",
        TOLERANCE.channel
    );
    if bad_fraction > TOLERANCE.bad_fraction || psnr < TOLERANCE.min_psnr {
        let out =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("voxy-golden-{name}-actual.png"));
        image::RgbaImage::from_raw(actual.width, actual.height, actual.pixels.clone())
            .unwrap()
            .save(&out)
            .unwrap();
        panic!(
            "{name}: mismatch (PSNR {psnr:.2} dB, {:.3}% bad pixels); actual frame saved to {}",
            bad_fraction * 100.0,
            out.display()
        );
    }
}
