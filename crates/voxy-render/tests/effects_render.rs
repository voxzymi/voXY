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
            settings: RenderSettings { scale },
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
