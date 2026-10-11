//! The in-process harness (O-A): a small plugin written against the raw C
//! API, linked into this test, driven through Load, Describe,
//! `DescribeInContext`, `CreateInstance`, the render actions and teardown.

#![allow(unsafe_code, clippy::unwrap_used, clippy::many_single_char_names)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use voxy_ofx_host::effect::{MessageKind, Project};
use voxy_ofx_host::image::{Components, Depth, Image, ImageSource, Premult, RectI};
use voxy_ofx_host::params::{LocalValues, ParamKind, ParamValue, ParamValues};
use voxy_ofx_host::suites::driver::{DriverError, Identity, Plugin};
use voxy_ofx_sys as ofx;

// ------------------------------------------------------------ the plugin

static HOST: AtomicPtr<ofx::OfxHost> = AtomicPtr::new(std::ptr::null_mut());
static PROPS: AtomicPtr<ofx::OfxPropertySuiteV1> = AtomicPtr::new(std::ptr::null_mut());
static FX: AtomicPtr<ofx::OfxImageEffectSuiteV1> = AtomicPtr::new(std::ptr::null_mut());
static PARAMS: AtomicPtr<ofx::OfxParameterSuiteV1> = AtomicPtr::new(std::ptr::null_mut());
static MSG: AtomicPtr<ofx::OfxMessageSuiteV2> = AtomicPtr::new(std::ptr::null_mut());
static LOADS: AtomicUsize = AtomicUsize::new(0);
static UNLOADS: AtomicUsize = AtomicUsize::new(0);
static INSTANCES: AtomicUsize = AtomicUsize::new(0);
/// Serialises the tests: the plugin's suites are process-wide.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn c(s: &[u8]) -> *const c_char {
    s.as_ptr().cast()
}

/// # Safety
/// Only after Load has fetched the suites.
unsafe fn props() -> &'static ofx::OfxPropertySuiteV1 {
    // SAFETY: set in Load to the host's static suite.
    unsafe { &*PROPS.load(Ordering::SeqCst) }
}

/// # Safety
/// As above.
unsafe fn fx() -> &'static ofx::OfxImageEffectSuiteV1 {
    // SAFETY: as above.
    unsafe { &*FX.load(Ordering::SeqCst) }
}

/// # Safety
/// As above.
unsafe fn params() -> &'static ofx::OfxParameterSuiteV1 {
    // SAFETY: as above.
    unsafe { &*PARAMS.load(Ordering::SeqCst) }
}

unsafe extern "C" fn set_host(host: *mut ofx::OfxHost) {
    HOST.store(host, Ordering::SeqCst);
}

/// # Safety
/// `host` was set by `setHost`.
unsafe fn fetch<T>(name: &[u8], version: c_int) -> *mut T {
    // SAFETY: the host struct stays valid while the plugin is loaded.
    unsafe {
        let host = &*HOST.load(Ordering::SeqCst);
        host.fetchSuite.unwrap()(host.host, c(name), version)
            .cast_mut()
            .cast()
    }
}

/// # Safety
/// `fx` is an instance handle; `name` a parameter of it.
unsafe fn param(fx_h: ofx::OfxImageEffectHandle, name: &[u8]) -> ofx::OfxParamHandle {
    // SAFETY: the caller's contract.
    unsafe {
        let mut set: ofx::OfxParamSetHandle = std::ptr::null_mut();
        fx().getParamSet.unwrap()(fx_h, &raw mut set);
        let mut p: ofx::OfxParamHandle = std::ptr::null_mut();
        params().paramGetHandle.unwrap()(set, c(name), &raw mut p, std::ptr::null_mut());
        p
    }
}

/// # Safety
/// As above, for a double parameter.
unsafe fn double_at(fx_h: ofx::OfxImageEffectHandle, name: &[u8], time: f64) -> f64 {
    let mut v = 0.0_f64;
    // SAFETY: the caller's contract.
    unsafe { params().paramGetValueAtTime.unwrap()(param(fx_h, name), time, &raw mut v) };
    v
}

/// # Safety
/// As above, for a boolean parameter.
unsafe fn bool_at(fx_h: ofx::OfxImageEffectHandle, name: &[u8], time: f64) -> bool {
    let mut v: c_int = 0;
    // SAFETY: the caller's contract.
    unsafe { params().paramGetValueAtTime.unwrap()(param(fx_h, name), time, &raw mut v) };
    v != 0
}

