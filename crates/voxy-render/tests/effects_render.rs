//! E-B: rendering effect chains on the GPU (goldens are Linux-only in CI,
//! like the other golden tests; elsewhere they run against a local adapter
//! when one exists).

#![allow(clippy::unwrap_used)]

mod common;

use common::{compare, gpu};
use voxy_core::command::{AddEffect, AddSolidLayer, Command, SetProperty};
use voxy_core::glam::DVec2;
use voxy_core::registry::{builtins, ids};
use voxy_core::{
    CompSettings, Document, EffectTypeId, LayerId, LinearRgba, PropId, Time, Value, WorkingSpace,
};
use voxy_render::{
    Compositor, FrameRequest, GpuContext, NoVideo, OutputAlpha, RenderSettings, RgbaImage,
    render_frame,
};

fn set(doc: &mut Document, prop: PropId, value: Value) {
    SetProperty {
        prop,
        time: Time::ZERO,
        value,
    }
    .apply(doc)
    .unwrap();
}

fn add_effect(doc: &mut Document, layer: LayerId, id: &str) -> usize {
    AddEffect {
        layer,
        index: None,
        def: builtins().latest(&EffectTypeId::new(id)).unwrap().clone(),
    }
    .apply(doc)
    .unwrap();
    doc.find_layer(layer).unwrap().1.effects.len() - 1
}

fn param(doc: &Document, layer: LayerId, fx: usize, name: &str) -> PropId {
    doc.find_layer(layer).unwrap().1.effects[fx]
        .param(name)
        .unwrap()
        .id
}

fn set_param(doc: &mut Document, layer: LayerId, fx: usize, name: &str, value: Value) {
    let prop = param(doc, layer, fx, name);
    set(doc, prop, value);
}

/// A background, an opaque solid and a half-transparent solid over it.
fn scene(space: WorkingSpace) -> (Document, Vec<LayerId>) {
    let mut doc = Document::with_comp(
        "Effects",
        CompSettings {
            width: 96,
            height: 64,
            background: LinearRgba::from_srgb8(30, 40, 70, 255),
            ..CompSettings::default()
        },
    );
    doc.meta.working_space = space;
    let comp = doc.main_comp.unwrap();
    let mut layers = Vec::new();
    for (color, size, pos, opacity) in [
        (
            LinearRgba::from_srgb8(230, 120, 40, 255),
            DVec2::new(48.0, 40.0),
            DVec2::new(30.0, 30.0),
            1.0,
        ),
        (
            LinearRgba::from_srgb8(40, 200, 120, 255),
            DVec2::new(40.0, 24.0),
            DVec2::new(60.0, 36.0),
            0.6,
        ),
    ] {
        AddSolidLayer {
            comp,
            name: None,
            color,
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let l = doc.main().unwrap().layers[0].clone();
        if let voxy_core::LayerKind::Solid { size: s, .. } = &l.kind {
            set(&mut doc, s.id, Value::Vec2(size));
        }
        set(&mut doc, l.transform.anchor.id, Value::Vec2(size / 2.0));
        set(&mut doc, l.transform.position.id, Value::Vec2(pos));
        set(&mut doc, l.transform.opacity.id, Value::Scalar(opacity));
        layers.push(l.id);
    }
    (doc, layers)
}

fn render(
    gpu: &GpuContext,
    compositor: &mut Compositor,
    doc: &Document,
    t: Time,
    scale: f64,
) -> RgbaImage {
    let (image, report) = render_frame(
        compositor,
        &gpu.device,
        &gpu.queue,
        doc,
        &NoVideo,
        &FrameRequest {
            comp: doc.main_comp.unwrap(),
            time: t,
            settings: RenderSettings::at_scale(scale),
            alpha: OutputAlpha::Straight,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!((report.unsupported_effects, report.missing_effects), (0, 0));
    image
}

#[test]
fn invert_golden() {
    let Some(gpu) = gpu() else { return };
    let mut c = Compositor::new(&gpu.device);
    for (space, name) in [
        (WorkingSpace::Linear, "effect_invert_linear"),
        (WorkingSpace::SrgbCompat, "effect_invert_srgb_compat"),
    ] {
        let (mut doc, layers) = scene(space);
        add_effect(&mut doc, layers[0], ids::INVERT);
        let fx = add_effect(&mut doc, layers[1], ids::INVERT);
        // The second layer inverts green only, half blended with the original.
        set_param(&mut doc, layers[1], fx, "channel", Value::Enum(2));
        set_param(&mut doc, layers[1], fx, "mix", Value::Scalar(0.5));
        compare(name, &render(&gpu, &mut c, &doc, Time::ZERO, 1.0));
    }
}

#[test]
fn identity_chains_match_the_plain_layer() {
    let Some(gpu) = gpu() else { return };
    let mut c = Compositor::new(&gpu.device);
    let (plain, layers) = scene(WorkingSpace::Linear);
    let expected = render(&gpu, &mut c, &plain, Time::ZERO, 1.0);

    // Inverting twice gives back the input.
    let mut twice = plain.clone();
    add_effect(&mut twice, layers[0], ids::INVERT);
    add_effect(&mut twice, layers[0], ids::INVERT);
    assert_eq!(render(&gpu, &mut c, &twice, Time::ZERO, 1.0), expected);

    // Fully blended with the original, the effect does nothing.
    let mut blended = plain.clone();
    let fx = add_effect(&mut blended, layers[1], ids::INVERT);
    set_param(&mut blended, layers[1], fx, "mix", Value::Scalar(1.0));
    assert_eq!(render(&gpu, &mut c, &blended, Time::ZERO, 1.0), expected);

    // A disabled effect is not rendered at all.
    let mut off = plain.clone();
    add_effect(&mut off, layers[0], ids::INVERT);
    off.layer_mut(layers[0]).unwrap().effects[0].enabled = false;
    assert_eq!(render(&gpu, &mut c, &off, Time::ZERO, 1.0), expected);
}

#[test]
fn chains_render_once_while_static_and_again_when_changed() {
    let Some(gpu) = gpu() else { return };
    let mut c = Compositor::new(&gpu.device);
    let (mut doc, layers) = scene(WorkingSpace::Linear);
    let fx = add_effect(&mut doc, layers[0], ids::INVERT);
    let a = render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    let b = render(&gpu, &mut c, &doc, Time::from_secs(1), 1.0);
    assert_eq!(a, b, "deterministic and static");
    let stats = c.chain_stats();
    assert_eq!(stats.renders, 1, "{stats:?}");
    assert!(stats.reused >= 1, "{stats:?}");

    // Moving the layer reuses the rendered chain.
    let pos = doc.find_layer(layers[0]).unwrap().1.transform.position.id;
    set(&mut doc, pos, Value::Vec2(DVec2::new(40.0, 28.0)));
    render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert_eq!(c.chain_stats().renders, 1);

    // Any parameter change renders it again; so does a resolution change.
    set_param(&mut doc, layers[0], fx, "mix", Value::Scalar(0.3));
    let changed = render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
    assert_eq!(c.chain_stats().renders, 2);
    assert_ne!(changed, b);
    let half = render(&gpu, &mut c, &doc, Time::ZERO, 0.5);
    assert_eq!(c.chain_stats().renders, 3);
    assert_eq!((half.width, half.height), (48, 32));
}

/// A golden: file name, effect type and parameter values.
type Case<'a> = (&'a str, &'a str, Vec<(&'a str, Value)>);

mod blurs {
    use super::*;

    fn blurred(id: &str, params: &[(&str, Value)]) -> (Document, Vec<LayerId>) {
        let (mut doc, layers) = scene(WorkingSpace::Linear);
        for l in &layers {
            let fx = add_effect(&mut doc, *l, id);
            for (name, v) in params {
                set_param(&mut doc, *l, fx, name, v.clone());
            }
        }
        (doc, layers)
    }

    #[test]
    fn blur_goldens() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let cases: [Case<'_>; 6] = [
            (
                "effect_blur_gaussian",
                ids::GAUSSIAN_BLUR,
                vec![("radius", Value::Scalar(8.0))],
            ),
            (
                "effect_blur_box",
                ids::BOX_BLUR,
                vec![
                    ("radius", Value::Scalar(3.0)),
                    ("passes", Value::Int(3)),
                    ("dimensions", Value::Enum(1)),
                ],
            ),
            (
                "effect_blur_directional",
                ids::DIRECTIONAL_BLUR,
                vec![
                    ("direction", Value::Scalar(30.0)),
                    ("length", Value::Scalar(16.0)),
                ],
            ),
            // Centred on each layer (where a new effect's point starts).
            (
                "effect_blur_radial",
                ids::RADIAL_BLUR,
                vec![("angle", Value::Scalar(40.0))],
            ),
            // Off-centre, so the streaks leave the layer on one side.
            (
                "effect_blur_zoom",
                ids::ZOOM_BLUR,
                vec![
                    ("amount", Value::Scalar(0.4)),
                    ("center", Value::Vec2(voxy_core::glam::DVec2::new(4.0, 6.0))),
                ],
            ),
            (
                "effect_blur_repeat_edges",
                ids::GAUSSIAN_BLUR,
                vec![
                    ("radius", Value::Scalar(8.0)),
                    ("repeat_edges", Value::Bool(true)),
                ],
            ),
        ];
        for (name, id, params) in cases {
            let (doc, _) = blurred(id, &params);
            let a = render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
            let b = render(
                &gpu,
                &mut Compositor::new(&gpu.device),
                &doc,
                Time::ZERO,
                1.0,
            );
            assert_eq!(a, b, "{name}: deterministic across compositors");
            compare(name, &a);
        }
    }

    fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.pixels[i..i + 4].try_into().unwrap()
    }

    /// The design's ROI test: a large blur on a solid that fills its layer
    /// must spread past the layer's edge instead of being clipped by it.
    #[test]
    fn a_large_blur_spreads_past_the_layer_edge() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let (plain, _) = scene(WorkingSpace::Linear);
        let (doc, _) = blurred(ids::GAUSSIAN_BLUR, &[("radius", Value::Scalar(24.0))]);
        let before = render(&gpu, &mut c, &plain, Time::ZERO, 1.0);
        let after = render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
        // The orange layer spans x 6..54, y 10..50; (3, 30) is 3 pixels
        // outside its left edge, on the background.
        assert_eq!(pixel(&before, 3, 30), pixel(&before, 1, 1), "background");
        let near = pixel(&after, 3, 30);
        assert!(
            near[0] > pixel(&before, 3, 30)[0] + 20,
            "the blur reaches past the edge: {near:?}"
        );
        // With repeated edges, nothing leaves the layer.
        let (rep, _) = blurred(
            ids::GAUSSIAN_BLUR,
            &[
                ("radius", Value::Scalar(24.0)),
                ("repeat_edges", Value::Bool(true)),
            ],
        );
        let inside = render(&gpu, &mut c, &rep, Time::ZERO, 1.0);
        assert_eq!(pixel(&inside, 3, 30), pixel(&before, 3, 30));
    }

    #[test]
    fn the_look_holds_at_half_preview_resolution() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let (doc, _) = blurred(ids::GAUSSIAN_BLUR, &[("radius", Value::Scalar(10.0))]);
        let full = render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
        let half = render(&gpu, &mut c, &doc, Time::ZERO, 0.5);
        // Box-downsample the full frame and compare.
        let mut sq = 0.0f64;
        for y in 0..half.height {
            for x in 0..half.width {
                let h = pixel(&half, x, y);
                for (ch, hv) in h.iter().take(3).enumerate() {
                    let avg = (0..4)
                        .map(|k| f64::from(pixel(&full, 2 * x + k % 2, 2 * y + k / 2)[ch]))
                        .sum::<f64>()
                        / 4.0;
                    sq += (avg - f64::from(*hv)).powi(2);
                }
            }
        }
        let mse = sq / f64::from(half.width * half.height * 3);
        let psnr = 10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10();
        assert!(psnr > 32.0, "PSNR {psnr:.1} dB");
    }

    #[test]
    fn the_largest_radius_renders() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let (doc, _) = blurred(ids::GAUSSIAN_BLUR, &[("radius", Value::Scalar(1000.0))]);
        let img = render(&gpu, &mut c, &doc, Time::ZERO, 0.25);
        assert_eq!((img.width, img.height), (24, 16));
    }
}

