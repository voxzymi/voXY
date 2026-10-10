//! Nested compositions on the GPU (docs/design-media-comps.md §8).
//!
//! These compare renders with each other (nesting must not change a simple
//! scene) and check specific pixels, so they need no reference images.
//! Without a GPU adapter they are skipped, unless `VOXY_REQUIRE_GPU=1`.

#![allow(clippy::unwrap_used)]
#![allow(clippy::many_single_char_names)] // a, b, c, h, l, p read best short in these scenarios.

use voxy_core::command::{
    AddImageLayer, AddSolidLayer, AddSourceLayer, Command, CreateComposition, ReplaceMedia,
    SetCollapse, SetLayerTiming, SetProperty,
};
use voxy_core::glam::DVec2;
use voxy_core::{
    CompId, CompSettings, Document, LayerId, LayerKind, LinearRgba, SourceRef, Time, Value,
};
use voxy_render::{
    Compositor, FrameRequest, GpuContext, NoVideo, OutputAlpha, RenderReport, RenderSettings,
    RgbaImage, render_frame,
};

fn gpu() -> Option<GpuContext> {
    match GpuContext::headless() {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            let required =
                std::env::var("VOXY_REQUIRE_GPU").is_ok_and(|v| !v.is_empty() && v != "0");
            assert!(
                !required,
                "VOXY_REQUIRE_GPU is set but no adapter is available: {e}"
            );
            eprintln!("skipping nested render tests: {e}");
            None
        }
    }
}

const RED: LinearRgba = LinearRgba {
    r: 1.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

fn parent(width: u32, height: u32) -> Document {
    Document::with_comp(
        "Parent",
        CompSettings {
            name: "Parent".into(),
            width,
            height,
            background: LinearRgba::BLACK,
            duration: Time::from_secs(10),
            ..CompSettings::default()
        },
    )
}

fn add_comp(doc: &mut Document, name: &str, w: u32, h: u32, background: LinearRgba) -> CompId {
    CreateComposition {
        settings: CompSettings {
            name: name.into(),
            width: w,
            height: h,
            background,
            duration: Time::from_secs(10),
            ..CompSettings::default()
        },
        folder: None,
    }
    .apply(doc)
    .unwrap();
    doc.comps.values().find(|c| c.name == name).unwrap().id
}

fn add_solid(doc: &mut Document, comp: CompId, color: LinearRgba) -> LayerId {
    AddSolidLayer {
        comp,
        name: None,
        color,
        index: 0,
    }
    .apply(doc)
    .unwrap();
    doc.comp(comp).unwrap().layers[0].id
}

fn nest(doc: &mut Document, into: CompId, child: CompId) -> LayerId {
    AddSourceLayer {
        comp: into,
        source: SourceRef::Comp(child),
        index: 0,
        in_point: Time::ZERO,
    }
    .apply(doc)
    .unwrap();
    doc.comp(into).unwrap().layers[0].id
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

fn opacity_of(doc: &Document, comp: CompId, layer: LayerId) -> voxy_core::PropId {
    doc.comp(comp)
        .unwrap()
        .layer(layer)
        .unwrap()
        .transform
        .opacity
        .id
}

fn render_at(
    gpu: &GpuContext,
    compositor: &mut Compositor,
    doc: &Document,
    time: Time,
    scale: f64,
) -> (RgbaImage, RenderReport) {
    render_frame(
        compositor,
        &gpu.device,
        &gpu.queue,
        doc,
        &NoVideo,
        &FrameRequest {
            comp: doc.main_comp.unwrap(),
            time,
            settings: RenderSettings::at_scale(scale),
            alpha: OutputAlpha::Straight,
        },
    )
    .unwrap()
    .unwrap()
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.width + x) * 4) as usize;
    [
        img.pixels[i],
        img.pixels[i + 1],
        img.pixels[i + 2],
        img.pixels[i + 3],
    ]
}

fn assert_close(a: &RgbaImage, b: &RgbaImage, tolerance: u8, what: &str) {
    assert_eq!((a.width, a.height), (b.width, b.height), "{what}: size");
    let worst = a
        .pixels
        .iter()
        .zip(&b.pixels)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0);
    assert!(worst <= tolerance, "{what}: pixels differ by up to {worst}");
}

#[test]
fn a_nested_composition_draws_centered_through_the_layer_transform() {
    let Some(gpu) = gpu() else { return };
    let mut doc = parent(200, 100);
    let main = doc.main_comp.unwrap();
    let child = add_comp(&mut doc, "Child", 100, 50, LinearRgba::BLACK);
    add_solid(&mut doc, child, RED);
    nest(&mut doc, main, child);
    let mut c = Compositor::new(&gpu.device);
    let (img, report) = render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert_eq!(report, RenderReport::default());
    assert_eq!(pixel(&img, 100, 50), [255, 0, 0, 255], "inside the child");
    assert_eq!(pixel(&img, 55, 30), [255, 0, 0, 255], "near its corner");
    assert_eq!(
        pixel(&img, 10, 10),
        [0, 0, 0, 255],
        "outside: the parent's black"
    );
    assert_eq!(pixel(&img, 190, 90), [0, 0, 0, 255]);
}