/// # Safety
/// Called by the host with valid arguments for `action`.
unsafe fn describe(h: ofx::OfxImageEffectHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let p = props();
        let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        fx().getPropertySet.unwrap()(h, &raw mut ph);
        p.propSetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, c(b"Test Gain\0"));
        p.propSetString.unwrap()(
            ph,
            c(ofx::kOfxImageEffectPluginPropGrouping),
            0,
            c(b"voXY tests\0"),
        );
        let contexts = [
            c(ofx::kOfxImageEffectContextFilter),
            c(ofx::kOfxImageEffectContextPaint),
        ];
        p.propSetStringN.unwrap()(
            ph,
            c(ofx::kOfxImageEffectPropSupportedContexts),
            2,
            contexts.as_ptr(),
        );
        let depths = [c(ofx::kOfxBitDepthByte), c(ofx::kOfxBitDepthFloat)];
        p.propSetStringN.unwrap()(
            ph,
            c(ofx::kOfxImageEffectPropSupportedPixelDepths),
            2,
            depths.as_ptr(),
        );
    }
    ofx::kOfxStatOK
}

/// # Safety
/// As above.
unsafe fn describe_in_context(h: ofx::OfxImageEffectHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let p = props();
        let mut cp: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        fx().clipDefine.unwrap()(h, c(ofx::kOfxImageEffectSimpleSourceClipName), &raw mut cp);
        let comps = [c(ofx::kOfxImageComponentRGBA)];
        p.propSetStringN.unwrap()(
            cp,
            c(ofx::kOfxImageEffectPropSupportedComponents),
            1,
            comps.as_ptr(),
        );
        fx().clipDefine.unwrap()(h, c(ofx::kOfxImageEffectOutputClipName), &raw mut cp);
        p.propSetStringN.unwrap()(
            cp,
            c(ofx::kOfxImageEffectPropSupportedComponents),
            1,
            comps.as_ptr(),
        );

        let mut set: ofx::OfxParamSetHandle = std::ptr::null_mut();
        fx().getParamSet.unwrap()(h, &raw mut set);
        let mut pp: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        params().paramDefine.unwrap()(set, c(ofx::kOfxParamTypeDouble), c(b"gain\0"), &raw mut pp);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropDefault), 0, 1.0);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropMin), 0, 0.0);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropDisplayMax), 0, 4.0);
        p.propSetString.unwrap()(pp, c(ofx::kOfxPropLabel), 0, c(b"Gain\0"));
        params().paramDefine.unwrap()(
            set,
            c(ofx::kOfxParamTypeBoolean),
            c(b"straight\0"),
            &raw mut pp,
        );
        p.propSetString.unwrap()(
            pp,
            c(ofx::kOfxParamPropHint),
            0,
            c(b"Render unpremultiplied\0"),
        );
    }
    ofx::kOfxStatOK
}

/// # Safety
/// As above.
unsafe fn clip_preferences(
    h: ofx::OfxImageEffectHandle,
    outs: ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        if bool_at(h, b"straight\0", 0.0) {
            props().propSetString.unwrap()(
                outs,
                c(ofx::kOfxImageEffectPropPreMultiplication),
                0,
                c(ofx::kOfxImageUnPreMultiplied),
            );
        }
    }
    ofx::kOfxStatOK
}

/// # Safety
/// As above.
unsafe fn is_identity(
    h: ofx::OfxImageEffectHandle,
    ins: ofx::OfxPropertySetHandle,
    outs: ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let mut time = 0.0;
        props().propGetDouble.unwrap()(ins, c(ofx::kOfxPropTime), 0, &raw mut time);
        if double_at(h, b"gain\0", time) == 1.0 && !bool_at(h, b"straight\0", time) {
            props().propSetString.unwrap()(
                outs,
                c(ofx::kOfxPropName),
                0,
                c(ofx::kOfxImageEffectSimpleSourceClipName),
            );
            return ofx::kOfxStatOK;
        }
    }
    ofx::kOfxStatReplyDefault
}

/// # Safety
/// As above.
unsafe fn image_data(img: ofx::OfxPropertySetHandle) -> (*mut f32, [c_int; 4], usize) {
    // SAFETY: an image handle from the host.
    unsafe {
        let p = props();
        let mut data: *mut c_void = std::ptr::null_mut();
        let mut bounds = [0; 4];
        let mut row_bytes: c_int = 0;
        p.propGetPointer.unwrap()(img, c(ofx::kOfxImagePropData), 0, &raw mut data);
        p.propGetIntN.unwrap()(img, c(ofx::kOfxImagePropBounds), 4, bounds.as_mut_ptr());
        p.propGetInt.unwrap()(img, c(ofx::kOfxImagePropRowBytes), 0, &raw mut row_bytes);
        (data.cast(), bounds, row_bytes as usize / 4)
    }
}

