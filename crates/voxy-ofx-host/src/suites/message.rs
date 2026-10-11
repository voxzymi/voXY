//! `OfxMessageSuiteV1` and `V2`. One static struct serves both versions,
//! as the spec allows (V2 extends V1 compatibly).
//!
//! Host choices: a message with a null handle is kept as an unattached
//! message; questions are refused with `kOfxStatFailed`, because the
//! sandbox has no user to ask (the spec makes a question an error outside
//! an interactive session); an unknown message type is refused with
//! `kOfxStatErrValue`.

use std::ffi::{c_char, c_void};
use std::sync::{Arc, LazyLock};

use voxy_ofx_sys as ofx;

use super::{EFFECTS, c_str, guard, printf};
use crate::effect::{Effect, Message, MessageKind};

pub(super) static SUITE: ofx::OfxMessageSuiteV2 = ofx::OfxMessageSuiteV2 {
    message: Some(message),
    setPersistentMessage: Some(set_persistent),
    clearPersistentMessage: Some(clear_persistent),
};

/// Where messages posted with a null handle go.
static UNATTACHED: LazyLock<Arc<Effect>> = LazyLock::new(|| Arc::new(Effect::new()));

/// Messages posted with a null handle, oldest first; clears them.
pub fn take_unattached_messages() -> Vec<Message> {
    UNATTACHED.take_messages()
}

/// Builds the message.
///
/// # Safety
/// The strings are null or valid; `args` matches `format`.
unsafe fn build(
    kind: *const c_char,
    id: *const c_char,
    format: *const c_char,
    args: &mut std::ffi::VaList<'_>,
) -> Result<Message, ofx::OfxStatus> {
    // SAFETY: the caller's contract.
    unsafe {
        let kind = c_str(kind)
            .and_then(|k| MessageKind::from_ofx(k.to_bytes()))
            .ok_or(ofx::kOfxStatErrValue)?;
        let format = c_str(format).ok_or(ofx::kOfxStatErrValue)?;
        Ok(Message {
            kind,
            id: c_str(id).map(|s| s.to_string_lossy().into_owned()),
            text: printf::format(format, args),
        })
    }
}

fn effect(handle: *mut c_void, allow_null: bool) -> Option<Arc<Effect>> {
    if handle.is_null() {
        return allow_null.then(|| Arc::clone(&UNATTACHED));
    }
    EFFECTS.get(handle as usize)
}

unsafe extern "C" fn message(
    handle: *mut c_void,
    kind: *const c_char,
    id: *const c_char,
    format: *const c_char,
    mut args: ...
) -> ofx::OfxStatus {
    guard(|| {
        let Some(fx) = effect(handle, true) else {
            return ofx::kOfxStatErrBadHandle;
        };
        // SAFETY: the plugin passes valid strings and matching arguments.
        match unsafe { build(kind, id, format, &mut args) } {
            Ok(m) => {
                let question = m.kind == MessageKind::Question;
                fx.post(m);
                if question {
                    ofx::kOfxStatFailed
                } else {
                    ofx::kOfxStatOK
                }
            }
            Err(status) => status,
        }
    })
}

unsafe extern "C" fn set_persistent(
    handle: *mut c_void,
    kind: *const c_char,
    id: *const c_char,
    format: *const c_char,
    mut args: ...
) -> ofx::OfxStatus {
    guard(|| {
        let Some(fx) = effect(handle, false) else {
            return ofx::kOfxStatErrBadHandle;
        };
        // SAFETY: as above.
        match unsafe { build(kind, id, format, &mut args) } {
            Ok(m)
                if matches!(
                    m.kind,
                    MessageKind::Error | MessageKind::Warning | MessageKind::Message
                ) =>
            {
                fx.set_persistent(Some(m));
                ofx::kOfxStatOK
            }
            Ok(_) => ofx::kOfxStatErrValue,
            Err(status) => status,
        }
    })
}

unsafe extern "C" fn clear_persistent(handle: *mut c_void) -> ofx::OfxStatus {
    guard(|| {
        let Some(fx) = effect(handle, false) else {
            return ofx::kOfxStatErrBadHandle;
        };
        fx.set_persistent(None);
        ofx::kOfxStatOK
    })
}
