//! E-D group 6: adjustment layers (design §1.5) on the GPU, checked against
//! exact expectations and the CPU blend reference.

#![allow(clippy::unwrap_used)]

mod common;

use common::{compare, gpu};
use voxy_core::command::{AddAdjustmentLayer, AddEffect, AddSolidLayer, Command, SetProperty};
use voxy_core::glam::DVec2;
use voxy_core::registry::{builtins, ids};
use voxy_core::{
    BlendMode, CompSettings, Document, EffectTypeId, LayerId, LinearRgba, Time, Value, WorkingSpace,
};
use voxy_render::{Compositor, FrameRequest, NoVideo, RgbaImage, render_frame};

fn set(doc: &mut Document, prop: voxy_core::PropId, value: Value) {
    SetProperty {
        prop,
        time: Time::ZERO,
        value,
    }
    .apply(doc)
    .unwrap();
}

/// A 32×16 composition with a navy background and an orange solid over
/// its left half (sRGB-compatible, so 8-bit values compare directly).
fn base() -> Document {
    let mut doc = Document::with_comp(
        "Adjust",
        CompSettings {
            width: 32,
            height: 16,
            background: LinearRgba::from_srgb8(30, 40, 70, 255),
            ..CompSettings::default()
        },
    );
    doc.meta.working_space = WorkingSpace::SrgbCompat;
    solid(&mut doc, [230, 120, 40], 0);
    doc
}

/// A 16×16 solid at the left, at stack `index` (0 = top).
fn solid(doc: &mut Document, c: [u8; 3], index: usize) -> LayerId {
    let comp = doc.main_comp.unwrap();
    AddSolidLayer {
        comp,
        name: None,
        color: LinearRgba::from_srgb8(c[0], c[1], c[2], 255),
        index,
    }
    .apply(doc)
    .unwrap();
    let l = doc.main().unwrap().layers[index].clone();
    if let voxy_core::LayerKind::Solid { size, .. } = &l.kind {
        set(doc, size.id, Value::Vec2(DVec2::new(16.0, 16.0)));
    }
    set(doc, l.transform.anchor.id, Value::Vec2(DVec2::ZERO));
    set(doc, l.transform.position.id, Value::Vec2(DVec2::ZERO));
    l.id
}

/// Adds an adjustment layer on top with `effect`.
fn adjust(doc: &mut Document, effect: &str) -> LayerId {
    let comp = doc.main_comp.unwrap();
    AddAdjustmentLayer {
        comp,
        name: None,
        index: 0,
    }
    .apply(doc)
    .unwrap();
    let id = doc.main().unwrap().layers[0].id;
    AddEffect {
        layer: id,
        index: None,
        def: builtins()
            .latest(&EffectTypeId::new(effect))
            .unwrap()
            .clone(),
    }
    .apply(doc)
    .unwrap();
    id
}

fn render(doc: &Document) -> RgbaImage {
    let gpu = gpu().unwrap();
    let mut c = Compositor::new(&gpu.device);
    render_frame(
        &mut c,
        &gpu.device,
        &gpu.queue,
        doc,
        &NoVideo,
        &FrameRequest::export(doc.main_comp.unwrap(), Time::ZERO),
    )
    .unwrap()
    .unwrap()
    .0
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.width + x) * 4) as usize;
    img.pixels[i..i + 4].try_into().unwrap()
}

fn close(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tol)
}

#[test]
fn an_adjustment_layer_changes_what_is_below_but_not_the_background() {
    if gpu().is_none() {
        return;
    }
    let mut doc = base();
    adjust(&mut doc, ids::INVERT);
    let img = render(&doc);
    assert_eq!(px(&img, 8, 8), [25, 135, 215, 255], "the solid, inverted");
    assert_eq!(
        px(&img, 24, 8),
        [30, 40, 70, 255],
        "the background is not a layer"
    );

    // A layer above the adjustment layer is not affected.
    let mut above = base();
    adjust(&mut above, ids::INVERT);
    solid(&mut above, [200, 30, 90], 0);
    assert_eq!(px(&render(&above), 8, 8), [200, 30, 90, 255]);

    // Without effects (or with them off) it does nothing.
    let mut off = base();
    let a = adjust(&mut off, ids::INVERT);
    off.layer_mut(a).unwrap().effects[0].enabled = false;
    assert_eq!(px(&render(&off), 8, 8), [230, 120, 40, 255]);
}

#[test]
fn opacity_mixes_and_blend_modes_blend_the_result_with_what_is_below() {
    if gpu().is_none() {
        return;
    }
    let mut doc = base();
    let a = adjust(&mut doc, ids::INVERT);
    let opacity = doc.find_layer(a).unwrap().1.transform.opacity.id;
    set(&mut doc, opacity, Value::Scalar(0.5));
    let half = px(&render(&doc), 8, 8);
    // Halfway between (230, 120, 40) and (25, 135, 215).
    assert!(close(half, [128, 128, 128, 255], 2), "{half:?}");

    let mut multiply = base();
    let a = adjust(&mut multiply, ids::INVERT);
    multiply.layer_mut(a).unwrap().blend_mode = BlendMode::Multiply;
    let got = px(&render(&multiply), 8, 8);
    let below = [230.0, 120.0, 40.0].map(|v: f64| v / 255.0);
    let fx = below.map(|v| 1.0 - v);
    let want = voxy_render::blend::composite(
        BlendMode::Multiply,
        [below[0], below[1], below[2], 1.0],
        [fx[0], fx[1], fx[2], 1.0],
    );
    let want = [want[0], want[1], want[2], 1.0].map(|v| (v * 255.0).round() as u8);
    assert!(close(got, want, 2), "{got:?} vs {want:?}");
}

/// A blur on an adjustment layer blurs the edge between the solid and the
/// background-free transparency, then the background goes under.
#[test]
fn adjustment_golden() {
    if gpu().is_none() {
        return;
    }
    let mut doc = base();
    solid(&mut doc, [40, 200, 120], 0);
    let green = doc.main().unwrap().layers[0].clone();
    set(
        &mut doc,
        green.transform.position.id,
        Value::Vec2(DVec2::new(10.0, 4.0)),
    );
    let a = adjust(&mut doc, ids::GAUSSIAN_BLUR);
    let radius = doc.find_layer(a).unwrap().1.effects[0].params[0].id;
    set(&mut doc, radius, Value::Scalar(4.0));
    compare("adjustment_blur", &render(&doc));
}
