//! The C side of the host: the `OfxHost` struct, `fetchSuite`, and the
//! suites the plugin calls (design §6.4).
//!
//! This is the only place in voXY with unsafe code for OpenFX. Every entry
//! point checks its handles against the live-handle registries before using
//! them, so a stale or made-up handle gets `kOfxStatErrBadHandle` instead
//! of undefined behaviour, and every entry point catches panics so none
//! crosses into the plugin. The editor never calls into a plugin: this code
//! runs in the in-process test harness (O-A) and in the sandbox (O-B on).

#![allow(unsafe_code)]

mod memory;
mod message;
mod param;
mod printf;
mod progress;
mod property;
mod thread;
mod timeline;

pub use memory::live_blocks;
pub use message::take_unattached_messages;
pub use param::{
    MAX_OPAQUE_BYTES, capture_bytes_defaults, param_set_handle, release_param_set_handle,
};

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use voxy_ofx_sys as ofx;

use crate::effect::Effect;
use crate::props::PropSet;

/// Live objects reachable through handles, keyed by address. A handle is
/// the address of the object's `Arc` payload, so it is stable and unique
/// while the object is registered.
#[derive(Debug)]
pub(crate) struct Registry<T> {
    live: Mutex<HashMap<usize, Arc<T>>>,
}

impl<T> Registry<T> {
    pub(crate) fn new() -> Registry<T> {
        Registry {
            live: Mutex::new(HashMap::new()),
        }
    }

    /// Makes `obj` reachable and returns its handle address.
    pub(crate) fn register(&self, obj: &Arc<T>) -> usize {
        let addr = Arc::as_ptr(obj) as usize;
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(addr, Arc::clone(obj));
        addr
    }

    /// Makes `obj` unreachable; later calls with its handle fail cleanly.
    pub(crate) fn unregister(&self, obj: &Arc<T>) {
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(Arc::as_ptr(obj) as usize));
    }

    pub(crate) fn unregister_addr(&self, addr: usize) {
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&addr);
    }

    pub(crate) fn get(&self, addr: usize) -> Option<Arc<T>> {
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&addr)
            .cloned()
    }
}

pub(crate) static PROP_SETS: LazyLock<Registry<PropSet>> = LazyLock::new(Registry::new);
pub(crate) static EFFECTS: LazyLock<Registry<Effect>> = LazyLock::new(Registry::new);

/// The handle the plugin sees for `set`, registering it if needed.
pub fn prop_handle(set: &Arc<PropSet>) -> ofx::OfxPropertySetHandle {
    PROP_SETS.register(set) as ofx::OfxPropertySetHandle
}

/// Withdraws `set`'s handle.
pub fn release_prop_handle(set: &Arc<PropSet>) {
    PROP_SETS.unregister(set);
}

/// The `OfxImageEffectHandle` the plugin sees for `fx`, registering it if
/// needed. The message, progress and time line suites take it (as a
/// `void *`).
pub fn effect_handle(fx: &Arc<Effect>) -> ofx::OfxImageEffectHandle {
    EFFECTS.register(fx) as ofx::OfxImageEffectHandle
}

/// Withdraws `fx`'s handle.
pub fn release_effect_handle(fx: &Arc<Effect>) {
    EFFECTS.unregister(fx);
}

/// Runs a suite body, turning a panic into `kOfxStatErrUnknown`.
pub(crate) fn guard(f: impl FnOnce() -> ofx::OfxStatus) -> ofx::OfxStatus {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(ofx::kOfxStatErrUnknown)
}

/// A C string argument, or `None` for null.
///
/// # Safety
/// `p` is null or points to a NUL-terminated string that outlives `'a`.
pub(crate) unsafe fn c_str<'a>(p: *const c_char) -> Option<&'a CStr> {
    // SAFETY: the caller's contract.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) })
}

/// The suites this host implements, by name and version. Anything else,
/// including the GPU render suites (owner decision D5), gets null.
fn suite(name: &CStr, version: c_int) -> *const c_void {
    let name = name.to_bytes_with_nul();
    let p: *const c_void = match (name, version) {
        (n, 1) if n == ofx::kOfxPropertySuite => (&raw const property::SUITE).cast(),
        (n, 1) if n == ofx::kOfxMemorySuite => (&raw const memory::SUITE).cast(),
        (n, 1 | 2) if n == ofx::kOfxMessageSuite => (&raw const message::SUITE).cast(),
        (n, 1) if n == ofx::kOfxProgressSuite => (&raw const progress::SUITE_V1).cast(),
        (n, 2) if n == ofx::kOfxProgressSuite => (&raw const progress::SUITE_V2).cast(),
        (n, 1) if n == ofx::kOfxParameterSuite => (&raw const param::SUITE).cast(),
        (n, 1) if n == ofx::kOfxTimeLineSuite => (&raw const timeline::SUITE).cast(),
        (n, 1) if n == ofx::kOfxMultiThreadSuite => (&raw const thread::SUITE).cast(),
        _ => std::ptr::null(),
    };
    p
}

unsafe extern "C" fn fetch_suite(
    host: ofx::OfxPropertySetHandle,
    name: *const c_char,
    version: c_int,
) -> *const c_void {
    catch_unwind(|| {
        if PROP_SETS.get(host as usize).is_none() {
            return std::ptr::null();
        }
        // SAFETY: a plugin passes a suite name constant.
        match unsafe { c_str(name) } {
            Some(name) => suite(name, version),
            None => std::ptr::null(),
        }
    })
    .unwrap_or(std::ptr::null())
}

/// The `OfxHost` a plugin receives in `setHost`: the host property set and
/// `fetchSuite`. Keep it alive (and in place) while the plugin is loaded.
#[derive(Debug)]
pub struct HostStruct {
    props: Arc<PropSet>,
    raw: Box<ofx::OfxHost>,
}

impl HostStruct {
    pub fn new(props: Arc<PropSet>) -> HostStruct {
        let raw = Box::new(ofx::OfxHost {
            host: prop_handle(&props),
            fetchSuite: Some(fetch_suite),
        });
        HostStruct { props, raw }
    }

    pub fn props(&self) -> &Arc<PropSet> {
        &self.props
    }

    /// The pointer to pass to the plugin's `setHost`.
    pub fn as_ptr(&mut self) -> *mut ofx::OfxHost {
        &raw mut *self.raw
    }

    /// `fetchSuite` as the plugin calls it, for tests and the harness.
    pub fn fetch_suite(&self, name: &[u8], version: i32) -> *const c_void {
        let Ok(name) = CStr::from_bytes_with_nul(name) else {
            return std::ptr::null();
        };
        // SAFETY: `name` is a valid C string; the handle is ours.
        unsafe { fetch_suite(self.raw.host, name.as_ptr(), version) }
    }
}

impl Drop for HostStruct {
    fn drop(&mut self) {
        release_prop_handle(&self.props);
    }
}
