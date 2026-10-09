//! Lossless round trip over the owner's real sample files.
//!
//! Put exported files (`.alightmotion` projects, preset/element XML) in
//! `tests/am-samples/` at the workspace root. Every file must:
//! - re-export byte-identically when unedited (zip packages), and
//! - write every XML document back byte-for-byte.
//!
//! With no samples present the test passes and says so.

use std::path::Path;

use voxy_am_compat::inspect::{Content, inspect};

#[test]
fn samples_round_trip_losslessly() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/am-samples");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .is_some_and(|n| !n.to_string_lossy().starts_with('.'))
        })
        .filter(|p| p.extension().is_none_or(|e| e != "md"))
        .collect();
    files.sort();
    if files.is_empty() {
        eprintln!("no samples in {}; nothing to check yet", dir.display());
        return;
    }
    let mut failures = Vec::new();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let report = match inspect(&name, std::fs::read(path).unwrap()) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        if report.package_round_trips == Some(false) {
            failures.push(format!("{name}: package re-export differs"));
        }
        for e in &report.entries {
            if let Content::Xml {
                round_trips: false, ..
            } = e.content
            {
                failures.push(format!("{name}!{}: XML round trip differs", e.name));
            }
        }
        eprintln!(
            "{name}: {} entries, {} XML documents",
            report.entries.len(),
            report.documents.len()
        );
    }
    assert!(
        failures.is_empty(),
        "lossless round trip failed:\n{}",
        failures.join("\n")
    );
}
