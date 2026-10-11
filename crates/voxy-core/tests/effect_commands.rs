//! E-A: effect-stack commands; each is one undo step.

use std::sync::Arc;

use voxy_core::command::{
    AddAdjustmentLayer, AddEffect, AddSolidLayer, CreateComposition, DuplicateComposition,
    DuplicateEffects, MoveEffect, PasteEffects, RemoveEffects, RemoveLayers, RenameEffect,
    SetBlendMode, SetEffectMatte, SetEffectsEnabled,
};
use voxy_core::registry::{EffectDef, EffectRegistry, ids};
use voxy_core::{
    BlendMode, CompId, CompSettings, Document, EffectId, EffectMatte, EffectTypeId, History,
    LayerId, LayerKind, LinearRgba, MatteChannel, MissingReason, Time, Value, persist,
};

struct Fixture {
    h: History,
    comp: CompId,
    a: LayerId,
    b: LayerId,
}

fn fixture() -> Fixture {
    let doc = Document::with_comp(
        "Commands",
        CompSettings {
            width: 64,
            height: 36,
            ..CompSettings::default()
        },
    );
    let comp = doc.main_comp.expect("main composition");
    let mut h = History::new(doc);
    for c in [0.2, 0.8] {
        h.execute(AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::new(c, c, c, 1.0),
            index: 0,
        })
        .expect("add solid");
    }
    let layers = &h.current().comp(comp).expect("comp").layers;
    let (a, b) = (layers[0].id, layers[1].id);
    Fixture { h, comp, a, b }
}

fn def(id: &str) -> Arc<EffectDef> {
    EffectRegistry::with_builtins()
        .latest(&EffectTypeId::new(id))
        .expect("built-in")
        .clone()
}

fn effects(h: &History, layer: LayerId) -> Vec<EffectId> {
    h.current()
        .find_layer(layer)
        .expect("layer")
        .1
        .effects
        .iter()
        .map(|e| e.id)
        .collect()
}

fn add(h: &mut History, layer: LayerId, id: &str) -> EffectId {
    let r = h
        .execute_reported(AddEffect {
            layer,
            index: None,
            def: def(id),
        })
        .expect("add effect");
    r.created_effects[0]
}

/// A layer-reference parameter on a built-in, for reference tests (group 1
/// has none yet), added straight to the document.
fn with_layer_param(doc: &mut Document, layer: LayerId, fx: EffectId, target: LayerId) {
    let ty = EffectTypeId::new(ids::TINT);
    let pid = doc.alloc_id();
    let desc = Arc::new(voxy_core::PropertyDesc {
        key: ty.param_key("source").into(),
        label: "Source".into(),
        kind: voxy_core::ValueKind::Layer,
        default: Value::Layer(None),
        range: None,
        soft_range: None,
        unit: voxy_core::Unit::None,
        animatable: true,
        spatial: false,
    });
    let p = voxy_core::Property::new(pid, desc, Value::Layer(Some(target))).expect("valid");
    doc.layer_mut(layer)
        .expect("layer")
        .effect_mut(fx)
        .expect("effect")
        .params
        .push(p);
}

fn source_ref(doc: &Document, layer: LayerId) -> Option<LayerId> {
    let l = doc.find_layer(layer).expect("layer").1;
    match l.effects[0].param("source")?.value_at(Time::ZERO) {
        Value::Layer(r) => r,
        _ => None,
    }
}

