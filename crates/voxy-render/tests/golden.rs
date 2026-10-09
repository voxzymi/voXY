//! Golden-image tests for the compositor.
//!
//! Each scene is rendered headless and compared with a reference PNG in
//! `tests/golden/render/` at the workspace root. Rendering is bit-identical
//! on one machine and driver; across GPUs it is only near-identical, so the
//! comparison allows a small tolerance (see [`Tolerance`]).
//!
//! - `VOXY_UPDATE_GOLDEN=1` rewrites the references instead of comparing.
//! - Without a GPU adapter the tests are skipped, unless `VOXY_REQUIRE_GPU=1`
//!   (set in CI), in which case they fail.

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use voxy_core::command::{AddImageLayer, AddSolidLayer, AddVideoLayer, Command, SetProperty};
use voxy_core::glam::DVec2;
use voxy_core::{CompSettings, Document, LayerKind, LinearRgba, Time, Value, WorkingSpace};
use voxy_media::VideoFrame;
use voxy_render::{
    Compositor, FrameRequest, GpuContext, NoVideo, OutputAlpha, RenderSettings, RgbaImage,
    VideoFrames, render_frame,
};

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

fn gpu() -> Option<GpuContext> {
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

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/render")
}

fn set(doc: &mut Document, prop: voxy_core::PropId, value: Value) {
    SetProperty {
        prop,
        time: Time::ZERO,
        value,
    }
    .apply(doc)
    .unwrap();
}

/// A small scene exercising transforms, parenting, opacity and an image with
/// partial alpha.
fn scene(image_path: &Path, space: WorkingSpace) -> Document {
    let mut doc = Document::with_comp(
        "Golden",
        CompSettings {
            width: 160,
            height: 90,
            background: LinearRgba::from_srgb8(20, 30, 60, 255),
            ..CompSettings::default()
        },
    );
    doc.meta.working_space = space;
    let comp = doc.main_comp.unwrap();

    let solid = |doc: &mut Document, color: LinearRgba, size: DVec2, pos: DVec2| {
        AddSolidLayer {
            comp,
            name: None,
            color,
            index: 0,
        }
        .apply(doc)
        .unwrap();
        let l = doc.main().unwrap().layers[0].clone();
        let LayerKind::Solid {
            size: size_prop, ..
        } = &l.kind
        else {
            unreachable!()
        };
        set(doc, size_prop.id, Value::Vec2(size));
        set(doc, l.transform.anchor.id, Value::Vec2(size / 2.0));
        set(doc, l.transform.position.id, Value::Vec2(pos));
        l
    };

    let red = solid(
        &mut doc,
        LinearRgba::from_srgb8(220, 40, 40, 255),
        DVec2::new(60.0, 40.0),
        DVec2::new(50.0, 40.0),
    );
    set(&mut doc, red.transform.rotation.id, Value::Scalar(30.0));
    set(
        &mut doc,
        red.transform.scale.id,
        Value::Vec2(DVec2::new(1.2, 0.8)),
    );

    let green = solid(
        &mut doc,
        LinearRgba::from_srgb8(40, 220, 90, 255),
        DVec2::new(70.0, 50.0),
        DVec2::new(80.0, 50.0),
    );
    set(&mut doc, green.transform.opacity.id, Value::Scalar(0.5));

    // Child of the red layer: follows its rotation and non-uniform scale.
    let blue = solid(
        &mut doc,
        LinearRgba::from_srgb8(60, 120, 255, 255),
        DVec2::new(16.0, 16.0),
        DVec2::new(60.0, 20.0),
    );
    doc.layer_mut(blue.id).unwrap().parent = Some(red.id);

    AddImageLayer {
        comp,
        path: image_path.to_string_lossy().into_owned(),
        pixel_size: [16, 8],
        index: 0,
    }
    .apply(&mut doc)
    .unwrap();
    let img = doc.main().unwrap().layers[0].clone();
    set(
        &mut doc,
        img.transform.position.id,
        Value::Vec2(DVec2::new(125.0, 70.0)),
    );
    set(
        &mut doc,
        img.transform.scale.id,
        Value::Vec2(DVec2::new(3.0, 3.0)),
    );
    set(&mut doc, img.transform.rotation.id, Value::Scalar(-10.0));
    doc.validate().unwrap();
    doc
}

/// A 16×8 RGBA test image: hue ramp left to right, alpha ramp top to bottom.
fn write_test_image(dir: &Path) -> PathBuf {
    let path = dir.join("ramp.png");
    let img = image::RgbaImage::from_fn(16, 8, |x, y| {
        let t = x as f32 / 15.0;
        image::Rgba([
            (255.0 * (1.0 - t)) as u8,
            (255.0 * t) as u8,
            200,
            (255.0 * (y as f32 + 1.0) / 8.0) as u8,
        ])
    });
    img.save(&path).unwrap();
    path
}

