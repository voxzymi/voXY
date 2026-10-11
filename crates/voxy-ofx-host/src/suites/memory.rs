//! `OfxMemorySuiteV1`: plain heap blocks, 64-byte aligned (the alignment
//! the sandbox's image slots use, design §6.7). Only blocks this suite
//! handed out can be freed; anything else is `kOfxStatErrBadHandle`.

use std::alloc::Layout;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{LazyLock, Mutex, PoisonError};

use voxy_ofx_sys as ofx;

use super::guard;

pub(super) static SUITE: ofx::OfxMemorySuiteV1 = ofx::OfxMemorySuiteV1 {
    memoryAlloc: Some(alloc),
    memoryFree: Some(free),
};

const ALIGN: usize = 64;

static BLOCKS: LazyLock<Mutex<HashMap<usize, Layout>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// How many blocks are allocated and not yet freed (for leak tests).
pub fn live_blocks() -> usize {
    BLOCKS.lock().unwrap_or_else(PoisonError::into_inner).len()
}

unsafe extern "C" fn alloc(
    _instance: *mut c_void,
    n_bytes: usize,
    out: *mut *mut c_void,
) -> ofx::OfxStatus {
    guard(|| {
        if out.is_null() {
            return ofx::kOfxStatErrValue;
        }
        let Ok(layout) = Layout::from_size_align(n_bytes.max(1), ALIGN) else {
            return ofx::kOfxStatErrMemory;
        };
        // SAFETY: the layout has a non-zero size.
        let p = unsafe { std::alloc::alloc(layout) };
        if p.is_null() {
            return ofx::kOfxStatErrMemory;
        }
        BLOCKS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(p as usize, layout);
        // SAFETY: `out` is a valid pointer from the plugin.
        unsafe { out.write(p.cast()) };
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn free(data: *mut c_void) -> ofx::OfxStatus {
    guard(|| {
        let layout = BLOCKS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(data as usize));
        let Some(layout) = layout else {
            return ofx::kOfxStatErrBadHandle;
        };
        // SAFETY: the block came from `alloc` with this layout.
        unsafe { std::alloc::dealloc(data.cast(), layout) };
        ofx::kOfxStatOK
    })
}
