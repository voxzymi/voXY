//! Reveal actions (design note `design-media-comps.md` §6).
//!
//! Reveal changes view state only (selection, scroll, folder expansion, the
//! open composition). Nothing here touches `History`.
//!
//! Everything is a pure function over a `Document` or a path, so the rules
//! are tested without a window. The only I/O is [`run`], which launches the
//! platform file manager from a plan [`reveal_plan`] built beforehand.

use std::ffi::OsString;
use std::path::Path;

use voxy_core::{CompId, Document, LayerId, LayerKind, PoolItem, SourceRef};

/// Layer → project (§6.1): the distinct project items the layers show, in
/// layer order. The panel re-sorts them into its display order.
///
/// Layers that have no project item (solids, placeholders) are skipped; if
/// none of the layers has one, the reason for the first is returned.
pub fn source_targets(doc: &Document, layers: &[LayerId]) -> Result<Vec<PoolItem>, &'static str> {
    let mut items = Vec::new();
    let mut reason = None;
    for &id in layers {
        let Some((_, layer)) = doc.find_layer(id) else {
            continue;
        };
        match &layer.kind {
            LayerKind::Solid { .. } => {
                reason.get_or_insert("Solids are not project items.");
            }
            LayerKind::Placeholder { .. } => {
                reason.get_or_insert("Source was removed from the project.");
            }
            kind => {
                let item = match kind.source() {
                    Some(SourceRef::Asset(a)) if doc.assets.get(a).is_some() => PoolItem::Asset(a),
                    Some(SourceRef::Comp(c)) if doc.comp(c).is_some() => PoolItem::Comp(c),
                    _ => {
                        reason.get_or_insert("Source was removed from the project.");
                        continue;
                    }
                };
                if !items.contains(&item) {
                    items.push(item);
                }
            }
        }
    }
    if items.is_empty() {
        Err(reason.unwrap_or("Select a layer first."))
    } else {
        Ok(items)
    }
}

/// One composition that uses the selected project items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompTarget {
    pub comp: CompId,
    /// Layers in stack order that show *any* of the selected items.
    pub layers: Vec<LayerId>,
}

/// Project → composition (§6.2): where the items (folders expanded) are
/// used, sorted by composition name (case-insensitive), then ID.
pub const NOT_USED: &str = "Not used in any composition";

pub fn comp_targets(doc: &Document, items: &[PoolItem]) -> Vec<CompTarget> {
    let ex = doc.expand_items(items);
    let sources: Vec<SourceRef> = ex
        .assets
        .iter()
        .map(|&a| SourceRef::Asset(a))
        .chain(ex.comps.iter().map(|&c| SourceRef::Comp(c)))
        .collect();
    doc.usages_of(&sources)
        .per_comp
        .into_iter()
        .map(|(comp, layers)| CompTarget { comp, layers })
        .collect()
}

/// The composition the keyboard command opens: the one with the most
/// matching layers, ties going to the first in name order (§6.2).
pub fn primary_target(targets: &[CompTarget]) -> Option<&CompTarget> {
    targets
        .iter()
        .enumerate()
        .max_by_key(|(i, t)| (t.layers.len(), std::cmp::Reverse(*i)))
        .map(|(_, t)| t)
}

/// Submenu labels: `Main (3 layers)`, with ` #id` added when two
/// compositions share a name.
pub fn menu_labels(doc: &Document, targets: &[CompTarget]) -> Vec<String> {
    let name = |c: CompId| doc.comp(c).map_or_else(String::new, |c| c.name.clone());
    targets
        .iter()
        .map(|t| {
            let n = name(t.comp);
            let clash = targets.iter().filter(|o| name(o.comp) == n).count() > 1;
            let layers = match t.layers.len() {
                1 => "1 layer".to_owned(),
                k => format!("{k} layers"),
            };
            if clash {
                format!("{n} #{} ({layers})", t.comp.raw())
            } else {
                format!("{n} ({layers})")
            }
        })
        .collect()
}

// ------------------------------------------------------- file manager

/// The host platform, a parameter so every branch is tested on any host.
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
}

/// One program to try. No shell is involved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// The single argument is passed verbatim (Windows `explorer.exe` does
    /// not follow the usual argv quoting).
    pub raw_windows_arg: bool,
    /// Wait for the exit status; a failure moves on to the next launch.
    pub wait: bool,
}