#[test]
fn a_nested_background_is_never_drawn() {
    let Some(gpu) = gpu() else { return };
    let mut doc = parent(200, 100);
    let main = doc.main_comp.unwrap();
    // The child has a loud blue background and no layers.
    let child = add_comp(
        &mut doc,
        "Child",
        100,
        50,
        LinearRgba::new(0.0, 0.0, 1.0, 1.0),
    );
    nest(&mut doc, main, child);
    let mut c = Compositor::new(&gpu.device);
    let (img, _) = render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert_eq!(
        pixel(&img, 100, 50),
        [0, 0, 0, 255],
        "the child is transparent"
    );
}

#[test]
fn nesting_does_not_change_a_simple_scene() {
    let Some(gpu) = gpu() else { return };
    // Nested: a precomp at 50% opacity holding a red solid.
    let mut nested = parent(200, 100);
    let main = nested.main_comp.unwrap();
    let child = add_comp(&mut nested, "Child", 200, 100, LinearRgba::BLACK);
    add_solid(&mut nested, child, RED);
    let l = nest(&mut nested, main, child);
    let p = opacity_of(&nested, main, l);
    set(&mut nested, p, Value::Scalar(0.5));
    // Direct: the same red solid at 50% opacity.
    let mut direct = parent(200, 100);
    let main = direct.main_comp.unwrap();
    let s = add_solid(&mut direct, main, RED);
    let p = opacity_of(&direct, main, s);
    set(&mut direct, p, Value::Scalar(0.5));
    let mut c = Compositor::new(&gpu.device);
    let (a, _) = render_at(&gpu, &mut c, &nested, Time::ZERO, 1.0);
    let (b, _) = render_at(&gpu, &mut c, &direct, Time::ZERO, 1.0);
    assert_close(&a, &b, 1, "nested vs direct at 50% opacity");
    assert_ne!(pixel(&a, 100, 50), [0, 0, 0, 255], "something was drawn");
}

#[test]
fn the_child_follows_the_preview_scale() {
    let Some(gpu) = gpu() else { return };
    let mut doc = parent(200, 100);
    let main = doc.main_comp.unwrap();
    let child = add_comp(&mut doc, "Child", 100, 50, LinearRgba::BLACK);
    add_solid(&mut doc, child, RED);
    nest(&mut doc, main, child);
    let mut c = Compositor::new(&gpu.device);
    let (img, _) = render_at(&gpu, &mut c, &doc, Time::ZERO, 0.5);
    assert_eq!((img.width, img.height), (100, 50));
    assert_eq!(pixel(&img, 50, 25), [255, 0, 0, 255]);
    assert_eq!(pixel(&img, 5, 5), [0, 0, 0, 255]);
}

#[test]
fn the_child_is_shown_at_the_mapped_time() {
    let Some(gpu) = gpu() else { return };
    let mut doc = parent(200, 100);
    let main = doc.main_comp.unwrap();
    let child = add_comp(&mut doc, "Child", 200, 100, LinearRgba::BLACK);
    let solid = add_solid(&mut doc, child, RED);
    // The solid only exists from child time 1 s.
    SetLayerTiming {
        layer: solid,
        in_point: Time::from_secs(1),
        out_point: Time::from_secs(10),
        start_offset: Time::ZERO,
    }
    .apply(&mut doc)
    .unwrap();
    nest(&mut doc, main, child);
    let mut c = Compositor::new(&gpu.device);
    let (before, _) = render_at(&gpu, &mut c, &doc, Time::from_millis(500), 1.0);
    let (after, _) = render_at(&gpu, &mut c, &doc, Time::from_millis(1500), 1.0);
    assert_eq!(
        pixel(&before, 100, 50),
        [0, 0, 0, 255],
        "child time 0.5 s: nothing yet"
    );
    assert_eq!(
        pixel(&after, 100, 50),
        [255, 0, 0, 255],
        "child time 1.5 s: the solid"
    );
}

