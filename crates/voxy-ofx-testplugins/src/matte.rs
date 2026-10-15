//! `org.voxy.test.Matte`: a general-context plugin (only) that multiplies
//! `Source` by the alpha of its extra `Matte` clip (optionally inverted).
//! It also defines an optional mask clip, `Mask`, and fails if the host
//! ever connects it.

use std::ffi::{CStr, c_char, c_int, c_void};

use voxy_ofx_sys as ofx;

use super::{StaticPlugin, bool_at, c, fetch_suites, fx, image_data, params, props, set_host};

/// # Safety
/// Called by the host with valid arguments.
unsafe fn describe(h: ofx::OfxImageEffectHandle) -> ofx::OfxStatus {
    // SAFETY: valid handles from the host.
    unsafe {
        let p = props();
        let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        fx().getPropertySet.unwrap()(h, &raw mut ph);
        p.propSetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, c(b"Test Matte\0"));
        p.propSetString.unwrap()(
            ph,
            c(ofx::kOfxImageEffectPropSupportedContexts),
            0,
            c(ofx::kOfxImageEffectContextGeneral),
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
            ofx::kOfxImageEffectSimpleSourceClipName.as_slice(),
            b"Matte\0",
            b"Mask\0",
            ofx::kOfxImageEffectOutputClipName,
        ] {
            fx().clipDefine.unwrap()(h, c(clip), &raw mut cp);
            p.propSetString.unwrap()(
                cp,
                c(ofx::kOfxImageEffectPropSupportedComponents),
                0,
                c(ofx::kOfxImageComponentRGBA),
            );
            if clip == b"Matte\0" {
                p.propSetString.unwrap()(cp, c(ofx::kOfxPropLabel), 0, c(b"Matte Layer\0"));
                p.propSetInt.unwrap()(cp, c(ofx::kOfxImageClipPropOptional), 0, 1);
            }
            if clip == b"Mask\0" {
                p.propSetInt.unwrap()(cp, c(ofx::kOfxImageClipPropOptional), 0, 1);
                p.propSetInt.unwrap()(cp, c(ofx::kOfxImageClipPropIsMask), 0, 1);
            }
        }
        let mut set: ofx::OfxParamSetHandle = std::ptr::null_mut();
        fx().getParamSet.unwrap()(h, &raw mut set);
        let mut pp: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        params().paramDefine.unwrap()(
            set,
            c(ofx::kOfxParamTypeBoolean),
            c(b"invert\0"),
            &raw mut pp,
        );
    }
    ofx::kOfxStatOK
}

/// # Safety
/// As above.
unsafe fn clip(h: ofx::OfxImageEffectHandle, name: &[u8]) -> ofx::OfxImageClipHandle {
    let mut ch: ofx::OfxImageClipHandle = std::ptr::null_mut();
    // SAFETY: valid handles from the host.
    unsafe { fx().clipGetHandle.unwrap()(h, c(name), &raw mut ch, std::ptr::null_mut()) };
    ch
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
        // The mask must stay unconnected (voXY masks with its own matte).
        let mut mask_props: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        fx().clipGetPropertySet.unwrap()(clip(h, b"Mask\0"), &raw mut mask_props);
        let mut connected: c_int = 0;
        p.propGetInt.unwrap()(
            mask_props,
            c(ofx::kOfxImageClipPropConnected),
            0,
            &raw mut connected,
        );
        if connected != 0 {
            return ofx::kOfxStatFailed;
        }
        let invert = bool_at(h, b"invert\0", time);
        let image = |name: &[u8]| {
            let mut img: ofx::OfxPropertySetHandle = std::ptr::null_mut();
            let status =
                fx().clipGetImage.unwrap()(clip(h, name), time, std::ptr::null(), &raw mut img);
            (status == ofx::kOfxStatOK).then_some(img)
        };
        let (Some(src), Some(matte), Some(dst)) = (
            image(ofx::kOfxImageEffectSimpleSourceClipName),
            image(b"Matte\0"),
            image(ofx::kOfxImageEffectOutputClipName),
        ) else {
            return ofx::kOfxStatFailed;
        };
        let (ps, sb, s_row) = image_data(src);
        let (pm, mb, m_row) = image_data(matte);
        let (d, db, d_row) = image_data(dst);
        for y in window[1]..window[3] {
            for x in window[0]..window[2] {
                let si = ((y - sb[1]) as usize * s_row) + (x - sb[0]) as usize * 4;
                let mi = ((y - mb[1]) as usize * m_row) + (x - mb[0]) as usize * 4;
                let di = ((y - db[1]) as usize * d_row) + (x - db[0]) as usize * 4;
                let a = *pm.add(mi + 3);
                let k = if invert { 1.0 - a } else { a };
                for i in 0..4 {
                    *d.add(di + i) = *ps.add(si + i) * k;
                }
            }
        }
        for img in [src, matte, dst] {
            fx().clipReleaseImage.unwrap()(img);
        }
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

/// Multiplies its source by another layer's alpha.
pub static MATTE: StaticPlugin = StaticPlugin(ofx::OfxPlugin {
    pluginApi: c"OfxImageEffectPluginAPI".as_ptr(),
    apiVersion: 1,
    pluginIdentifier: c"org.voxy.test.Matte".as_ptr(),
    pluginVersionMajor: 1,
    pluginVersionMinor: 0,
    setHost: Some(set_host),
    mainEntry: Some(main_entry),
});