mod color {
    use super::*;

    fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.pixels[i..i + 4].try_into().unwrap()
    }

    const COLOR: [&str; 7] = [
        ids::BRIGHTNESS_CONTRAST,
        ids::EXPOSURE,
        ids::TINT,
        ids::HUE_SATURATION,
        ids::LEVELS,
        ids::CURVES,
        ids::COLOR_BALANCE,
    ];

    #[test]
    fn defaults_pass_through_exactly() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let (plain, layers) = scene(WorkingSpace::SrgbCompat);
        let expected = render(&gpu, &mut c, &plain, Time::ZERO, 1.0);
        for id in COLOR {
            let mut doc = plain.clone();
            add_effect(&mut doc, layers[0], id);
            add_effect(&mut doc, layers[1], id);
            if id == ids::TINT {
                // Tint's default is a full black-to-white tint; zero is the
                // identity.
                for l in &layers {
                    set_param(&mut doc, *l, 0, "amount", Value::Scalar(0.0));
                }
            }
            assert_eq!(
                render(&gpu, &mut c, &doc, Time::ZERO, 1.0),
                expected,
                "{id}"
            );
        }
    }

    fn curve(points: &[(f64, f64)]) -> Value {
        let points = points
            .iter()
            .map(|(x, y)| voxy_core::glam::DVec2::new(*x, *y))
            .collect();
        Value::Curve(std::sync::Arc::new(voxy_core::Curve::new(points).unwrap()))
    }

    #[test]
    fn color_goldens() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let cases: [Case<'_>; 12] = [
            (
                "effect_brightness_contrast",
                ids::BRIGHTNESS_CONTRAST,
                vec![
                    ("brightness", Value::Scalar(0.15)),
                    ("contrast", Value::Scalar(0.5)),
                ],
            ),
            (
                "effect_exposure",
                ids::EXPOSURE,
                vec![
                    ("exposure", Value::Scalar(1.5)),
                    ("gamma", Value::Scalar(1.3)),
                ],
            ),
            (
                "effect_tint",
                ids::TINT,
                vec![
                    (
                        "map_black",
                        Value::Color(LinearRgba::from_srgb8(40, 0, 80, 255)),
                    ),
                    (
                        "map_white",
                        Value::Color(LinearRgba::from_srgb8(255, 230, 150, 255)),
                    ),
                    ("amount", Value::Scalar(0.8)),
                ],
            ),
            (
                "effect_hue_saturation",
                ids::HUE_SATURATION,
                vec![
                    ("hue", Value::Scalar(120.0)),
                    ("saturation", Value::Scalar(0.5)),
                    ("lightness", Value::Scalar(-0.2)),
                ],
            ),
            (
                "effect_hue_colorize",
                ids::HUE_SATURATION,
                vec![
                    ("colorize", Value::Bool(true)),
                    ("colorize_hue", Value::Scalar(200.0)),
                    ("colorize_saturation", Value::Scalar(0.6)),
                ],
            ),
            (
                "effect_levels",
                ids::LEVELS,
                vec![
                    ("input_black", Value::Scalar(0.1)),
                    ("input_white", Value::Scalar(0.8)),
                    ("gamma", Value::Scalar(1.6)),
                    ("output_black", Value::Scalar(0.05)),
                ],
            ),
            // One channel only, with the points crossed (an inverting ramp).
            (
                "effect_levels_red_inverted",
                ids::LEVELS,
                vec![
                    ("channel", Value::Enum(1)),
                    ("input_black", Value::Scalar(0.9)),
                    ("input_white", Value::Scalar(0.1)),
                ],
            ),
            // An S-curve on RGB after lifting the red shadows.
            (
                "effect_curves",
                ids::CURVES,
                vec![
                    (
                        "master",
                        curve(&[(0.0, 0.0), (0.3, 0.2), (0.7, 0.85), (1.0, 1.0)]),
                    ),
                    ("red", curve(&[(0.0, 0.2), (1.0, 1.0)])),
                ],
            ),
            // Warm highlights, cool shadows.
            (
                "effect_color_balance",
                ids::COLOR_BALANCE,
                vec![
                    ("shadow_blue", Value::Scalar(0.3)),
                    ("shadow_red", Value::Scalar(-0.1)),
                    ("high_red", Value::Scalar(0.25)),
                    ("high_blue", Value::Scalar(-0.2)),
                    ("mid_green", Value::Scalar(0.1)),
                ],
            ),
            (
                "effect_color_balance_free",
                ids::COLOR_BALANCE,
                vec![
                    ("mid_red", Value::Scalar(0.4)),
                    ("preserve_luminosity", Value::Bool(false)),
                ],
            ),
            // A duotone-like map with the midtone pushed towards the shadows.
            (
                "effect_gradient_map",
                ids::GRADIENT_MAP,
                vec![
                    (
                        "shadow",
                        Value::Color(LinearRgba::from_srgb8(20, 10, 60, 255)),
                    ),
                    (
                        "midtone",
                        Value::Color(LinearRgba::from_srgb8(200, 60, 90, 255)),
                    ),
                    (
                        "highlight",
                        Value::Color(LinearRgba::from_srgb8(255, 240, 180, 255)),
                    ),
                    ("midpoint", Value::Scalar(0.35)),
                    ("amount", Value::Scalar(0.9)),
                ],
            ),
            (
                "effect_posterize",
                ids::POSTERIZE,
                vec![("levels", Value::Int(3))],
            ),
        ];
        for (name, id, params) in cases {
            let (mut doc, layers) = scene(WorkingSpace::SrgbCompat);
            for l in &layers {
                let fx = add_effect(&mut doc, *l, id);
                for (n, v) in &params {
                    set_param(&mut doc, *l, fx, n, v.clone());
                }
            }
            compare(name, &render(&gpu, &mut c, &doc, Time::ZERO, 1.0));
        }
    }

    /// Exposure computes in linear light, so it means the same in both
    /// working spaces (on opaque pixels, where blending plays no part).
    #[test]
    fn exposure_is_the_same_in_both_working_spaces() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let mut out = Vec::new();
        for space in [WorkingSpace::Linear, WorkingSpace::SrgbCompat] {
            let (mut doc, layers) = scene(space);
            let fx = add_effect(&mut doc, layers[0], ids::EXPOSURE);
            set_param(&mut doc, layers[0], fx, "exposure", Value::Scalar(-1.0));
            out.push(pixel(&render(&gpu, &mut c, &doc, Time::ZERO, 1.0), 12, 20));
        }
        for ch in 0..3 {
            assert!(out[0][ch].abs_diff(out[1][ch]) <= 2, "{out:?}");
        }
    }

    /// Extreme settings, followed by a blur that would spread any infinity
    /// or NaN, still give sane pixels.
    #[test]
    fn extreme_settings_stay_finite() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let (mut doc, layers) = scene(WorkingSpace::Linear);
        let l = layers[0];
        let exposure = add_effect(&mut doc, l, ids::EXPOSURE);
        set_param(&mut doc, l, exposure, "exposure", Value::Scalar(20.0));
        set_param(&mut doc, l, exposure, "gamma", Value::Scalar(0.01));
        let contrast = add_effect(&mut doc, l, ids::BRIGHTNESS_CONTRAST);
        set_param(&mut doc, l, contrast, "brightness", Value::Scalar(1.0));
        set_param(&mut doc, l, contrast, "contrast", Value::Scalar(1.0));
        let blur = add_effect(&mut doc, l, ids::GAUSSIAN_BLUR);
        set_param(&mut doc, l, blur, "radius", Value::Scalar(6.0));
        let img = render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
        // Inside the layer: saturated white, never the black a NaN gives.
        assert_eq!(pixel(&img, 30, 30), [255, 255, 255, 255]);
    }
}