#[test]
fn collapse_draws_outside_the_child_bounds_and_otherwise_matches() {
    let Some(gpu) = gpu() else { return };
    let build = |collapse: bool, big: bool| {
        let mut doc = parent(200, 100);
        let main = doc.main_comp.unwrap();
        let child = add_comp(&mut doc, "Child", 100, 50, LinearRgba::BLACK);
        let s = add_solid(&mut doc, child, RED);
        if big {
            // A solid larger than the child composition.
            let LayerKind::Solid { size, .. } =
                doc.comp(child).unwrap().layer(s).unwrap().kind.clone()
            else {
                panic!()
            };
            set(&mut doc, size.id, Value::Vec2(DVec2::new(300.0, 200.0)));
        }
        let l = nest(&mut doc, main, child);
        SetCollapse { layer: l, collapse }.apply(&mut doc).unwrap();
        doc
    };
    let mut c = Compositor::new(&gpu.device);
    // Content that fits the child looks the same either way.
    let (flat, _) = render_at(&gpu, &mut c, &build(false, false), Time::ZERO, 1.0);
    let (collapsed, _) = render_at(&gpu, &mut c, &build(true, false), Time::ZERO, 1.0);
    assert_close(
        &flat,
        &collapsed,
        1,
        "collapse on vs off, content inside the bounds",
    );
    // Content bigger than the child is clipped normally and visible collapsed.
    let (clipped, _) = render_at(&gpu, &mut c, &build(false, true), Time::ZERO, 1.0);
    let (open, _) = render_at(&gpu, &mut c, &build(true, true), Time::ZERO, 1.0);
    // The child sits at x 50..150, y 25..75 of the parent; its big solid
    // extends right and down from the child's top-left corner.
    assert_eq!(
        pixel(&clipped, 180, 90),
        [0, 0, 0, 255],
        "clipped to the child"
    );
    assert_eq!(
        pixel(&open, 180, 90)[0],
        255,
        "visible outside the child's bounds"
    );
}

#[test]
fn a_static_child_renders_once_and_a_change_renders_again() {
    let Some(gpu) = gpu() else { return };
    let mut doc = parent(200, 100);
    let main = doc.main_comp.unwrap();
    let child = add_comp(&mut doc, "Child", 100, 50, LinearRgba::BLACK);
    let solid = add_solid(&mut doc, child, RED);
    nest(&mut doc, main, child);
    let mut c = Compositor::new(&gpu.device);
    render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert_eq!(c.nested_stats().renders, 1);
    // Another parent time resolves to the same child picture: reused.
    render_at(&gpu, &mut c, &doc, Time::from_secs(2), 1.0);
    let s = c.nested_stats();
    assert_eq!((s.renders, s.reused), (1, 1), "{s:?}");
    // Changing the child's content changes the plan, so it is rendered again.
    let LayerKind::Solid { color, .. } =
        doc.comp(child).unwrap().layer(solid).unwrap().kind.clone()
    else {
        panic!()
    };
    set(
        &mut doc,
        color.id,
        Value::Color(LinearRgba::new(0.0, 1.0, 0.0, 1.0)),
    );
    let (img, _) = render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert_eq!(c.nested_stats().renders, 2, "no stale picture");
    assert_eq!(pixel(&img, 100, 50), [0, 255, 0, 255]);
    // A different preview scale is a different picture.
    render_at(&gpu, &mut c, &doc, Time::ZERO, 0.5);
    assert_eq!(c.nested_stats().renders, 3);
}

#[test]
fn texture_allocations_are_recycled_for_an_animated_child() {
    let Some(gpu) = gpu() else { return };
    let mut doc = parent(64, 64);
    let main = doc.main_comp.unwrap();
    let child = add_comp(&mut doc, "Child", 64, 64, LinearRgba::BLACK);
    let solid = add_solid(&mut doc, child, RED);
    let op = opacity_of(&doc, child, solid);
    nest(&mut doc, main, child);
    let mut c = Compositor::new(&gpu.device);
    for i in 0..8 {
        set(&mut doc, op, Value::Scalar(f64::from(i) / 8.0 + 0.05));
        render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    }
    let s = c.nested_stats();
    assert_eq!(s.renders, 8);
    assert!(s.allocations <= 2, "textures are recycled: {s:?}");
}

#[test]
fn a_missing_image_in_a_child_is_reported_and_retried() {
    let Some(gpu) = gpu() else { return };
    let mut doc = parent(64, 64);
    let main = doc.main_comp.unwrap();
    let child = add_comp(&mut doc, "Child", 64, 64, LinearRgba::BLACK);
    AddImageLayer {
        comp: child,
        path: "does/not/exist.png".into(),
        pixel_size: [8, 8],
        index: 0,
    }
    .apply(&mut doc)
    .unwrap();
    nest(&mut doc, main, child);
    let mut c = Compositor::new(&gpu.device);
    let (_, report) = render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert_eq!(report.missing_assets.len(), 1, "{report:?}");
    // An incomplete picture is not cached: it is rendered again.
    render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert_eq!(c.nested_stats().renders, 2);
    assert_eq!(c.nested_stats().reused, 0);
}

