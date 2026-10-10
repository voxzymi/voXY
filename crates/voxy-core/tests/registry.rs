//! E-A: the effect registry and the built-in definitions.

use voxy_core::registry::{EffectRegistry, builtin_definitions, ids};
use voxy_core::{CompSettings, Document, EffectTypeId, Time, Value};

fn doc() -> Document {
    Document::with_comp("Registry", CompSettings::default())
}

#[test]
fn every_builtin_registers_and_is_consistent() {
    let reg = EffectRegistry::with_builtins();
    assert_eq!(reg.len(), builtin_definitions().len());
    for def in reg.latest_all() {
        assert!(!def.name.is_empty() && !def.description.is_empty());
        assert!(def.type_id.as_str().starts_with("voxy."));
        for p in def.params.iter() {
            let d = &p.desc;
            if let (Some((lo, hi)), Some(v)) = (d.range, d.default.as_scalar()) {
                assert!(lo <= v && v <= hi, "{}: default outside range", d.key);
            }
            if let (Some((lo, hi)), Some((slo, shi))) = (d.range, d.soft_range) {
                assert!(lo <= slo && shi <= hi, "{}: soft range outside hard", d.key);
            }
            if let Value::Enum(_) = d.default {
                assert!(p.choices.is_some(), "{}: enum without labels", d.key);
            }
        }
    }
}

#[test]
fn group_one_is_registered() {
    let reg = EffectRegistry::with_builtins();
    for id in [
        ids::GAUSSIAN_BLUR,
        ids::BOX_BLUR,
        ids::DIRECTIONAL_BLUR,
        ids::BRIGHTNESS_CONTRAST,
        ids::EXPOSURE,
        ids::INVERT,
        ids::TINT,
        ids::HUE_SATURATION,
    ] {
        assert!(reg.latest(&EffectTypeId::new(id)).is_some(), "{id}");
    }
}

#[test]
fn instances_start_at_their_defaults_with_fresh_ids() {
    let reg = EffectRegistry::with_builtins();
    let mut d = doc();
    let def = reg.latest(&EffectTypeId::new(ids::GAUSSIAN_BLUR)).unwrap();
    let a = def.instantiate(&mut d);
    let b = def.instantiate(&mut d);
    assert_ne!(a.id, b.id);
    assert_eq!(a.params.len(), def.params.len());
    for (p, pd) in a.params.iter().zip(def.params.iter()) {
        assert_eq!(p.value_at(Time::ZERO), pd.desc.default);
        assert_eq!(a.param_name(p), &*pd.name);
    }
    let ids_a: Vec<u64> = a.ids().collect();
    assert!(b.ids().all(|i| !ids_a.contains(&i)));
}

#[test]
fn duplicate_and_invalid_definitions_are_refused() {
    let mut reg = EffectRegistry::with_builtins();
    let def = builtin_definitions().remove(0);
    assert!(reg.register(def.clone()).is_err(), "same type and version");
    let mut newer = def.clone();
    newer.version = 2;
    reg.register(newer).unwrap();
    assert_eq!(reg.latest(&def.type_id).unwrap().version, 2);
    assert_eq!(reg.get(&def.type_id, 1).unwrap().version, 1);

    let mut bad = def.clone();
    bad.type_id = EffectTypeId::new("voxy.test.renamed");
    let mut fresh = EffectRegistry::new();
    assert!(
        fresh.register(bad).is_err(),
        "parameter keys of another type"
    );
}

#[test]
fn enum_parameters_clamp_to_their_choices() {
    let reg = EffectRegistry::with_builtins();
    let mut d = doc();
    let mut fx = reg
        .latest(&EffectTypeId::new(ids::INVERT))
        .unwrap()
        .instantiate(&mut d);
    let channel = fx.param_mut("channel").unwrap();
    channel.set_static(Value::Enum(99)).unwrap();
    assert_eq!(channel.value_at(Time::ZERO), Value::Enum(4));
}
