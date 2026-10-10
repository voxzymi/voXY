//! Names that changed when the Media panel became the Project panel
//! (`design-project-panel.md` §1), and the migration of saved UI state.
//!
//! Nothing shipped under the old names, so this is a defensive layer: a
//! layout, a shortcut map or a `ui_state` written by an earlier build still
//! loads, and anything unknown is dropped instead of failing.

use serde_json::{Map, Value};

use crate::comp_draft::Tab as DialogTab;
use voxy_core::{CompId, LayerId, Time};

use crate::project_view::{Column, SortKey, TypeFilter, ViewSettings};
use crate::workspace::{NavStep, SavedTab};

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
        "Effects & Presets",
    ]
    .into_iter()
    .find(|n| *n == saved)
}

/// The `ui_state` this build saves: the Project panel's settings, the last
/// Composition Settings tab, and the open composition tabs.
pub fn to_ui_state(
    view: &ViewSettings,
    dialog_tab: DialogTab,
    tabs: &[SavedTab],
    active: Option<usize>,
) -> Value {
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
    let tabs: Vec<Value> = tabs
        .iter()
        .map(|t| {
            let path: Vec<Value> = t
                .path
                .iter()
                .map(|s| {
                    let mut step = Map::new();
                    step.insert("comp".into(), s.comp.raw().into());
                    if let Some(l) = s.via_layer {
                        step.insert("via_layer".into(), l.raw().into());
                    }
                    Value::Object(step)
                })
                .collect();
            serde_json::json!({ "path": path, "time": t.time.flicks() })
        })
        .collect();
    map.insert("tabs".into(), Value::Array(tabs));
    map.insert(
        "active_tab".into(),
        active.map_or(Value::Null, |a| Value::from(a as u64)),
    );
    Value::Object(map)
}

/// The tabs saved in a `ui_state`, or `None` if it has no `tabs` key (a
/// project from before tabs: show the main composition). Entries that cannot
/// be read are dropped one by one; an empty list is a real answer (every tab
/// was closed).
pub fn tabs_from_ui_state(state: &Value) -> Option<(Vec<SavedTab>, Option<usize>)> {
    let list = state.get("tabs")?.as_array()?;
    let wanted = state
        .get("active_tab")
        .and_then(Value::as_u64)
        .map(|a| a as usize);
    let mut tabs = Vec::new();
    let mut active = None;
    for (index, entry) in list.iter().enumerate() {
        let path: Vec<NavStep> = entry
            .get("path")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .filter_map(|s| {
                        Some(NavStep {
                            comp: CompId::from(s.get("comp")?.as_u64()?),
                            via_layer: s
                                .get("via_layer")
                                .and_then(Value::as_u64)
                                .map(LayerId::from),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        if wanted == Some(index) {
            active = Some(tabs.len());
        }
        let time = Time::from_flicks(entry.get("time").and_then(Value::as_i64).unwrap_or(0));
        tabs.push(SavedTab { path, time });
    }
    Some((tabs, active))
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
        let state = to_ui_state(&view, DialogTab::Advanced, &[], None);
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
        let again = to_ui_state(&view, DialogTab::Basic, &[], None);
        assert!(again.get("media_panel").is_none() && again.get("project_panel").is_some());
    }

    #[test]
    fn tabs_are_saved_and_read_back_and_bad_entries_drop_one_by_one() {
        let saved = vec![
            SavedTab {
                path: vec![
                    NavStep {
                        comp: CompId::from(2),
                        via_layer: None,
                    },
                    NavStep {
                        comp: CompId::from(5),
                        via_layer: Some(LayerId::from(9)),
                    },
                ],
                time: Time::from_secs(3),
            },
            SavedTab {
                path: vec![NavStep {
                    comp: CompId::from(7),
                    via_layer: None,
                }],
                time: Time::ZERO,
            },
        ];
        let state = to_ui_state(&ViewSettings::default(), DialogTab::Basic, &saved, Some(1));
        assert_eq!(tabs_from_ui_state(&state), Some((saved.clone(), Some(1))));
        // No `tabs` key: an older file, so no opinion (the caller shows the
        // main composition). An empty list is an answer: every tab was closed.
        assert_eq!(tabs_from_ui_state(&json!({"version": 2})), None);
        assert_eq!(
            tabs_from_ui_state(&to_ui_state(
                &ViewSettings::default(),
                DialogTab::Basic,
                &[],
                None
            )),
            Some((Vec::new(), None))
        );
        // Bad entries are dropped alone and the active index follows.
        let messy = json!({
            "tabs": [
                {"path": []},
                "nonsense",
                {"path": [{"comp": 4}, {"comp": "x"}, {"via_layer": 1}], "time": "soon"},
                {"path": [{"comp": 8, "via_layer": 2}], "time": 705_600_000}
            ],
            "active_tab": 3
        });
        let (tabs, active) = tabs_from_ui_state(&messy).expect("has tabs");
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].path.len(), 1, "only the readable step of the first");
        assert_eq!(tabs[0].time, Time::ZERO, "a bad time is 0");
        assert_eq!(tabs[1].time, Time::from_secs(1));
        assert_eq!(active, Some(1), "the fourth entry is now the second");
        assert_eq!(tabs_from_ui_state(&json!({"tabs": 5})), None);
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
