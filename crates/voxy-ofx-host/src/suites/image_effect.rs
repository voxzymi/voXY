//! `OfxImageEffectSuiteV1` over [`Effect`], its clips and its images.
//!
//! Host choices (also in `docs/ofx-host.md`):
//! - `clipDefine` works on descriptors only; defining a name again returns
//!   the existing clip. `clipGetHandle` and `clipGetImage` work on
//!   instances only. Other handles get `kOfxStatErrBadHandle`.
//! - `clipGetImage` returns the whole image the host has for that time;
//!   the requested region is a hint. No image, or an unconnected clip,
//!   gives `kOfxStatFailed`.
//! - Image memory may be allocated with a null instance, as the spec
//!   allows; it is 64-byte aligned, and lock and unlock nest.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};

use voxy_ofx_sys as ofx;

use super::buffer::PixelBuffer;
use super::param::{param_set_handle, release_param_set_handle};
use super::{EFFECTS, Registry, c_str, guard, prop_handle, release_prop_handle};
use crate::clip::Clip;
use crate::effect::Effect;
use crate::image::Image;
use crate::params::Role;
use crate::props::ofx_str;

pub(super) static SUITE: ofx::OfxImageEffectSuiteV1 = ofx::OfxImageEffectSuiteV1 {
    getPropertySet: Some(get_property_set),
    getParamSet: Some(get_param_set),
    clipDefine: Some(clip_define),
    clipGetHandle: Some(clip_get_handle),
    clipGetPropertySet: Some(clip_get_property_set),
    clipGetImage: Some(clip_get_image),
    clipReleaseImage: Some(clip_release_image),
    clipGetRegionOfDefinition: Some(clip_get_rod),
    abort: Some(abort),
    imageMemoryAlloc: Some(memory_alloc),
    imageMemoryFree: Some(memory_free),
    imageMemoryLock: Some(memory_lock),
    imageMemoryUnlock: Some(memory_unlock),
};

/// A live clip and the effect it belongs to.
type ClipEntry = (Arc<Clip>, Weak<Effect>);

/// Live clips by handle.
static CLIPS: LazyLock<Mutex<HashMap<usize, ClipEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Images the plugin holds, by their property-set handle.
static IMAGES: LazyLock<Mutex<HashMap<usize, Arc<Image>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Image memory blocks and their lock counts.
#[derive(Debug)]
struct Memory {
    buf: PixelBuffer,
    locks: AtomicUsize,
}

static MEMORY: LazyLock<Registry<Memory>> = LazyLock::new(Registry::new);

/// How many images plugins hold and have not released (for leak tests and
/// the sandbox's cleanup after an action).
pub fn live_images() -> usize {
    IMAGES.lock().unwrap_or_else(PoisonError::into_inner).len()
}

fn register_clip(c: &Arc<Clip>, fx: &Arc<Effect>) -> ofx::OfxImageClipHandle {
    prop_handle(&c.props);
    let addr = Arc::as_ptr(c) as usize;
    CLIPS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(addr, (Arc::clone(c), Arc::downgrade(fx)));
    addr as ofx::OfxImageClipHandle
}

/// The `OfxImageEffectHandle` the plugin sees for `fx`. Registers the
/// effect, its property set, its parameter set and its clips.
pub fn effect_handle(fx: &Arc<Effect>) -> ofx::OfxImageEffectHandle {
    prop_handle(&fx.props);
    param_set_handle(&fx.params);
    for c in fx.clips() {
        register_clip(&c, fx);
    }
    EFFECTS.register(fx) as ofx::OfxImageEffectHandle
}

/// Withdraws `fx`'s handle and everything [`effect_handle`] registered.
pub fn release_effect_handle(fx: &Arc<Effect>) {
    let mut clips = CLIPS.lock().unwrap_or_else(PoisonError::into_inner);
    for c in fx.clips() {
        clips.remove(&(Arc::as_ptr(&c) as usize));
        release_prop_handle(&c.props);
    }
    drop(clips);
    release_param_set_handle(&fx.params);
    release_prop_handle(&fx.props);
    EFFECTS.unregister(fx);
}