#[test]
fn a_chain_deeper_than_the_limit_renders_without_failing() {
    let Some(gpu) = gpu() else { return };
    let mut doc = parent(32, 32);
    let main = doc.main_comp.unwrap();
    let mut comps = vec![main];
    for i in 0..voxy_core::MAX_NESTING + 2 {
        comps.push(add_comp(
            &mut doc,
            &format!("C{i}"),
            32,
            32,
            LinearRgba::BLACK,
        ));
    }
    for pair in comps.windows(2) {
        nest(&mut doc, pair[0], pair[1]);
    }
    add_solid(&mut doc, *comps.last().unwrap(), RED);
    let mut c = Compositor::new(&gpu.device);
    let (img, report) = render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert!(report.skipped_precomps >= 1, "{report:?}");
    assert_eq!(
        pixel(&img, 16, 16),
        [0, 0, 0, 255],
        "the too-deep part draws nothing"
    );
}

#[test]
fn rendering_a_nested_scene_is_deterministic() {
    let Some(gpu) = gpu() else { return };
    let mut doc = parent(200, 100);
    let main = doc.main_comp.unwrap();
    let child = add_comp(&mut doc, "Child", 100, 50, LinearRgba::BLACK);
    let s = add_solid(&mut doc, child, RED);
    let op = opacity_of(&doc, child, s);
    set(&mut doc, op, Value::Scalar(0.37));
    let l = nest(&mut doc, main, child);
    let p = opacity_of(&doc, main, l);
    set(&mut doc, p, Value::Scalar(0.81));
    let mut a = Compositor::new(&gpu.device);
    let mut b = Compositor::new(&gpu.device);
    let (first, _) = render_at(&gpu, &mut a, &doc, Time::ZERO, 1.0);
    let (again, _) = render_at(&gpu, &mut a, &doc, Time::ZERO, 1.0);
    let (fresh, _) = render_at(&gpu, &mut b, &doc, Time::ZERO, 1.0);
    assert!(first.pixels == again.pixels, "cached picture, same frame");
    assert!(first.pixels == fresh.pixels, "fresh compositor, same frame");
}

fn write_color(path: &std::path::Path, rgba: [u8; 4]) {
    let mut img = image::RgbaImage::new(8, 8);
    for p in img.pixels_mut() {
        *p = image::Rgba(rgba);
    }
    img.save(path).unwrap();
}

#[test]
fn relinking_an_image_inside_a_precomp_refreshes_the_cached_picture() {
    let Some(gpu) = gpu() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.png"), dir.path().join("b.png"));
    write_color(&a, [255, 0, 0, 255]);
    write_color(&b, [0, 255, 0, 255]);
    let mut doc = parent(32, 32);
    let main = doc.main_comp.unwrap();
    let child = add_comp(&mut doc, "Child", 32, 32, LinearRgba::BLACK);
    AddImageLayer {
        comp: child,
        path: a.to_string_lossy().into_owned(),
        pixel_size: [8, 8],
        index: 0,
    }
    .apply(&mut doc)
    .unwrap();
    let asset = doc.comp(child).unwrap().layers[0].kind.asset().unwrap();
    nest(&mut doc, main, child);
    let mut c = Compositor::new(&gpu.device);
    // The 8x8 image sits at the child's center (16, 16).
    let (first, report) = render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert_eq!(report, RenderReport::default());
    assert_eq!(pixel(&first, 16, 16), [255, 0, 0, 255], "the first file");
    // Relink: the same asset now points at another file. The plan is
    // identical, so only the cache's dependency check can notice.
    ReplaceMedia {
        asset,
        path: b.to_string_lossy().into_owned(),
        pixel_size: Some([8, 8]),
        duration: None,
        frame_rate: None,
        audio: None,
        content_hash: None,
    }
    .apply(&mut doc)
    .unwrap();
    let (second, _) = render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert_eq!(
        pixel(&second, 16, 16),
        [0, 255, 0, 255],
        "the picture was rendered again from the new file"
    );
    assert_eq!(c.nested_stats().renders, 2);
}

#[test]
fn a_composition_used_twice_is_rendered_once_per_frame() {
    let Some(gpu) = gpu() else { return };
    let mut doc = parent(200, 100);
    let main = doc.main_comp.unwrap();
    let child = add_comp(&mut doc, "Child", 100, 50, LinearRgba::BLACK);
    add_solid(&mut doc, child, RED);
    nest(&mut doc, main, child);
    nest(&mut doc, main, child);
    let mut c = Compositor::new(&gpu.device);
    render_at(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    let s = c.nested_stats();
    assert_eq!(
        (s.renders, s.reused),
        (1, 1),
        "two uses, one picture: {s:?}"
    );
}
