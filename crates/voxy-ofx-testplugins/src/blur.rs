//! `org.voxy.test.Blur`: a filter that box-blurs by `radius` (canonical
//! pixels). It reports a region of definition grown by the radius, asks for
//! a region of interest grown by the radius, and is an identity at radius
//! 0. Pixels outside the source are transparent.

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
        p.propSetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, c(b"Test Blur\0"));
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
            c(ofx::kOfxParamTypeDouble),
            c(b"radius\0"),
            &raw mut pp,
        );
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropDefault), 0, 2.0);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropMin), 0, 0.0);
        p.propSetDouble.unwrap()(pp, c(ofx::kOfxParamPropMax), 0, 100.0);
        p.propSetString.unwrap()(pp, c(ofx::kOfxPropLabel), 0, c(b"Radius\0"));
    }
    ofx::kOfxStatOK
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
        let p = props();
        let mut time = 0.0;
        p.propGetDouble.unwrap()(ins, c(ofx::kOfxPropTime), 0, &raw mut time);
        let r = double_at(h, b"radius\0", time);
        let mut rod = ofx::OfxRectD {
            x1: 0.0,
            y1: 0.0,
            x2: 0.0,
            y2: 0.0,
        };
        fx().clipGetRegionOfDefinition.unwrap()(source_clip(h), time, &raw mut rod);
        let v = [rod.x1 - r, rod.y1 - r, rod.x2 + r, rod.y2 + r];
        p.propSetDoubleN.unwrap()(
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
        let p = props();
        let mut time = 0.0;
        let mut region = [0.0_f64; 4];
        p.propGetDouble.unwrap()(ins, c(ofx::kOfxPropTime), 0, &raw mut time);
        p.propGetDoubleN.unwrap()(
            ins,
            c(ofx::kOfxImageEffectPropRegionOfInterest),
            4,
            region.as_mut_ptr(),
        );
        let r = double_at(h, b"radius\0", time);
        let v = [region[0] - r, region[1] - r, region[2] + r, region[3] + r];
        p.propSetDoubleN.unwrap()(outs, c(b"OfxImageClipPropRoI_Source\0"), 4, v.as_ptr());
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
        if double_at(h, b"radius\0", time) == 0.0 {
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
    // SAFETY: valid handles from the host; every pixel read is checked
    // against the source bounds, every write lies in the render window.
    unsafe {
        let p = props();
        let mut time = 0.0;
        let mut window = [0; 4];
        let mut scale = [1.0_f64; 2];
        p.propGetDouble.unwrap()(ins, c(ofx::kOfxPropTime), 0, &raw mut time);
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
            scale.as_mut_ptr(),
        );
        let r = (double_at(h, b"radius\0", time) * scale[0]).round() as i32;

        let image = |ch: ofx::OfxImageClipHandle| {
            let mut img: ofx::OfxPropertySetHandle = std::ptr::null_mut();
            let status = fx().clipGetImage.unwrap()(ch, time, std::ptr::null(), &raw mut img);
            (status == ofx::kOfxStatOK).then_some(img)
        };
        let mut out_clip: ofx::OfxImageClipHandle = std::ptr::null_mut();
        fx().clipGetHandle.unwrap()(
            h,
            c(ofx::kOfxImageEffectOutputClipName),
            &raw mut out_clip,
            std::ptr::null_mut(),
        );
        let (Some(src), Some(dst)) = (image(source_clip(h)), image(out_clip)) else {
            return ofx::kOfxStatFailed;
        };
        let (s, sb, s_row) = image_data(src);
        let (d, db, d_row) = image_data(dst);
        let n = ((2 * r + 1) * (2 * r + 1)) as f32;
        for y in window[1]..window[3] {
            for x in window[0]..window[2] {
                let mut sum = [0.0_f32; 4];
                for sy in (y - r).max(sb[1])..(y + r + 1).min(sb[3]) {
                    for sx in (x - r).max(sb[0])..(x + r + 1).min(sb[2]) {
                        let si = ((sy - sb[1]) as usize * s_row) + (sx - sb[0]) as usize * 4;
                        for (k, v) in sum.iter_mut().enumerate() {
                            *v += *s.add(si + k);
                        }
                    }
                }
                let di = ((y - db[1]) as usize * d_row) + (x - db[0]) as usize * 4;
                for (k, v) in sum.iter().enumerate() {
                    *d.add(di + k) = v / n;
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

/// A box blur with a region of definition and interest.
pub static BLUR: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Blur".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(set_host),
    mainEntry: Some(main_entry),
});