fn clip(h: ofx::OfxImageClipHandle) -> Option<(Arc<Clip>, Arc<Effect>)> {
    let clips = CLIPS.lock().unwrap_or_else(PoisonError::into_inner);
    let (c, fx) = clips.get(&(h as usize))?;
    Some((Arc::clone(c), fx.upgrade()?))
}

/// Writes `value` to `out`, or reports a null out pointer.
///
/// # Safety
/// `out` is null or valid for a write.
unsafe fn put<T>(out: *mut T, value: T) -> ofx::OfxStatus {
    if out.is_null() {
        return ofx::kOfxStatErrValue;
    }
    // SAFETY: the caller's contract.
    unsafe { out.write(value) };
    ofx::kOfxStatOK
}

unsafe extern "C" fn get_property_set(
    fx: ofx::OfxImageEffectHandle,
    out: *mut ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    guard(|| match EFFECTS.get(fx as usize) {
        // SAFETY: a valid out pointer from the plugin.
        Some(fx) => unsafe { put(out, prop_handle(&fx.props)) },
        None => ofx::kOfxStatErrBadHandle,
    })
}

unsafe extern "C" fn get_param_set(
    fx: ofx::OfxImageEffectHandle,
    out: *mut ofx::OfxParamSetHandle,
) -> ofx::OfxStatus {
    guard(|| match EFFECTS.get(fx as usize) {
        // SAFETY: a valid out pointer from the plugin.
        Some(fx) => unsafe { put(out, param_set_handle(&fx.params)) },
        None => ofx::kOfxStatErrBadHandle,
    })
}

unsafe extern "C" fn clip_define(
    fx: ofx::OfxImageEffectHandle,
    name: *const c_char,
    out: *mut ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    guard(|| {
        let Some(fx) = EFFECTS.get(fx as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        // SAFETY: the plugin passes a valid name.
        let Some(name) = (unsafe { c_str(name) }) else {
            return ofx::kOfxStatErrValue;
        };
        let Some(c) = fx.define_clip(&name.to_string_lossy()) else {
            return ofx::kOfxStatErrBadHandle;
        };
        register_clip(&c, &fx);
        if out.is_null() {
            return ofx::kOfxStatOK;
        }
        // SAFETY: a valid out pointer.
        unsafe { put(out, prop_handle(&c.props)) }
    })
}

unsafe extern "C" fn clip_get_handle(
    fx: ofx::OfxImageEffectHandle,
    name: *const c_char,
    out: *mut ofx::OfxImageClipHandle,
    props: *mut ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    guard(|| {
        let Some(fx) = EFFECTS
            .get(fx as usize)
            .filter(|fx| fx.role == Role::Instance)
        else {
            return ofx::kOfxStatErrBadHandle;
        };
        // SAFETY: the plugin passes a valid name.
        let Some(name) = (unsafe { c_str(name) }) else {
            return ofx::kOfxStatErrValue;
        };
        let Some(c) = fx.clip(&name.to_string_lossy()) else {
            return ofx::kOfxStatErrUnknown;
        };
        let h = register_clip(&c, &fx);
        // SAFETY: valid out pointers (props may be null).
        unsafe {
            if !props.is_null() {
                props.write(prop_handle(&c.props));
            }
            put(out, h)
        }
    })
}

unsafe extern "C" fn clip_get_property_set(
    c: ofx::OfxImageClipHandle,
    out: *mut ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    guard(|| match clip(c) {
        // SAFETY: a valid out pointer from the plugin.
        Some((c, _)) => unsafe { put(out, prop_handle(&c.props)) },
        None => ofx::kOfxStatErrBadHandle,
    })
}

fn connected(c: &Clip) -> bool {
    c.props
        .int(ofx_str(ofx::kOfxImageClipPropConnected), 0)
        .unwrap_or(1)
        != 0
}

unsafe extern "C" fn clip_get_image(
    c: ofx::OfxImageClipHandle,
    time: f64,
    _region: *const ofx::OfxRectD,
    out: *mut ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    guard(|| {
        let Some((c, fx)) = clip(c) else {
            return ofx::kOfxStatErrBadHandle;
        };
        let Some(images) = &fx.images else {
            return ofx::kOfxStatErrBadHandle;
        };
        if out.is_null() {
            return ofx::kOfxStatErrValue;
        }
        if !connected(&c) {
            return ofx::kOfxStatFailed;
        }
        let Some(img) = images.image(&c.name, time) else {
            return ofx::kOfxStatFailed;
        };
        let h = prop_handle(&img.props);
        IMAGES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(h as usize, img);
        // SAFETY: checked above.
        unsafe { put(out, h) }
    })
}

unsafe extern "C" fn clip_release_image(h: ofx::OfxPropertySetHandle) -> ofx::OfxStatus {
    guard(|| {
        let img = IMAGES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(h as usize));
        match img {
            Some(img) => {
                release_prop_handle(&img.props);
                ofx::kOfxStatOK
            }
            None => ofx::kOfxStatErrBadHandle,
        }
    })
}

