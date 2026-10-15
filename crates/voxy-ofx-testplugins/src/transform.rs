//! `org.voxy.test.Transform`: a filter that scales its source by `scale`
//! about the canonical origin, then moves it by `offset` (canonical
//! pixels), sampling the nearest pixel. Its region of definition and of
//! interest follow the mapping; it is an identity at offset 0, scale 1.

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
        p.propSetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, c(b"Test Transform\0"));
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
        params().paramDefine.unwrap()(
            set,
            c(ofx::kOfxParamTypeDouble2D),
            c(b"offset\0"),
            &raw mut pp,
        );
        p.propSetString.unwrap()(
            pp,
            c(ofx::kOfxParamPropDoubleType),
            0,
            c(ofx::kOfxParamDoubleTypeXYAbsolute),
        );
        p.propSetString.unwrap()(pp, c(ofx::kOfxPropLabel), 0, c(b"Offset\0"));
        params().paramDefine.unwrap()(set, c(ofx::kOfxParamTypeDouble), c(b"scale\0"), &raw mut pp);
        p.propSetString.unwrap()(
            pp,
            c(ofx::kOfxParamPropDoubleType),
            0,
            c(ofx::kOfxParamDoubleTypeScale),
        );
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropDefault), 0, 1.0);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropMin), 0, 0.01);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropMax), 0, 100.0);
        p.propSetString.unwrap()(pp, c(ofx::kOfxPropLabel), 0, c(b"Scale\0"));
    }
    ofx::kOfxStatOK
}

/// `(offset, scale)` at `time`.
///
/// # Safety
/// As above.
unsafe fn mapping(h: ofx::OfxImageEffectHandle, time: f64) -> ([f64; 2], f64) {
    let (mut x, mut y) = (0.0_f64, 0.0_f64);
    // SAFETY: valid handles from the host.
    unsafe {
        params().paramGetValueAtTime.unwrap()(param(h, b"offset\0"), time, &raw mut x, &raw mut y);
        ([x, y], double_at(h, b"scale\0", time))
    }
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
unsafe fn source_clip(h: ofx::OfxImageEffectHandle) -> ofx::OfxImageClipHandle {
    let mut ch: ofx::OfxImageClipHandle = std::ptr::null_mut();
    // SAFETY: valid handles from the host.
    unsafe {
        fx().clipGetHandle.unwrap()(
            h,
            c(ofx::kOfxImageEffectSimpleSourceClipName),
            &raw mut ch,
            std::ptr::null_mut(),
        );
    }
    ch
}

/// # Safety
/// As above.
unsafe fn region_of_definition(
    h: ofx::OfxImageEffectHandle,
    ins: ofx::OfxPropertySetHandle,
    outs: ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let time = time_of(ins);
        let ([ox, oy], s) = mapping(h, time);
        let mut r = ofx::OfxRectD {
            x1: 0.0,
            y1: 0.0,
            x2: 0.0,
            y2: 0.0,
        };
        fx().clipGetRegionOfDefinition.unwrap()(source_clip(h), time, &raw mut r);
        let v = [r.x1 * s + ox, r.y1 * s + oy, r.x2 * s + ox, r.y2 * s + oy];
        props().propSetDoubleN.unwrap()(
            outs,
            c(ofx::kOfxImageEffectPropRegionOfDefinition),
            4,
            v.as_ptr(),
        );
    }
    ofx::kOfxStatOK
}

/// # Safety
/// As above.
unsafe fn regions_of_interest(
    h: ofx::OfxImageEffectHandle,
    ins: ofx::OfxPropertySetHandle,
    outs: ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let ([ox, oy], s) = mapping(h, time_of(ins));
        let mut r = [0.0_f64; 4];
        props().propGetDoubleN.unwrap()(
            ins,
            c(ofx::kOfxImageEffectPropRegionOfInterest),
            4,
            r.as_mut_ptr(),
        );
        let v = [
            (r[0] - ox) / s,
            (r[1] - oy) / s,
            (r[2] - ox) / s,
            (r[3] - oy) / s,
        ];
        props().propSetDoubleN.unwrap()(outs, c(b"OfxImageClipPropRoI_Source\0"), 4, v.as_ptr());
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
        if mapping(h, time_of(ins)) == ([0.0, 0.0], 1.0) {
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
    // SAFETY: valid handles from the host; every read is checked against
    // the source bounds, every write lies in the render window.
    unsafe {
        let p = props();
        let time = time_of(ins);
        let mut window = [0; 4];
        let mut rs = [1.0_f64; 2];
        p.propGetIntN.unwrap()(
            ins,
            c(ofx::kOfxImageEffectPropRenderWindow),
            4,
            window.as_mut_ptr(),
        );
        p.propGetDoubleN.unwrap()(
            ins,
            c(ofx::kOfxImageEffectPropRenderScale),
            2,
            rs.as_mut_ptr(),
        );
        let ([ox, oy], s) = mapping(h, time);
        let mut out_clip: ofx::OfxImageClipHandle = std::ptr::null_mut();
        fx().clipGetHandle.unwrap()(
            h,
            c(ofx::kOfxImageEffectOutputClipName),
            &raw mut out_clip,
            std::ptr::null_mut(),
        );
        let image = |ch: ofx::OfxImageClipHandle| {
            let mut img: ofx::OfxPropertySetHandle = std::ptr::null_mut();
            let status = fx().clipGetImage.unwrap()(ch, time, std::ptr::null(), &raw mut img);
            (status == ofx::kOfxStatOK).then_some(img)
        };
        let (Some(src), Some(dst)) = (image(source_clip(h)), image(out_clip)) else {
            return ofx::kOfxStatFailed;
        };
        let (ps, sb, s_row) = image_data(src);
        let (d, db, d_row) = image_data(dst);
        for y in window[1]..window[3] {
            for x in window[0]..window[2] {
                // The pixel centre in canonical coordinates, mapped back.
                let cx = (f64::from(x) + 0.5) / rs[0];
                let cy = (f64::from(y) + 0.5) / rs[1];
                let sx = (((cx - ox) / s) * rs[0]).floor() as i32;
                let sy = (((cy - oy) / s) * rs[1]).floor() as i32;
                let di = ((y - db[1]) as usize * d_row) + (x - db[0]) as usize * 4;
                let inside = sx >= sb[0] && sx < sb[2] && sy >= sb[1] && sy < sb[3];
                for k in 0..4 {
                    *d.add(di + k) = if inside {
                        *ps.add(((sy - sb[1]) as usize * s_row) + (sx - sb[0]) as usize * 4 + k)
                    } else {
                        0.0
                    };
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
            a if a == ofx::kOfxImageEffectActionGetRegionOfDefinition => {
                region_of_definition(h, ins, outs)
            }
            a if a == ofx::kOfxImageEffectActionGetRegionsOfInterest => {
                regions_of_interest(h, ins, outs)
            }
            a if a == ofx::kOfxImageEffectActionIsIdentity => is_identity(h, ins, outs),
            a if a == ofx::kOfxImageEffectActionRender => render(h, ins),
            _ => ofx::kOfxStatReplyDefault,
        }
    }
}

/// Scales and moves its source.
pub static TRANSFORM: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Transform".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(set_host),
    mainEntry: Some(main_entry),
});