mod sources_and_determinism {
    use super::*;
    use voxy_core::SourceRef;
    use voxy_core::command::{AddImageLayer, AddSourceLayer, CreateComposition};

    fn image_file(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("ramp.png");
        image::RgbaImage::from_fn(16, 8, |x, y| {
            image::Rgba([
                (x * 16) as u8,
                255 - (x * 16) as u8,
                90,
                (32 * y + 31) as u8,
            ])
        })
        .save(&path)
        .unwrap();
        path
    }

    /// Every group 1 kernel on one layer stack, and blend modes on top.
    fn everything(space: WorkingSpace) -> Document {
        let (mut doc, layers) = scene(space);
        for id in [
            ids::GAUSSIAN_BLUR,
            ids::HUE_SATURATION,
            ids::BRIGHTNESS_CONTRAST,
            ids::DIRECTIONAL_BLUR,
        ] {
            add_effect(&mut doc, layers[0], id);
        }
        set_param(&mut doc, layers[0], 1, "hue", Value::Scalar(40.0));
        set_param(&mut doc, layers[0], 2, "contrast", Value::Scalar(0.3));
        for id in [ids::BOX_BLUR, ids::EXPOSURE, ids::TINT, ids::INVERT] {
            add_effect(&mut doc, layers[1], id);
        }
        set_param(&mut doc, layers[1], 1, "exposure", Value::Scalar(0.7));
        doc.layer_mut(layers[1]).unwrap().blend_mode = voxy_core::BlendMode::Overlay;
        doc
    }

