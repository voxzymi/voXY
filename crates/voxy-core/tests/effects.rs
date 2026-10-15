//! E-A: the effect data model (docs/design-effects-ofx.md §1).

use std::sync::Arc;

use voxy_core::command::AddSolidLayer;
use voxy_core::effect::Placeholder;
use voxy_core::{
    CompSettings, Document, EffectId, EffectInstance, EffectTypeId, ForeignData, History, LayerId,
    LinearRgba, MissingReason, Property, PropertyDesc, Unit, Value, ValueKind, persist,
};

fn doc_with_solid() -> (Document, LayerId) {
    let doc = Document::with_comp(
        "Effects",
        CompSettings {
            width: 64,
            height: 36,
            ..CompSettings::default()
        },
    );
    let comp = doc.main_comp.expect("main composition");
    let mut h = History::new(doc);
    h.execute(AddSolidLayer {
        comp,
        name: None,
        color: LinearRgba::new(1.0, 0.5, 0.25, 1.0),
        index: 0,
    })
    .expect("add solid");
    let doc = (**h.current()).clone();
    let layer = doc.comp(comp).expect("comp").layers[0].id;
    (doc, layer)
}

fn desc(key: String, default: Value) -> Arc<PropertyDesc> {
    Arc::new(PropertyDesc {
        key: key.clone().into(),
        label: key.into(),
        kind: default.kind(),
        default,
        range: None,
        soft_range: None,
        unit: Unit::None,
        animatable: true,
        spatial: false,
    })
}

/// Adds an effect with the given parameters straight to the layer (the
/// commands arrive in a later step).
fn add_effect(doc: &mut Document, layer: LayerId, ty: &str, params: &[(&str, Value)]) -> EffectId {
    let type_id = EffectTypeId::new(ty);
    let id: EffectId = doc.alloc_id();
    let params = params
        .iter()
        .map(|(name, v)| {
            let pid = doc.alloc_id();
            Property::new(pid, desc(type_id.param_key(name), v.clone()), v.clone())
                .expect("valid parameter")
        })
        .collect();
    let fx = EffectInstance {
        id,
        type_id,
        version: 1,
        name: ty.to_owned(),
        enabled: true,
        params,
        matte: None,
        ofx: None,
        extra: ForeignData::default(),
        placeholder: None,
    };
    doc.layer_mut(layer).expect("layer").effects.push(fx);
    id
}

fn find_effects(v: &serde_json::Value) -> &serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            if let Some(e) = m.get("effects") {
                return e;
            }
            m.values()
                .map(find_effects)
                .find(|e| !e.is_null())
                .unwrap_or(&serde_json::Value::Null)
        }
        serde_json::Value::Array(a) => a
            .iter()
            .map(find_effects)
            .find(|e| !e.is_null())
            .unwrap_or(&serde_json::Value::Null),
        _ => &serde_json::Value::Null,
    }
}

#[test]
fn parameter_keys_carry_the_type_prefix() {
    let builtin = EffectTypeId::new("voxy.blur.gaussian");
    assert_eq!(builtin.param_key("radius"), "fx:voxy.blur.gaussian/radius");
    let ofx = EffectTypeId::new("ofx:net.sf.openfx.Invert");
    assert!(ofx.is_ofx());
    assert_eq!(ofx.param_key("mix"), "ofx:net.sf.openfx.Invert/mix");
}

#[test]
fn effect_parameters_are_layer_properties() {
    let (mut doc, layer) = doc_with_solid();
    let fx = add_effect(
        &mut doc,
        layer,
        "voxy.blur.gaussian",
        &[("radius", Value::Scalar(4.0))],
    );
    doc.validate().unwrap();
    let l = doc.find_layer(layer).unwrap().1;
    let radius = l.effect(fx).unwrap().param("radius").unwrap().id;
    assert!(l.properties().iter().any(|p| p.id == radius));
    // The generic property path edits it, like any other property.
    doc.property_mut(radius)
        .unwrap()
        .set_static(Value::Scalar(9.0))
        .unwrap();
    let l = doc.find_layer(layer).unwrap().1;
    assert_eq!(
        l.effect(fx)
            .unwrap()
            .param("radius")
            .unwrap()
            .value_at(voxy_core::Time::ZERO),
        Value::Scalar(9.0)
    );
}