unsafe extern "C" fn clip_get_rod(
    c: ofx::OfxImageClipHandle,
    time: f64,
    out: *mut ofx::OfxRectD,
) -> ofx::OfxStatus {
    guard(|| {
        let Some((c, fx)) = clip(c) else {
            return ofx::kOfxStatErrBadHandle;
        };
        let Some(images) = &fx.images else {
            return ofx::kOfxStatErrBadHandle;
        };
        match images.region_of_definition(&c.name, time) {
            // SAFETY: a valid out pointer from the plugin.
            Some([x1, y1, x2, y2]) => unsafe { put(out, ofx::OfxRectD { x1, y1, x2, y2 }) },
            None => ofx::kOfxStatFailed,
        }
    })
}

unsafe extern "C" fn abort(fx: ofx::OfxImageEffectHandle) -> c_int {
    std::panic::catch_unwind(|| {
        EFFECTS
            .get(fx as usize)
            .map_or(0, |fx| c_int::from(fx.aborted()))
    })
    .unwrap_or(1)
}

unsafe extern "C" fn memory_alloc(
    fx: ofx::OfxImageEffectHandle,
    n_bytes: usize,
    out: *mut ofx::OfxImageMemoryHandle,
) -> ofx::OfxStatus {
    guard(|| {
        if out.is_null() {
            return ofx::kOfxStatErrValue;
        }
        if !fx.is_null() && EFFECTS.get(fx as usize).is_none() {
            // SAFETY: a valid out pointer.
            unsafe { out.write(std::ptr::null_mut()) };
            return ofx::kOfxStatErrBadHandle;
        }
        let Some(buf) = PixelBuffer::zeroed(n_bytes) else {
            // SAFETY: as above.
            unsafe { out.write(std::ptr::null_mut()) };
            return ofx::kOfxStatErrMemory;
        };
        let m = Arc::new(Memory {
            buf,
            locks: AtomicUsize::new(0),
        });
        let h = MEMORY.register(&m);
        // SAFETY: as above.
        unsafe { put(out, h as ofx::OfxImageMemoryHandle) }
    })
}

unsafe extern "C" fn memory_free(h: ofx::OfxImageMemoryHandle) -> ofx::OfxStatus {
    guard(|| {
        if MEMORY.get(h as usize).is_none() {
            return ofx::kOfxStatErrBadHandle;
        }
        MEMORY.unregister_addr(h as usize);
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn memory_lock(
    h: ofx::OfxImageMemoryHandle,
    out: *mut *mut c_void,
) -> ofx::OfxStatus {
    guard(|| {
        let Some(m) = MEMORY.get(h as usize) else {
            if !out.is_null() {
                // SAFETY: a valid out pointer.
                unsafe { out.write(std::ptr::null_mut()) };
            }
            return ofx::kOfxStatErrBadHandle;
        };
        m.locks.fetch_add(1, Ordering::Relaxed);
        // SAFETY: a valid out pointer from the plugin.
        unsafe { put(out, m.buf.as_ptr().cast()) }
    })
}

unsafe extern "C" fn memory_unlock(h: ofx::OfxImageMemoryHandle) -> ofx::OfxStatus {
    guard(|| match MEMORY.get(h as usize) {
        Some(m) => {
            let _ = m
                .locks
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
            ofx::kOfxStatOK
        }
        None => ofx::kOfxStatErrBadHandle,
    })
}
