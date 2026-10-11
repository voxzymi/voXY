//! Plugin discovery (design §6.2): the search order for every platform,
//! tested on every platform, and the folder scan.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use voxy_ofx_host::discovery::{
    Arch, FoundBundle, Os, arch_dirs, bundle_binaries, is_bundle_name, roots, scan,
};

fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
        .collect();
    move |k| map.get(k).cloned()
}

fn paths(v: &[&str]) -> Vec<PathBuf> {
    v.iter().map(PathBuf::from).collect()
}

#[test]
fn windows_order_is_env_then_common_files_then_the_deprecated_folder() {
    let e = env(&[
        ("OFX_PLUGIN_PATH", r"D:\fx;;E:\more fx"),
        ("CommonProgramFiles", r"C:\Programme\Gemeinsame Dateien"),
    ]);
    assert_eq!(
        roots(Os::Windows, &e, &[PathBuf::from(r"F:\mine")]),
        paths(&[
            r"D:\fx",
            r"E:\more fx",
            r"C:\Programme\Gemeinsame Dateien\OFX\Plugins",
            r"C:\Program Files\Common Files\OFX\Plugins",
            r"F:\mine",
        ])
    );
    // On an English system the standard folder is the deprecated one, so it
    // is listed once (case and a trailing separator do not matter).
    let e = env(&[("CommonProgramFiles", r"c:\program files\common files\")]);
    assert_eq!(
        roots(Os::Windows, &e, &[]),
        paths(&[r"c:\program files\common files\OFX\Plugins"])
    );
    assert_eq!(
        roots(Os::Windows, &env(&[]), &[]),
        paths(&[r"C:\Program Files\Common Files\OFX\Plugins"])
    );
}

#[test]
fn macos_splits_on_semicolons_and_ends_with_library() {
    let e = env(&[("OFX_PLUGIN_PATH", "/Users/me/fx:with colon;/opt/fx")]);
    assert_eq!(
        roots(Os::Mac, &e, &[PathBuf::from("/library/ofx/plugins")]),
        paths(&["/Users/me/fx:with colon", "/opt/fx", "/Library/OFX/Plugins"])
    );
}

#[test]
fn linux_splits_on_colons_and_ends_with_usr() {
    let e = env(&[("OFX_PLUGIN_PATH", "/opt/a:/opt/b;c:")]);
    assert_eq!(
        roots(Os::Linux, &e, &paths(&["/home/me/ofx", "/opt/a/"])),
        paths(&["/opt/a", "/opt/b;c", "/usr/OFX/Plugins", "/home/me/ofx"])
    );
}

#[test]
fn bundle_layout_per_platform() {
    let b = Path::new("plugins").join("Blur.ofx.bundle");
    let at = |dir: &str| b.join("Contents").join(dir).join("Blur.ofx");
    assert_eq!(
        bundle_binaries(&b, Os::Windows, Arch::X86_64),
        [at("Win64")]
    );
    assert_eq!(
        bundle_binaries(&b, Os::Linux, Arch::X86_64),
        [at("Linux-x86-64")]
    );
    assert_eq!(bundle_binaries(&b, Os::Mac, Arch::Aarch64), [at("MacOS")]);
    assert_eq!(
        bundle_binaries(&b, Os::Mac, Arch::X86_64),
        [at("MacOS"), at("MacOS-x86-64")]
    );
    assert!(!arch_dirs(Os::Mac, Arch::Aarch64).contains(&"MacOS-arm-64"));
    assert_eq!(
        bundle_binaries(Path::new("Blur.ofx"), Os::Linux, Arch::X86_64),
        Vec::<PathBuf>::new()
    );
    assert!(!is_bundle_name(".ofx.bundle"));
}

/// Makes `root/rel/Contents/<dir>/<NAME>.ofx` (an empty file).
fn plant(root: &Path, rel: &str, dir: &str) {
    let bundle = root.join(rel);
    let name = bundle
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_suffix(".bundle")
        .unwrap()
        .to_owned();
    let d = bundle.join("Contents").join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(name), b"").unwrap();
}

#[test]
fn scan_recurses_skips_at_names_and_stops_at_bundles() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    let (os, arch) = (Os::current(), Arch::current());
    let dir = arch_dirs(os, arch)[0];
    plant(r, "b/Zed.ofx.bundle", dir);
    plant(r, "a/deep/er/Alpha.ofx.bundle", dir);
    plant(r, "@hidden/Skip.ofx.bundle", dir);
    plant(r, "a/@Off.ofx.bundle", dir);
    // A bundle inside a bundle is part of it, not a second plugin.
    plant(
        r,
        "b/Zed.ofx.bundle/Contents/Resources/Inner.ofx.bundle",
        dir,
    );
    // No binary for this platform.
    plant(r, "c/Other.ofx.bundle", "Some-Other-Arch");

    let roots = vec![r.to_path_buf(), r.join("missing"), r.join("b")];
    let found = scan(&roots, os, arch);
    let bundle = |rel: &str| r.join(rel);
    let with = |rel: &str| FoundBundle {
        bundle: bundle(rel),
        binary: bundle_binaries(&bundle(rel), os, arch).into_iter().next(),
    };
    assert_eq!(
        found,
        vec![
            with("a/deep/er/Alpha.ofx.bundle"),
            with("b/Zed.ofx.bundle"),
            FoundBundle {
                bundle: bundle("c/Other.ofx.bundle"),
                binary: None,
            },
        ],
        "sorted, skipping @ names, each folder once"
    );
}
