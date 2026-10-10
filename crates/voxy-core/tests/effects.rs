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
    let (mut doc, layer) = doc_with_solid();
    let other = layer;
    add_effect(
        &mut doc,
        layer,
        "voxy.color.tint",
        &[
            ("amount", Value::Scalar(0.5)),
            (
                "map_black",
                Value::Color(LinearRgba::new(0.0, 0.0, 0.1, 1.0)),
            ),
            ("source", Value::Layer(Some(other))),
            ("mode", Value::Enum(2)),
        ],
    );
    let json = persist::to_json(&doc).unwrap();
    let back = persist::from_json(&json).unwrap();
    back.validate().unwrap();
    assert_eq!(persist::to_json(&back).unwrap(), json);
    let saved = find_effects(&serde_json::from_str(&json).unwrap()).clone();
    assert_eq!(saved[0]["type"], "voxy.color.tint");
    assert_eq!(saved[0]["params"][0]["key"], "amount", "short keys on disk");
    let l = back.find_layer(layer).unwrap().1;
    let p = l.effects[0].param("source").unwrap();
    assert_eq!(p.desc.kind, ValueKind::Layer);
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
    assert!(e.editable_params().is_empty());
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