#[test]
fn effects_survive_save_and_load() {
    // A built-in at its defaults, then edited: it loads with the real
    // descriptors and compares equal.
    let (mut doc, layer) = doc_with_solid();
    let reg = voxy_core::registry::builtins();
    let tint = reg
        .latest(&EffectTypeId::new(voxy_core::registry::ids::TINT))
        .unwrap();
    let mut fx = tint.instantiate(&mut doc);
    fx.param_mut("amount")
        .unwrap()
        .set_static(Value::Scalar(0.5))
        .unwrap();
    doc.layer_mut(layer).unwrap().effects.push(fx);
    let json = persist::to_json(&doc).unwrap();
    let back = persist::from_json(&json).unwrap();
    assert_eq!(back, doc);
    let saved = find_effects(&serde_json::from_str(&json).unwrap()).clone();
    assert_eq!(saved[0]["type"], "voxy.color.tint");
    assert_eq!(
        saved[0]["params"][0]["key"], "map_black",
        "short keys on disk"
    );

    // An effect this build does not know keeps every value, including
    // layer references, and saves the same data.
    let (mut doc, layer) = doc_with_solid();
    add_effect(
        &mut doc,
        layer,
        "voxy.test.unknown",
        &[
            ("amount", Value::Scalar(0.5)),
            (
                "map_black",
                Value::Color(LinearRgba::new(0.0, 0.0, 0.1, 1.0)),
            ),
            ("source", Value::Layer(Some(layer))),
            ("mode", Value::Enum(2)),
        ],
    );
    let json = persist::to_json(&doc).unwrap();
    let back = persist::from_json(&json).unwrap();
    back.validate().unwrap();
    let l = back.find_layer(layer).unwrap().1;
    assert!(l.effects[0].is_placeholder());
    let p = l.effects[0].param("source").unwrap();
    assert_eq!(p.desc.kind, ValueKind::Layer);
    let a: serde_json::Value = serde_json::from_str(&json).unwrap();
    let b: serde_json::Value = serde_json::from_str(&persist::to_json(&back).unwrap()).unwrap();
    assert_eq!(find_effects(&a), find_effects(&b));
}

#[test]
fn layers_without_effects_do_not_write_the_field() {
    let (doc, _) = doc_with_solid();
    let json = persist::to_json(&doc).unwrap();
    assert!(!json.contains("\"effects\""));
}

#[test]
fn unreadable_effects_load_as_placeholders_and_save_verbatim() {
    let (mut doc, layer) = doc_with_solid();
    add_effect(
        &mut doc,
        layer,
        "voxy.future.thing",
        &[("x", Value::Scalar(1.0))],
    );
    // Simulate a newer build: an unknown value type and an unknown field.
    let mut v: serde_json::Value = serde_json::from_str(&persist::to_json(&doc).unwrap()).unwrap();
    let fx = find_effects_mut(&mut v).unwrap();
    fx[0]["params"][0]["static"] = serde_json::json!({ "type": "hologram", "value": [1, 2, 3] });
    fx[0]["future_field"] = serde_json::json!({ "nested": [1.5, "a", null] });
    let original = fx[0].clone();
    let json = serde_json::to_string(&v).unwrap();

    let back = persist::from_json(&json).unwrap();
    back.validate().unwrap();
    let l = back.find_layer(layer).unwrap().1;
    let e = &l.effects[0];
    assert!(matches!(
        e.placeholder,
        Some(Placeholder {
            reason: MissingReason::Unreadable(_),
            ..
        })
    ));
    assert_eq!(e.editable_params().len(), 0);
    // Its parameter ID stays reserved.
    let reserved = e.placeholder.as_ref().unwrap().reserved.clone();
    assert_eq!(reserved.len(), 1);
    assert!(back.next_id() > reserved[0].raw());
    assert!(l.properties().iter().all(|p| p.id != reserved[0]));

    let resaved: serde_json::Value =
        serde_json::from_str(&persist::to_json(&back).unwrap()).unwrap();
    assert_eq!(find_effects(&resaved)[0], original);
}