/// Commands to try, in order, until one succeeds. Empty when `path` cannot
/// be revealed safely (not absolute, or unquotable on Windows).
pub fn reveal_plan(os: Os, path: &Path) -> Vec<Launch> {
    let p = path.to_string_lossy();
    match os {
        Os::Windows => {
            let p = windows_path(&p);
            // A Windows path cannot contain `"`, so wrapping it is unambiguous.
            if p.is_empty() || p.contains('"') || !is_windows_absolute(&p) {
                return Vec::new();
            }
            vec![Launch {
                program: "explorer.exe".into(),
                args: vec![format!("/select,\"{p}\"").into()],
                raw_windows_arg: true,
                wait: false,
            }]
        }
        Os::Mac => {
            // Absolute, so it can never start with `-` and read as an option.
            if !p.starts_with('/') {
                return Vec::new();
            }
            vec![Launch {
                program: "open".into(),
                args: vec!["-R".into(), p.as_ref().into()],
                raw_windows_arg: false,
                wait: false,
            }]
        }
        Os::Linux => {
            if !p.starts_with('/') {
                return Vec::new();
            }
            let uri = format!("file://{}", percent_encode(&p));
            let parent = match p.rfind('/') {
                Some(0) | None => "/",
                Some(i) => &p[..i],
            };
            vec![
                Launch {
                    program: "dbus-send".into(),
                    args: [
                        "--session",
                        "--print-reply",
                        "--dest=org.freedesktop.FileManager1",
                        "--type=method_call",
                        "/org/freedesktop/FileManager1",
                        "org.freedesktop.FileManager1.ShowItems",
                    ]
                    .iter()
                    .map(OsString::from)
                    .chain([format!("array:string:{uri}").into(), "string:".into()])
                    .collect(),
                    raw_windows_arg: false,
                    wait: true,
                },
                Launch {
                    program: "xdg-open".into(),
                    args: vec![parent.into()],
                    raw_windows_arg: false,
                    wait: false,
                },
            ]
        }
    }
}

/// Windows form of a path: no `\\?\` prefix (`\\?\UNC\x` becomes `\\x`) and
/// backslash separators. UNC paths are otherwise unchanged.
fn windows_path(p: &str) -> String {
    let p = if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = p.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        p.to_owned()
    };
    p.replace('/', "\\")
}

fn is_windows_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with(r"\\")
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
}