    #[test]
    fn a_full_stack_renders_identically_every_time() {
        let Some(gpu) = gpu() else { return };
        for space in [WorkingSpace::Linear, WorkingSpace::SrgbCompat] {
            let doc = everything(space);
            let a = render(
                &gpu,
                &mut Compositor::new(&gpu.device),
                &doc,
                Time::ZERO,
                1.0,
            );
            let mut c = Compositor::new(&gpu.device);
            let b = render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
            // Cached on the second render of the same compositor.
            let c2 = render(&gpu, &mut c, &doc, Time::from_secs(1), 1.0);
            assert_eq!(a, b, "{space:?}");
            assert_eq!(b, c2, "{space:?}: static, so every frame is the same");
        }
    }

    #[test]
    fn effects_apply_to_images_and_precomps() {
        let Some(gpu) = gpu() else { return };
        let dir = tempfile::tempdir().unwrap();
        let mut doc = Document::with_comp(
            "Sources",
            CompSettings {
                width: 64,
                height: 48,
                ..CompSettings::default()
            },
        );
        let main = doc.main_comp.unwrap();
        AddImageLayer {
            comp: main,
            path: image_file(dir.path()).to_string_lossy().into_owned(),
            pixel_size: [16, 8],
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let image_layer = doc.main().unwrap().layers[0].id;

        // A child composition with a solid, used as a precomp.
        let mut report = voxy_core::CommandReport::default();
        CreateComposition {
            settings: CompSettings {
                width: 20,
                height: 20,
                background: LinearRgba::WHITE,
                ..CompSettings::default()
            },
            folder: None,
        }
        .apply_reported(&mut doc, &mut report)
        .unwrap();
        let child = report.created_comps[0];
        AddSolidLayer {
            comp: child,
            name: None,
            color: LinearRgba::new(0.9, 0.2, 0.1, 1.0),
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        AddSourceLayer {
            comp: main,
            source: SourceRef::Comp(child),
            index: 0,
            in_point: Time::ZERO,
        }
        .apply(&mut doc)
        .unwrap();
        let precomp = doc.main().unwrap().layers[0].id;

        let mut c = Compositor::new(&gpu.device);
        c.set_asset_root(Some(dir.path().to_path_buf()));
        let plain = render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
        let mut fx = doc.clone();
        add_effect(&mut fx, image_layer, ids::INVERT);
        add_effect(&mut fx, precomp, ids::GAUSSIAN_BLUR);
        let with = render(&gpu, &mut c, &fx, Time::ZERO, 1.0);
        assert_ne!(with, plain);
        let stats = c.chain_stats();
        assert_eq!(stats.renders, 2, "{stats:?}");

        // A collapsed precomp with effects still renders through its chain
        // (collapse needs the child as one picture to apply effects to).
        if let voxy_core::LayerKind::Precomp { collapse, .. } =
            &mut fx.layer_mut(precomp).unwrap().kind
        {
            *collapse = true;
        }
        let collapsed = render(&gpu, &mut c, &fx, Time::ZERO, 1.0);
        assert_eq!(collapsed, with);
    }
}

mod keys {
    use std::sync::Arc;

    use voxy_core::{EffectId, EffectTypeId, EvaluatedEffect, Value, WorkingSpace};
    use voxy_render::kernels::Kernel;
    use voxy_render::{ChainPlan, DrawSource, EffectNode};

    fn chain(kernel: Kernel, version: u32, params: Vec<(Arc<str>, Value)>, scale: f64) -> u64 {
        ChainPlan::new(
            DrawSource::Solid([1.0, 0.5, 0.25, 1.0]),
            Some(voxy_core::glam::DVec2::new(10.0, 10.0)),
            vec![EffectNode {
                kernel,
                effect: EvaluatedEffect {
                    id: EffectId(1),
                    type_id: EffectTypeId::new(kernel.type_id()),
                    version,
                    params,
                    matte: None,
                    frame: 0,
                    layer_progress: 0.0,
                },
                seed: 0,
            }],
            scale,
            WorkingSpace::Linear,
        )
        .content_key()
    }

    /// Changing any parameter, the effect version or the resolution gives a
    /// different chain key, so no stale frame can be shown (design §3.3).
    #[test]
    fn every_parameter_version_and_scale_changes_the_key() {
        let reg = voxy_core::registry::builtins();
        for kernel in Kernel::ALL {
            let def = reg.latest(&EffectTypeId::new(kernel.type_id())).unwrap();
            let base: Vec<(Arc<str>, Value)> = def
                .params
                .iter()
                .map(|p| (p.name.clone(), p.desc.default.clone()))
                .collect();
            let key = chain(kernel, 1, base.clone(), 1.0);
            assert_ne!(
                chain(kernel, 2, base.clone(), 1.0),
                key,
                "{kernel:?} version"
            );
            assert_ne!(chain(kernel, 1, base.clone(), 0.5), key, "{kernel:?} scale");
            for i in 0..base.len() {
                let mut changed = base.clone();
                changed[i].1 = match &changed[i].1 {
                    Value::Scalar(v) => Value::Scalar(v + 0.125),
                    Value::Int(v) => Value::Int(v + 1),
                    Value::Enum(v) => Value::Enum(v + 1),
                    Value::Bool(b) => Value::Bool(!b),
                    Value::Vec2(v) => Value::Vec2(*v + voxy_core::glam::DVec2::new(0.125, 0.0)),
                    Value::Asset(_) => Value::Asset(Some(voxy_core::AssetId(7))),
                    Value::Layer(_) => Value::Layer(Some(voxy_core::LayerId(7))),
                    Value::Curve(_) => Value::Curve(Arc::new(
                        voxy_core::Curve::new(vec![
                            voxy_core::glam::DVec2::new(0.0, 0.125),
                            voxy_core::glam::DVec2::ONE,
                        ])
                        .unwrap(),
                    )),
                    Value::Color(c) => Value::Color(voxy_core::LinearRgba::new(c.r, c.g, 0.5, c.a)),
                    other => other.clone(),
                };
                assert_ne!(
                    chain(kernel, 1, changed, 1.0),
                    key,
                    "{kernel:?} parameter {}",
                    base[i].0
                );
            }
        }
    }
}

mod lut {
    use super::*;
    use voxy_core::lut::CubeLut;
    use voxy_core::{Asset, AssetKind};

    /// A table that is not linear in any channel, so interpolation shows.
    fn table() -> CubeLut {
        let mut lut = CubeLut::identity(5);
        for e in &mut lut.data {
            let [r, g, b] = *e;
            *e = [g * g, 1.0 - b, f32::midpoint(r, b)];
        }
        lut
    }

    fn cube_text(lut: &CubeLut) -> String {
        let rows: Vec<String> = lut
            .data
            .iter()
            .map(|[r, g, b]| format!("{r} {g} {b}"))
            .collect();
        format!("LUT_3D_SIZE {}\n{}\n", lut.size, rows.join("\n"))
    }

    /// One opaque solid with the LUT effect reading `path`.
    fn doc_with_lut(
        space: WorkingSpace,
        path: &str,
        color: [u8; 3],
    ) -> (Document, voxy_core::AssetId) {
        let mut doc = Document::with_comp(
            "LUT",
            CompSettings {
                width: 8,
                height: 8,
                ..CompSettings::default()
            },
        );
        doc.meta.working_space = space;
        let comp = doc.main_comp.unwrap();
        AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::from_srgb8(color[0], color[1], color[2], 255),
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let layer = doc.main().unwrap().layers[0].id;
        let asset = doc.add_asset(Asset::new(AssetKind::Lut, "table", path));
        let fx = add_effect(&mut doc, layer, ids::LUT);
        set_param(&mut doc, layer, fx, "lut", Value::Asset(Some(asset)));
        (doc, asset)
    }

    fn centre(img: &RgbaImage) -> [u8; 4] {
        let i = ((4 * img.width + 4) * 4) as usize;
        img.pixels[i..i + 4].try_into().unwrap()
    }

    /// The GPU matches `CubeLut::apply` on sRGB-encoded values, in both
    /// working spaces.
    #[test]
    fn the_lut_matches_the_cpu_reference() {
        let Some(gpu) = gpu() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("table.cube");
        let lut = table();
        std::fs::write(&path, cube_text(&lut)).unwrap();
        let path = path.to_string_lossy().replace('\\', "/");
        for space in [WorkingSpace::SrgbCompat, WorkingSpace::Linear] {
            for color in [[230, 120, 40], [12, 200, 250], [128, 128, 128]] {
                let mut c = Compositor::new(&gpu.device);
                let (doc, _) = doc_with_lut(space, &path, color);
                let px = centre(&render(&gpu, &mut c, &doc, Time::ZERO, 1.0));
                // The solid's colour as sRGB-encoded values (what the
                // table expects), through the table, back to 8-bit sRGB.
                let srgb = LinearRgba::from_srgb8(color[0], color[1], color[2], 255).to_srgb();
                let want = lut.apply([srgb[0], srgb[1], srgb[2]]);
                for ch in 0..3 {
                    let w = (want[ch].clamp(0.0, 1.0) * 255.0).round() as u8;
                    assert!(
                        px[ch].abs_diff(w) <= 2,
                        "{space:?} {color:?} channel {ch}: {} vs {w}",
                        px[ch]
                    );
                }
            }
        }
    }

    /// A missing LUT file passes the layer through and is reported.
    #[test]
    fn a_missing_lut_passes_through_and_is_reported() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let (doc, asset) = doc_with_lut(
            WorkingSpace::SrgbCompat,
            "/no/such/table.cube",
            [230, 120, 40],
        );
        let (img, report) = render_frame(
            &mut c,
            &gpu.device,
            &gpu.queue,
            &doc,
            &NoVideo,
            &FrameRequest::export(doc.main_comp.unwrap(), Time::ZERO),
        )
        .unwrap()
        .unwrap();
        assert_eq!(report.missing_assets, vec![asset]);
        assert_eq!(&centre(&img)[..3], &[230, 120, 40]);
    }
}

mod stylize {
    use super::*;

