//! Shared by the sandbox tests: a real bundle from our test plugins.

#![allow(clippy::unwrap_used, dead_code)]

use std::path::{Path, PathBuf};

use voxy_ofx_host::discovery::{Arch, Os, arch_dirs};

/// Copies the test-plugin library into `dir/VoxyTest.ofx.bundle` with the
/// layout OpenFX specifies, and returns the bundle path.
pub fn bundle(dir: &Path) -> PathBuf {
    let deps = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let lib = format!(
        "{}voxy_ofx_testplugins{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let src = [deps.join(&lib), deps.parent().unwrap().join(&lib)]
        .into_iter()
        .find(|p| p.is_file())
        .expect("the test-plugin library is built with the tests");
    let bundle = dir.join("VoxyTest.ofx.bundle");
    let bin_dir = bundle
        .join("Contents")
        .join(arch_dirs(Os::current(), Arch::current())[0]);
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::copy(&src, bin_dir.join("VoxyTest.ofx")).unwrap();
    bundle
}