#[test]
fn stack_edits_are_single_undo_steps() {
    let Fixture { mut h, a, .. } = fixture();
    let start = h.current().clone();
    let blur = add(&mut h, a, ids::GAUSSIAN_BLUR);
    let tint = add(&mut h, a, ids::TINT);
    assert_eq!(effects(&h, a), vec![blur, tint]);
    h.execute(MoveEffect {
        layer: a,
        id: tint,
        to_index: 0,
    })
    .unwrap();
    assert_eq!(effects(&h, a), vec![tint, blur]);
    h.execute(SetEffectsEnabled {
        layer: a,
        ids: vec![blur],
        enabled: false,
    })
    .unwrap();
    h.execute(RenameEffect {
        layer: a,
        id: blur,
        name: "  Soft  ".into(),
    })
    .unwrap();
    let r = h
        .execute_reported(DuplicateEffects {
            layer: a,
            ids: vec![tint],
        })
        .unwrap();
    let copy = r.created_effects[0];
    assert_eq!(effects(&h, a), vec![tint, copy, blur]);
    let l = h.current().find_layer(a).unwrap().1;
    assert_eq!(l.effect(blur).unwrap().name, "Soft");
    assert!(!l.effect(blur).unwrap().enabled);
    h.current().validate().unwrap();
    h.execute(RemoveEffects {
        layer: a,
        ids: vec![tint, blur],
    })
    .unwrap();
    assert_eq!(effects(&h, a), vec![copy]);

    // Seven commands, seven undo steps, back to the start exactly.
    for _ in 0..7 {
        assert!(h.undo());
    }
    assert_eq!(**h.current(), *start);
}

#[test]
fn bad_edits_fail_and_change_nothing() {
    let Fixture { mut h, a, b, .. } = fixture();
    let fx = add(&mut h, a, ids::INVERT);
    let before = h.current().clone();
    assert!(
        h.execute(RenameEffect {
            layer: a,
            id: fx,
            name: "   ".into()
        })
        .is_err()
    );
    assert!(
        h.execute(RemoveEffects {
            layer: b,
            ids: vec![fx]
        })
        .is_err()
    );
    assert!(
        h.execute(SetEffectMatte {
            layer: a,
            id: fx,
            matte: Some(EffectMatte {
                layer: a,
                channel: MatteChannel::Alpha,
                invert: false
            }),
        })
        .is_err(),
        "a layer cannot matte its own effect"
    );
    assert_eq!(**h.current(), *before);
}

#[test]
fn mattes_must_be_in_the_same_composition() {
    let Fixture { mut h, a, b, .. } = fixture();
    let fx = add(&mut h, a, ids::INVERT);
    let matte = EffectMatte {
        layer: b,
        channel: MatteChannel::Luma,
        invert: true,
    };
    h.execute(SetEffectMatte {
        layer: a,
        id: fx,
        matte: Some(matte),
    })
    .unwrap();
    let other = h
        .execute_reported(CreateComposition {
            settings: CompSettings::default(),
            folder: None,
        })
        .unwrap()
        .created_comps[0];
    h.execute(AddSolidLayer {
        comp: other,
        name: None,
        color: LinearRgba::WHITE,
        index: 0,
    })
    .unwrap();
    let far = h.current().comp(other).unwrap().layers[0].id;
    assert!(
        h.execute(SetEffectMatte {
            layer: a,
            id: fx,
            matte: Some(EffectMatte {
                layer: far,
                ..matte
            }),
        })
        .is_err()
    );
    let json = persist::to_json(h.current()).unwrap();
    assert_eq!(&persist::from_json(&json).unwrap(), &**h.current());
}

#[test]
fn deleting_a_layer_clears_references_to_it() {
    let Fixture { mut h, a, b, .. } = fixture();
    let fx = add(&mut h, a, ids::TINT);
    h.execute(SetEffectMatte {
        layer: a,
        id: fx,
        matte: Some(EffectMatte {
            layer: b,
            channel: MatteChannel::Alpha,
            invert: false,
        }),
    })
    .unwrap();
    let mut doc = (**h.current()).clone();
    with_layer_param(&mut doc, a, fx, b);
    let mut h = History::new(doc);
    let before = h.current().clone();
    h.execute(RemoveLayers { layers: vec![b] }).unwrap();
    let l = h.current().find_layer(a).unwrap().1;
    assert_eq!(l.effects[0].matte, None);
    assert_eq!(source_ref(h.current(), a), None);
    h.current().validate().unwrap();
    assert!(h.undo());
    assert_eq!(**h.current(), *before);
    assert_eq!(source_ref(h.current(), a), Some(b));
}