fn render(gpu: &GpuContext, compositor: &mut Compositor, doc: &Document) -> RgbaImage {
    let (image, report) = render_frame(
        compositor,
        &gpu.device,
        &gpu.queue,
        doc,
        &NoVideo,
        &FrameRequest::export(doc.main_comp.unwrap(), Time::ZERO),
    )
    .expect("root comp exists")
    .expect("readback");
    assert!(report.missing_assets.is_empty(), "{report:?}");
    image
}

fn compare(name: &str, actual: &RgbaImage) {
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

#[test]
fn golden_scenes() {
    let Some(gpu) = gpu() else { return };
    let dir = tempfile::tempdir().unwrap();
    let image_path = write_test_image(dir.path());
    let mut compositor = Compositor::new(&gpu.device);

    let linear = render(
        &gpu,
        &mut compositor,
        &scene(&image_path, WorkingSpace::Linear),
    );
    let compat = render(
        &gpu,
        &mut compositor,
        &scene(&image_path, WorkingSpace::SrgbCompat),
    );
    compare("layers_linear", &linear);
    compare("layers_srgb_compat", &compat);
    assert_ne!(
        linear.pixels, compat.pixels,
        "working space must affect blending"
    );
}

#[test]
fn rendering_is_deterministic() {
    let Some(gpu) = gpu() else { return };
    let dir = tempfile::tempdir().unwrap();
    let image_path = write_test_image(dir.path());
    let doc = scene(&image_path, WorkingSpace::Linear);

    let mut a = Compositor::new(&gpu.device);
    let first = render(&gpu, &mut a, &doc);
    let again = render(&gpu, &mut a, &doc);
    let mut b = Compositor::new(&gpu.device);
    let fresh = render(&gpu, &mut b, &doc);
    assert!(first.pixels == again.pixels, "same compositor, same frame");
    assert!(first.pixels == fresh.pixels, "fresh compositor, same frame");
}

#[test]
fn opaque_solid_is_exact() {
    // A full-frame opaque solid must come out as exactly its sRGB value in
    // both working spaces (no gamma drift through the pipeline).
    let Some(gpu) = gpu() else { return };
    let mut compositor = Compositor::new(&gpu.device);
    for space in [WorkingSpace::Linear, WorkingSpace::SrgbCompat] {
        let mut doc = Document::with_comp(
            "Solid",
            CompSettings {
                width: 8,
                height: 4,
                ..CompSettings::default()
            },
        );
        doc.meta.working_space = space;
        let comp = doc.main_comp.unwrap();
        AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::from_srgb8(200, 100, 50, 255),
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let img = render(&gpu, &mut compositor, &doc);
        for px in img.pixels.as_chunks::<4>().0 {
            assert_eq!(*px, [200, 100, 50, 255], "{space:?}");
        }
    }
}

#[test]
fn missing_images_are_reported_not_fatal() {
    let Some(gpu) = gpu() else { return };
    let mut doc = Document::with_comp(
        "Missing",
        CompSettings {
            width: 8,
            height: 8,
            ..CompSettings::default()
        },
    );
    let comp = doc.main_comp.unwrap();
    AddImageLayer {
        comp,
        path: "/nonexistent/voxy/missing.png".into(),
        pixel_size: [4, 4],
        index: 0,
    }
    .apply(&mut doc)
    .unwrap();
    let mut compositor = Compositor::new(&gpu.device);
    let (image, report) = render_frame(
        &mut compositor,
        &gpu.device,
        &gpu.queue,
        &doc,
        &NoVideo,
        &FrameRequest::export(comp, Time::ZERO),
    )
    .unwrap()
    .unwrap();
    assert_eq!(report.missing_assets.len(), 1);
    assert!(
        image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0, 0, 0, 255])
    );
}