    #[test]
    fn stylize_goldens() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let cases: [Case<'_>; 12] = [
            (
                "effect_vignette",
                ids::VIGNETTE,
                vec![("amount", Value::Scalar(0.8))],
            ),
            (
                "effect_vignette_tinted",
                ids::VIGNETTE,
                vec![
                    ("size", Value::Scalar(0.2)),
                    ("feather", Value::Scalar(0.4)),
                    (
                        "color",
                        Value::Color(LinearRgba::from_srgb8(255, 240, 200, 255)),
                    ),
                ],
            ),
            // Blocks that do not divide the layers, so edge blocks are partial.
            (
                "effect_pixelate",
                ids::PIXELATE,
                vec![("cell", Value::Scalar(7.0))],
            ),
            (
                "effect_grain",
                ids::GRAIN,
                vec![("amount", Value::Scalar(0.25))],
            ),
            (
                "effect_chromatic_aberration",
                ids::CHROMATIC_ABERRATION,
                vec![("amount", Value::Scalar(0.08))],
            ),
            (
                "effect_drop_shadow",
                ids::DROP_SHADOW,
                vec![
                    ("distance", Value::Scalar(6.0)),
                    ("softness", Value::Scalar(4.0)),
                    ("opacity", Value::Scalar(0.8)),
                ],
            ),
            (
                "effect_drop_shadow_only",
                ids::DROP_SHADOW,
                vec![
                    ("direction", Value::Scalar(-60.0)),
                    ("softness", Value::Scalar(0.0)),
                    ("shadow_only", Value::Bool(true)),
                    (
                        "color",
                        Value::Color(LinearRgba::from_srgb8(200, 30, 120, 255)),
                    ),
                ],
            ),
            (
                "effect_stroke",
                ids::STROKE,
                vec![("width", Value::Scalar(3.0))],
            ),
            (
                "effect_stroke_outline_only",
                ids::STROKE,
                vec![
                    ("width", Value::Scalar(2.0)),
                    ("outline_only", Value::Bool(true)),
                    (
                        "color",
                        Value::Color(LinearRgba::from_srgb8(255, 220, 60, 255)),
                    ),
                ],
            ),
            (
                "effect_glow",
                ids::GLOW,
                vec![
                    ("threshold", Value::Scalar(0.15)),
                    ("radius", Value::Scalar(10.0)),
                    ("intensity", Value::Scalar(1.5)),
                ],
            ),
            (
                "effect_inner_shadow",
                ids::INNER_SHADOW,
                vec![
                    ("distance", Value::Scalar(4.0)),
                    ("softness", Value::Scalar(3.0)),
                    ("opacity", Value::Scalar(0.9)),
                ],
            ),
            (
                "effect_grain_colour",
                ids::GRAIN,
                vec![
                    ("amount", Value::Scalar(0.3)),
                    ("size", Value::Scalar(3.0)),
                    ("monochrome", Value::Bool(false)),
                    ("seed", Value::Int(42)),
                ],
            ),
        ];
        for (name, id, params) in cases {
            let (mut doc, layers) = scene(WorkingSpace::SrgbCompat);
            for l in &layers {
                let fx = add_effect(&mut doc, *l, id);
                for (n, v) in &params {
                    set_param(&mut doc, *l, fx, n, v.clone());
                }
            }
            let a = render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
            let b = render(
                &gpu,
                &mut Compositor::new(&gpu.device),
                &doc,
                Time::ZERO,
                1.0,
            );
            assert_eq!(a, b, "{name}: deterministic across compositors");
            compare(name, &a);
        }
    }

