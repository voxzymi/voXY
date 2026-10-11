//! `OfxTimeLineSuiteV1`. The host sets the time line each effect sees;
//! `gotoTime` is refused with `kOfxStatFailed`, because a plugin rendering
//! in the sandbox cannot move the editor's playhead.

use std::ffi::c_void;

use voxy_ofx_sys as ofx;

use super::{EFFECTS, guard};

pub(super) static SUITE: ofx::OfxTimeLineSuiteV1 = ofx::OfxTimeLineSuiteV1 {
    getTime: Some(get_time),
    gotoTime: Some(goto_time),
    getTimeBounds: Some(get_bounds),
};

unsafe extern "C" fn get_time(fx: *mut c_void, time: *mut f64) -> ofx::OfxStatus {
    guard(|| {
        let Some(fx) = EFFECTS.get(fx as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        if time.is_null() {
            return ofx::kOfxStatErrValue;
        }
        // SAFETY: a valid out pointer from the plugin.
        unsafe { time.write(fx.time_line().time) };
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn goto_time(fx: *mut c_void, _time: f64) -> ofx::OfxStatus {
    guard(|| {
        if EFFECTS.get(fx as usize).is_none() {
            return ofx::kOfxStatErrBadHandle;
        }
        ofx::kOfxStatFailed
    })
}

unsafe extern "C" fn get_bounds(
    fx: *mut c_void,
    first: *mut f64,
    last: *mut f64,
) -> ofx::OfxStatus {
    guard(|| {
        let Some(fx) = EFFECTS.get(fx as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        if first.is_null() || last.is_null() {
            return ofx::kOfxStatErrValue;
        }
        let t = fx.time_line();
        // SAFETY: valid out pointers from the plugin.
        unsafe {
            first.write(t.first);
            last.write(t.last);
        }
        ofx::kOfxStatOK
    })
}
