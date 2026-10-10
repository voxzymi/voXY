//! Saving and restoring the dock layout (which panels sit where).
//!
//! The layout is kept in `preferences.toml` as one JSON string, never in a
//! project file. A saved layout is only used if it is sound: every panel
//! appears exactly once and the tree is well formed. Anything else (a file
//! from a build with other panels, a hand edit gone wrong) gives the default
//! layout, so a bad file can never leave the user without a panel or crash
//! the dock.

use egui_dock::{DockState, Node};
use serde_json::Value;

use crate::panels::{Tab, default_layout};

/// Fields `egui_dock` recomputes every frame. Left out of the saved text so
/// that resizing the window does not rewrite the preferences file.
const VOLATILE: [&str; 4] = ["rect", "viewport", "scroll", "collapsed_leaf_count"];

fn strip(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in VOLATILE {
                map.remove(key);
            }
            map.values_mut().for_each(strip);
        }
        Value::Array(items) => items.iter_mut().for_each(strip),
        _ => {}
    }
}

/// The layout as text, or `None` if it cannot be written (never expected).
pub fn to_text(dock: &DockState<Tab>) -> Option<String> {
    let mut value = serde_json::to_value(dock).ok()?;
    strip(&mut value);
    serde_json::to_string(&value).ok()
}

/// A saved layout, if it is readable and sound.
pub fn from_text(text: &str) -> Option<DockState<Tab>> {
    let dock: DockState<Tab> = match serde_json::from_str(text) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("ignoring unreadable dock layout: {e}");
            return None;
        }
    };
    if sound(&dock) {
        Some(dock)
    } else {
        tracing::warn!("ignoring a dock layout that is not well formed");
        None
    }
}

/// The saved layout, else the default.
pub fn restore(saved: Option<&str>) -> DockState<Tab> {
    saved.and_then(from_text).unwrap_or_else(default_layout)
}

/// Whether the layout is safe to show: one surface (no floating windows are
/// offered), a well-formed tree, and each panel exactly once.
pub fn sound(dock: &DockState<Tab>) -> bool {
    if dock.iter_surfaces().count() != 1 {
        return false;
    }
    let nodes: Vec<&Node<Tab>> = dock.main_surface().iter().collect();
    match nodes.first() {
        None | Some(Node::Empty) => return false,
        Some(_) => {}
    }
    for (i, node) in nodes.iter().enumerate() {
        match node {
            Node::Empty => {}
            Node::Leaf(leaf) => {
                if leaf.tabs.is_empty() || leaf.active.0 >= leaf.tabs.len() {
                    return false;
                }
            }
            Node::Horizontal(split) | Node::Vertical(split) => {
                let (a, b) = (i * 2 + 1, i * 2 + 2);
                let children_ok = nodes
                    .get(a)
                    .zip(nodes.get(b))
                    .is_some_and(|(x, y)| !matches!(x, Node::Empty) && !matches!(y, Node::Empty));
                if !children_ok
                    || split.fraction <= 0.0
                    || split.fraction >= 1.0
                    || split.fraction.is_nan()
                {
                    return false;
                }
            }
        }
        // Every node except the root hangs from a split.
        if i > 0 && !matches!(node, Node::Empty) {
            let parent = (i - 1) / 2;
            if !matches!(nodes[parent], Node::Horizontal(_) | Node::Vertical(_)) {
                return false;
            }
        }
    }
    let mut seen: Vec<Tab> = Vec::new();
    for (_, tab) in dock.iter_all_tabs() {
        if seen.contains(tab) {
            return false;
        }
        seen.push(*tab);
    }
    Tab::ALL.iter().all(|t| seen.contains(t)) && seen.len() == Tab::ALL.len()
}

#[cfg(test)]
mod tests {
    use egui_dock::NodeIndex;

    use super::*;

    fn node_of(dock: &DockState<Tab>, tab: Tab) -> Option<NodeIndex> {
        dock.find_tab(&tab).map(|path| path.node)
    }

    #[test]
    fn the_default_layout_is_sound_and_round_trips() {
        let dock = default_layout();
        assert!(sound(&dock));
        let text = to_text(&dock).unwrap();
        let back = from_text(&text).expect("sound and readable");
        assert_eq!(to_text(&back).unwrap(), text, "the same layout");
        assert!(sound(&back));
    }

    #[test]
    fn the_saved_text_has_no_per_frame_rectangles() {
        let text = to_text(&default_layout()).unwrap();
        for volatile in VOLATILE {
            assert!(
                !text.contains(&format!("\"{volatile}\"")),
                "{volatile} in {text}"
            );
        }
    }

