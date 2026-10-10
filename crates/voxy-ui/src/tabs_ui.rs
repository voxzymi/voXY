//! The composition tab strip and the breadcrumb (design §7.1), drawn at the
//! top of the timeline panel.

use egui::{RichText, Ui};
use voxy_core::CompId;

use crate::state::EditorState;

/// Breadcrumb levels shown before the middle is folded into a "…" menu.
const MAX_SHOWN: usize = 4;

/// One slot of a breadcrumb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Crumb {
    /// Entry `index` of the path.
    Entry(usize),
    /// The hidden middle entries (`start..end`), listed in a menu.
    Hidden { start: usize, end: usize },
}

/// How a path of `len` entries is shown: all of them up to four, otherwise
/// the first, a "…" for the middle, and the last two.
pub fn crumb_plan(len: usize) -> Vec<Crumb> {
    if len <= MAX_SHOWN {
        return (0..len).map(Crumb::Entry).collect();
    }
    vec![
        Crumb::Entry(0),
        Crumb::Hidden {
            start: 1,
            end: len - 2,
        },
        Crumb::Entry(len - 2),
        Crumb::Entry(len - 1),
    ]
}

/// What the tab strip asked for this frame.
#[derive(Debug, Default, PartialEq, Eq)]
struct Request {
    switch: Option<usize>,
    close: Option<usize>,
    crumb: Option<usize>,
}

/// Draws the tabs and, when the active tab has navigated into precomps, the
/// breadcrumb. Does nothing when no tab is open.
pub fn show(ui: &mut Ui, state: &mut EditorState) {
    if state.workspace.tabs.is_empty() {
        return;
    }
    let doc = state.history.current().clone();
    let name = |c: CompId| {
        doc.comp(c)
            .map_or_else(|| "?".to_owned(), |c| c.name.clone())
    };
    let mut req = Request::default();
    ui.horizontal_wrapped(|ui| {
        for (i, tab) in state.workspace.tabs.iter().enumerate() {
            let active = state.workspace.active == Some(i);
            if ui
                .selectable_label(active, name(tab.comp()))
                .on_hover_text("Switch to this composition")
                .clicked()
            {
                req.switch = Some(i);
            }
            if ui
                .small_button("×")
                .on_hover_text("Close the tab (the composition stays in the project)")
                .clicked()
            {
                req.close = Some(i);
            }
            ui.add_space(6.0);
        }
    });
    if let Some(tab) = state
        .workspace
        .active
        .and_then(|i| state.workspace.tabs.get(i))
        .filter(|t| t.path.len() > 1)
    {
        let last = tab.path.len() - 1;
        ui.horizontal(|ui| {
            ui.weak("In:");
            for (n, crumb) in crumb_plan(tab.path.len()).into_iter().enumerate() {
                if n > 0 {
                    ui.weak("›");
                }
                match crumb {
                    Crumb::Entry(i) if i == last => {
                        ui.label(RichText::new(name(tab.path[i].comp)).strong());
                    }
                    Crumb::Entry(i) => {
                        if ui.link(name(tab.path[i].comp)).clicked() {
                            req.crumb = Some(i);
                        }
                    }
                    Crumb::Hidden { start, end } => {
                        ui.menu_button("…", |ui| {
                            for i in start..end {
                                if ui.button(name(tab.path[i].comp)).clicked() {
                                    req.crumb = Some(i);
                                    ui.close();
                                }
                            }
                        });
                    }
                }
            }
        });
    }
    // Apply after drawing: the workspace was borrowed above.
    if let Some(i) = req.close {
        state.close_tab(i);
    } else if let Some(i) = req.switch {
        state.switch_tab(i);
    } else if let Some(i) = req.crumb {
        state.go_to_crumb(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_paths_show_every_level() {
        for len in 1..=4 {
            let plan = crumb_plan(len);
            assert_eq!(
                plan,
                (0..len).map(Crumb::Entry).collect::<Vec<_>>(),
                "{len}"
            );
        }
    }

    #[test]
    fn long_paths_keep_the_first_and_last_two_and_fold_the_middle() {
        assert_eq!(
            crumb_plan(5),
            [
                Crumb::Entry(0),
                Crumb::Hidden { start: 1, end: 3 },
                Crumb::Entry(3),
                Crumb::Entry(4)
            ]
        );
        let plan = crumb_plan(9);
        assert_eq!(plan[1], Crumb::Hidden { start: 1, end: 7 });
        assert_eq!(plan.len(), 4);
    }

    #[test]
    fn every_entry_is_reachable_exactly_once() {
        for len in 1..20 {
            let mut seen = Vec::new();
            for c in crumb_plan(len) {
                match c {
                    Crumb::Entry(i) => seen.push(i),
                    Crumb::Hidden { start, end } => seen.extend(start..end),
                }
            }
            assert_eq!(seen, (0..len).collect::<Vec<_>>(), "{len}");
        }
    }
}
