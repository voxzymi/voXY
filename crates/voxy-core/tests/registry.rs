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

mod resolve {
    use std::sync::Arc;

    use voxy_core::command::{AddEffect, AddSolidLayer};
    use voxy_core::registry::{EffectRegistry, builtins, ids, resolve_effects};
    use voxy_core::{
        CompSettings, Document, EffectTypeId, History, LayerId, LinearRgba, MissingReason, persist,
    };

    fn project_with(id: &str) -> (History, LayerId) {
        let doc = Document::with_comp("Resolve", CompSettings::default());
        let comp = doc.main_comp.expect("main");
        let mut h = History::new(doc);
        h.execute(AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        })
        .expect("solid");
        let layer = h.current().comp(comp).expect("comp").layers[0].id;
        h.execute(AddEffect {
            layer,
            index: None,
            def: builtins()
                .latest(&EffectTypeId::new(id))
                .expect("def")
                .clone(),
        })
        .expect("effect");
        (h, layer)
    }

    fn edit(json: &str, f: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v: serde_json::Value = serde_json::from_str(json).expect("json");
        let fx = &mut v["project"]["comps"][0]["layers"][0]["effects"][0];
        assert!(fx.is_object(), "effect not found");
        f(fx);
        serde_json::to_string(&v).expect("json")
    }

    #[test]
    fn damaged_values_reset_with_a_warning() {
        let (h, layer) = project_with(ids::GAUSSIAN_BLUR);
        let json = persist::to_json(h.current()).unwrap();
        let bad = edit(&json, |fx| {
            fx["params"][0]["static"] = serde_json::json!({ "type": "bool", "value": true });
        });
        let loaded = persist::from_json_resolved(&bad, builtins()).unwrap();
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
        let fx = &loaded.doc.find_layer(layer).unwrap().1.effects[0];
        assert!(!fx.is_placeholder());
        assert_eq!(
            fx.param("radius").unwrap().value_at(voxy_core::Time::ZERO),
            fx.param("radius").unwrap().desc.default
        );
    }

    #[test]
    fn unknown_parameters_and_newer_versions_become_placeholders() {
        let (h, layer) = project_with(ids::BOX_BLUR);
        let json = persist::to_json(h.current()).unwrap();
        let next = h.current().next_id();
        let extra = edit(&json, |fx| {
            let p = serde_json::json!({ "key": "future", "id": next,
                "static": { "type": "scalar", "value": 3.0 } });
            fx["params"].as_array_mut().unwrap().push(p);
        });
        let mut extra: serde_json::Value = serde_json::from_str(&extra).unwrap();
        extra["project"]["next_id"] = (next + 1).into();
        let extra = extra.to_string();
        let loaded = persist::from_json_resolved(&extra, builtins()).unwrap();
        let fx = &loaded.doc.find_layer(layer).unwrap().1.effects[0];
        assert!(matches!(
            fx.placeholder.as_ref().unwrap().reason,
            MissingReason::Unreadable(_)
        ));

        let newer = edit(&json, |fx| fx["version"] = 7.into());
        let loaded = persist::from_json_resolved(&newer, builtins()).unwrap();
        let fx = &loaded.doc.find_layer(layer).unwrap().1.effects[0];
        assert_eq!(
            fx.placeholder.as_ref().unwrap().reason,
            MissingReason::NewerVersion { known: 1 }
        );
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
    }

    #[test]
    fn placeholders_come_back_when_the_definition_is_installed() {
        let (h, layer) = project_with(ids::INVERT);
        let json = persist::to_json(h.current()).unwrap();
        // Pretend the effect is a plugin that is not installed yet.
        let plugin = json.replace(ids::INVERT, "ofx:org.example.Invert");
        let loaded = persist::from_json_resolved(&plugin, builtins()).unwrap();
        let mut doc = loaded.doc;
        assert!(doc.find_layer(layer).unwrap().1.effects[0].is_placeholder());
        let saved = persist::to_json(&doc).unwrap();

        // Running again with the same registry changes nothing.
        let before = doc.clone();
        assert_eq!(resolve_effects(&mut doc, builtins()), Vec::<String>::new());
        assert_eq!(doc, before);

        // Install it: the same definition under the plugin's type ID.
        let mut def = (**builtins().latest(&EffectTypeId::new(ids::INVERT)).unwrap()).clone();
        let ty = EffectTypeId::new("ofx:org.example.Invert");
        def.params = def
            .params
            .iter()
            .map(|p| {
                let mut p = p.clone();
                let mut desc = (*p.desc).clone();
                desc.key = ty.param_key(&p.name).into();
                p.desc = Arc::new(desc);
                p
            })
            .collect();
        def.type_id = ty;
        let mut reg = EffectRegistry::with_builtins();
        reg.register(def).unwrap();
        resolve_effects(&mut doc, &reg);
        let fx = &doc.find_layer(layer).unwrap().1.effects[0];
        assert!(!fx.is_placeholder());
        assert_eq!(fx.editable_params().len(), 2);
        doc.validate().unwrap();
        // Nothing was lost on the way.
        let a: serde_json::Value = serde_json::from_str(&saved).unwrap();
        let b: serde_json::Value = serde_json::from_str(&persist::to_json(&doc).unwrap()).unwrap();
        assert_eq!(a["project"], b["project"]);
    }
}
