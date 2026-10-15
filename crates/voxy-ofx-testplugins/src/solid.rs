//! `org.voxy.test.Solid`: a generator (generator context only) that fills
//! the render window with its `color` parameter, premultiplied.

use std::ffi::{CStr, c_char, c_void};

use voxy_ofx_sys as ofx;

use super::{StaticPlugin, c, fetch_suites, fx, image_data, param, params, props, set_host};

/// # Safety
/// Called by the host with valid arguments.
unsafe fn describe(h: ofx::OfxImageEffectHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let p = props();
        let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        fx().getPropertySet.unwrap()(h, &raw mut ph);
        p.propSetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, c(b"Test Solid\0"));
        p.propSetString.unwrap()(
            ph,
            c(ofx::kOfxImageEffectPluginPropGrouping),
            0,
            c(b"voXY tests\0"),
        );
        p.propSetString.unwrap()(
            ph,
            c(ofx::kOfxImageEffectPropSupportedContexts),
            0,
            c(ofx::kOfxImageEffectContextGenerator),
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
        fx().clipDefine.unwrap()(h, c(ofx::kOfxImageEffectOutputClipName), &raw mut cp);
        p.propSetString.unwrap()(
            cp,
            c(ofx::kOfxImageEffectPropSupportedComponents),
            0,
            c(ofx::kOfxImageComponentRGBA),
        );
        let mut set: ofx::OfxParamSetHandle = std::ptr::null_mut();
        fx().getParamSet.unwrap()(h, &raw mut set);
        let mut pp: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        params().paramDefine.unwrap()(set, c(ofx::kOfxParamTypeRGBA), c(b"color\0"), &raw mut pp);
        for (i, v) in [1.0, 0.5, 0.25, 1.0].into_iter().enumerate() {
            p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropDefault), i as i32, v);
        }
        p.propSetString.unwrap()(pp, c(ofx::kOfxPropLabel), 0, c(b"Colour\0"));
    }
    ofx::kOfxStatOK
}

/// # Safety
/// As above.
unsafe fn render(h: ofx::OfxImageEffectHandle, ins: ofx::OfxPropertySetHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host; pixels are addressed inside the
    // render window, which lies inside the output image.
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
        let mut rgba = [0.0_f64; 4];
        let [r, g, b, a] = &mut rgba;
        params().paramGetValueAtTime.unwrap()(
            param(h, b"color\0"),
            time,
            std::ptr::from_mut(r),
            std::ptr::from_mut(g),
            std::ptr::from_mut(b),
            std::ptr::from_mut(a),
        );
        let px = [
            (rgba[0] * rgba[3]) as f32,
            (rgba[1] * rgba[3]) as f32,
            (rgba[2] * rgba[3]) as f32,
            rgba[3] as f32,
        ];
        let mut ch: ofx::OfxImageClipHandle = std::ptr::null_mut();
        fx().clipGetHandle.unwrap()(
            h,
            c(ofx::kOfxImageEffectOutputClipName),
            &raw mut ch,
            std::ptr::null_mut(),
        );
        let mut img: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        if fx().clipGetImage.unwrap()(ch, time, std::ptr::null(), &raw mut img) != ofx::kOfxStatOK {
            return ofx::kOfxStatFailed;
        }
        let (d, db, d_row) = image_data(img);
        for y in window[1]..window[3] {
            for x in window[0]..window[2] {
                let di = ((y - db[1]) as usize * d_row) + (x - db[0]) as usize * 4;
                for (k, v) in px.iter().enumerate() {
                    *d.add(di + k) = *v;
                }
            }
        }
        fx().clipReleaseImage.unwrap()(img);
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
            _ => ofx::kOfxStatReplyDefault,
        }
    }
}

/// A generator that fills its output with one colour.
pub static SOLID: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Solid".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(set_host),
    mainEntry: Some(main_entry),
});
