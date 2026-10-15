//! `org.voxy.test.Fade`: a transition (transition context only) that mixes
//! `SourceFrom` into `SourceTo` by its `Transition` parameter. It is an
//! identity on `SourceFrom` at 0 and on `SourceTo` at 1.

use std::ffi::{CStr, c_char, c_void};

use voxy_ofx_sys as ofx;

use super::{StaticPlugin, c, double_at, fetch_suites, fx, image_data, params, props, set_host};

/// # Safety
/// Called by the host with valid arguments.
unsafe fn describe(h: ofx::OfxImageEffectHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let p = props();
        let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        fx().getPropertySet.unwrap()(h, &raw mut ph);
        p.propSetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, c(b"Test Fade\0"));
        p.propSetString.unwrap()(
            ph,
            c(ofx::kOfxImageEffectPropSupportedContexts),
            0,
            c(ofx::kOfxImageEffectContextTransition),
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
            ofx::kOfxImageEffectTransitionSourceFromClipName.as_slice(),
            ofx::kOfxImageEffectTransitionSourceToClipName,
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
        params().paramDefine.unwrap()(
            set,
            c(ofx::kOfxParamTypeDouble),
            c(ofx::kOfxImageEffectTransitionParamName),
            &raw mut pp,
        );
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropMin), 0, 0.0);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropMax), 0, 1.0);
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
        let t = double_at(h, ofx::kOfxImageEffectTransitionParamName, time);
        let clip: &[u8] = if t <= 0.0 {
            ofx::kOfxImageEffectTransitionSourceFromClipName
        } else if t >= 1.0 {
            ofx::kOfxImageEffectTransitionSourceToClipName
        } else {
            return ofx::kOfxStatReplyDefault;
        };
        props().propSetString.unwrap()(outs, c(ofx::kOfxPropName), 0, c(clip));
    }
    ofx::kOfxStatOK
}

/// # Safety
/// As above.
unsafe fn render(h: ofx::OfxImageEffectHandle, ins: ofx::OfxPropertySetHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host; pixels are addressed inside the
    // render window, which lies inside every image here.
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
        let t = double_at(h, ofx::kOfxImageEffectTransitionParamName, time) as f32;
        let image = |name: &[u8]| {
            let mut ch: ofx::OfxImageClipHandle = std::ptr::null_mut();
            fx().clipGetHandle.unwrap()(h, c(name), &raw mut ch, std::ptr::null_mut());
            let mut img: ofx::OfxPropertySetHandle = std::ptr::null_mut();
            let status = fx().clipGetImage.unwrap()(ch, time, std::ptr::null(), &raw mut img);
            (status == ofx::kOfxStatOK).then_some(img)
        };
        let (Some(a), Some(b), Some(dst)) = (
            image(ofx::kOfxImageEffectTransitionSourceFromClipName),
            image(ofx::kOfxImageEffectTransitionSourceToClipName),
            image(ofx::kOfxImageEffectOutputClipName),
        ) else {
            return ofx::kOfxStatFailed;
        };
        let (pa, ab, a_row) = image_data(a);
        let (pb, bb, b_row) = image_data(b);
        let (d, db, d_row) = image_data(dst);
        for y in window[1]..window[3] {
            for x in window[0]..window[2] {
                let ai = ((y - ab[1]) as usize * a_row) + (x - ab[0]) as usize * 4;
                let bi = ((y - bb[1]) as usize * b_row) + (x - bb[0]) as usize * 4;
                let di = ((y - db[1]) as usize * d_row) + (x - db[0]) as usize * 4;
                for k in 0..4 {
                    *d.add(di + k) = *pa.add(ai + k) * (1.0 - t) + *pb.add(bi + k) * t;
                }
            }
        }
        for img in [a, b, dst] {
            fx().clipReleaseImage.unwrap()(img);
        }
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

/// A cross-fade transition.
pub static FADE: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Fade".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(set_host),
    mainEntry: Some(main_entry),
});
