//! Shared memory between the editor and one sandbox (design §6.7, owner
//! decision D4): a file-backed mapping (`memmap2`) in a private
//! per-session folder, `/dev/shm` on Linux and the temp folder elsewhere.
//!
//! Layout: a 4 KiB header (magic, layout version, mapping length, abort
//! flag, heartbeat counter), then image slots, each 64-byte aligned and
//! described by an [`ImageSlot`](crate::protocol::ImageSlot) in the
//! request. The mapping grows by being replaced; the new path goes with the
//! next request.
//!
//! The session folder holds a `lock` file, locked while the session lives.
//! Folders are removed when the session is dropped, and [`clean_stale`]
//! removes any whose lock is free (left by a crash), on the next launch.

#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use memmap2::MmapMut;

/// The first 8 bytes of every mapping.
pub const MAGIC: [u8; 8] = *b"VOXYOFX\0";
/// The header layout version.
pub const LAYOUT: u32 = 1;
/// Bytes before the first slot.
pub const HEADER: usize = 4096;
/// Slot alignment.
pub const ALIGN: usize = 64;

const OFF_LAYOUT: usize = 8;
const OFF_ABORT: usize = 12;
const OFF_LEN: usize = 16;
const OFF_HEARTBEAT: usize = 24;

/// Session folder names start with this.
const PREFIX: &str = "voxy-ofx-";

/// Where session folders go: `/dev/shm` on Linux when it is usable, the
/// temp folder otherwise.
pub fn default_base() -> PathBuf {
    let shm = Path::new("/dev/shm");
    if cfg!(target_os = "linux") && shm.is_dir() {
        return shm.to_path_buf();
    }
    std::env::temp_dir()
}

/// A private folder for one editor session's mappings.
#[derive(Debug)]
pub struct SessionDir {
    path: PathBuf,
    _lock: File,
}

