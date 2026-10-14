//! OpenFX at start-up (design §6.2, §6.3): find the plugin bundles,
//! describe them (from the cache when unchanged), and hand back the
//! runtime and the effect definitions to register. The editor never loads
//! a plugin itself: everything runs in `voxy-ofx-sandbox`.

use std::path::PathBuf;
use std::sync::Arc;

use voxy_core::registry::EffectDef;
use voxy_ofx_host::client::{Timeouts, default_sandbox_exe};
use voxy_ofx_host::discovery::{Arch, Os, roots, scan};
use voxy_ofx_host::runtime::OfxRuntime;

/// What start-up found.
#[derive(Debug, Default)]
pub struct OfxStartup {
    /// `None` when there are no plugins or no sandbox to run them.
    pub runtime: Option<Arc<OfxRuntime>>,
    pub definitions: Vec<EffectDef>,
    /// `(bundle or plugin, reason)`, for the log and the Effects panel.
    pub problems: Vec<(String, String)>,
}

/// Searches the standard folders plus `extra`, and describes what it finds.
pub fn start(extra: &[PathBuf], blocked: Vec<(String, String)>) -> OfxStartup {
    let os = Os::current();
    let search = roots(os, &|k| std::env::var_os(k), extra);
    let found = scan(&search, os, Arch::current());
    let mut out = OfxStartup::default();
    let mut bundles = Vec::new();
    for b in found {
        if b.binary.is_some() {
            bundles.push(b.bundle);
        } else {
            out.problems.push((
                b.bundle.display().to_string(),
                "no binary for this platform".into(),
            ));
        }
    }
    if bundles.is_empty() {
        return out;
    }
    let Some(exe) = default_sandbox_exe() else {
        out.problems.push((
            "voxy-ofx-sandbox".into(),
            "the plugin host is missing next to voXY, so plugins are off".into(),
        ));
        return out;
    };
    let runtime = match OfxRuntime::new(
        &exe,
        &voxy_ofx_host::shm::default_base(),
        Timeouts::default(),
    ) {
        Ok(r) => r,
        Err(e) => {
            out.problems.push(("plugin host".into(), e.to_string()));
            return out;
        }
    };
    runtime.restore_blocks(blocked);
    let cache =
        crate::prefs::config_dir_with(crate::reveal::Os::current(), &|k| std::env::var_os(k))
            .map(|d| d.join("ofx-cache.json"));
    let result = match &cache {
        Some(path) => runtime.scan_cached(&bundles, path),
        None => runtime.scan(&bundles),
    };
    out.definitions = result.definitions;
    out.problems.extend(result.problems);
    out.runtime = Some(Arc::new(runtime));
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use voxy_core::EffectTypeId;
    use voxy_core::registry::{Category, EffectDef, EffectOrigin, EffectSpace};

    use super::*;
    use crate::state::EditorState;

    #[test]
    fn no_plugins_means_no_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let s = start(&[dir.path().to_path_buf()], Vec::new());
        assert!(s.runtime.is_none());
        assert_eq!(s.definitions.len(), 0);
    }

    #[test]
    fn a_bundle_without_a_binary_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Broken.ofx.bundle/Contents")).unwrap();
        let s = start(&[dir.path().to_path_buf()], Vec::new());
        assert!(s.runtime.is_none());
        assert!(
            s.problems
                .iter()
                .any(|(what, why)| what.contains("Broken.ofx.bundle") && why.contains("no binary")),
            "{:?}",
            s.problems
        );
    }

    #[test]
    fn found_plugins_join_the_registry() {
        let mut state = EditorState::new(EditorState::untitled());
        let before = state.effects.len();
        let def = EffectDef {
            type_id: EffectTypeId::new("ofx:org.example.Thing"),
            version: 1,
            name: "Thing".into(),
            category: Category::Plugin("Example".into()),
            description: "".into(),
            params: Arc::new([]),
            groups: Arc::new([]),
            space: EffectSpace::Working,
            seeded: false,
            origin: EffectOrigin::Ofx {
                binary: "thing.ofx".into(),
                index: 0,
            },
        };
        state.install_ofx(OfxStartup {
            runtime: None,
            definitions: vec![def.clone(), def],
            problems: vec![("x".into(), "y".into())],
        });
        assert_eq!(state.effects.len(), before + 1, "the duplicate is skipped");
        assert!(
            state
                .effects
                .latest(&EffectTypeId::new("ofx:org.example.Thing"))
                .is_some()
        );
        assert_eq!(state.ofx_problems.len(), 1);
    }
}
