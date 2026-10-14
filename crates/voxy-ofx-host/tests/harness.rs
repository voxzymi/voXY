//! The in-process harness (O-A): our gain test plugin
//! (`voxy-ofx-testplugins`, written against the raw C API) linked into this
//! test and driven through Load, Describe,
//! `DescribeInContext`, `CreateInstance`, the render actions and teardown.

#![allow(unsafe_code, clippy::unwrap_used, clippy::many_single_char_names)]

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use voxy_ofx_host::effect::{MessageKind, Project};
use voxy_ofx_host::image::{Components, Depth, Image, ImageSource, Premult, RectI};
use voxy_ofx_host::params::{LocalValues, ParamKind, ParamValue, ParamValues};
use voxy_ofx_host::suites::driver::{DriverError, Identity, Plugin};
use voxy_ofx_sys as ofx;

use voxy_ofx_testplugins::{GAIN, INSTANCES, LOADS, StaticPlugin, UNLOADS};

/// Serialises the tests: the plugin's suites are process-wide.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

unsafe extern "C" fn no_host(_: *mut ofx::OfxHost) {}

static NOT_AN_EFFECT: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"SomeOtherAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Other".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(no_host),
    mainEntry: GAIN.0.mainEntry,
});

// -------------------------------------------------------------- the host

/// A 2×2 premultiplied source, and the output buffer.
#[derive(Debug)]
struct Frames {
    source: Arc<Image>,
    output: Arc<Image>,
}

const SOURCE: [f32; 16] = [
    0.1, 0.2, 0.3, 1.0, //
    0.2, 0.1, 0.0, 0.5, //
    0.0, 0.0, 0.0, 0.0, //
    0.25, 0.25, 0.25, 0.25,
];

impl Frames {
    fn new() -> Frames {
        let b = RectI::new(0, 0, 2, 2);
        Frames {
            source: Arc::new(
                Image::from_rgba(
                    b,
                    &SOURCE,
                    Depth::Float,
                    Components::Rgba,
                    Premult::Premultiplied,
                )
                .unwrap(),
            ),
            output: Arc::new(
                Image::new(
                    b,
                    Depth::Float,
                    Components::Rgba,
                    Premult::Premultiplied,
                    [1.0, 1.0],
                    1.0,
                )
                .unwrap(),
            ),
        }
    }
}

impl ImageSource for Frames {
    fn image(&self, clip: &str, _time: f64) -> Option<Arc<Image>> {
        Some(Arc::clone(if clip == "Output" {
            &self.output
        } else {
            &self.source
        }))
    }

    fn region_of_definition(&self, _clip: &str, _time: f64) -> Option<[f64; 4]> {
        Some([0.0, 0.0, 2.0, 2.0])
    }
}

fn scaled(gain: f32) -> Vec<f32> {
    SOURCE
        .iter()
        .enumerate()
        .map(|(i, v)| if i % 4 == 3 { *v } else { v * gain })
        .collect()
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
}