    /// Grain: the same frame gives the same noise, another frame other
    /// noise (unless Animate is off), and two layers never share a pattern
    /// (design §3.4).
    #[test]
    fn grain_is_seeded_by_layer_seed_and_frame() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let grainy = |animate: bool| {
            let (mut doc, layers) = scene(WorkingSpace::SrgbCompat);
            for l in &layers {
                let fx = add_effect(&mut doc, *l, ids::GRAIN);
                set_param(&mut doc, *l, fx, "amount", Value::Scalar(0.5));
                set_param(&mut doc, *l, fx, "animate", Value::Bool(animate));
            }
            doc
        };
        let frame = |doc: &Document, n: i64| {
            let rate = doc.main().unwrap().frame_rate;
            render(
                &gpu,
                &mut Compositor::new(&gpu.device),
                doc,
                rate.frame_to_time(n),
                1.0,
            )
        };
        let doc = grainy(true);
        let a = render(&gpu, &mut c, &doc, Time::ZERO, 1.0);
        assert_eq!(a, render(&gpu, &mut c, &doc, Time::ZERO, 1.0), "same frame");
        assert_eq!(a, frame(&doc, 0), "same frame, fresh compositor");
        assert_ne!(a, frame(&doc, 1), "the next frame has new noise");
        let still = grainy(false);
        assert_eq!(frame(&still, 0), frame(&still, 5), "Animate off");

        // Each layer's chain is seeded by its own layer ID.
        let comp = doc.main_comp.unwrap();
        let ids: Vec<u64> = doc
            .comp(comp)
            .unwrap()
            .layers
            .iter()
            .map(|l| l.id.raw())
            .collect();
        let eval = voxy_core::evaluate(&doc, comp, Time::ZERO).unwrap();
        let plan = voxy_render::RenderPlan::build(&eval, &RenderSettings::default());
        let mut seeds: Vec<u64> = plan
            .draws
            .iter()
            .filter_map(|d| match &d.source {
                voxy_render::DrawSource::Chain { chain, .. } => Some(chain.effects[0].seed),
                _ => None,
            })
            .collect();
        let mut want = ids.clone();
        seeds.sort_unstable();
        want.sort_unstable();
        assert_eq!(seeds, want);
    }

    /// Stroke reaches exactly its width out from an opaque edge.
    #[test]
    fn the_stroke_has_its_width() {
        let Some(gpu) = gpu() else { return };
        let mut doc = Document::with_comp(
            "Stroke",
            CompSettings {
                width: 64,
                height: 32,
                ..CompSettings::default()
            },
        );
        doc.meta.working_space = WorkingSpace::SrgbCompat;
        let comp = doc.main_comp.unwrap();
        AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::from_srgb8(0, 0, 0, 255),
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let l = doc.main().unwrap().layers[0].clone();
        if let voxy_core::LayerKind::Solid { size, .. } = &l.kind {
            set(&mut doc, size.id, Value::Vec2(DVec2::new(20.0, 10.0)));
        }
        set(&mut doc, l.transform.anchor.id, Value::Vec2(DVec2::ZERO));
        set(
            &mut doc,
            l.transform.position.id,
            Value::Vec2(DVec2::new(20.0, 10.0)),
        );
        let fx = add_effect(&mut doc, l.id, ids::STROKE);
        set_param(&mut doc, l.id, fx, "width", Value::Scalar(5.0));
        let img = render(
            &gpu,
            &mut Compositor::new(&gpu.device),
            &doc,
            Time::ZERO,
            1.0,
        );
        let at = |x: u32, y: u32| img.pixels[((y * img.width + x) * 4) as usize];
        // The solid covers x 20..40; the white line reaches 5 px past it on
        // each side, and nothing beyond.
        assert_eq!(at(30, 15), 0, "inside: the black solid");
        for x in [16, 19, 40, 44] {
            assert_eq!(at(x, 15), 255, "x = {x} is on the line");
        }
        for x in [13, 47] {
            assert_eq!(at(x, 15), 0, "x = {x} is past the line");
        }
    }
}

mod keying {
    use super::*;

    /// One `w`×`h` solid of `color` at the origin of a transparent
    /// composition, with effect `id`.
    fn keyed(color: [u8; 3], size: (f64, f64), id: &str, params: &[(&str, Value)]) -> RgbaImage {
        let gpu = gpu().unwrap();
        let mut doc = Document::with_comp(
            "Key",
            CompSettings {
                width: 32,
                height: 16,
                background: LinearRgba::new(0.0, 0.0, 0.0, 0.0),
                ..CompSettings::default()
            },
        );
        doc.meta.working_space = WorkingSpace::SrgbCompat;
        let comp = doc.main_comp.unwrap();
        AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::from_srgb8(color[0], color[1], color[2], 255),
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let l = doc.main().unwrap().layers[0].clone();
        if let voxy_core::LayerKind::Solid { size: s, .. } = &l.kind {
            set(&mut doc, s.id, Value::Vec2(DVec2::new(size.0, size.1)));
        }
        set(&mut doc, l.transform.anchor.id, Value::Vec2(DVec2::ZERO));
        set(
            &mut doc,
            l.transform.position.id,
            Value::Vec2(DVec2::new(8.0, 4.0)),
        );
        let fx = add_effect(&mut doc, l.id, id);
        for (n, v) in params {
            set_param(&mut doc, l.id, fx, n, v.clone());
        }
        render(
            &gpu,
            &mut Compositor::new(&gpu.device),
            &doc,
            Time::ZERO,
            1.0,
        )
    }

    fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn chroma_key_removes_the_key_and_its_spill() {
        if gpu().is_none() {
            return;
        }
        let full = (16.0, 8.0);
        // The key colour itself, and a close one, go; orange stays.
        assert_eq!(
            px(&keyed([0, 177, 64], full, ids::CHROMA_KEY, &[]), 12, 8)[3],
            0
        );
        assert_eq!(
            px(&keyed([10, 170, 70], full, ids::CHROMA_KEY, &[]), 12, 8)[3],
            0
        );
        let orange = px(&keyed([230, 120, 40], full, ids::CHROMA_KEY, &[]), 12, 8);
        assert_eq!(orange[3], 255);
        // A greenish grey keeps its alpha but loses the green tint.
        let p = &[
            ("spill", Value::Scalar(1.0)),
            ("tolerance", Value::Scalar(0.0)),
            ("softness", Value::Scalar(0.01)),
        ];
        let grey = px(&keyed([120, 160, 120], full, ids::CHROMA_KEY, p), 12, 8);
        assert_eq!(grey[3], 255);
        assert!(grey[1] < 150 && grey[0] > 120, "{grey:?}");
    }

    #[test]
    fn luma_key_keys_by_brightness() {
        if gpu().is_none() {
            return;
        }
        let full = (16.0, 8.0);
        let dark = [("threshold", Value::Scalar(0.2))];
        assert_eq!(
            px(&keyed([20, 20, 20], full, ids::LUMA_KEY, &dark), 12, 8)[3],
            0
        );
        assert_eq!(
            px(&keyed([200, 200, 200], full, ids::LUMA_KEY, &dark), 12, 8)[3],
            255
        );
        let bright = [("mode", Value::Enum(1)), ("threshold", Value::Scalar(0.6))];
        assert_eq!(
            px(&keyed([230, 230, 230], full, ids::LUMA_KEY, &bright), 12, 8)[3],
            0
        );
        assert_eq!(
            px(&keyed([40, 40, 40], full, ids::LUMA_KEY, &bright), 12, 8)[3],
            255
        );
    }

