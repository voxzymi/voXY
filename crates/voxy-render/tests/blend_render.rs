//! E-B: blend modes on the GPU against the CPU reference
//! (`voxy_render::blend`), plus one golden.

#![allow(clippy::unwrap_used)]

mod common;

use common::{compare, gpu};
use voxy_core::command::{AddSolidLayer, Command, SetProperty};
use voxy_core::glam::DVec2;
use voxy_core::{
    BlendMode, CompSettings, Document, LinearRgba, Time, Value, WorkingSpace, linear_to_srgb,
    srgb_to_linear,
};
use voxy_render::blend::composite;
use voxy_render::{Compositor, FrameRequest, GpuContext, NoVideo, RgbaImage, render_frame};

/// Colour pairs (backdrop, source, source opacity) that reach every branch:
/// dark and light sources and backdrops, greys, saturated colours, and a
/// half-transparent source.
const PAIRS: [([f32; 3], [f32; 3], f64); 5] = [
    ([0.2, 0.5, 0.8], [0.6, 0.5, 0.1], 1.0),
    ([0.9, 0.1, 0.3], [0.2, 0.8, 0.7], 1.0),
    ([0.5, 0.5, 0.5], [0.95, 0.05, 0.6], 1.0),
    ([0.1, 0.15, 0.05], [0.3, 0.3, 0.3], 1.0),
    ([0.7, 0.4, 0.2], [0.1, 0.6, 0.9], 0.5),
];

/// A 4×4 composition: an opaque backdrop and one solid blended over it.
fn scene(space: WorkingSpace, b: [f32; 3], s: [f32; 3], opacity: f64, mode: BlendMode) -> Document {
    // Colours are given as working-space values; the model stores linear.
    let lin = |c: [f32; 3]| match space {
        WorkingSpace::Linear => LinearRgba::new(c[0], c[1], c[2], 1.0),
        WorkingSpace::SrgbCompat => LinearRgba::new(
            srgb_to_linear(c[0]),
            srgb_to_linear(c[1]),
            srgb_to_linear(c[2]),
            1.0,
        ),
    };
    let mut doc = Document::with_comp(
        "Blend",
        CompSettings {
            width: 4,
            height: 4,
            background: lin(b),
            ..CompSettings::default()
        },
    );
    doc.meta.working_space = space;
    let comp = doc.main_comp.unwrap();
    AddSolidLayer {
        comp,
        name: None,
        color: lin(s),
        index: 0,
    }
    .apply(&mut doc)
    .unwrap();
    let l = doc.main().unwrap().layers[0].clone();
    SetProperty {
        prop: l.transform.opacity.id,
        time: Time::ZERO,
        value: Value::Scalar(opacity),
    }
    .apply(&mut doc)
    .unwrap();
    doc.layer_mut(l.id).unwrap().blend_mode = mode;
    doc
}

fn render(gpu: &GpuContext, c: &mut Compositor, doc: &Document) -> RgbaImage {
    render_frame(
        c,
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

#[test]
fn every_mode_matches_the_reference() {
    let Some(gpu) = gpu() else { return };
    let mut c = Compositor::new(&gpu.device);
    let mut worst = 0u8;
    for space in [WorkingSpace::SrgbCompat, WorkingSpace::Linear] {
        for mode in BlendMode::ALL {
            for (b, s, opacity) in PAIRS {
                let img = render(&gpu, &mut c, &scene(space, b, s, opacity, mode));
                let px = &img.pixels[(5 * 4)..(5 * 4 + 4)]; // pixel (1, 1)
                let f = |v: [f32; 3]| v.map(f64::from);
                let dst = [f(b)[0], f(b)[1], f(b)[2], 1.0];
                let src = [
                    f(s)[0] * opacity,
                    f(s)[1] * opacity,
                    f(s)[2] * opacity,
                    opacity,
                ];
                let out = composite(mode, dst, src);
                for ch in 0..3 {
                    let mut v = (out[ch] / out[3]).clamp(0.0, 1.0);
                    if space == WorkingSpace::Linear {
                        v = f64::from(linear_to_srgb(v as f32));
                    }
                    let want = (v * 255.0).round() as u8;
                    let d = px[ch].abs_diff(want);
                    worst = worst.max(d);
                    assert!(
                        d <= 2,
                        "{space:?} {mode:?} b={b:?} s={s:?} α={opacity}: channel {ch} is {}, want {want}",
                        px[ch]
                    );
                }
                assert_eq!(px[3], 255);
            }
        }
    }
    eprintln!("largest difference from the reference: {worst}/255");
}

#[test]
fn blend_modes_golden() {
    let Some(gpu) = gpu() else { return };
    let mut c = Compositor::new(&gpu.device);
    // One column per mode: a horizontal ramp of backdrops (four solids)
    // under a vertical stack of three sources.
    let cols = BlendMode::ALL.len() as u32;
    let mut doc = Document::with_comp(
        "Blend modes",
        CompSettings {
            width: cols * 8,
            height: 24,
            background: LinearRgba::from_srgb8(40, 60, 90, 255),
            ..CompSettings::default()
        },
    );
    doc.meta.working_space = WorkingSpace::SrgbCompat;
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
        if let voxy_core::LayerKind::Solid { size: sp, .. } = &l.kind {
            SetProperty {
                prop: sp.id,
                time: Time::ZERO,
                value: Value::Vec2(size),
            }
            .apply(doc)
            .unwrap();
        }
        for (p, v) in [
            (l.transform.anchor.id, DVec2::ZERO),
            (l.transform.position.id, pos),
        ] {
            SetProperty {
                prop: p,
                time: Time::ZERO,
                value: Value::Vec2(v),
            }
            .apply(doc)
            .unwrap();
        }
        l.id
    };
    let w = f64::from(cols * 8);
    for (i, shade) in [30u8, 110, 180, 245].iter().enumerate() {
        solid(
            &mut doc,
            LinearRgba::from_srgb8(*shade, shade / 2 + 60, 255 - shade, 255),
            DVec2::new(w, 6.0),
            DVec2::new(0.0, 6.0 * i as f64),
        );
    }
    for (m, mode) in BlendMode::ALL.iter().enumerate() {
        for (k, color) in [
            LinearRgba::from_srgb8(230, 80, 40, 255),
            LinearRgba::from_srgb8(60, 220, 120, 255),
            LinearRgba::from_srgb8(128, 128, 128, 255),
        ]
        .into_iter()
        .enumerate()
        {
            let id = solid(
                &mut doc,
                color,
                DVec2::new(6.0, 8.0),
                DVec2::new(8.0 * m as f64 + 1.0, 8.0 * k as f64),
            );
            doc.layer_mut(id).unwrap().blend_mode = *mode;
        }
    }
    compare("blend_modes", &render(&gpu, &mut c, &doc));
}