impl SessionDir {
    /// Creates `base/voxy-ofx-<pid>-<n>` with only the user's access, and
    /// locks it.
    pub fn create(base: &Path) -> io::Result<SessionDir> {
        let pid = std::process::id();
        for n in 0..1000u32 {
            let path = base.join(format!("{PREFIX}{pid}-{n}"));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    restrict(&path)?;
                    let lock = File::create(path.join("lock"))?;
                    lock.try_lock().map_err(io::Error::other)?;
                    return Ok(SessionDir { path, _lock: lock });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("no free session folder name"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SessionDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn restrict(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> io::Result<()> {
    // The per-user temp folder is already private on Windows.
    Ok(())
}

/// Removes session folders under `base` that no live session holds
/// (their lock can be taken). Returns how many were removed.
pub fn clean_stale(base: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(base) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.filter_map(Result::ok) {
        let name = e.file_name();
        if !name.to_string_lossy().starts_with(PREFIX) || !e.path().is_dir() {
            continue;
        }
        let free = match File::open(e.path().join("lock")) {
            Ok(f) => f.try_lock().is_ok(),
            // No lock file: a folder that never finished being created.
            Err(_) => true,
        };
        if free && std::fs::remove_dir_all(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Why a mapping could not be opened.
#[derive(Debug)]
pub enum ShmError {
    Io(io::Error),
    /// The file is not a voXY mapping, or has another layout.
    BadHeader,
}

impl std::fmt::Display for ShmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShmError::Io(e) => write!(f, "shared memory: {e}"),
            ShmError::BadHeader => f.write_str("shared memory: not a voXY mapping"),
        }
    }
}

impl std::error::Error for ShmError {}

impl From<io::Error> for ShmError {
    fn from(e: io::Error) -> Self {
        ShmError::Io(e)
    }
}

/// One mapping, seen from either process.
#[derive(Debug)]
pub struct SharedMemory {
    path: PathBuf,
    map: MmapMut,
}

impl SharedMemory {
    /// Creates a zeroed mapping of at least `len` bytes at `path` (host
    /// side) and writes its header.
    pub fn create(path: &Path, len: usize) -> Result<SharedMemory, ShmError> {
        let len = len.max(HEADER).next_multiple_of(ALIGN);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        file.set_len(len as u64)?;
        // SAFETY: the file is ours, sized above, and only voXY's processes
        // map it; both treat the contents as plain bytes and the header
        // fields as atomics.
        let mut map = unsafe { MmapMut::map_mut(&file) }?;
        map[..8].copy_from_slice(&MAGIC);
        map[OFF_LAYOUT..OFF_LAYOUT + 4].copy_from_slice(&LAYOUT.to_le_bytes());
        map[OFF_LEN..OFF_LEN + 8].copy_from_slice(&(len as u64).to_le_bytes());
        Ok(SharedMemory {
            path: path.to_path_buf(),
            map,
        })
    }

    /// Opens a mapping the host created (sandbox side), checking the header.
    pub fn open(path: &Path) -> Result<SharedMemory, ShmError> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let file_len = usize::try_from(file.metadata()?.len()).map_err(|_| ShmError::BadHeader)?;
        if file_len < HEADER {
            return Err(ShmError::BadHeader);
        }
        // SAFETY: as in `create`.
        let map = unsafe { MmapMut::map_mut(&file) }?;
        let header_len =
            u64::from_le_bytes(map[OFF_LEN..OFF_LEN + 8].try_into().unwrap_or_default());
        if map[..8] != MAGIC
            || map[OFF_LAYOUT..OFF_LAYOUT + 4] != LAYOUT.to_le_bytes()
            || header_len != file_len as u64
        {
            return Err(ShmError::BadHeader);
        }
        Ok(SharedMemory {
            path: path.to_path_buf(),
            map,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    #[allow(clippy::cast_ptr_alignment)] // checked: page-aligned base
    fn atomic_u32(&self, offset: usize) -> &AtomicU32 {
        debug_assert!(
            (self.map.as_ptr() as usize + offset).is_multiple_of(align_of::<AtomicU32>())
        );
        // SAFETY: the offset is inside the header, 4-byte aligned (the
        // mapping is page aligned), and the mapping outlives the borrow.
        unsafe { &*self.map.as_ptr().add(offset).cast::<AtomicU32>() }
    }

    #[allow(clippy::cast_ptr_alignment)] // checked: page-aligned base
    fn atomic_u64(&self, offset: usize) -> &AtomicU64 {
        debug_assert!(
            (self.map.as_ptr() as usize + offset).is_multiple_of(align_of::<AtomicU64>())
        );
        // SAFETY: as above, 8-byte aligned.
        unsafe { &*self.map.as_ptr().add(offset).cast::<AtomicU64>() }
    }

    /// The abort flag the plugin's `abort()` and progress calls read.
    pub fn set_abort(&self, abort: bool) {
        self.atomic_u32(OFF_ABORT)
            .store(u32::from(abort), Ordering::SeqCst);
    }

    pub fn aborted(&self) -> bool {
        self.atomic_u32(OFF_ABORT).load(Ordering::SeqCst) != 0
    }

    /// The sandbox bumps this while it works; the host treats a counter
    /// that stops moving as a hang.
    pub fn beat(&self) -> u64 {
        self.atomic_u64(OFF_HEARTBEAT)
            .fetch_add(1, Ordering::SeqCst)
            + 1
    }

    pub fn heartbeat(&self) -> u64 {
        self.atomic_u64(OFF_HEARTBEAT).load(Ordering::SeqCst)
    }

    /// `len` bytes at `offset`, if they lie past the header and inside the
    /// mapping.
    pub fn bytes(&self, offset: u64, len: usize) -> Option<&[u8]> {
        let r = self.range(offset, len)?;
        Some(&self.map[r])
    }

    pub fn bytes_mut(&mut self, offset: u64, len: usize) -> Option<&mut [u8]> {
        let r = self.range(offset, len)?;
        Some(&mut self.map[r])
    }

    /// The address of `offset`, for handing an image to a plugin; checked
    /// like [`SharedMemory::bytes`].
    pub fn ptr(&mut self, offset: u64, len: usize) -> Option<*mut u8> {
        let r = self.range(offset, len)?;
        // SAFETY: inside the mapping, checked by `range`.
        Some(unsafe { self.map.as_mut_ptr().add(r.start) })
    }

    fn range(&self, offset: u64, len: usize) -> Option<std::ops::Range<usize>> {
        let start = usize::try_from(offset).ok()?;
        let end = start.checked_add(len)?;
        (start >= HEADER && end <= self.map.len() && start.is_multiple_of(ALIGN))
            .then_some(start..end)
    }
}

/// Hands out 64-byte-aligned slots after the header, host side. Reset per
/// render.
#[derive(Debug, Clone)]
pub struct Slots {
    next: usize,
    len: usize,
}

impl Slots {
    pub fn new(mapping_len: usize) -> Slots {
        Slots {
            next: HEADER,
            len: mapping_len,
        }
    }

    /// An offset for `size` bytes, or `None` when the mapping is full (the
    /// host then replaces it with a bigger one).
    pub fn take(&mut self, size: usize) -> Option<u64> {
        let start = self.next;
        let end = start.checked_add(size)?.next_multiple_of(ALIGN);
        if end > self.len {
            return None;
        }
        self.next = end;
        Some(start as u64)
    }

    /// How many bytes are in use, header included.
    pub fn used(&self) -> usize {
        self.next
    }

    pub fn reset(&mut self) {
        self.next = HEADER;
    }
}