#[test]
fn a_plugin_runs_through_its_whole_life() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    let loads = LOADS.load(Ordering::SeqCst);
    let unloads = UNLOADS.load(Ordering::SeqCst);
    {
        // SAFETY: a valid static plugin.
        let plugin = unsafe { Plugin::load(&GAIN.0) }.unwrap();
        assert_eq!(LOADS.load(Ordering::SeqCst), loads + 1);
        assert_eq!(plugin.identifier, "org.voxy.test.Gain");
        assert_eq!(plugin.version, (1, 2));

        let d = plugin.describe().unwrap();
        assert_eq!(
            d.contexts
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            ["OfxImageEffectContextFilter"],
            "paint is not a context voXY hosts"
        );
        assert_eq!(
            d.general.props.string("OfxPropLabel", 0).as_deref(),
            Some("Test Gain")
        );
        let filter = d.context("OfxImageEffectContextFilter").unwrap();
        let names: Vec<(String, ParamKind)> = filter
            .params
            .params()
            .iter()
            .map(|p| (p.name.clone(), p.kind))
            .collect();
        assert_eq!(
            names,
            [
                ("gain".to_owned(), ParamKind::Double),
                ("straight".to_owned(), ParamKind::Boolean)
            ]
        );
        assert_eq!(
            filter
                .clips()
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["Source", "Output"]
        );

        let values = Arc::new(LocalValues::new());
        let frames = Arc::new(Frames::new());
        let project = Project {
            size: [2.0, 2.0],
            extent: [2.0, 2.0],
            ..Project::default()
        };
        let mut inst = plugin
            .create_instance(
                &d,
                "OfxImageEffectContextFilter",
                project,
                values.clone(),
                frames.clone(),
            )
            .unwrap();
        assert_eq!(INSTANCES.load(Ordering::SeqCst), 1);
        assert_eq!(
            values.value_at("gain", 0.0),
            Some(ParamValue::Doubles(vec![1.0])),
            "the default"
        );

        let prefs = inst.clip_preferences().unwrap();
        assert_eq!(
            prefs.clips["Source"],
            (Depth::Float, Components::Rgba),
            "float preferred"
        );
        assert_eq!(prefs.output_premult, Premult::Premultiplied);
        assert_eq!(
            inst.region_of_definition(0.0, [1.0, 1.0]).unwrap(),
            [0.0, 0.0, 2.0, 2.0],
            "default: the source's"
        );
        let window = [0, 0, 2, 2];
        assert_eq!(
            inst.is_identity(0.0, window, [1.0, 1.0]).unwrap(),
            Identity::Clip {
                name: "Source".into(),
                time: 0.0
            }
        );

        values.set("gain", None, ParamValue::Doubles(vec![2.0]));
        assert_eq!(
            inst.is_identity(0.0, window, [1.0, 1.0]).unwrap(),
            Identity::No
        );
        inst.render(0.0, window, [1.0, 1.0]).unwrap();
        assert!(close(&frames.output.to_rgba(), &scaled(2.0)));

        // A plugin that renders unpremultiplied gets premultiplied (D2).
        values.set("straight", None, ParamValue::Ints(vec![1]));
        assert_eq!(
            inst.clip_preferences().unwrap().output_premult,
            Premult::Unpremultiplied
        );
        frames.output.write_rgba(&[0.0; 16]);
        inst.render(0.0, window, [1.0, 1.0]).unwrap();
        assert!(
            close(&frames.output.to_rgba(), &scaled(2.0)),
            "{:?}",
            frames.output.to_rgba()
        );

        inst.param_changed("gain", 0.0).unwrap();
        let msgs = inst.fx.take_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            (msgs[0].kind, msgs[0].text.as_str()),
            (MessageKind::Log, "gain changed")
        );

        drop(inst);
        assert_eq!(INSTANCES.load(Ordering::SeqCst), 0, "DestroyInstance ran");
        assert_eq!(UNLOADS.load(Ordering::SeqCst), unloads);
    }
    assert_eq!(UNLOADS.load(Ordering::SeqCst), unloads + 1, "Unload ran");
    assert_eq!(
        voxy_ofx_host::suites::live_images(),
        0,
        "every image released"
    );
}

#[test]
fn other_apis_and_unknown_contexts_are_refused() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    // SAFETY: a valid static plugin.
    let err = unsafe { Plugin::load(&NOT_AN_EFFECT.0) }.unwrap_err();
    assert_eq!(
        err,
        DriverError::WrongApi {
            api: "SomeOtherAPI".into(),
            version: 1
        }
    );

    // SAFETY: as above.
    let plugin = unsafe { Plugin::load(&GAIN.0) }.unwrap();
    let d = plugin.describe().unwrap();
    let err = plugin
        .create_instance(
            &d,
            "OfxImageEffectContextGenerator",
            Project::default(),
            Arc::new(LocalValues::new()),
            Arc::new(Frames::new()),
        )
        .unwrap_err();
    assert_eq!(
        err,
        DriverError::UnknownContext("OfxImageEffectContextGenerator".into())
    );
}
