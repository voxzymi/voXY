//! Where OpenFX plugins are looked for, and how a bundle's binary is found
//! (OpenFX 1.5.1, "Packaging OFX Plug-ins"; design §6.2).
//!
//! [`roots`] and [`bundle_binaries`] are pure: the operating system, the
//! architecture and the environment are parameters, so every platform's
//! rules are tested on every CI machine. Only [`scan`] reads the file
//! system, and it never loads a binary (design §6.3).

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The platform whose rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        }
    }

    /// The separator of `OFX_PLUGIN_PATH`: `;` on Windows and macOS, `:`
    /// elsewhere, as the spec says.
    fn list_separator(self) -> char {
        match self {
            Os::Windows | Os::Mac => ';',
            Os::Linux => ':',
        }
    }

    fn dir_separator(self) -> &'static str {
        match self {
            Os::Windows => "\\",
            Os::Mac | Os::Linux => "/",
        }
    }
}

/// The CPU architecture the host was built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn current() -> Arch {
        if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else {
            Arch::X86_64
        }
    }
}

/// The directories to search, first to last:
///
/// 1. `OFX_PLUGIN_PATH`, split on the platform's separator.
/// 2. The standard location: on Windows the common program files folder
///    (`CommonProgramFiles`, which is what `CSIDL_PROGRAM_FILES_COMMON`
///    resolves to) plus `\OFX\Plugins`, then the deprecated
///    `C:\Program Files\Common Files\OFX\Plugins`; `/Library/OFX/Plugins`
///    on macOS; `/usr/OFX/Plugins` elsewhere.
/// 3. `extra`: the user's folders from Preferences.
///
/// Empty entries are skipped, and a folder listed twice is kept only the
/// first time (compared without case on Windows and macOS, whose file
/// systems usually ignore it).
pub fn roots(os: Os, env: &dyn Fn(&str) -> Option<OsString>, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        let key = |p: &Path| {
            let s = p.to_string_lossy();
            let s = s.trim_end_matches(['/', '\\']);
            match os {
                Os::Windows => s.replace('/', "\\").to_lowercase(),
                Os::Mac => s.to_lowercase(),
                Os::Linux => s.to_owned(),
            }
        };
        if !p.as_os_str().is_empty() && !out.iter().any(|q| key(q) == key(&p)) {
            out.push(p);
        }
    };
    if let Some(list) = env("OFX_PLUGIN_PATH") {
        for part in list.to_string_lossy().split(os.list_separator()) {
            push(PathBuf::from(part.trim()));
        }
    }
    let sep = os.dir_separator();
    match os {
        Os::Windows => {
            if let Some(common) = env("CommonProgramFiles") {
                let common = common.to_string_lossy().trim_end_matches('\\').to_owned();
                if !common.is_empty() {
                    push(PathBuf::from(format!("{common}{sep}OFX{sep}Plugins")));
                }
            }
            push(PathBuf::from(r"C:\Program Files\Common Files\OFX\Plugins"));
        }
        Os::Mac => push(PathBuf::from("/Library/OFX/Plugins")),
        Os::Linux => push(PathBuf::from("/usr/OFX/Plugins")),
    }
    for p in extra {
        push(p.clone());
    }
    out
}

/// The `Contents/<arch>` folder names to try, best first. macOS uses
/// `MacOS` (a universal binary; OpenFX 1.5 has no arm64 folder), with the
/// deprecated `MacOS-x86-64` as a fallback on Intel. The arm64 names for
/// Windows and Linux follow the spec's informational section.
pub fn arch_dirs(os: Os, arch: Arch) -> &'static [&'static str] {
    match (os, arch) {
        (Os::Windows, Arch::X86_64) => &["Win64"],
        (Os::Windows, Arch::Aarch64) => &["Win-arm64"],
        (Os::Mac, Arch::X86_64) => &["MacOS", "MacOS-x86-64"],
        (Os::Mac, Arch::Aarch64) => &["MacOS"],
        (Os::Linux, Arch::X86_64) => &["Linux-x86-64"],
        (Os::Linux, Arch::Aarch64) => &["Linux-aarch64"],
    }
}

/// Whether a file or folder name marks a bundle: `NAME.ofx.bundle`.
pub fn is_bundle_name(name: &str) -> bool {
    name.len() > ".ofx.bundle".len() && name.ends_with(".ofx.bundle")
}

/// Whether the spec says to skip a folder or bundle with this name.
pub fn is_skipped_name(name: &str) -> bool {
    name.starts_with('@')
}

/// Where `bundle`'s binary may be, best first:
/// `NAME.ofx.bundle/Contents/<arch>/NAME.ofx`. Empty if `bundle` is not
/// named like a bundle.
pub fn bundle_binaries(bundle: &Path, os: Os, arch: Arch) -> Vec<PathBuf> {
    let Some(name) = bundle.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let Some(binary) = name
        .strip_suffix(".bundle")
        .filter(|_| is_bundle_name(name))
    else {
        return Vec::new();
    };
    arch_dirs(os, arch)
        .iter()
        .map(|dir| bundle.join("Contents").join(dir).join(binary))
        .collect()
}

/// One bundle found by [`scan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundBundle {
    pub bundle: PathBuf,
    /// The binary for this platform, or `None` if the bundle has none
    /// (it is listed so the user can be told why it is missing).
    pub binary: Option<PathBuf>,
}

/// How deep [`scan`] goes below each root.
const MAX_DEPTH: usize = 16;

/// Finds every bundle under `roots`, recursively, in a stable order (roots
/// in order, then names sorted). Folders and bundles whose name starts with
/// `@` are skipped. A bundle is not searched further. Missing or unreadable
/// folders are ignored, and a folder reached twice (for example through a
/// link) is scanned once.
pub fn scan(roots: &[PathBuf], os: Os, arch: Arch) -> Vec<FoundBundle> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        walk(root, 0, os, arch, &mut seen, &mut out);
    }
    out
}

fn walk(
    dir: &Path,
    depth: usize,
    os: Os,
    arch: Arch,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<FoundBundle>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(real) = dir.canonicalize() else {
        return;
    };
    if !seen.insert(real) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .filter(|(name, _)| !is_skipped_name(name))
        .collect();
    children.sort();
    for (name, path) in children {
        if is_bundle_name(&name) {
            let binary = bundle_binaries(&path, os, arch)
                .into_iter()
                .find(|b| b.is_file());
            out.push(FoundBundle {
                bundle: path,
                binary,
            });
        } else {
            walk(&path, depth + 1, os, arch, seen, out);
        }
    }
}
