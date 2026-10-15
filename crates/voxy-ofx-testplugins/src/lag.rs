//! `org.voxy.test.Lag`: a filter that multiplies colour by its `gain` one
//! frame earlier (a parameter query at another time) and writes the number
//! of `gain`'s keys into alpha, so tests can see both. Its push button
//! `double` doubles `gain` (a plugin edit) and logs a message. Alpha also
//! gets 10 when the render says it is sequential, and 100 between
//! `BeginSequenceRender` and `EndSequenceRender`.

use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

use voxy_ofx_sys as ofx;

use super::{
    StaticPlugin, c, double_at, fetch_suites, fx, image_data, param, params, props, set_host,
};

/// Between Begin and End of a sequence.
static IN_SEQUENCE: AtomicBool = AtomicBool::new(false);

/// # Safety
/// Called by the host with valid arguments.
unsafe fn describe(h: ofx::OfxImageEffectHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let p = props();
        let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        fx().getPropertySet.unwrap()(h, &raw mut ph);
        p.propSetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, c(b"Test Lag\0"));
        p.propSetString.unwrap()(
            ph,
            c(ofx::kOfxImageEffectPropSupportedContexts),
            0,
            c(ofx::kOfxImageEffectContextFilter),
        );
        p.propSetString.unwrap()(
            ph,
            c(ofx::kOfxImageEffectPropSupportedPixelDepths),
            0,
            c(ofx::kOfxBitDepthFloat),
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
        for clip in [
            ofx::kOfxImageEffectSimpleSourceClipName,
            ofx::kOfxImageEffectOutputClipName,
        ] {
            fx().clipDefine.unwrap()(h, c(clip), &raw mut cp);
            p.propSetString.unwrap()(
                cp,
                c(ofx::kOfxImageEffectPropSupportedComponents),
                0,
                c(ofx::kOfxImageComponentRGBA),
            );
        }
        let mut set: ofx::OfxParamSetHandle = std::ptr::null_mut();
        fx().getParamSet.unwrap()(h, &raw mut set);
        let mut pp: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        params().paramDefine.unwrap()(set, c(ofx::kOfxParamTypeDouble), c(b"gain\0"), &raw mut pp);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropDefault), 0, 1.0);
        params().paramDefine.unwrap()(
            set,
            c(ofx::kOfxParamTypePushButton),
            c(b"double\0"),
            &raw mut pp,
        );
        p.propSetString.unwrap()(pp, c(ofx::kOfxPropLabel), 0, c(b"Double Gain\0"));
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
        let mut time = 0.0;
        props().propGetString.unwrap()(ins, c(ofx::kOfxPropName), 0, &raw mut name);
        props().propGetDouble.unwrap()(ins, c(ofx::kOfxPropTime), 0, &raw mut time);
        if CStr::from_ptr(name).to_bytes() == b"double" {
            let gain = double_at(h, b"gain\0", time);
            params().paramSetValue.unwrap()(param(h, b"gain\0"), gain * 2.0);
            let msg = &*super::MSG.load(std::sync::atomic::Ordering::SeqCst);
            msg.message.unwrap()(
                h.cast(),
                c(ofx::kOfxMessageMessage),
                std::ptr::null(),
                c(b"Gain doubled\0"),
            );
        }
    }
    ofx::kOfxStatOK
}

/// # Safety
/// As above.
unsafe fn render(h: ofx::OfxImageEffectHandle, ins: ofx::OfxPropertySetHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host; pixels are addressed inside the
    // render window, which lies inside both images.
    unsafe {
        let p = props();
        let mut time = 0.0;
        let mut window = [0; 4];
        p.propGetDouble.unwrap()(ins, c(ofx::kOfxPropTime), 0, &raw mut time);
        p.propGetIntN.unwrap()(
            ins,
            c(ofx::kOfxImageEffectPropRenderWindow),
            4,
            window.as_mut_ptr(),
        );
        let gain = double_at(h, b"gain\0", time - 1.0) as f32;
        let mut keys: c_uint = 0;
        params().paramGetNumKeys.unwrap()(param(h, b"gain\0"), &raw mut keys);
        let mut sequential: c_int = 0;
        p.propGetInt.unwrap()(
            ins,
            c(ofx::kOfxImageEffectPropSequentialRenderStatus),
            0,
            &raw mut sequential,
        );
        let alpha = keys as f32
            + 10.0 * sequential as f32
            + if IN_SEQUENCE.load(Ordering::SeqCst) {
                100.0
            } else {
                0.0
            };
        let image = |name: &[u8]| {
            let mut ch: ofx::OfxImageClipHandle = std::ptr::null_mut();
            fx().clipGetHandle.unwrap()(h, c(name), &raw mut ch, std::ptr::null_mut());
            let mut img: ofx::OfxPropertySetHandle = std::ptr::null_mut();
            let status = fx().clipGetImage.unwrap()(ch, time, std::ptr::null(), &raw mut img);
            (status == ofx::kOfxStatOK).then_some(img)
        };
        let (Some(src), Some(dst)) = (
            image(ofx::kOfxImageEffectSimpleSourceClipName),
            image(ofx::kOfxImageEffectOutputClipName),
        ) else {
            return ofx::kOfxStatFailed;
        };
        let (s, sb, s_row) = image_data(src);
        let (d, db, d_row) = image_data(dst);
        for y in window[1]..window[3] {
            for x in window[0]..window[2] {
                let si = ((y - sb[1]) as usize * s_row) + (x - sb[0]) as usize * 4;
                let di = ((y - db[1]) as usize * d_row) + (x - db[0]) as usize * 4;
                for k in 0..3 {
                    *d.add(di + k) = *s.add(si + k) * gain;
                }
                *d.add(di + 3) = alpha;
            }
        }
        fx().clipReleaseImage.unwrap()(src);
        fx().clipReleaseImage.unwrap()(dst);
    }
    ofx::kOfxStatOK
}

unsafe extern "C" fn main_entry(
    action: *const c_char,
    handle: *const c_void,
    ins: ofx::OfxPropertySetHandle,
    _outs: ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    // SAFETY: the host calls with valid arguments for each action.
    unsafe {
        let action = CStr::from_ptr(action).to_bytes_with_nul();
        let h = handle.cast_mut().cast::<ofx::OfxImageEffectStruct>();
        match action {
            a if a == ofx::kOfxActionLoad => {
                fetch_suites();
                ofx::kOfxStatOK
            }
            a if a == ofx::kOfxActionDescribe => describe(h),
            a if a == ofx::kOfxImageEffectActionDescribeInContext => describe_in_context(h),
            a if a == ofx::kOfxImageEffectActionRender => render(h, ins),
            a if a == ofx::kOfxActionInstanceChanged => instance_changed(h, ins),
            a if a == ofx::kOfxImageEffectActionBeginSequenceRender => {
                IN_SEQUENCE.store(true, Ordering::SeqCst);
                ofx::kOfxStatOK
            }
            a if a == ofx::kOfxImageEffectActionEndSequenceRender => {
                IN_SEQUENCE.store(false, Ordering::SeqCst);
                ofx::kOfxStatOK
            }
            _ => ofx::kOfxStatReplyDefault,
        }
    }
}

/// Reads its parameter one frame back.
pub static LAG: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Lag".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(set_host),
    mainEntry: Some(main_entry),
});