    #[test]
    fn a_moved_panel_is_restored_where_it_was_put() {
        let mut dock = default_layout();
        // Move Properties next to the Graph Editor's tabs.
        let from = dock.find_tab(&Tab::Properties).unwrap();
        let tab = dock
            .main_surface_mut()
            .remove_tab((from.node, from.tab))
            .unwrap();
        let graph = dock.find_tab(&Tab::Graph).unwrap();
        dock.main_surface_mut().set_focused_node(graph.node);
        dock.main_surface_mut().push_to_focused_leaf(tab);
        assert!(sound(&dock), "the edit left a sound tree");
        let text = to_text(&dock).unwrap();
        assert_ne!(text, to_text(&default_layout()).unwrap());
        let back = from_text(&text).unwrap();
        assert_eq!(
            node_of(&back, Tab::Properties),
            node_of(&back, Tab::Graph),
            "Properties now shares the Graph Editor's node"
        );
    }

    #[test]
    fn old_panel_names_load_as_the_current_ones() {
        let text = to_text(&default_layout()).unwrap();
        assert!(text.contains("\"Project\""));
        let old = text.replace("\"Project\"", "\"Media\"");
        let dock = from_text(&old).expect("Media is the old name of Project");
        assert!(dock.find_tab(&Tab::Project).is_some());
    }

    #[test]
    fn unsound_layouts_fall_back_to_the_default() {
        let default_text = to_text(&default_layout()).unwrap();
        let cases: Vec<(&str, String)> = vec![
            ("not json", "this is not json".to_owned()),
            ("empty object", "{}".to_owned()),
            (
                "unknown panel",
                default_text.replace("\"Viewport\"", "\"Sculpting\""),
            ),
            (
                "duplicate panel",
                default_text.replace("\"Graph Editor\"", "\"Timeline\""),
            ),
        ];
        for (name, text) in cases {
            assert!(from_text(&text).is_none(), "{name}");
            let restored = restore(Some(&text));
            assert!(sound(&restored), "{name}: default used");
            assert_eq!(to_text(&restored).unwrap(), default_text, "{name}");
        }
        assert_eq!(to_text(&restore(None)).unwrap(), default_text);
    }

    #[test]
    fn a_missing_panel_is_refused() {
        let mut dock = default_layout();
        let at = dock.find_tab(&Tab::Project).unwrap();
        dock.main_surface_mut().remove_tab((at.node, at.tab));
        assert!(!sound(&dock));
        assert!(from_text(&serde_json::to_string(&dock).unwrap()).is_none());
    }

    /// The array of nodes inside the saved text of the default layout.
    fn nodes_mut(v: &mut Value) -> &mut Vec<Value> {
        fn find(v: &mut Value) -> Option<&mut Vec<Value>> {
            match v {
                Value::Object(map) => {
                    if map.contains_key("nodes") {
                        return map.get_mut("nodes").and_then(Value::as_array_mut);
                    }
                    map.values_mut().find_map(find)
                }
                Value::Array(items) => items.iter_mut().find_map(find),
                _ => None,
            }
        }
        find(v).expect("the saved text holds a node list")
    }

    #[test]
    fn broken_trees_are_refused_instead_of_reaching_the_dock() {
        let default_text = to_text(&default_layout()).unwrap();
        // A leaf whose active tab does not exist.
        let mut v: Value = serde_json::from_str(&default_text).unwrap();
        let leaf = nodes_mut(&mut v)
            .iter_mut()
            .find(|n| n.get("Leaf").is_some())
            .expect("a leaf");
        leaf["Leaf"]["active"] = Value::from(99);
        assert!(from_text(&v.to_string()).is_none(), "bad active index");
        // Splits with their children cut off.
        let mut v: Value = serde_json::from_str(&default_text).unwrap();
        nodes_mut(&mut v).truncate(2);
        assert!(from_text(&v.to_string()).is_none(), "missing children");
        // A split fraction outside (0, 1).
        let mut v: Value = serde_json::from_str(&default_text).unwrap();
        let split = nodes_mut(&mut v)
            .iter_mut()
            .find(|n| n.get("Vertical").is_some() || n.get("Horizontal").is_some())
            .expect("a split");
        let key = if split.get("Vertical").is_some() {
            "Vertical"
        } else {
            "Horizontal"
        };
        split[key]["fraction"] = Value::from(7.0);
        assert!(from_text(&v.to_string()).is_none(), "bad fraction");
    }
}
