//! `cargo xtask ofx-fixtures [--out DIR] [--source DIR] [--print-hash]`:
//! builds the OpenFX sample plugins listed in `third_party/ofx-fixtures.toml`
//! as test fixtures (owner decisions P-0 Q1 and Q-C1).
//!
//! The pinned source tarball is downloaded (with `curl`), checked against
//! its sha256, and only the listed paths are extracted; their content hash
//! is checked too. `--source DIR` uses a git checkout of the pinned commit
//! instead of the download (its content hash is still checked).
//! `--print-hash` prints the content hash and stops.
//!
//! Each fixture is compiled with the platform's C++ compiler (found the way
//! build scripts find it) into `DIR/bundles/<bundle>.ofx.bundle`. A fixture
//! that does not build is reported and, unless its `skip` list names this
//! platform, makes the task fail. `DIR/report.txt` lists the outcome of
//! each fixture.
//!
//! The source is third-party code: it is compiled, never executed here,
//! and nothing in it is read as instructions.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    source: Source,
    fixture: Vec<Fixture>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    url: String,
    commit: String,
    sha256: String,
    content_sha256: String,
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    bundle: String,
    #[allow(dead_code)] // checked by `cargo xtask credits`
    credit: String,
    sources: Vec<String>,
    #[serde(default)]
    support: bool,
    #[serde(default)]
    opengl: bool,
    #[serde(default)]
    skip: Vec<String>,
}

/// This platform's name in `skip` lists.
fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

/// The bundle folder for this platform's binaries (OpenFX "Installation
/// Location"); macOS uses the universal folder.
fn arch_dir() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", _) => "MacOS",
        ("windows", _) => "Win64",
        (_, "aarch64") => "Linux-aarch64",
        _ => "Linux-x86-64",
    }
}

/// The target triple to ask `cc` about (this machine).
fn host_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", _) => "x86_64-apple-darwin",
        ("windows", _) => "x86_64-pc-windows-msvc",
        (_, "aarch64") => "aarch64-unknown-linux-gnu",
        _ => "x86_64-unknown-linux-gnu",
    }
}

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let mut out = root.join("target/ofx-fixtures");
    let mut source = None;
    let mut print_hash = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(it.next().context("--out needs a folder")?),
            "--source" => {
                source = Some(PathBuf::from(it.next().context("--source needs a folder")?));
            }
            "--print-hash" => print_hash = true,
            other => bail!("unknown argument {other}"),
        }
    }
    let text = std::fs::read_to_string(root.join("third_party/ofx-fixtures.toml"))?;
    let manifest: Manifest = toml::from_str(&text).context("third_party/ofx-fixtures.toml")?;
    if manifest.schema != 1 {
        bail!("ofx-fixtures.toml: unknown schema {}", manifest.schema);
    }
    std::fs::create_dir_all(&out)?;

    let src = match source {
        Some(dir) => dir,
        None => fetch(&manifest.source, &out)?,
    };
    let hash = content_hash(&src, &manifest.source.paths)?;
    if print_hash {
        println!("content_sha256 = \"{hash}\"");
        return Ok(());
    }
    if hash != manifest.source.content_sha256 {
        bail!(
            "the OpenFX source does not match ofx-fixtures.toml: content sha256 {hash}, expected {}",
            manifest.source.content_sha256
        );
    }

    let bundles = out.join("bundles");
    if bundles.exists() {
        std::fs::remove_dir_all(&bundles)?;
    }
    let objects = out.join("obj");
    let mut report = String::new();
    let mut failed = Vec::new();
    for f in &manifest.fixture {
        if f.skip.iter().any(|s| s == platform()) {
            writeln!(
                report,
                "{}: skipped on {} (listed in skip)",
                f.bundle,
                platform()
            )?;
            continue;
        }
        match build(&src, &bundles, &objects, f) {
            Ok(()) => writeln!(report, "{}: built", f.bundle)?,
            Err(e) => {
                writeln!(report, "{}: FAILED: {e:#}", f.bundle)?;
                failed.push(f.bundle.clone());
            }
        }
    }
    std::fs::write(out.join("report.txt"), &report)?;
    print!("{report}");
    if !failed.is_empty() {
        bail!(
            "fixtures that did not build on {}: {} (add the platform to their `skip` list and record why in docs/ofx-plugins.md §5)",
            platform(),
            failed.join(", ")
        );
    }
    println!("fixtures in {}", bundles.display());
    Ok(())
}