#[test]
fn placeholder_keeps_voxy_owned_edits() {
    let (mut doc, layer) = doc_with_solid();
    let id = add_effect(
        &mut doc,
        layer,
        "ofx:com.example.gone",
        &[("gain", Value::Scalar(2.0))],
    );
    let fx = doc.layer_mut(layer).unwrap().effect_mut(id).unwrap();
    fx.extra
        .0
        .insert("plugin_note".into(), serde_json::json!("kept"));
    fx.mark_missing(MissingReason::UnknownType);
    fx.name = "Renamed".into();
    fx.enabled = false;
    doc.validate().unwrap();
    let saved: serde_json::Value = serde_json::from_str(&persist::to_json(&doc).unwrap()).unwrap();
    let e = &find_effects(&saved)[0];
    assert_eq!(e["name"], "Renamed");
    assert_eq!(e["enabled"], false);
    assert_eq!(e["plugin_note"], "kept");
    assert_eq!(e["params"][0]["key"], "gain");
}

#[test]
fn validation_rejects_bad_effect_ids_and_keys() {
    let (mut doc, layer) = doc_with_solid();
    let fx = add_effect(
        &mut doc,
        layer,
        "voxy.blur.box",
        &[("radius", Value::Scalar(1.0))],
    );
    // Two parameters with one name.
    let mut dup = doc.clone();
    let e = dup.layer_mut(layer).unwrap().effect_mut(fx).unwrap();
    let mut p = e.params[0].clone();
    p.id = voxy_core::PropId(9_999);
    e.params.push(p);
    dup.reserve_ids(10_000);
    assert!(dup.validate().is_err());
    // A parameter keyed for another effect type.
    let mut wrong = doc.clone();
    let e = wrong.layer_mut(layer).unwrap().effect_mut(fx).unwrap();
    let p = &e.params[0];
    e.params[0] = Property::new(
        p.id,
        desc("fx:voxy.other/radius".into(), Value::Scalar(1.0)),
        Value::Scalar(1.0),
    )
    .unwrap();
    assert!(wrong.validate().is_err());
    // An effect ID shared with the layer.
    let mut shared = doc.clone();
    shared.layer_mut(layer).unwrap().effects[0].id = EffectId(layer.raw());
    assert!(shared.validate().is_err());
}

fn find_effects_mut(v: &mut serde_json::Value) -> Option<&mut serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => {
            if m.contains_key("effects") {
                return m.get_mut("effects");
            }
            m.values_mut().find_map(find_effects_mut)
        }
        serde_json::Value::Array(a) => a.iter_mut().find_map(find_effects_mut),
        _ => None,
    }
}

#[test]
fn effects_are_written_as_format_0_7_0_only_when_present() {
    let (mut doc, layer) = doc_with_solid();
    let version = |d: &Document| {
        let v: serde_json::Value =
            serde_json::from_str(&persist::to_json(d).expect("save")).expect("json");
        v["format_version"].as_str().expect("version").to_owned()
    };
    assert_eq!(version(&doc), "0.5.0");
    let fx = add_effect(
        &mut doc,
        layer,
        "voxy.blur.box",
        &[("radius", Value::Scalar(2.0))],
    );
    assert_eq!(version(&doc), "0.7.0");
    let json = persist::to_json(&doc).unwrap();
    // Parameters the file lacks are filled in at their defaults.
    let back = persist::from_json(&json).unwrap();
    let def_len = voxy_core::registry::builtins()
        .latest(&EffectTypeId::new(voxy_core::registry::ids::BOX_BLUR))
        .unwrap()
        .params
        .len();
    assert_eq!(
        back.find_layer(layer).unwrap().1.effects[0].params.len(),
        def_len
    );

    // A placeholder counts as an effect too.
    doc.layer_mut(layer)
        .unwrap()
        .effect_mut(fx)
        .unwrap()
        .mark_missing(MissingReason::UnknownType);
    assert_eq!(version(&doc), "0.7.0");

    // Removing the last effect goes back to the older version.
    doc.layer_mut(layer).unwrap().effects.clear();
    assert_eq!(version(&doc), "0.5.0");

    // A file from a newer build is refused, not misread.
    let newer = json.replace(
        "\"format_version\": \"0.7.0\"",
        "\"format_version\": \"0.8.0\"",
    );
    assert!(matches!(
        persist::from_json(&newer),
        Err(persist::LoadError::TooNew { .. })
    ));
}