#[test]
fn preview_scale_renders_smaller_frame() {
    let Some(gpu) = gpu() else { return };
    let doc = Document::with_comp(
        "Scale",
        CompSettings {
            width: 200,
            height: 100,
            ..CompSettings::default()
        },
    );
    let mut compositor = Compositor::new(&gpu.device);
    let (image, _) = render_frame(
        &mut compositor,
        &gpu.device,
        &gpu.queue,
        &doc,
        &NoVideo,
        &FrameRequest {
            comp: doc.main_comp.unwrap(),
            time: Time::ZERO,
            settings: RenderSettings { scale: 0.25 },
            alpha: OutputAlpha::Premultiplied,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!((image.width, image.height), (50, 25));
}

/// One layer showing the 16×8 ramp, scaled and rotated, as an image or a
/// video layer.
fn media_scene(video: bool, image_path: &Path) -> Document {
    let mut doc = Document::with_comp(
        "Media",
        CompSettings {
            width: 96,
            height: 64,
            background: LinearRgba::from_srgb8(30, 30, 30, 255),
            ..CompSettings::default()
        },
    );
    let comp = doc.main_comp.unwrap();
    let path = image_path.to_string_lossy().into_owned();
    if video {
        AddVideoLayer {
            comp,
            path,
            pixel_size: [16, 8],
            duration: None,
            frame_rate: None,
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
    } else {
        AddImageLayer {
            comp,
            path,
            pixel_size: [16, 8],
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
    }
    let layer = doc.main().unwrap().layers[0].clone();
    set(
        &mut doc,
        layer.transform.scale.id,
        Value::Vec2(DVec2::new(4.0, 4.0)),
    );
    set(&mut doc, layer.transform.rotation.id, Value::Scalar(12.0));
    doc
}

fn render_with(
    gpu: &GpuContext,
    compositor: &mut Compositor,
    doc: &Document,
    video: &dyn VideoFrames,
    time: Time,
) -> (RgbaImage, voxy_render::RenderReport) {
    render_frame(
        compositor,
        &gpu.device,
        &gpu.queue,
        doc,
        video,
        &FrameRequest::export(doc.main_comp.unwrap(), time),
    )
    .unwrap()
    .unwrap()
}

#[test]
fn video_frames_render_exactly_like_images() {
    let Some(gpu) = gpu() else { return };
    let dir = tempfile::tempdir().unwrap();
    let image_path = write_test_image(dir.path());
    let ramp = image::open(&image_path).unwrap().into_rgba8();
    let frame = Arc::new(VideoFrame {
        time: Time::ZERO,
        duration: Time::from_millis(40),
        width: 16,
        height: 8,
        pixels: ramp.into_raw(),
    });
    let mut compositor = Compositor::new(&gpu.device);
    let as_image = render(&gpu, &mut compositor, &media_scene(false, &image_path));
    let provider = |_, _| Some(frame.clone());
    let (as_video, report) = render_with(
        &gpu,
        &mut compositor,
        &media_scene(true, &image_path),
        &provider,
        Time::ZERO,
    );
    assert_eq!(report.pending_video, []);
    assert_eq!(as_video, as_image, "video uses the image upload path");
    compare("video_layer", &as_video);
}

#[test]
fn video_frames_upload_once_and_reuse_textures() {
    let Some(gpu) = gpu() else { return };
    let dir = tempfile::tempdir().unwrap();
    let doc = media_scene(true, &dir.path().join("unused.mp4"));
    let frame = |v: u8| {
        Arc::new(VideoFrame {
            time: Time::ZERO,
            duration: Time::from_millis(40),
            width: 16,
            height: 8,
            pixels: [v, v, v, 255].repeat(16 * 8),
        })
    };
    // Twelve distinct frames, one per 40 ms of source time.
    let frames: Vec<_> = (0..12).map(|i| frame(20 * i)).collect();
    let provider = |_, t: Time| {
        let i = usize::try_from(t.0 / Time::from_millis(40).0).ok()?;
        frames.get(i).cloned()
    };
    let at = |i: i64| Time(Time::from_millis(40).0 * i);
    let mut compositor = Compositor::new(&gpu.device);

    let (first, _) = render_with(&gpu, &mut compositor, &doc, &provider, at(0));
    let (again, _) = render_with(&gpu, &mut compositor, &doc, &provider, at(0));
    assert_eq!(first, again);
    let stats = compositor.video_stats();
    assert_eq!((stats.uploads, stats.reused, stats.allocations), (1, 1, 1));

    // Playing through: every new frame is uploaded, the image changes, and
    // once the slots are full textures are reused instead of reallocated.
    let mut previous = first;
    for i in 1..12 {
        let (img, _) = render_with(&gpu, &mut compositor, &doc, &provider, at(i));
        assert_ne!(img, previous, "frame {i}");
        previous = img;
    }
    let stats = compositor.video_stats();
    assert_eq!(stats.uploads, 12);
    assert_eq!(stats.allocations, 8, "texture pairs are pooled");
    // A recent frame is still resident.
    render_with(&gpu, &mut compositor, &doc, &provider, at(10));
    assert_eq!(compositor.video_stats().uploads, 12);

    // No frame yet: the layer is skipped and reported, not an error.
    let (blank, report) = render_with(&gpu, &mut compositor, &doc, &NoVideo, at(0));
    assert_eq!(report.pending_video.len(), 1);
    assert!(
        blank
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [30, 30, 30, 255])
    );
}