/// # Safety
/// As above.
unsafe fn render(h: ofx::OfxImageEffectHandle, ins: ofx::OfxPropertySetHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host; pixels are addressed inside the
    // render window, which lies inside both images.
    unsafe {
        let p = props();
        let mut time = 0.0;
        let mut window = [0 as c_int; 4];
        p.propGetDouble.unwrap()(ins, c(ofx::kOfxPropTime), 0, &raw mut time);
        p.propGetIntN.unwrap()(
            ins,
            c(ofx::kOfxImageEffectPropRenderWindow),
            4,
            window.as_mut_ptr(),
        );
        let gain = double_at(h, b"gain\0", time) as f32;
        let straight = bool_at(h, b"straight\0", time);

        let clip = |name: &[u8]| {
            let mut ch: ofx::OfxImageClipHandle = std::ptr::null_mut();
            fx().clipGetHandle.unwrap()(h, c(name), &raw mut ch, std::ptr::null_mut());
            let mut img: ofx::OfxPropertySetHandle = std::ptr::null_mut();
            let status = fx().clipGetImage.unwrap()(ch, time, std::ptr::null(), &raw mut img);
            (status == ofx::kOfxStatOK).then_some(img)
        };
        let (Some(src), Some(dst)) = (
            clip(ofx::kOfxImageEffectSimpleSourceClipName),
            clip(ofx::kOfxImageEffectOutputClipName),
        ) else {
            return ofx::kOfxStatFailed;
        };
        let (s, sb, s_row) = image_data(src);
        let (d, db, d_row) = image_data(dst);
        for y in window[1]..window[3] {
            for x in window[0]..window[2] {
                let si = ((y - sb[1]) as usize * s_row) + (x - sb[0]) as usize * 4;
                let di = ((y - db[1]) as usize * d_row) + (x - db[0]) as usize * 4;
                let a = *s.add(si + 3);
                for k in 0..3 {
                    let v = *s.add(si + k) * gain;
                    *d.add(di + k) = if straight && a > 0.0 { v / a } else { v };
                }
                *d.add(di + 3) = a;
            }
            if fx().abort.unwrap()(h) != 0 {
                break;
            }
        }
        fx().clipReleaseImage.unwrap()(src);
        fx().clipReleaseImage.unwrap()(dst);
    }
    ofx::kOfxStatOK
}

/// # Safety
/// As above.
unsafe fn instance_changed(
    h: ofx::OfxImageEffectHandle,
    ins: ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let mut name: *mut c_char = std::ptr::null_mut();
        props().propGetString.unwrap()(ins, c(ofx::kOfxPropName), 0, &raw mut name);
        let msg = &*MSG.load(Ordering::SeqCst);
        msg.message.unwrap()(
            h.cast(),
            c(ofx::kOfxMessageLog),
            c(b"changed\0"),
            c(b"%s changed\0"),
            name.cast_const(),
        );
    }
    ofx::kOfxStatOK
}

unsafe extern "C" fn main_entry(
    action: *const c_char,
    handle: *const c_void,
    ins: ofx::OfxPropertySetHandle,
    outs: ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    // SAFETY: the host calls with valid arguments for each action.
    unsafe {
        let action = CStr::from_ptr(action).to_bytes_with_nul();
        let h = handle.cast_mut().cast::<ofx::OfxImageEffectStruct>();
        match action {
            a if a == ofx::kOfxActionLoad => {
                PROPS.store(fetch(ofx::kOfxPropertySuite, 1), Ordering::SeqCst);
                FX.store(fetch(ofx::kOfxImageEffectSuite, 1), Ordering::SeqCst);
                PARAMS.store(fetch(ofx::kOfxParameterSuite, 1), Ordering::SeqCst);
                MSG.store(fetch(ofx::kOfxMessageSuite, 2), Ordering::SeqCst);
                LOADS.fetch_add(1, Ordering::SeqCst);
                ofx::kOfxStatOK
            }
            a if a == ofx::kOfxActionUnload => {
                UNLOADS.fetch_add(1, Ordering::SeqCst);
                ofx::kOfxStatOK
            }
            a if a == ofx::kOfxActionDescribe => describe(h),
            a if a == ofx::kOfxImageEffectActionDescribeInContext => describe_in_context(h),
            a if a == ofx::kOfxActionCreateInstance => {
                INSTANCES.fetch_add(1, Ordering::SeqCst);
                ofx::kOfxStatOK
            }
            a if a == ofx::kOfxActionDestroyInstance => {
                INSTANCES.fetch_sub(1, Ordering::SeqCst);
                ofx::kOfxStatOK
            }
            a if a == ofx::kOfxImageEffectActionGetClipPreferences => clip_preferences(h, outs),
            a if a == ofx::kOfxImageEffectActionIsIdentity => is_identity(h, ins, outs),
            a if a == ofx::kOfxImageEffectActionRender => render(h, ins),
            a if a == ofx::kOfxActionInstanceChanged => instance_changed(h, ins),
            _ => ofx::kOfxStatReplyDefault,
        }
    }
}

/// An `OfxPlugin` in a static: its pointers are to `'static` data.
struct StaticPlugin(ofx::OfxPlugin);
// SAFETY: the struct only points at static strings and functions.
unsafe impl Sync for StaticPlugin {}

static GAIN: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Gain".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 2,
    setHost: Some(set_host),
    mainEntry: Some(main_entry),
});

static NOT_AN_EFFECT: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"SomeOtherAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Other".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(set_host),
    mainEntry: Some(main_entry),
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
