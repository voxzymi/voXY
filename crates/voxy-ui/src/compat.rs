//! Names that changed when the Media panel became the Project panel
//! (`design-project-panel.md` §1), and the migration of saved UI state.
//!
//! Nothing shipped under the old names, so this is a defensive layer: a
//! layout, a shortcut map or a `ui_state` written by an earlier build still
//! loads, and anything unknown is dropped instead of failing.

use serde_json::{Map, Value};

use crate::comp_draft::Tab as DialogTab;
use crate::project_view::{Column, SortKey, TypeFilter, ViewSettings};

/// Version of the `ui_state` section this build writes. Version 1 stored
/// the panel under `media_panel`; version 2 uses `project_panel` (and adds
/// `columns`, `filter` and the Type sort).
pub const UI_STATE_VERSION: u32 = 2;

/// Panel names a saved dock layout may contain, mapped to the current ones.
/// The Media panel is now the Project panel.
#[allow(
    dead_code,
    reason = "used when the dock layout is saved (Preferences, P-D); covered by tests"
)]
pub const PANEL_ALIASES: &[(&str, &str)] = &[
    ("Media", "Project"),
    ("media", "Project"),
    ("media_panel", "Project"),
    ("Media panel", "Project"),
    ("project", "Project"),
    ("project_panel", "Project"),
];

/// The current name of a saved panel name, or `None` if it is not a panel.
#[allow(
    dead_code,
    reason = "used when the dock layout is saved (Preferences, P-D); covered by tests"
)]
pub fn panel_name(saved: &str) -> Option<&'static str> {
    if let Some((_, to)) = PANEL_ALIASES.iter().find(|(from, _)| *from == saved) {
        return Some(to);
    }
    [
        "Viewport",
        "Timeline",
        "Graph Editor",
        "Properties",
        "Project",
    ]
    .into_iter()
    .find(|n| *n == saved)
}

/// The `ui_state` this build saves for the panel.
pub fn to_ui_state(view: &ViewSettings, dialog_tab: DialogTab) -> Value {
    let mut map = Map::new();
    map.insert("version".into(), UI_STATE_VERSION.into());
    map.insert(
        "project_panel".into(),
        serde_json::to_value(view).unwrap_or(Value::Null),
    );
    map.insert(
        "comp_settings".into(),
        serde_json::json!({ "tab": serde_json::to_value(dialog_tab).unwrap_or(Value::Null) }),
    );
    Value::Object(map)
}

/// The Composition Settings tab last used, or the default if missing or
/// unreadable.
pub fn dialog_tab_from(state: &Value) -> DialogTab {
    state
        .get("comp_settings")
        .and_then(|c| c.get("tab"))
        .and_then(|t| serde_json::from_value(t.clone()).ok())
        .unwrap_or_default()
}