#[test]
fn paste_renumbers_and_keeps_only_local_references() {
    let Fixture { mut h, comp, a, b } = fixture();
    let fx = add(&mut h, a, ids::TINT);
    let mut doc = (**h.current()).clone();
    with_layer_param(&mut doc, a, fx, b);
    let mut h = History::new(doc);
    let clip = h.current().find_layer(a).unwrap().1.effects.clone();

    // Onto both layers at once: one step. On `b` the reference would point
    // at itself, so it is cleared there.
    let r = h
        .execute_reported(PasteEffects {
            layers: vec![a, b],
            index: Some(0),
            items: clip.clone(),
        })
        .unwrap();
    assert_eq!(r.created_effects.len(), 2);
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    h.current().validate().unwrap();
    let doc = h.current();
    assert_eq!(source_ref(doc, a), Some(b));
    assert_eq!(source_ref(doc, b), None);
    assert_ne!(effects(&h, a)[0], fx);

    // Into another composition: the reference is cleared.
    let other = h
        .execute_reported(CreateComposition {
            settings: CompSettings::default(),
            folder: None,
        })
        .unwrap()
        .created_comps[0];
    h.execute(AddSolidLayer {
        comp: other,
        name: None,
        color: LinearRgba::WHITE,
        index: 0,
    })
    .unwrap();
    let far = h.current().comp(other).unwrap().layers[0].id;
    h.execute(PasteEffects {
        layers: vec![far],
        index: None,
        items: clip,
    })
    .unwrap();
    assert_eq!(source_ref(h.current(), far), None);
    assert!(h.undo() && h.undo() && h.undo() && h.undo());
    assert!(h.current().comp(comp).is_some());
}

#[test]
fn duplicated_compositions_get_fresh_effect_ids_and_local_references() {
    let Fixture { mut h, comp, a, b } = fixture();
    let fx = add(&mut h, a, ids::TINT);
    let gone = add(&mut h, b, ids::BOX_BLUR);
    let mut doc = (**h.current()).clone();
    with_layer_param(&mut doc, a, fx, b);
    doc.layer_mut(b)
        .unwrap()
        .effect_mut(gone)
        .unwrap()
        .mark_missing(MissingReason::UnknownType);
    let mut h = History::new(doc);
    let copy = h
        .execute_reported(DuplicateComposition { comp })
        .unwrap()
        .created_comps[0];
    h.current().validate().unwrap();
    let c = h.current().comp(copy).unwrap();
    let (ca, cb) = (c.layers[0].id, c.layers[1].id);
    assert_eq!(source_ref(h.current(), ca), Some(cb), "points at the copy");
    // The placeholder's preserved JSON was renumbered too, so the saved
    // file still has unique IDs.
    let json = persist::to_json(h.current()).unwrap();
    persist::from_json(&json).unwrap().validate().unwrap();
}

#[test]
fn blend_modes_and_adjustment_layers_are_commands() {
    let Fixture { mut h, comp, a, b } = fixture();
    h.execute(SetBlendMode {
        layers: vec![a, b],
        mode: BlendMode::Screen,
    })
    .unwrap();
    let r = h
        .execute_reported(AddAdjustmentLayer {
            comp,
            name: None,
            index: 0,
        })
        .unwrap();
    let adj = r.created_layers[0];
    let doc = h.current();
    let l = doc.find_layer(adj).unwrap().1;
    assert!(matches!(l.kind, LayerKind::Adjustment));
    assert_eq!(l.name, "Adjustment Layer 1");
    assert_eq!(l.out_point, doc.comp(comp).unwrap().duration);
    assert_eq!(doc.find_layer(b).unwrap().1.blend_mode, BlendMode::Screen);
    let _ = add(&mut h, adj, ids::EXPOSURE);
    h.current().validate().unwrap();
    assert!(h.undo() && h.undo() && h.undo());
    assert_eq!(
        h.current().find_layer(a).unwrap().1.blend_mode,
        BlendMode::Normal
    );
}

#[test]
fn reset_puts_every_parameter_back_in_one_step() {
    use voxy_core::command::{ResetEffect, SetAnimated, SetProperty};
    let Fixture { mut h, a, .. } = fixture();
    let fx = add(&mut h, a, ids::GAUSSIAN_BLUR);
    let fresh = h
        .current()
        .find_layer(a)
        .unwrap()
        .1
        .effect(fx)
        .unwrap()
        .clone();
    let radius = fresh.param("radius").unwrap().id;
    h.execute(SetAnimated {
        prop: radius,
        time: Time::ZERO,
        animated: true,
    })
    .unwrap();
    h.execute(SetProperty {
        prop: radius,
        time: Time::from_secs(1),
        value: Value::Scalar(40.0),
    })
    .unwrap();
    let edited = h.current().clone();
    h.execute(ResetEffect { layer: a, id: fx }).unwrap();
    assert_eq!(
        h.current().find_layer(a).unwrap().1.effect(fx).unwrap(),
        &fresh
    );
    assert!(h.undo());
    assert_eq!(**h.current(), *edited);
}