    /// The solid spans x 8..24; choking by 2 moves each edge in by 2 px,
    /// a negative choke moves it out.
    #[test]
    fn matte_choker_moves_the_edges() {
        if gpu().is_none() {
            return;
        }
        let none = [("softness", Value::Scalar(0.0))];
        let choke = |c: f64| {
            let mut p = none.to_vec();
            p.push(("choke", Value::Scalar(c)));
            keyed([230, 120, 40], (16.0, 8.0), ids::MATTE_CHOKER, &p)
        };
        let shrunk = choke(2.0);
        assert_eq!(px(&shrunk, 9, 8)[3], 0);
        assert_eq!(px(&shrunk, 10, 8)[3], 255);
        assert_eq!(px(&shrunk, 21, 8)[3], 255);
        assert_eq!(px(&shrunk, 22, 8)[3], 0);
        // Shrinking keeps each pixel's own colour.
        assert_eq!(&px(&shrunk, 12, 8)[..3], &[230, 120, 40]);
        let grown = choke(-2.0);
        assert_eq!(px(&grown, 6, 8)[3], 255);
        assert_eq!(px(&grown, 5, 8)[3], 0);
        assert_eq!(
            &px(&grown, 6, 8)[..3],
            &[230, 120, 40],
            "grown edges take their neighbours' colour"
        );
    }

    #[test]
    fn keying_goldens() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let cases: [Case<'_>; 2] = [
            // Keys out the scene's green layer, and softens the orange edge.
            (
                "effect_chroma_key",
                ids::CHROMA_KEY,
                vec![
                    (
                        "key_color",
                        Value::Color(LinearRgba::from_srgb8(40, 200, 120, 255)),
                    ),
                    ("tolerance", Value::Scalar(0.05)),
                ],
            ),
            (
                "effect_matte_choker",
                ids::MATTE_CHOKER,
                vec![
                    ("choke", Value::Scalar(4.0)),
                    ("softness", Value::Scalar(3.0)),
                ],
            ),
        ];
        for (name, id, params) in cases {
            let (mut doc, layers) = scene(WorkingSpace::SrgbCompat);
            for l in &layers {
                let fx = add_effect(&mut doc, *l, id);
                for (n, v) in &params {
                    set_param(&mut doc, *l, fx, n, v.clone());
                }
            }
            compare(name, &render(&gpu, &mut c, &doc, Time::ZERO, 1.0));
        }
    }
}

mod distort {
    use super::*;

    /// Transform moves, flips and removes pixels exactly as its map says.
    #[test]
    fn transform_maps_pixels() {
        let Some(gpu) = gpu() else { return };
        let one = |params: &[(&str, Value)]| {
            let mut doc = Document::with_comp(
                "Transform",
                CompSettings {
                    width: 40,
                    height: 20,
                    background: LinearRgba::new(0.0, 0.0, 0.0, 0.0),
                    ..CompSettings::default()
                },
            );
            let comp = doc.main_comp.unwrap();
            AddSolidLayer {
                comp,
                name: None,
                color: LinearRgba::WHITE,
                index: 0,
            }
            .apply(&mut doc)
            .unwrap();
            let l = doc.main().unwrap().layers[0].clone();
            if let voxy_core::LayerKind::Solid { size, .. } = &l.kind {
                set(&mut doc, size.id, Value::Vec2(DVec2::new(10.0, 10.0)));
            }
            set(&mut doc, l.transform.anchor.id, Value::Vec2(DVec2::ZERO));
            set(
                &mut doc,
                l.transform.position.id,
                Value::Vec2(DVec2::new(10.0, 5.0)),
            );
            let fx = add_effect(&mut doc, l.id, ids::TRANSFORM);
            for (n, v) in params {
                set_param(&mut doc, l.id, fx, n, v.clone());
            }
            let img = render(
                &gpu,
                &mut Compositor::new(&gpu.device),
                &doc,
                Time::ZERO,
                1.0,
            );
            move |x: u32, y: u32| img.pixels[((y * img.width + x) * 4 + 3) as usize]
        };
        // The solid covers x 10..20, y 5..15; anchor and position start at
        // its centre, (5, 5) in layer pixels. Position (7, 5) moves it 2 px
        // right.
        let moved = one(&[("position", Value::Vec2(DVec2::new(7.0, 5.0)))]);
        assert_eq!(moved(10, 10), 0);
        assert_eq!(moved(12, 10), 255);
        assert_eq!(moved(21, 10), 255);
        assert_eq!(moved(22, 10), 0);
        // Half width, around the centre: x 12.5..17.5.
        let narrow = one(&[("scale", Value::Vec2(DVec2::new(0.5, 1.0)))]);
        assert_eq!(narrow(11, 10), 0);
        assert_eq!(narrow(15, 10), 255);
        assert_eq!(narrow(18, 10), 0);
        // Scaled to nothing: nothing is drawn.
        let gone = one(&[("scale", Value::Vec2(DVec2::new(0.0, 1.0)))]);
        assert_eq!(gone(15, 10), 0);
        // Opacity applies to the transformed pixels.
        let faint = one(&[("opacity", Value::Scalar(0.5))]);
        assert!((127..=129).contains(&faint(15, 10)));
    }

    /// Tiles of a solid meet without seams, and the tiled area can grow
    /// past the layer.
    #[test]
    fn tiles_meet_without_seams() {
        let Some(gpu) = gpu() else { return };
        let mut doc = Document::with_comp(
            "Tile",
            CompSettings {
                width: 48,
                height: 32,
                background: LinearRgba::new(0.0, 0.0, 0.0, 0.0),
                ..CompSettings::default()
            },
        );
        let comp = doc.main_comp.unwrap();
        AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let l = doc.main().unwrap().layers[0].clone();
        if let voxy_core::LayerKind::Solid { size, .. } = &l.kind {
            set(&mut doc, size.id, Value::Vec2(DVec2::new(16.0, 16.0)));
        }
        set(&mut doc, l.transform.anchor.id, Value::Vec2(DVec2::ZERO));
        set(
            &mut doc,
            l.transform.position.id,
            Value::Vec2(DVec2::new(16.0, 8.0)),
        );
        let fx = add_effect(&mut doc, l.id, ids::TILE);
        set_param(
            &mut doc,
            l.id,
            fx,
            "tile_size",
            Value::Vec2(DVec2::new(0.3, 0.45)),
        );
        set_param(
            &mut doc,
            l.id,
            fx,
            "output_size",
            Value::Vec2(DVec2::new(2.0, 1.0)),
        );
        let img = render(
            &gpu,
            &mut Compositor::new(&gpu.device),
            &doc,
            Time::ZERO,
            1.0,
        );
        let alpha = |x: u32, y: u32| img.pixels[((y * img.width + x) * 4 + 3) as usize];
        // The tiled area: twice the layer's width around its centre,
        // x 8..40, y 8..24.
        for y in 8..24 {
            for x in 8..40 {
                assert_eq!(alpha(x, y), 255, "seam at ({x}, {y})");
            }
        }
        assert_eq!(alpha(7, 12), 0);
        assert_eq!(alpha(40, 12), 0);
    }

