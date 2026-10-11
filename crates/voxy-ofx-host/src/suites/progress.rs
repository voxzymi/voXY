//! `OfxProgressSuiteV1` and `V2`. An update returns `kOfxStatReplyNo` once
//! the host has asked the effect to abort. Updating or ending without a
//! start is accepted (and does nothing).

use std::ffi::{c_char, c_void};

use voxy_ofx_sys as ofx;

use super::{EFFECTS, c_str, guard};
use crate::effect::Progress;

pub(super) static SUITE_V1: ofx::OfxProgressSuiteV1 = ofx::OfxProgressSuiteV1 {
    progressStart: Some(start_v1),
    progressUpdate: Some(update),
    progressEnd: Some(end),
};

pub(super) static SUITE_V2: ofx::OfxProgressSuiteV2 = ofx::OfxProgressSuiteV2 {
    progressStart: Some(start_v2),
    progressUpdate: Some(update),
    progressEnd: Some(end),
};

/// # Safety
/// The strings are null or valid.
unsafe fn start(fx: *mut c_void, label: *const c_char, id: *const c_char) -> ofx::OfxStatus {
    guard(|| {
        let Some(fx) = EFFECTS.get(fx as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        // SAFETY: the caller's contract.
        let (label, id) = unsafe { (c_str(label), c_str(id)) };
        fx.set_progress(Some(Progress {
            label: label
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            id: id.map(|s| s.to_string_lossy().into_owned()),
            fraction: 0.0,
        }));
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn start_v1(fx: *mut c_void, label: *const c_char) -> ofx::OfxStatus {
    // SAFETY: the plugin passes a valid label.
    unsafe { start(fx, label, std::ptr::null()) }
}

unsafe extern "C" fn start_v2(
    fx: *mut c_void,
    label: *const c_char,
    id: *const c_char,
) -> ofx::OfxStatus {
    // SAFETY: the plugin passes valid strings.
    unsafe { start(fx, label, id) }
}

unsafe extern "C" fn update(fx: *mut c_void, fraction: f64) -> ofx::OfxStatus {
    guard(|| {
        let Some(fx) = EFFECTS.get(fx as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        fx.update_progress(if fraction.is_finite() { fraction } else { 0.0 });
        if fx.aborted() {
            ofx::kOfxStatReplyNo
        } else {
            ofx::kOfxStatOK
        }
    })
}

unsafe extern "C" fn end(fx: *mut c_void) -> ofx::OfxStatus {
    guard(|| {
        let Some(fx) = EFFECTS.get(fx as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        fx.set_progress(None);
        ofx::kOfxStatOK
    })
}