/// Reads the panel's settings out of a saved `ui_state` (any version).
///
/// Errors are swallowed per field: an unknown value, a wrong type or a
/// missing key leaves that setting at its default. A version newer than
/// this build's is read the same way, since `ui_state` never affects
/// rendering.
pub fn from_ui_state(state: &Value) -> ViewSettings {
    // `media_panel` is the version 1 name; `project_panel` wins if both exist.
    let panel = state
        .get("project_panel")
        .or_else(|| state.get("media_panel"))
        .and_then(Value::as_object);
    let mut view = ViewSettings::default();
    let Some(p) = panel else { return view };
    let field = |name: &str| p.get(name).cloned();
    if let Some(v) = field("sort").and_then(|v| serde_json::from_value::<SortKey>(v).ok()) {
        view.sort = v;
    }
    if let Some(v) = field("descending").and_then(|v| v.as_bool()) {
        view.descending = v;
    }
    if let Some(v) = field("filter").and_then(|v| serde_json::from_value::<TypeFilter>(v).ok()) {
        view.filter = v;
    }
    if let Some(v) = field("search").and_then(|v| v.as_str().map(str::to_owned)) {
        view.search = v;
    }
    if let Some(Value::Array(cols)) = field("columns") {
        view.columns = cols
            .into_iter()
            .filter_map(|c| serde_json::from_value::<Column>(c).ok())
            .collect();
    }
    if let Some(Value::Array(ids)) = field("expanded") {
        view.expanded = ids
            .into_iter()
            .filter_map(|c| serde_json::from_value(c).ok())
            .collect();
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use voxy_core::FolderId;

    #[test]
    fn old_panel_names_load_as_the_project_panel() {
        for old in ["Media", "media", "media_panel", "Media panel"] {
            assert_eq!(panel_name(old), Some("Project"), "{old}");
        }
        assert_eq!(panel_name("Project"), Some("Project"));
        assert_eq!(panel_name("Timeline"), Some("Timeline"));
        assert_eq!(panel_name("Nonsense"), None, "unknown panels are dropped");
    }

    #[test]
    fn the_dock_titles_are_the_current_names() {
        // Every panel the dock can show must be a name `panel_name` accepts.
        for tab in crate::panels::Tab::ALL {
            assert_eq!(panel_name(tab.title()), Some(tab.title()));
        }
        assert_eq!(crate::panels::Tab::Project.title(), "Project");
        assert_eq!(
            crate::panels::Tab::from_saved("Media"),
            Some(crate::panels::Tab::Project)
        );
    }

    #[test]
    fn saving_writes_the_new_name_at_version_2() {
        let mut view = ViewSettings::default();
        view.expanded.insert(FolderId::from(3));
        view.sort = SortKey::Type;
        let state = to_ui_state(&view, DialogTab::Advanced);
        assert_eq!(dialog_tab_from(&state), DialogTab::Advanced);
        assert_eq!(state["version"], 2);
        assert!(state.get("media_panel").is_none());
        assert_eq!(from_ui_state(&state), view);
    }

    #[test]
    fn version_1_media_panel_state_migrates() {
        let v1 = json!({
            "version": 1,
            "media_panel": { "sort": "name", "search": "clip", "expanded": [4, 9] }
        });
        let view = from_ui_state(&v1);
        assert_eq!(view.search, "clip");
        assert_eq!(view.sort, SortKey::Name);
        assert!(view.expanded.contains(&FolderId::from(9)));
        assert_eq!(view.filter, TypeFilter::All, "new fields take defaults");
        assert_eq!(view.columns, ViewSettings::default().columns);
        // Saving it again writes only the new name.
        let again = to_ui_state(&view, DialogTab::Basic);
        assert!(again.get("media_panel").is_none() && again.get("project_panel").is_some());
    }

    #[test]
    fn the_dialog_tab_defaults_when_missing_or_bad() {
        assert_eq!(dialog_tab_from(&json!({})), DialogTab::Basic);
        assert_eq!(
            dialog_tab_from(&json!({"comp_settings": {"tab": "3d"}})),
            DialogTab::Basic
        );
        assert_eq!(
            dialog_tab_from(&json!({"comp_settings": {"tab": "advanced"}})),
            DialogTab::Advanced
        );
    }

    #[test]
    fn the_new_name_wins_when_both_exist() {
        let both = json!({
            "media_panel": { "search": "old" },
            "project_panel": { "search": "new" }
        });
        assert_eq!(from_ui_state(&both).search, "new");
    }

    #[test]
    fn bad_fields_fall_back_one_by_one() {
        let bad = json!({
            "version": 99,
            "project_panel": {
                "sort": "sideways", "descending": "yes", "filter": "missing",
                "search": 5, "columns": ["size", "weight", 3], "expanded": [1, "x"],
                "future_field": true
            }
        });
        let v = from_ui_state(&bad);
        assert_eq!(v.sort, SortKey::Name, "unknown sort");
        assert!(!v.descending, "wrong type");
        assert_eq!(v.filter, TypeFilter::Missing, "valid fields still load");
        assert_eq!(v.search, "");
        assert_eq!(v.columns, [Column::Size].into());
        assert_eq!(v.expanded, [FolderId::from(1)].into());
        for junk in [
            json!(null),
            json!(7),
            json!([]),
            json!({"project_panel": 4}),
        ] {
            assert_eq!(from_ui_state(&junk), ViewSettings::default());
        }
    }
}
