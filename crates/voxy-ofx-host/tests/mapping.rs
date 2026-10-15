//! OpenFX descriptions as voXY effect definitions (design §6.6): the gain
//! test plugin registers as a normal effect; values cross both ways.

#![allow(unsafe_code, clippy::unwrap_used)]

use std::path::Path;

use voxy_core::glam::DVec2;
use voxy_core::registry::{Category, EffectOrigin, EffectRegistry, ParamVisibility};
use voxy_core::{LinearRgba, Unit, Value, ValueKind};
use voxy_ofx_host::mapping::{
    base64_decode, base64_encode, effect_def, to_param_value, to_value, type_id,
};
use voxy_ofx_host::params::{ParamKind, ParamValue};
use voxy_ofx_host::protocol::{ContextDesc, ParamDesc, PluginDesc, Prop, PropList};
use voxy_ofx_host::suites::driver::Plugin;

#[test]
fn the_gain_plugin_becomes_a_registered_effect() {
    // SAFETY: our own static test plugin.
    let plugin = unsafe { Plugin::load(&voxy_ofx_testplugins::GAIN.0) }.unwrap();
    let d = plugin.describe().unwrap();
    let desc = PluginDesc::from_description(&plugin.identifier, plugin.version, &d);
    let def = effect_def(&desc, Path::new("/x/VoxyTest.ofx"), 0).unwrap();
    assert_eq!(def.type_id, type_id("org.voxy.test.Gain"));
    assert_eq!(def.type_id.as_str(), "ofx:org.voxy.test.Gain");
    assert_eq!(&*def.name, "Test Gain");
    assert_eq!(def.category, Category::Plugin("voXY tests".into()));
    assert_eq!(def.version, 1, "the plugin's major version");
    assert_eq!(
        def.origin,
        EffectOrigin::Ofx {
            binary: "/x/VoxyTest.ofx".into(),
            index: 0
        }
    );

    let gain = def.param("gain").unwrap();
    assert_eq!(gain.desc.kind, ValueKind::Scalar);
    assert_eq!(gain.desc.default, Value::Scalar(1.0));
    assert_eq!(&*gain.desc.label, "Gain");
    assert_eq!(gain.desc.range, Some((0.0, f64::MAX)), "min only");
    assert_eq!(gain.desc.soft_range, Some((0.0, 4.0)), "display range");
    assert_eq!(&*gain.desc.key, "ofx:org.voxy.test.Gain/gain");
    let straight = def.param("straight").unwrap();
    assert_eq!(straight.desc.default, Value::Bool(false));
    assert_eq!(straight.hint.as_deref(), Some("Render unpremultiplied"));

    let mut reg = EffectRegistry::with_builtins();
    reg.register(def).unwrap();
    assert!(reg.latest(&type_id("org.voxy.test.Gain")).is_some());
}

fn param(name: &str, kind: ParamKind, props: Vec<(&str, Prop)>) -> ParamDesc {
    ParamDesc {
        name: name.into(),
        kind,
        props: PropList(props.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()),
    }
}

fn plugin(params: Vec<ParamDesc>) -> PluginDesc {
    PluginDesc {
        identifier: "org.example.Many".into(),
        version: (2, 3),
        props: PropList::default(),
        contexts: vec![ContextDesc {
            context: "OfxImageEffectContextFilter".into(),
            props: PropList::default(),
            clips: vec![],
            params,
        }],
    }
}