/// Downloads the tarball (once), checks its sha256, and extracts the
/// manifest's paths into `out/src`. Returns that folder.
fn fetch(source: &Source, out: &Path) -> Result<PathBuf> {
    let tarball = out.join(format!("openfx-{}.tar.gz", &source.commit[..12]));
    if !tarball.is_file() {
        let partial = tarball.with_extension("part");
        let status = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--output",
            ])
            .arg(&partial)
            .arg(&source.url)
            .status()
            .context("running curl")?;
        if !status.success() {
            bail!("downloading {} failed ({status})", source.url);
        }
        std::fs::rename(&partial, &tarball)?;
    }
    let bytes = std::fs::read(&tarball)?;
    let actual = hex(&Sha256::digest(&bytes));
    if source.sha256.is_empty() {
        // The content hash below still pins every file used.
        eprintln!("warning: ofx-fixtures.toml has no tarball sha256; this download's is {actual}");
    } else if actual != source.sha256 {
        std::fs::remove_file(&tarball)?;
        bail!("tarball sha256 {actual}, expected {}", source.sha256);
    }

    let dest = out.join("src");
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    std::fs::create_dir_all(&dest)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.into_owned();
        // Drop the archive's top folder; keep only listed, plain paths.
        let rel: PathBuf = path.components().skip(1).collect();
        if rel.components().any(|c| !matches!(c, Component::Normal(_)))
            || !listed(&rel, &source.paths)
        {
            continue;
        }
        let target = dest.join(&rel);
        std::fs::create_dir_all(target.parent().context("a file path")?)?;
        entry.unpack(&target)?;
    }
    Ok(dest)
}

/// Whether `rel` is one of `paths` or lies under one.
fn listed(rel: &Path, paths: &[String]) -> bool {
    paths.iter().any(|p| rel.starts_with(p))
}

/// The content hash of every file under `paths` in `root` (see the
/// manifest's header for the exact definition).
fn content_hash(root: &Path, paths: &[String]) -> Result<String> {
    let mut files = BTreeMap::new();
    for p in paths {
        collect(root, &root.join(p), &mut files)?;
    }
    let mut h = Sha256::new();
    for (rel, full) in &files {
        let data = std::fs::read(full)?;
        h.update(rel.as_bytes());
        h.update([0u8]);
        h.update((data.len() as u64).to_le_bytes());
        h.update(&data);
    }
    Ok(hex(&h.finalize()))
}

fn collect(root: &Path, path: &Path, files: &mut BTreeMap<String, PathBuf>) -> Result<()> {
    let meta = std::fs::metadata(path).with_context(|| format!("{}", path.display()))?;
    if meta.is_dir() {
        for e in std::fs::read_dir(path)? {
            collect(root, &e?.path(), files)?;
        }
    } else {
        let rel = path
            .strip_prefix(root)?
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        files.insert(rel, path.to_path_buf());
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Compiles and links one fixture into its bundle.
fn build(src: &Path, bundles: &Path, objects: &Path, f: &Fixture) -> Result<()> {
    let compiler = cc::Build::new()
        .cpp(true)
        .opt_level(1)
        .target(host_triple())
        .host(host_triple())
        .cargo_metadata(false)
        .cargo_warnings(false)
        .try_get_compiler()
        .context("finding a C++ compiler")?;
    let bin_dir = bundles
        .join(format!("{}.ofx.bundle", f.bundle))
        .join("Contents")
        .join(arch_dir());
    std::fs::create_dir_all(&bin_dir)?;
    let output = bin_dir.join(format!("{}.ofx", f.bundle));
    let obj_dir = objects.join(&f.bundle);
    std::fs::create_dir_all(&obj_dir)?;

    let mut sources: Vec<PathBuf> = f.sources.iter().map(|s| src.join(s)).collect();
    if f.support {
        let mut lib: Vec<PathBuf> = std::fs::read_dir(src.join("Support/Library"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "cpp"))
            .collect();
        lib.sort();
        sources.extend(lib);
    }
    let includes = [src.join("include"), src.join("Support/include")];

    let mut cmd = compiler.to_command();
    if compiler.is_like_msvc() {
        cmd.args(["/nologo", "/LD", "/EHsc", "/std:c++17", "/DNOMINMAX"]);
        for i in &includes {
            cmd.arg(format!("/I{}", i.display()));
        }
        cmd.args(&sources);
        cmd.arg(format!("/Fo{}\\", obj_dir.display()));
        cmd.arg(format!("/Fe{}", output.display()));
        cmd.arg("/link");
        if f.opengl {
            cmd.arg("opengl32.lib");
        }
    } else {
        cmd.args(["-std=c++17", "-fPIC", "-fvisibility=hidden", "-shared"]);
        for i in &includes {
            cmd.arg("-I").arg(i);
        }
        cmd.args(&sources);
        cmd.arg("-o").arg(&output);
        if f.opengl {
            if std::env::consts::OS == "macos" {
                cmd.args(["-framework", "OpenGL"]);
            } else {
                cmd.arg("-lGL");
            }
        }
    }
    cmd.current_dir(&obj_dir);
    let out = cmd.output().context("running the C++ compiler")?;
    if !out.status.success() {
        let log = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = log
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(8)
            .collect();
        bail!("{}", tail.join(" | "));
    }
    Ok(())
}
