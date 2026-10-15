//! Loads a plugin bundle's binary and lists its plugins. Runs only inside
//! the sandbox process (design §6.3): the editor never calls it.

use std::ffi::c_int;
use std::path::{Path, PathBuf};

use voxy_ofx_sys as ofx;

use crate::discovery::{Arch, Os, bundle_binaries};

/// Why a bundle could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    /// No binary for this platform in the bundle.
    NoBinary(PathBuf),
    /// The binary would not load.
    Library(String),
    /// A required export is missing.
    MissingExport(&'static str),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::NoBinary(p) => write!(f, "no binary for this platform in {}", p.display()),
            LoadError::Library(e) => write!(f, "the plugin binary did not load: {e}"),
            LoadError::MissingExport(s) => write!(f, "the plugin binary has no {s}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// The plugins a binary exports, and the binary's path.
#[derive(Debug)]
pub struct Bundle {
    pub binary: PathBuf,
    pub plugins: Vec<&'static ofx::OfxPlugin>,
}

/// Loads `path`, a `.ofx.bundle` folder or a binary itself, and asks it for
/// its plugins. The library stays loaded for the life of the process, so
/// the plugin structs it returns are `'static`.
///
/// # Safety
/// Loading runs the binary's initialisers: call it only in the sandbox.
pub unsafe fn load_bundle(path: &Path) -> Result<Bundle, LoadError> {
    let binary = if path.is_dir() {
        bundle_binaries(path, Os::current(), Arch::current())
            .into_iter()
            .find(|b| b.is_file())
            .ok_or_else(|| LoadError::NoBinary(path.to_path_buf()))?
    } else {
        path.to_path_buf()
    };
    // SAFETY: the caller's contract; this process exists to run it.
    let lib = unsafe { libloading::Library::new(&binary) }
        .map_err(|e| LoadError::Library(e.to_string()))?;
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));
    // SAFETY: the signatures OpenFX defines for these exports.
    let (count, get) = unsafe {
        let count = lib
            .get::<unsafe extern "C" fn() -> c_int>(b"OfxGetNumberOfPlugins\0")
            .map_err(|_| LoadError::MissingExport("OfxGetNumberOfPlugins"))?;
        let get = lib
            .get::<unsafe extern "C" fn(c_int) -> *mut ofx::OfxPlugin>(b"OfxGetPlugin\0")
            .map_err(|_| LoadError::MissingExport("OfxGetPlugin"))?;
        (count, get)
    };
    // SAFETY: calling the exports as OpenFX defines them; non-null
    // results point into the library, which is never unloaded.
    let plugins = unsafe {
        (0..count().max(0))
            .filter_map(|i| get(i).as_ref())
            .collect()
    };
    Ok(Bundle { binary, plugins })
}