#[test]
fn each_parameter_type_maps_as_designed() {
    let s = |v: &str| Prop::String(vec![v.into()]);
    let desc = plugin(vec![
        param(
            "box",
            ParamKind::Group,
            vec![
                ("OfxPropLabel", s("Box")),
                ("OfxParamPropGroupOpen", Prop::Int(vec![0])),
            ],
        ),
        param(
            "angle",
            ParamKind::Double,
            vec![
                ("OfxParamPropDoubleType", s("OfxParamDoubleTypeAngle")),
                ("OfxParamPropDefault", Prop::Double(vec![45.0])),
                ("OfxParamPropParent", s("box")),
                ("OfxParamPropAnimates", Prop::Int(vec![1])),
            ],
        ),
        param(
            "centre",
            ParamKind::Double2D,
            vec![
                (
                    "OfxParamPropDefaultCoordinateSystem",
                    s("OfxParamCoordinatesNormalised"),
                ),
                ("OfxParamPropDefault", Prop::Double(vec![0.5, 0.5])),
            ],
        ),
        param(
            "count",
            ParamKind::Integer,
            vec![
                ("OfxParamPropDefault", Prop::Int(vec![3])),
                ("OfxParamPropMin", Prop::Int(vec![1])),
                ("OfxParamPropMax", Prop::Int(vec![10])),
            ],
        ),
        param(
            "mode",
            ParamKind::Choice,
            vec![
                (
                    "OfxParamPropChoiceOption",
                    Prop::String(vec!["A".into(), "B".into()]),
                ),
                ("OfxParamPropDefault", Prop::Int(vec![7])),
            ],
        ),
        param(
            "look",
            ParamKind::StrChoice,
            vec![(
                "OfxParamPropChoiceEnum",
                Prop::String(vec!["warm".into(), "cool".into()]),
            )],
        ),
        param(
            "tint",
            ParamKind::Rgb,
            vec![("OfxParamPropDefault", Prop::Double(vec![1.0, 0.5, 0.0]))],
        ),
        param("blob", ParamKind::Bytes, vec![]),
        param(
            "state",
            ParamKind::Custom,
            vec![("OfxParamPropAnimates", Prop::Int(vec![1]))],
        ),
        param(
            "secret",
            ParamKind::Double,
            vec![("OfxParamPropSecret", Prop::Int(vec![1]))],
        ),
        param("page", ParamKind::Page, vec![]),
    ]);
    let def = effect_def(&desc, Path::new("m.ofx"), 4).unwrap();
    assert_eq!(def.version, 2);
    assert_eq!(&*def.name, "org.example.Many", "no label: the identifier");
    assert_eq!(def.groups.len(), 1);
    assert!(!def.groups[0].open_by_default);
    assert!(
        def.param("page").is_none() && def.param("box").is_none(),
        "no values"
    );

    let angle = def.param("angle").unwrap();
    assert_eq!(
        (angle.desc.unit, angle.group, angle.desc.animatable),
        (Unit::Degrees, Some(0), true)
    );
    assert_eq!(def.param("centre").unwrap().desc.unit, Unit::Normalized);
    assert_eq!(
        def.param("centre").unwrap().desc.default,
        Value::Vec2(DVec2::new(0.5, 0.5))
    );
    let count = def.param("count").unwrap();
    assert_eq!(
        (count.desc.default.clone(), count.desc.range),
        (Value::Int(3), Some((1.0, 10.0)))
    );
    let mode = def.param("mode").unwrap();
    assert_eq!(
        mode.desc.default,
        Value::Enum(0),
        "a default past the options is clamped"
    );
    assert_eq!(mode.choices.as_ref().unwrap().len(), 2);
    assert_eq!(def.param("look").unwrap().desc.kind, ValueKind::String);
    assert!(
        def.param("look")
            .unwrap()
            .hint
            .as_deref()
            .unwrap()
            .contains("warm, cool")
    );
    let tint = def.param("tint").unwrap();
    assert!(tint.flags.hide_alpha);
    assert_eq!(
        tint.desc.default,
        Value::Color(LinearRgba::new(1.0, 0.5, 0.0, 1.0))
    );
    assert_eq!(
        def.param("blob").unwrap().flags.visibility,
        ParamVisibility::Hidden
    );
    assert!(
        !def.param("state").unwrap().desc.animatable,
        "plugin state is hold-only"
    );
    assert_eq!(
        def.param("secret").unwrap().flags.visibility,
        ParamVisibility::Hidden
    );

    let mut reg = EffectRegistry::new();
    reg.register(def).unwrap();
}

#[test]
fn a_plugin_with_only_unmapped_contexts_is_not_mapped() {
    let mut desc = plugin(vec![]);
    desc.contexts[0].context = "OfxImageEffectContextPaint".into();
    assert!(effect_def(&desc, Path::new("x"), 0).is_err());
}

#[test]
fn the_filter_context_is_preferred_and_reserved_names_are_refused() {
    let mut desc = plugin(vec![param("amount", ParamKind::Double, vec![])]);
    let mut transition = desc.contexts[0].clone();
    transition.context = "OfxImageEffectContextTransition".into();
    desc.contexts.insert(0, transition);
    let def = effect_def(&desc, Path::new("x"), 0).unwrap();
    assert_eq!(def.category, Category::Plugin("OpenFX".into()), "filter");
    desc.contexts.remove(1);
    let def = effect_def(&desc, Path::new("x"), 0).unwrap();
    assert_eq!(def.category, Category::Transition);
    assert_eq!(
        def.params.iter().map(|p| &*p.name).collect::<Vec<_>>(),
        ["amount", "voxy.auto", "voxy.clip.SourceTo"]
    );
    assert_eq!(
        def.param("voxy.clip.SourceTo").unwrap().desc.default,
        Value::Layer(None)
    );

    let bad = plugin(vec![param("voxy.auto", ParamKind::Boolean, vec![])]);
    assert!(effect_def(&bad, Path::new("x"), 0).is_err());
}

#[test]
fn values_round_trip_between_voxy_and_plugins() {
    let cases = [
        (ParamKind::Integer, ParamValue::Ints(vec![-4])),
        (ParamKind::Boolean, ParamValue::Ints(vec![1])),
        (ParamKind::Choice, ParamValue::Ints(vec![2])),
        (ParamKind::Integer2D, ParamValue::Ints(vec![3, -5])),
        (ParamKind::Double, ParamValue::Doubles(vec![0.25])),
        (
            ParamKind::Double3D,
            ParamValue::Doubles(vec![1.0, 2.0, 3.0]),
        ),
        (
            ParamKind::Rgba,
            ParamValue::Doubles(vec![0.5, 0.25, 0.125, 1.0]),
        ),
        (ParamKind::String, ParamValue::Text("hello".into())),
        (
            ParamKind::Bytes,
            ParamValue::Bytes(vec![0, 1, 2, 250, 251, 252, 253]),
        ),
    ];
    for (kind, v) in cases {
        let voxy = to_value(kind, &v).unwrap();
        assert_eq!(to_param_value(kind, &voxy), Some(v), "{kind:?}");
    }
    assert_eq!(
        to_param_value(ParamKind::Bytes, &Value::String("not base64!".into())),
        None
    );
    assert_eq!(to_param_value(ParamKind::Double, &Value::Bool(true)), None);
}

#[test]
fn base64_is_standard_and_strict() {
    for (raw, text) in [
        (&b""[..], ""),
        (b"f", "Zg=="),
        (b"fo", "Zm8="),
        (b"foo", "Zm9v"),
        (b"foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64_encode(raw), text);
        assert_eq!(base64_decode(text).as_deref(), Some(raw));
    }
    for bad in ["Zg=", "Z===", "Zg==Zg==", "Zm9v!", "===="] {
        assert_eq!(base64_decode(bad), None, "{bad}");
    }
}