    #[test]
    fn distort_goldens() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        let cases: [Case<'_>; 7] = [
            (
                "effect_transform",
                ids::TRANSFORM,
                vec![
                    ("rotation", Value::Scalar(20.0)),
                    ("scale", Value::Vec2(DVec2::new(0.8, 1.1))),
                    ("opacity", Value::Scalar(0.9)),
                ],
            ),
            (
                "effect_mirror",
                ids::MIRROR,
                vec![("angle", Value::Scalar(30.0))],
            ),
            (
                "effect_wave",
                ids::WAVE,
                vec![
                    ("amplitude", Value::Scalar(3.0)),
                    ("wavelength", Value::Scalar(16.0)),
                    ("direction", Value::Scalar(90.0)),
                ],
            ),
            (
                "effect_ripple",
                ids::RIPPLE,
                vec![
                    ("amplitude", Value::Scalar(2.0)),
                    ("wavelength", Value::Scalar(8.0)),
                ],
            ),
            (
                "effect_tile",
                ids::TILE,
                vec![("tile_size", Value::Vec2(DVec2::new(0.4, 0.5)))],
            ),
            (
                "effect_tile_mirrored_wide",
                ids::TILE,
                vec![
                    ("tile_size", Value::Vec2(DVec2::new(0.3, 0.3))),
                    ("output_size", Value::Vec2(DVec2::new(1.6, 1.2))),
                    ("mirror", Value::Bool(true)),
                ],
            ),
            (
                // Large enough to cross the solids' edges, where it shows.
                "effect_bulge",
                ids::BULGE,
                vec![
                    ("radius", Value::Scalar(26.0)),
                    ("amount", Value::Scalar(0.8)),
                ],
            ),
        ];
        for (name, id, params) in cases {
            let (mut doc, layers) = scene(WorkingSpace::SrgbCompat);
            for l in &layers {
                let fx = add_effect(&mut doc, *l, id);
                for (n, v) in &params {
                    set_param(&mut doc, *l, fx, n, v.clone());
                }
            }
            compare(name, &render(&gpu, &mut c, &doc, Time::ZERO, 1.0));
        }
    }
}

mod transitions {
    use super::*;

    /// A 20×10 white solid at (10, 5) in a transparent 40×20 composition,
    /// with transition `id`.
    fn doc(id: &str, params: &[(&str, Value)]) -> Document {
        let mut doc = Document::with_comp(
            "Transition",
            CompSettings {
                width: 40,
                height: 20,
                background: LinearRgba::new(0.0, 0.0, 0.0, 0.0),
                ..CompSettings::default()
            },
        );
        let comp = doc.main_comp.unwrap();
        AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        }
        .apply(&mut doc)
        .unwrap();
        let l = doc.main().unwrap().layers[0].clone();
        if let voxy_core::LayerKind::Solid { size, .. } = &l.kind {
            set(&mut doc, size.id, Value::Vec2(DVec2::new(20.0, 10.0)));
        }
        set(&mut doc, l.transform.anchor.id, Value::Vec2(DVec2::ZERO));
        set(
            &mut doc,
            l.transform.position.id,
            Value::Vec2(DVec2::new(10.0, 5.0)),
        );
        let fx = add_effect(&mut doc, l.id, id);
        for (n, v) in params {
            set_param(&mut doc, l.id, fx, n, v.clone());
        }
        doc
    }

    fn alpha_row(doc: &Document, t: Time) -> Vec<u8> {
        let gpu = gpu().unwrap();
        let img = render(&gpu, &mut Compositor::new(&gpu.device), doc, t, 1.0);
        (0..img.width)
            .map(|x| img.pixels[((10 * img.width + x) * 4 + 3) as usize])
            .collect()
    }

    fn manual(t: f64) -> Vec<(&'static str, Value)> {
        vec![("auto", Value::Bool(false)), ("progress", Value::Scalar(t))]
    }

    #[test]
    fn transitions_move_from_the_layer_to_nothing() {
        if gpu().is_none() {
            return;
        }
        // Crossfade at a quarter: three quarters left.
        let a = alpha_row(&doc(ids::CROSSFADE, &manual(0.25)), Time::ZERO);
        assert!((190..=192).contains(&a[15]), "{}", a[15]);

        // Wipe halfway, no feather: the left half of the layer is gone.
        let mut p = manual(0.5);
        p.push(("feather", Value::Scalar(0.0)));
        let a = alpha_row(&doc(ids::WIPE, &p), Time::ZERO);
        assert_eq!((a[12], a[18], a[21], a[28]), (0, 0, 255, 255), "{a:?}");

        // Slide halfway to the right: the layer's right half shows its
        // left half; nothing leaves the frame.
        let a = alpha_row(&doc(ids::SLIDE, &manual(0.5)), Time::ZERO);
        assert_eq!((a[15], a[21], a[29], a[31]), (0, 255, 255, 0), "{a:?}");

        // Zoom at the start changes nothing; halfway it is half faded.
        let a = alpha_row(&doc(ids::ZOOM, &manual(0.5)), Time::ZERO);
        assert!((127..=129).contains(&a[20]), "{}", a[20]);
        assert_eq!(a[5], 0);
    }

    /// With Auto on, a transition runs over the layer's own duration.
    #[test]
    fn auto_follows_the_layer_duration() {
        if gpu().is_none() {
            return;
        }
        let d = doc(ids::CROSSFADE, &[]);
        let layer = &d.main().unwrap().layers[0];
        let (start, end) = (layer.in_point, layer.out_point);
        let mid = Time::from_flicks(i64::midpoint(start.flicks(), end.flicks()));
        assert_eq!(alpha_row(&d, start)[20], 255, "start: all there");
        let half = alpha_row(&d, mid)[20];
        assert!((126..=129).contains(&half), "middle: {half}");
        // The plan's key moves with time only while Auto is on.
        let key = |d: &Document, t: Time| {
            let eval = voxy_core::evaluate(d, d.main_comp.unwrap(), t).unwrap();
            voxy_render::RenderPlan::build(&eval, &RenderSettings::default()).content_key()
        };
        assert_ne!(key(&d, start), key(&d, mid));
        let still = doc(ids::CROSSFADE, &manual(0.3));
        assert_eq!(key(&still, start), key(&still, mid));
    }

    #[test]
    fn transition_goldens() {
        let Some(gpu) = gpu() else { return };
        let mut c = Compositor::new(&gpu.device);
        for (name, id, extra) in [
            (
                "effect_wipe",
                ids::WIPE,
                vec![("angle", Value::Scalar(30.0))],
            ),
            (
                "effect_slide",
                ids::SLIDE,
                vec![("direction", Value::Scalar(-90.0))],
            ),
            ("effect_zoom_transition", ids::ZOOM, vec![]),
        ] {
            let (mut doc, layers) = scene(WorkingSpace::SrgbCompat);
            for l in &layers {
                let fx = add_effect(&mut doc, *l, id);
                for (n, v) in manual(0.4).into_iter().chain(extra.clone()) {
                    set_param(&mut doc, *l, fx, n, v);
                }
            }
            compare(name, &render(&gpu, &mut c, &doc, Time::ZERO, 1.0));
        }
    }
}