/// Percent-encodes every byte except unreserved characters and `/`, so
/// commas (which `dbus-send` reads as array separators), spaces, quotes and
/// non-ASCII bytes cannot change the meaning.
fn percent_encode(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Runs the plan until a launch succeeds. Blocks while waiting, so call it
/// off the UI thread.
pub fn run(plan: &[Launch]) -> Result<(), String> {
    let mut last = "nothing to launch".to_owned();
    for launch in plan {
        let mut cmd = std::process::Command::new(&launch.program);
        #[cfg(windows)]
        if launch.raw_windows_arg {
            use std::os::windows::process::CommandExt as _;
            for a in &launch.args {
                cmd.raw_arg(a);
            }
        } else {
            cmd.args(&launch.args);
        }
        #[cfg(not(windows))]
        cmd.args(&launch.args);
        let result = if launch.wait {
            cmd.output().map(|o| (o.status.success(), String::new()))
        } else {
            cmd.spawn().map(|_| (true, String::new()))
        };
        match result {
            Ok((true, _)) => return Ok(()),
            Ok((false, _)) => last = format!("{} failed", launch.program.to_string_lossy()),
            Err(e) => last = format!("{}: {e}", launch.program.to_string_lossy()),
        }
    }
    Err(last)
}

#[cfg(test)]
#[allow(clippy::many_single_char_names)] // a, b, c, h, x, y read best short in these scenarios.
mod tests {
    use super::*;
    use voxy_core::command::{
        AddSolidLayer, AddSourceLayer, CreateComposition, CreateFolder, ImportAssets, MoveToFolder,
    };
    use voxy_core::{Asset, AssetKind, CompSettings, Document, History, LinearRgba};

    fn comp_named(h: &mut History, name: &str) -> CompId {
        h.execute_reported(CreateComposition {
            settings: CompSettings {
                name: name.into(),
                ..CompSettings::default()
            },
            folder: None,
        })
        .unwrap()
        .created_comps[0]
    }

    fn asset(h: &mut History, name: &str) -> voxy_core::AssetId {
        h.execute_reported(ImportAssets {
            assets: vec![Asset::new(AssetKind::Image, name, format!("{name}.png"))],
            folder: None,
        })
        .unwrap()
        .created_assets[0]
    }

    fn put(h: &mut History, comp: CompId, source: SourceRef) -> LayerId {
        h.execute_reported(AddSourceLayer {
            comp,
            source,
            index: 0,
            in_point: voxy_core::Time::ZERO,
        })
        .unwrap()
        .created_layers[0]
    }

    #[test]
    fn layer_source_resolves_assets_and_comps_once() {
        let mut h = History::new(Document::empty("T"));
        let main = comp_named(&mut h, "Main");
        let inner = comp_named(&mut h, "Inner");
        let a = asset(&mut h, "a");
        let l1 = put(&mut h, main, SourceRef::Asset(a));
        let l2 = put(&mut h, main, SourceRef::Asset(a));
        let l3 = put(&mut h, main, SourceRef::Comp(inner));
        let doc = h.current();
        assert_eq!(
            source_targets(doc, &[l1, l2, l3]),
            Ok(vec![PoolItem::Asset(a), PoolItem::Comp(inner)]),
            "the same source twice is listed once"
        );
    }

    #[test]
    fn solids_and_placeholders_cannot_be_revealed() {
        let mut h = History::new(Document::empty("T"));
        let main = comp_named(&mut h, "Main");
        h.execute(AddSolidLayer {
            comp: main,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        })
        .unwrap();
        let solid = h.current().comp(main).unwrap().layers[0].id;
        assert_eq!(
            source_targets(h.current(), &[solid]),
            Err("Solids are not project items.")
        );
        let a = asset(&mut h, "a");
        let l = put(&mut h, main, SourceRef::Asset(a));
        assert_eq!(
            source_targets(h.current(), &[solid, l]),
            Ok(vec![PoolItem::Asset(a)]),
            "a mixed selection reveals what it can"
        );
        h.execute(voxy_core::command::RemoveItems {
            items: vec![PoolItem::Asset(a)],
            mode: voxy_core::command::RemoveMode::PlaceholderLayers,
        })
        .unwrap();
        assert_eq!(
            source_targets(h.current(), &[l]),
            Err("Source was removed from the project.")
        );
        assert_eq!(
            source_targets(h.current(), &[]),
            Err("Select a layer first.")
        );
    }

    #[test]
    fn usage_targets_are_sorted_and_count_any_selected_item() {
        let mut h = History::new(Document::empty("T"));
        let zed = comp_named(&mut h, "zed");
        let alpha = comp_named(&mut h, "Alpha");
        let a = asset(&mut h, "a");
        let b = asset(&mut h, "b");
        let c = asset(&mut h, "c");
        put(&mut h, zed, SourceRef::Asset(a));
        put(&mut h, alpha, SourceRef::Asset(a));
        put(&mut h, alpha, SourceRef::Asset(b));
        put(&mut h, alpha, SourceRef::Asset(c));
        let doc = h.current();
        let t = comp_targets(doc, &[PoolItem::Asset(a), PoolItem::Asset(b)]);
        assert_eq!(
            t.iter()
                .map(|t| (t.comp, t.layers.len()))
                .collect::<Vec<_>>(),
            vec![(alpha, 2), (zed, 1)],
            "name order, counting layers of any selected item"
        );
        assert_eq!(primary_target(&t).unwrap().comp, alpha);
        assert_eq!(
            menu_labels(doc, &t),
            vec!["Alpha (2 layers)", "zed (1 layer)"]
        );
        assert_eq!(comp_targets(doc, &[PoolItem::Asset(c)])[0].comp, alpha);
    }

    #[test]
    fn primary_ties_go_to_the_first_by_name_and_unused_is_empty() {
        let mut h = History::new(Document::empty("T"));
        let b = comp_named(&mut h, "B");
        let a = comp_named(&mut h, "A");
        let x = asset(&mut h, "x");
        let y = asset(&mut h, "y");
        put(&mut h, b, SourceRef::Asset(x));
        put(&mut h, a, SourceRef::Asset(x));
        let t = comp_targets(h.current(), &[PoolItem::Asset(x)]);
        assert_eq!(primary_target(&t).unwrap().comp, a, "tie: first by name");
        assert_eq!(comp_targets(h.current(), &[PoolItem::Asset(y)]).len(), 0);
        assert!(primary_target(&[]).is_none());
    }

    #[test]
    fn folders_expand_and_equal_names_get_an_id_hint() {
        let mut h = History::new(Document::empty("T"));
        let one = comp_named(&mut h, "Same");
        let two = comp_named(&mut h, "Same");
        let a = asset(&mut h, "a");
        let f = h
            .execute_reported(CreateFolder {
                name: "Clips".into(),
                parent: None,
            })
            .unwrap()
            .created_folders[0];
        h.execute(MoveToFolder {
            items: vec![PoolItem::Asset(a)],
            folder: Some(f),
        })
        .unwrap();
        put(&mut h, one, SourceRef::Asset(a));
        put(&mut h, two, SourceRef::Asset(a));
        let t = comp_targets(h.current(), &[PoolItem::Folder(f)]);
        assert_eq!(t.len(), 2, "a folder stands for what is inside it");
        let labels = menu_labels(h.current(), &t);
        assert!(labels[0].starts_with("Same #") && labels[1].starts_with("Same #"));
        assert_ne!(labels[0], labels[1]);
    }

    fn args(l: &Launch) -> Vec<String> {
        l.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn windows_uses_one_raw_select_argument() {
        for (input, shown) in [
            (r"C:\Users\me\clip.mp4", r"C:\Users\me\clip.mp4"),
            (r"C:\My Files\a b.png", r"C:\My Files\a b.png"),
            (r"C:\a,b\100%#1 é.png", r"C:\a,b\100%#1 é.png"),
            (r"\\?\C:\x\y.png", r"C:\x\y.png"),
            (r"\\?\UNC\srv\share\f.png", r"\\srv\share\f.png"),
            (r"\\srv\share\f.png", r"\\srv\share\f.png"),
            ("C:/mixed/sep.png", r"C:\mixed\sep.png"),
        ] {
            let plan = reveal_plan(Os::Windows, Path::new(input));
            assert_eq!(plan.len(), 1, "{input}");
            let l = &plan[0];
            assert_eq!(l.program, "explorer.exe");
            assert!(l.raw_windows_arg && !l.wait);
            assert_eq!(args(l), vec![format!("/select,\"{shown}\"")], "{input}");
        }
        for bad in [r#"C:\a"b.png"#, "relative.png", ""] {
            assert!(reveal_plan(Os::Windows, Path::new(bad)).is_empty(), "{bad}");
        }
    }

    #[test]
    fn macos_reveals_an_absolute_path_after_the_option() {
        let plan = reveal_plan(Os::Mac, Path::new("/Users/me/My Clips/é,#%.mov"));
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].program, "open");
        assert_eq!(args(&plan[0]), vec!["-R", "/Users/me/My Clips/é,#%.mov"]);
        assert!(!plan[0].raw_windows_arg);
        for relative in ["-evil", "clip.mov", ""] {
            assert!(
                reveal_plan(Os::Mac, Path::new(relative)).is_empty(),
                "{relative}"
            );
        }
        // A file named like an option is fine once it is absolute.
        let p = reveal_plan(Os::Mac, Path::new("/tmp/-R"));
        assert_eq!(args(&p[0]), vec!["-R", "/tmp/-R"]);
    }

    #[test]
    fn linux_tries_the_file_manager_service_then_xdg_open() {
        let plan = reveal_plan(Os::Linux, Path::new("/home/me/My Clips/a,b é%#'\".mp4"));
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].program, "dbus-send");
        assert!(plan[0].wait);
        let a = args(&plan[0]);
        assert!(a.contains(&"--dest=org.freedesktop.FileManager1".to_owned()));
        assert!(a.contains(&"org.freedesktop.FileManager1.ShowItems".to_owned()));
        let uri = &a[a.len() - 2];
        assert_eq!(
            uri,
            "array:string:file:///home/me/My%20Clips/a%2Cb%20%C3%A9%25%23%27%22.mp4"
        );
        assert!(
            !uri["array:string:".len()..].contains(','),
            "commas split the array"
        );
        assert_eq!(a.last().unwrap(), "string:");
        assert_eq!(plan[1].program, "xdg-open");
        assert_eq!(args(&plan[1]), vec!["/home/me/My Clips"]);
        assert!(!plan[1].wait);
    }

    #[test]
    fn linux_edge_cases() {
        let p = reveal_plan(Os::Linux, Path::new("/top.png"));
        assert_eq!(args(&p[1]), vec!["/"], "a file at the root opens the root");
        assert_eq!(reveal_plan(Os::Linux, Path::new("-n")).len(), 0);
        assert_eq!(reveal_plan(Os::Linux, Path::new("rel/x.png")).len(), 0);
    }

    #[test]
    fn percent_encoding_keeps_only_unreserved_bytes() {
        assert_eq!(percent_encode("/a-b_c.d~e/Z9"), "/a-b_c.d~e/Z9");
        assert_eq!(
            percent_encode(" ,%#?&=+:@!$'()*;\"<>"),
            "%20%2C%25%23%3F%26%3D%2B%3A%40%21%24%27%28%29%2A%3B%22%3C%3E"
        );
        assert_eq!(percent_encode("日"), "%E6%97%A5");
    }

    #[test]
    fn run_reports_an_empty_plan() {
        assert!(run(&[]).is_err());
    }
}
