//! Test plugins for voXY's OpenFX host, written by us against the raw C
//! API (owner decision P-0 Q2). Built as an `.ofx` binary for the sandbox
//! tests and linked directly into the in-process harness. Never shipped.
//!
//! - `org.voxy.test.Gain`: a filter that multiplies colour by `gain`, can
//!   render unpremultiplied (`straight`) and logs parameter changes.
//! - `org.voxy.test.Crash`: the same, but Render dereferences null.
//! - `org.voxy.test.Hang`: the same, but Render never returns and ignores
//!   abort.

// A test fixture: a missing suite entry may panic (and abort) loudly.
#![allow(
    unsafe_code,
    clippy::missing_safety_doc,
    clippy::many_single_char_names,
    clippy::unwrap_used
)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use voxy_ofx_sys as ofx;

static HOST: AtomicPtr<ofx::OfxHost> = AtomicPtr::new(std::ptr::null_mut());
static PROPS: AtomicPtr<ofx::OfxPropertySuiteV1> = AtomicPtr::new(std::ptr::null_mut());
static FX: AtomicPtr<ofx::OfxImageEffectSuiteV1> = AtomicPtr::new(std::ptr::null_mut());
static PARAMS: AtomicPtr<ofx::OfxParameterSuiteV1> = AtomicPtr::new(std::ptr::null_mut());
static MSG: AtomicPtr<ofx::OfxMessageSuiteV2> = AtomicPtr::new(std::ptr::null_mut());
/// Load actions so far.
pub static LOADS: AtomicUsize = AtomicUsize::new(0);
/// Unload actions so far.
pub static UNLOADS: AtomicUsize = AtomicUsize::new(0);
/// Live instances.
pub static INSTANCES: AtomicUsize = AtomicUsize::new(0);

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
#[derive(Debug)]
pub struct StaticPlugin(pub ofx::OfxPlugin);
// SAFETY: the struct only points at static strings and functions.
unsafe impl Sync for StaticPlugin {}

/// A filter that multiplies colour by `gain`.
pub static GAIN: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Gain".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 2,
    setHost: Some(set_host),
    mainEntry: Some(main_entry),
});

unsafe extern "C" fn crash_entry(
    action: *const c_char,
    handle: *const c_void,
    ins: ofx::OfxPropertySetHandle,
    outs: ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    // SAFETY: the host passes a valid action name.
    if unsafe { CStr::from_ptr(action) }.to_bytes_with_nul() == ofx::kOfxImageEffectActionRender {
        // Deliberately crash, as a buggy plugin would.
        // SAFETY: none; this is the point of the plugin.
        unsafe { std::ptr::read_volatile(std::ptr::null::<u8>()) };
    }
    // SAFETY: forwarding the host's own arguments.
    unsafe { main_entry(action, handle, ins, outs) }
}

unsafe extern "C" fn hang_entry(
    action: *const c_char,
    handle: *const c_void,
    ins: ofx::OfxPropertySetHandle,
    outs: ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    // SAFETY: the host passes a valid action name.
    if unsafe { CStr::from_ptr(action) }.to_bytes_with_nul() == ofx::kOfxImageEffectActionRender {
        loop {
            std::thread::park();
        }
    }
    // SAFETY: forwarding the host's own arguments.
    unsafe { main_entry(action, handle, ins, outs) }
}

/// Crashes in Render.
pub static CRASH: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Crash".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(set_host),
    mainEntry: Some(crash_entry),
});

/// Hangs in Render.
pub static HANG: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Hang".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(set_host),
    mainEntry: Some(hang_entry),
});

/// The bundle's plugins, in `OfxGetPlugin` order.
pub static ALL: [&StaticPlugin; 3] = [&GAIN, &CRASH, &HANG];

/// The OpenFX export: how many plugins the binary holds.
#[unsafe(no_mangle)]
pub extern "C" fn OfxGetNumberOfPlugins() -> c_int {
    ALL.len() as c_int
}

/// The OpenFX export: the `nth` plugin, or null.
#[unsafe(no_mangle)]
pub extern "C" fn OfxGetPlugin(nth: c_int) -> *mut ofx::OfxPlugin {
    usize::try_from(nth)
        .ok()
        .and_then(|i| ALL.get(i))
        .map_or(std::ptr::null_mut(), |p| {
            std::ptr::from_ref(&p.0).cast_mut()
        })
}
