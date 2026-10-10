//! E-B: planning layers with effects (no GPU needed).

#![allow(clippy::unwrap_used)]

use voxy_core::command::{AddEffect, AddSolidLayer, Command, SetProperty};
use voxy_core::registry::{builtins, ids};
use voxy_core::{
    CompId, CompSettings, Document, EffectTypeId, LayerId, LinearRgba, MissingReason, Time, Value,
    evaluate,
};
use voxy_render::kernels::{Kernel, Rect, chain_rod, chain_roi};
use voxy_render::{DrawSource, EffectsView, RenderPlan, RenderSettings};

fn doc() -> (Document, CompId, LayerId) {
    let mut doc = Document::with_comp(
        "Plan",
        CompSettings {
            width: 100,
            height: 50,
            ..CompSettings::default()
        },
    );
    let comp = doc.main_comp.unwrap();
    AddSolidLayer {
        comp,
        name: None,
        color: LinearRgba::new(1.0, 0.5, 0.0, 1.0),
        index: 0,
    }
    .apply(&mut doc)
    .unwrap();
    let layer = doc.main().unwrap().layers[0].id;
    (doc, comp, layer)
}

fn add(doc: &mut Document, layer: LayerId, id: &str) {
    AddEffect {
        layer,
        index: None,
        def: builtins().latest(&EffectTypeId::new(id)).unwrap().clone(),
    }
    .apply(doc)
    .unwrap();
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

fn plan(doc: &Document, comp: CompId, t: Time, scale: f64) -> RenderPlan {
    RenderPlan::build(
        &evaluate(doc, comp, t).unwrap(),
        &RenderSettings::at_scale(scale),
    )
}

fn chain_key(p: &RenderPlan) -> u64 {
    match &p.draws[0].source {
        DrawSource::Chain { chain, .. } => chain.content_key(),
        other => panic!("not a chain: {other:?}"),
    }
}

#[test]
fn a_layer_with_effects_becomes_a_chain() {
    let (mut doc, comp, layer) = doc();
    let opacity = doc.main().unwrap().layers[0].transform.opacity.id;
    set(&mut doc, opacity, Value::Scalar(0.5));
    add(&mut doc, layer, ids::INVERT);
    let p = plan(&doc, comp, Time::ZERO, 1.0);
    assert_eq!(p.draws.len(), 1);
    let DrawSource::Chain { chain, opacity } = &p.draws[0].source else {
        panic!("expected a chain");
    };
    // Effects see the source at full opacity; opacity applies afterwards.
    assert_eq!(*opacity, 0.5);
    assert_eq!(chain.source, DrawSource::Solid([1.0, 0.5, 0.0, 1.0]));
    assert_eq!(chain.effects.len(), 1);
    assert_eq!(chain.effects[0].kernel, Kernel::Invert);
    assert_eq!((p.unsupported_effects, p.missing_effects), (0, 0));
}

#[test]
fn effects_without_kernels_and_placeholders_pass_through() {
    let (mut doc, comp, layer) = doc();
    // A version this build has no kernel for (a newer Tint).
    let mut newer = (**builtins().latest(&EffectTypeId::new(ids::TINT)).unwrap()).clone();
    newer.version = 2;
    AddEffect {
        layer,
        index: None,
        def: std::sync::Arc::new(newer),
    }
    .apply(&mut doc)
    .unwrap();
    add(&mut doc, layer, ids::TINT);
    doc.layer_mut(layer).unwrap().effects[1].mark_missing(MissingReason::UnknownType);
    let p = plan(&doc, comp, Time::ZERO, 1.0);
    assert!(matches!(p.draws[0].source, DrawSource::Solid(_)));
    assert_eq!((p.unsupported_effects, p.missing_effects), (1, 1));
}

/// Kernels of each draw, top layer first (`None` for a plain draw).
fn kernels(p: &RenderPlan) -> Vec<Option<Vec<Kernel>>> {
    p.draws
        .iter()
        .rev()
        .map(|d| match &d.source {
            DrawSource::Chain { chain, .. } => {
                Some(chain.effects.iter().map(|n| n.kernel).collect())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn solo_and_bypass_are_preview_only_views() {
    let (mut doc, comp, under) = doc();
    add(&mut doc, under, ids::TINT);
    AddSolidLayer {
        comp,
        name: None,
        color: LinearRgba::new(0.0, 0.5, 1.0, 1.0),
        index: 0,
    }
    .apply(&mut doc)
    .unwrap();
    let top = doc.main().unwrap().layers[0].id;
    add(&mut doc, top, ids::INVERT);
    add(&mut doc, top, ids::GAUSSIAN_BLUR);
    let blur = doc.find_layer(top).unwrap().1.effects[1].id;
    let eval = evaluate(&doc, comp, Time::ZERO).unwrap();
    let with = |effects| {
        RenderPlan::build(
            &eval,
            &RenderSettings {
                effects,
                ..RenderSettings::default()
            },
        )
    };
    let all = with(EffectsView::All);
    assert_eq!(all, RenderPlan::build(&eval, &RenderSettings::default()));
    assert_eq!(
        kernels(&all),
        [
            Some(vec![Kernel::Invert, Kernel::GaussianBlur]),
            Some(vec![Kernel::Tint])
        ]
    );
    // Bypass: every layer draws its plain source.
    assert_eq!(kernels(&with(EffectsView::Bypass)), [None, None]);
    // Solo: only that effect on its layer; other layers are untouched.
    assert_eq!(
        kernels(&with(EffectsView::Solo(blur))),
        [Some(vec![Kernel::GaussianBlur]), Some(vec![Kernel::Tint])]
    );
    // Soloing an effect that no longer exists changes nothing.
    let gone = voxy_core::EffectId(u64::MAX);
    assert_eq!(with(EffectsView::Solo(gone)), all);
}

#[test]
fn chain_keys_follow_content_not_placement_or_time() {
    let (mut doc, comp, layer) = doc();
    add(&mut doc, layer, ids::INVERT);
    let base = chain_key(&plan(&doc, comp, Time::ZERO, 1.0));
    // Static: the same at another frame.
    assert_eq!(chain_key(&plan(&doc, comp, Time::from_secs(2), 1.0)), base);
    // Moving the layer does not change what the chain renders.
    let pos = doc.main().unwrap().layers[0].transform.position.id;
    set(
        &mut doc,
        pos,
        Value::Vec2(voxy_core::glam::DVec2::new(10.0, 3.0)),
    );
    assert_eq!(chain_key(&plan(&doc, comp, Time::ZERO, 1.0)), base);
    // The preview scale does.
    assert_ne!(chain_key(&plan(&doc, comp, Time::ZERO, 0.5)), base);
    // So does every parameter.
    let mix = doc.main().unwrap().layers[0].effects[0]
        .param("mix")
        .unwrap()
        .id;
    set(&mut doc, mix, Value::Scalar(0.25));
    let mixed = chain_key(&plan(&doc, comp, Time::ZERO, 1.0));
    assert_ne!(mixed, base);
    let channel = doc.main().unwrap().layers[0].effects[0]
        .param("channel")
        .unwrap()
        .id;
    set(&mut doc, channel, Value::Enum(3));
    assert_ne!(chain_key(&plan(&doc, comp, Time::ZERO, 1.0)), mixed);
    // And the source.
    let color = match &doc.main().unwrap().layers[0].kind {
        voxy_core::LayerKind::Solid { color, .. } => color.id,
        _ => unreachable!(),
    };
    let before = chain_key(&plan(&doc, comp, Time::ZERO, 1.0));
    set(&mut doc, color, Value::Color(LinearRgba::WHITE));
    assert_ne!(chain_key(&plan(&doc, comp, Time::ZERO, 1.0)), before);
}

#[test]
fn per_pixel_kernels_keep_regions() {
    let (mut doc, comp, layer) = doc();
    add(&mut doc, layer, ids::INVERT);
    let eval = evaluate(&doc, comp, Time::ZERO).unwrap();
    let fx = &eval.layers[0].effects[0];
    let r = Rect::of_size(voxy_core::glam::DVec2::new(100.0, 50.0));
    let chain = [(Kernel::Invert, fx), (Kernel::Invert, fx)];
    assert_eq!(chain_rod(&chain, r, 1.0), r);
    assert_eq!(chain_roi(&chain, r, 1.0), r);
}

mod blur_rules {
    use std::sync::Arc;

    use voxy_core::glam::DVec2;
    use voxy_core::{EffectId, EffectTypeId, EvaluatedEffect, Value};
    use voxy_render::kernels::{Kernel, MAX_TAPS, Rect, box_weights, gaussian_weights};

    fn fx(id: &str, params: &[(&str, Value)]) -> EvaluatedEffect {
        EvaluatedEffect {
            id: EffectId(1),
            type_id: EffectTypeId::new(id),
            version: 1,
            params: params
                .iter()
                .map(|(n, v)| (Arc::from(*n), v.clone()))
                .collect(),
            matte: None,
            frame: 0,
        }
    }

    #[test]
    fn weights_are_normalized_and_finite_across_the_range() {
        for r in [
            0.0,
            0.1,
            0.5,
            1.0,
            2.5,
            10.0,
            99.9,
            400.0,
            1000.0,
            5000.0,
            f64::NAN,
        ] {
            let w = gaussian_weights(r);
            assert!(w.len() <= MAX_TAPS + 1);
            assert!(w.iter().all(|v| v.is_finite() && *v >= 0.0), "{r}");
            let total = f64::from(w[0]) + 2.0 * w[1..].iter().map(|v| f64::from(*v)).sum::<f64>();
            assert!((total - 1.0).abs() < 1e-4, "{r}: {total}");
            // Monotone falloff from the centre.
            assert!(w.windows(2).all(|p| p[0] >= p[1]), "{r}");
        }
        for t in [0, 1, 7, 2000] {
            let w = box_weights(t);
            let total = f64::from(w[0]) + 2.0 * w[1..].iter().map(|v| f64::from(*v)).sum::<f64>();
            assert!((total - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    #[allow(clippy::many_single_char_names)] // r, g, h, b, d read best short here.
    fn blurs_grow_their_region_unless_edges_repeat() {
        let r = Rect::of_size(DVec2::new(40.0, 20.0));
        let g = fx(
            voxy_core::registry::ids::GAUSSIAN_BLUR,
            &[
                ("radius", Value::Scalar(10.0)),
                ("dimensions", Value::Enum(0)),
                ("repeat_edges", Value::Bool(false)),
            ],
        );
        let rod = Kernel::GaussianBlur.rod(&g, r, 1.0);
        assert_eq!(rod, r.pad(DVec2::splat(11.0)));
        // Horizontal only grows in x.
        let mut h = g.clone();
        h.params[1].1 = Value::Enum(1);
        assert_eq!(
            Kernel::GaussianBlur.rod(&h, r, 1.0),
            r.pad(DVec2::new(11.0, 0.0))
        );
        // Repeat edge pixels keeps the output inside the input.
        let mut rep = g.clone();
        rep.params[2].1 = Value::Bool(true);
        assert_eq!(Kernel::GaussianBlur.rod(&rep, r, 1.0), r);
        // Box blur: each pass adds its radius to the reach.
        let b = fx(
            voxy_core::registry::ids::BOX_BLUR,
            &[("radius", Value::Scalar(4.0)), ("passes", Value::Int(3))],
        );
        assert_eq!(Kernel::BoxBlur.rod(&b, r, 1.0), r.pad(DVec2::splat(13.0)));
        assert_eq!(
            Kernel::BoxBlur
                .passes(&b, 1.0, voxy_core::WorkingSpace::Linear)
                .len(),
            6
        );
        // Directional: along its angle only.
        let d = fx(
            voxy_core::registry::ids::DIRECTIONAL_BLUR,
            &[
                ("direction", Value::Scalar(90.0)),
                ("length", Value::Scalar(20.0)),
            ],
        );
        let rod = Kernel::DirectionalBlur.rod(&d, r, 1.0);
        assert!(
            (rod.min.x - 0.0).abs() < 1e-9 && (rod.min.y + 11.0).abs() < 1e-9,
            "{rod:?}"
        );
        // Zero radius or length: no passes at all.
        let none = fx(
            voxy_core::registry::ids::GAUSSIAN_BLUR,
            &[("radius", Value::Scalar(0.0))],
        );
        assert_eq!(
            Kernel::GaussianBlur
                .passes(&none, 1.0, voxy_core::WorkingSpace::Linear)
                .len(),
            0
        );
    }

    #[test]
    fn pixel_parameters_follow_the_preview_scale() {
        let g = fx(
            voxy_core::registry::ids::GAUSSIAN_BLUR,
            &[("radius", Value::Scalar(20.0))],
        );
        let full = &Kernel::GaussianBlur.passes(&g, 1.0, voxy_core::WorkingSpace::Linear)[0];
        let half = &Kernel::GaussianBlur.passes(&g, 0.5, voxy_core::WorkingSpace::Linear)[0];
        assert_eq!(full.i[1], 20);
        assert_eq!(half.i[1], 10);
        assert_eq!(full.w, gaussian_weights(20.0));
        assert_eq!(half.w, gaussian_weights(10.0));
    }
}

/// Every kernel's shader parameters are finite for every combination of
/// minimum, default and maximum parameter values (design §9: no NaN or
/// infinity across the grid).
#[test]
fn kernel_parameters_are_finite_across_the_grid() {
    use voxy_core::{EffectId, EvaluatedEffect, ValueKind, WorkingSpace};
    for kernel in Kernel::ALL {
        let def = builtins()
            .latest(&EffectTypeId::new(kernel.type_id()))
            .unwrap();
        // Three settings per parameter: low, default, high.
        let choices: Vec<Vec<Value>> = def
            .params
            .iter()
            .map(|p| {
                let d = &p.desc;
                let (lo, hi) = d.range.or(d.soft_range).unwrap_or((-1e6, 1e6));
                match d.kind {
                    ValueKind::Scalar => {
                        vec![Value::Scalar(lo), d.default.clone(), Value::Scalar(hi)]
                    }
                    ValueKind::Int => vec![
                        Value::Int(lo as i64),
                        d.default.clone(),
                        Value::Int(hi as i64),
                    ],
                    ValueKind::Enum => {
                        vec![Value::Enum(0), d.default.clone(), Value::Enum(hi as u32)]
                    }
                    ValueKind::Bool => vec![Value::Bool(false), Value::Bool(true)],
                    _ => vec![d.default.clone()],
                }
            })
            .collect();
        let total: usize = choices.iter().map(Vec::len).product();
        for n in 0..total {
            let mut rest = n;
            let params = def
                .params
                .iter()
                .zip(&choices)
                .map(|(p, c)| {
                    let v = c[rest % c.len()].clone();
                    rest /= c.len();
                    (p.name.clone(), v)
                })
                .collect();
            let fx = EvaluatedEffect {
                id: EffectId(1),
                type_id: def.type_id.clone(),
                version: 1,
                params,
                matte: None,
                frame: 0,
            };
            for space in [WorkingSpace::Linear, WorkingSpace::SrgbCompat] {
                for scale in [1.0, 0.25] {
                    for pass in kernel.passes(&fx, scale, space) {
                        assert!(
                            pass.p.iter().flatten().all(|v| v.is_finite())
                                && pass.w.iter().all(|v| v.is_finite()),
                            "{kernel:?} {:?}",
                            fx.params
                        );
                    }
                    let r = Rect::of_size(voxy_core::glam::DVec2::new(10.0, 10.0));
                    let rod = kernel.rod(&fx, r, scale);
                    assert!(rod.min.is_finite() && rod.max.is_finite(), "{kernel:?}");
                }
            }
        }
    }
}