#[test]
fn blend_modes_and_adjustment_layers_need_format_0_7_0() {
    let version = |d: &Document| {
        let v: serde_json::Value =
            serde_json::from_str(&persist::to_json(d).expect("save")).expect("json");
        v["format_version"].as_str().expect("version").to_owned()
    };
    let (mut doc, layer) = doc_with_solid();
    for mode in voxy_core::BlendMode::ALL {
        doc.layer_mut(layer).unwrap().blend_mode = mode;
        let want = if mode == voxy_core::BlendMode::Normal {
            "0.5.0"
        } else {
            "0.7.0"
        };
        assert_eq!(version(&doc), want, "{mode:?}");
        let back = persist::from_json(&persist::to_json(&doc).unwrap()).unwrap();
        assert_eq!(back.find_layer(layer).unwrap().1.blend_mode, mode);
    }
    doc.layer_mut(layer).unwrap().blend_mode = voxy_core::BlendMode::Normal;
    doc.layer_mut(layer).unwrap().kind = voxy_core::LayerKind::Adjustment;
    assert_eq!(version(&doc), "0.7.0");
    let json = persist::to_json(&doc).unwrap();
    assert!(json.contains("\"type\": \"adjustment\""));
    assert_eq!(persist::from_json(&json).unwrap(), doc);
}

#[test]
fn evaluation_resolves_enabled_effects_at_the_time() {
    let (mut doc, layer) = doc_with_solid();
    let comp = doc.main_comp.unwrap();
    let reg = voxy_core::registry::builtins();
    let def = |id: &str| reg.latest(&EffectTypeId::new(id)).unwrap().clone();
    let mut blur = def(voxy_core::registry::ids::GAUSSIAN_BLUR).instantiate(&mut doc);
    let radius = blur.param_mut("radius").unwrap();
    radius.set_animated(voxy_core::Time::ZERO, true).unwrap();
    radius
        .set_value_at(voxy_core::Time::ZERO, Value::Scalar(0.0))
        .unwrap();
    radius
        .set_value_at(voxy_core::Time::from_secs(2), Value::Scalar(20.0))
        .unwrap();
    let mut off = def(voxy_core::registry::ids::INVERT).instantiate(&mut doc);
    off.enabled = false;
    let mut gone = def(voxy_core::registry::ids::TINT).instantiate(&mut doc);
    gone.mark_missing(MissingReason::UnknownType);
    let l = doc.layer_mut(layer).unwrap();
    l.effects.extend([blur, off, gone]);

    let t = voxy_core::Time::from_secs(1);
    let eval = voxy_core::evaluate(&doc, comp, t).unwrap();
    let e = &eval.layers[0];
    assert_eq!(
        e.effects.len(),
        1,
        "disabled and missing effects are left out"
    );
    assert_eq!(e.missing_effects, 1);
    let fx = &e.effects[0];
    assert_eq!(fx.type_id.as_str(), voxy_core::registry::ids::GAUSSIAN_BLUR);
    assert!((fx.scalar("radius").unwrap() - 10.0).abs() < 1e-9);
    assert_eq!(fx.param("dimensions"), Some(&Value::Enum(0)));
    // 30 fps by default: one second is frame 30.
    assert_eq!(fx.frame, 30);
}

