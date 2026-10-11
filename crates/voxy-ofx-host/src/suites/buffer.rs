//! A zeroed, 64-byte-aligned pixel buffer whose address can be handed to a
//! plugin, which then writes through it.

use std::alloc::Layout;

/// Owned pixel memory. The plugin may write through [`PixelBuffer::as_ptr`]
/// while it renders; the host reads it with [`PixelBuffer::to_vec`] only
/// after the action that wrote it has returned.
#[derive(Debug)]
pub struct PixelBuffer {
    ptr: *mut u8,
    layout: Layout,
    len: usize,
}

// SAFETY: the buffer is plain bytes; the host and the plugin take turns
// (the plugin during an action, the host after it), never at once.
unsafe impl Send for PixelBuffer {}
// SAFETY: as above.
unsafe impl Sync for PixelBuffer {}

impl PixelBuffer {
    pub const ALIGN: usize = 64;

    /// `len` zero bytes, or `None` if the allocation fails.
    pub fn zeroed(len: usize) -> Option<PixelBuffer> {
        let layout = Layout::from_size_align(len.max(1), Self::ALIGN).ok()?;
        // SAFETY: the layout has a non-zero size.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        (!ptr.is_null()).then_some(PixelBuffer { ptr, layout, len })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The address handed to the plugin.
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// A copy of the bytes. Call only when no plugin action is writing.
    pub fn to_vec(&self) -> Vec<u8> {
        // SAFETY: `ptr` holds `len` initialised bytes (zeroed, then written
        // by the plugin), and no action is running.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }.to_vec()
    }

    /// Overwrites the start of the buffer with `bytes` (cut to fit).
    pub fn write(&self, bytes: &[u8]) {
        let n = bytes.len().min(self.len);
        // SAFETY: `n` bytes fit in both; the regions cannot overlap because
        // `bytes` is a Rust slice the buffer does not own.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr, n) };
    }
}

impl Drop for PixelBuffer {
    fn drop(&mut self) {
        // SAFETY: allocated in `zeroed` with this layout.
        unsafe { std::alloc::dealloc(self.ptr, self.layout) };
    }
}
