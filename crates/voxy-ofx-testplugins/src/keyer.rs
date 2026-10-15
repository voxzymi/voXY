//! `org.voxy.test.Keyer`: a filter that keys out pixels whose
//! (unpremultiplied) colour is within `tolerance` of `key` on every
//! channel. It is an identity at tolerance 0.

use std::ffi::{CStr, c_char, c_void};

use voxy_ofx_sys as ofx;

use super::{
    StaticPlugin, c, double_at, fetch_suites, fx, image_data, param, params, props, set_host,
};

/// # Safety
/// Called by the host with valid arguments.
unsafe fn describe(h: ofx::OfxImageEffectHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let p = props();
        let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        fx().getPropertySet.unwrap()(h, &raw mut ph);
        p.propSetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, c(b"Test Keyer\0"));
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
        params().paramDefine.unwrap()(set, c(ofx::kOfxParamTypeRGB), c(b"key\0"), &raw mut pp);
        for (i, v) in [0.0, 1.0, 0.0].into_iter().enumerate() {
            p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropDefault), i as i32, v);
        }
        p.propSetString.unwrap()(pp, c(ofx::kOfxPropLabel), 0, c(b"Key Colour\0"));
        params().paramDefine.unwrap()(
            set,
            c(ofx::kOfxParamTypeDouble),
            c(b"tolerance\0"),
            &raw mut pp,
        );
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropDefault), 0, 0.2);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropMin), 0, 0.0);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropMax), 0, 1.0);
        p.propSetString.unwrap()(pp, c(ofx::kOfxPropLabel), 0, c(b"Tolerance\0"));
    }
    ofx::kOfxStatOK
}

/// # Safety
/// As above.
unsafe fn time_of(ins: ofx::OfxPropertySetHandle) -> f64 {
    let mut time = 0.0;
    // SAFETY: valid handles from the host.
    unsafe { props().propGetDouble.unwrap()(ins, c(ofx::kOfxPropTime), 0, &raw mut time) };
    time
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
        if double_at(h, b"tolerance\0", time_of(ins)) <= 0.0 {
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
unsafe fn render(h: ofx::OfxImageEffectHandle, ins: ofx::OfxPropertySetHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host; pixels are addressed inside the
    // render window, which lies inside both images.
    unsafe {
        let time = time_of(ins);
        let mut window = [0; 4];
        props().propGetIntN.unwrap()(
            ins,
            c(ofx::kOfxImageEffectPropRenderWindow),
            4,
            window.as_mut_ptr(),
        );
        let mut key = [0.0_f64; 3];
        let [kr, kg, kb] = &mut key;
        params().paramGetValueAtTime.unwrap()(
            param(h, b"key\0"),
            time,
            std::ptr::from_mut(kr),
            std::ptr::from_mut(kg),
            std::ptr::from_mut(kb),
        );
        let tolerance = double_at(h, b"tolerance\0", time) as f32;
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
                let a = *s.add(si + 3);
                let close = a > 0.0
                    && (0..3).all(|k| (*s.add(si + k) / a - key[k] as f32).abs() < tolerance);
                for k in 0..4 {
                    *d.add(di + k) = if close { 0.0 } else { *s.add(si + k) };
                }
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
    outs: ofx::OfxPropertySetHandle,
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
            a if a == ofx::kOfxImageEffectActionIsIdentity => is_identity(h, ins, outs),
            a if a == ofx::kOfxImageEffectActionRender => render(h, ins),
            _ => ofx::kOfxStatReplyDefault,
        }
    }
}

/// Keys out one colour.
pub static KEYER: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Keyer".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(set_host),
    mainEntry: Some(main_entry),
});