/// O-C: an OpenFX effect with keys carries its animation for the plugin's
/// queries at other times; built-in effects and static ones carry none.
#[test]
fn ofx_effects_with_keys_carry_their_timeline() {
    use voxy_core::command::{Command, SetAnimated};
    let (mut doc, layer) = doc_with_solid();
    let ofx = add_effect(
        &mut doc,
        layer,
        "ofx:org.example.Blur",
        &[("size", Value::Scalar(2.0)), ("mix", Value::Scalar(1.0))],
    );
    add_effect(
        &mut doc,
        layer,
        "voxy.blur.gaussian",
        &[("radius", Value::Scalar(2.0))],
    );
    let comp = doc.main_comp.expect("comp");
    let eval =
        |doc: &Document| voxy_core::evaluate(doc, comp, voxy_core::Time::ZERO).expect("eval");
    assert!(
        eval(&doc).layers[0]
            .effects
            .iter()
            .all(|e| e.timeline.is_none())
    );

    let size = doc.find_layer(layer).expect("layer").1.effects[0].params[0].id;
    SetAnimated {
        prop: size,
        time: voxy_core::Time::ZERO,
        animated: true,
    }
    .apply(&mut doc)
    .expect("animate");
    let e = eval(&doc);
    let fx = e.layers[0]
        .effects
        .iter()
        .find(|e| e.id == ofx)
        .expect("ofx");
    let t = fx.timeline.as_ref().expect("a timeline");
    assert_eq!(
        t.params.iter().map(|(n, _)| &**n).collect::<Vec<_>>(),
        ["size"],
        "only animated parameters"
    );
    assert_eq!(t.key_frames("size"), Some(vec![0.0]));
    assert_eq!(t.value_at("size", 7.5), Some(Value::Scalar(2.0)));
    assert_eq!(t.value_at("mix", 0.0), None);
}

/// O-C: a plugin's edits in response to a user edit join that edit's undo
/// step; on their own (a push button) they are one step.
#[test]
fn plugin_edits_join_the_edit_that_caused_them() {
    use voxy_core::Time;
    use voxy_core::command::{PluginEdit, PluginParamEdits, SetProperty, set_property_merge_key};
    let (mut doc, layer) = doc_with_solid();
    let fx = add_effect(
        &mut doc,
        layer,
        "ofx:org.example.Linked",
        &[("a", Value::Scalar(1.0)), ("b", Value::Scalar(1.0))],
    );
    let param = |doc: &Document, name: &str| {
        doc.find_layer(layer).expect("layer").1.effects[0]
            .param(name)
            .expect("param")
            .clone()
    };
    let a = param(&doc, "a").id;
    let mut h = History::new(doc);
    let plugin = |merge, value: f64| PluginParamEdits {
        layer,
        effect: fx,
        edits: vec![
            PluginEdit::Set {
                name: "b".into(),
                time: Time::ZERO,
                value: Value::Scalar(value),
            },
            PluginEdit::Set {
                name: "missing".into(),
                time: Time::ZERO,
                value: Value::Scalar(0.0),
            },
        ],
        label: "Plugin Edit".into(),
        merge,
    };
    // A drag of `a` over two frames, each answered by the plugin setting `b`.
    for v in [2.0, 3.0] {
        h.execute(SetProperty {
            prop: a,
            time: Time::ZERO,
            value: Value::Scalar(v),
        })
        .expect("edit");
        h.execute(plugin(Some(set_property_merge_key(a)), v * 10.0))
            .expect("plugin edit");
    }
    h.end_interaction();
    assert_eq!(
        param(h.current(), "b").value_at(Time::ZERO),
        Value::Scalar(30.0)
    );
    assert!(h.undo());
    assert_eq!(
        param(h.current(), "a").value_at(Time::ZERO),
        Value::Scalar(1.0)
    );
    assert_eq!(
        param(h.current(), "b").value_at(Time::ZERO),
        Value::Scalar(1.0)
    );
    assert!(h.redo());

    // A push button: its own step.
    h.execute(plugin(None, 7.0)).expect("button");
    assert!(h.undo());
    assert_eq!(
        param(h.current(), "b").value_at(Time::ZERO),
        Value::Scalar(30.0)
    );
}