/// Point parameters of a new effect start at the centre of the layer's
/// content; an adjustment layer covers its composition.
#[test]
fn new_point_parameters_start_at_the_content_centre() {
    let mut f = fixture();
    let centre = |h: &History, layer: LayerId| {
        h.current().find_layer(layer).expect("layer").1.effects[0].params[1].value_at(Time::ZERO)
    };
    add(&mut f.h, f.a, ids::RADIAL_BLUR);
    let size = f.h.current().content_size(f.a).expect("solid size");
    assert_eq!(centre(&f.h, f.a), Value::Vec2(size / 2.0));

    f.h.execute(AddAdjustmentLayer {
        comp: f.comp,
        name: None,
        index: 0,
    })
    .expect("adjustment");
    let adj = f.h.current().comp(f.comp).expect("comp").layers[0].id;
    add(&mut f.h, adj, ids::ZOOM_BLUR);
    assert_eq!(
        centre(&f.h, adj),
        Value::Vec2(voxy_core::glam::DVec2::new(32.0, 18.0))
    );
    // The edit is the same single undo step as any added effect.
    assert!(f.h.undo());
    assert_eq!(effects(&f.h, adj).len(), 0);
}

/// LUT assets (format 0.7.0) feed the LUT effect only: they cannot become
/// layers, and removing one clears every parameter that used it.
#[test]
fn luts_are_effect_inputs_not_layer_sources() {
    use voxy_core::command::{AddSourceLayer, ImportAssets, RemoveItems, RemoveMode};
    use voxy_core::{Asset, AssetKind, PoolItem, SourceRef};
    let mut f = fixture();
    // An unused LUT alone already needs 0.7.0 (older readers do not know
    // the kind).
    let report =
        f.h.execute_reported(ImportAssets {
            assets: vec![Asset::new(AssetKind::Lut, "Look", "looks/look.cube")],
            folder: None,
        })
        .expect("import");
    let lut = report.created_assets[0];
    let mut doc = (**f.h.current()).clone();
    for l in &mut doc.comp_mut(f.comp).unwrap().layers {
        std::sync::Arc::make_mut(l).effects.clear();
    }
    assert_eq!(persist::required_version(&doc), persist::FORMAT_VERSION);

    let err = f.h.execute(AddSourceLayer {
        comp: f.comp,
        source: SourceRef::Asset(lut),
        index: 0,
        in_point: Time::ZERO,
    });
    assert!(err.is_err(), "a LUT is not a layer source");

    let fx = add(&mut f.h, f.a, ids::LUT);
    let prop =
        f.h.current()
            .find_layer(f.a)
            .unwrap()
            .1
            .effect(fx)
            .unwrap()
            .params[0]
            .id;
    f.h.execute(voxy_core::command::SetProperty {
        prop,
        time: Time::ZERO,
        value: Value::Asset(Some(lut)),
    })
    .expect("point at the LUT");
    f.h.execute(RemoveItems {
        items: vec![PoolItem::Asset(lut)],
        mode: RemoveMode::PlaceholderLayers,
    })
    .expect("remove");
    let value =
        f.h.current()
            .find_layer(f.a)
            .unwrap()
            .1
            .effect(fx)
            .unwrap()
            .params[0]
            .value_at(Time::ZERO);
    assert_eq!(value, Value::Asset(None));
    // One undo brings the LUT and the reference back.
    assert!(f.h.undo());
    let value =
        f.h.current()
            .find_layer(f.a)
            .unwrap()
            .1
            .effect(fx)
            .unwrap()
            .params[0]
            .value_at(Time::ZERO);
    assert_eq!(value, Value::Asset(Some(lut)));
}
