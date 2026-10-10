//! Composition tabs and navigation (`docs/design-media-comps.md` §7).
//!
//! The workspace is **view state**: it never touches the document, and
//! nothing here runs a command. There is one tab per composition. A tab can
//! also *navigate* into a precomp (its breadcrumb grows) and back up again.
//!
//! The editor keeps the active tab's playhead, selection, timeline and graph
//! view as live fields of `EditorState`; the other tabs keep theirs in
//! [`TabView`]. `EditorState` swaps them when the active tab changes, so
//! this file only has to say *which* tab is active and what its path is.

use voxy_core::{CompId, Document, LayerId, Time};

use crate::graph_editor::GraphView;
use crate::timeline::TimelineView;

/// One step of a tab's breadcrumb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavStep {
    pub comp: CompId,
    /// The precomp layer this step was entered through (`None` for the first
    /// step: a composition opened from the Project panel).
    pub via_layer: Option<LayerId>,
}

/// What a tab remembers about how it was being looked at.
#[derive(Debug, Default)]
pub struct TabView {
    pub time: Time,
    pub selection: Option<LayerId>,
    pub timeline: TimelineView,
    pub graph: GraphView,
}

/// A tab: a breadcrumb path whose last step is the composition shown.
#[derive(Debug)]
pub struct CompTab {
    /// How the tab got here. Never empty; the last step is the composition
    /// shown.
    pub path: Vec<NavStep>,
    /// The views of the compositions above the current one, so going back up
    /// finds the playhead and selection where they were.
    pub trail: Vec<TabView>,
    /// The view while the tab is not the active one.
    pub saved: TabView,
}

impl CompTab {
    fn new(comp: CompId) -> CompTab {
        CompTab {
            path: vec![NavStep {
                comp,
                via_layer: None,
            }],
            trail: Vec::new(),
            saved: TabView::default(),
        }
    }

    /// The composition the tab shows.
    pub fn comp(&self) -> CompId {
        self.path.last().expect("a tab's path is never empty").comp
    }
}

/// A tab as saved in the project's `ui_state`: its breadcrumb and playhead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedTab {
    pub path: Vec<NavStep>,
    pub time: Time,
}

/// The open tabs and which one is active. No tabs means the empty-state
/// screen.
#[derive(Debug, Default)]
pub struct Workspace {
    pub tabs: Vec<CompTab>,
    pub active: Option<usize>,
}

impl Workspace {
    /// The tabs in their saved form. `live_time` is the active tab's
    /// playhead (the others keep theirs in [`TabView`]).
    pub fn to_saved(&self, live_time: Time) -> (Vec<SavedTab>, Option<usize>) {
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| SavedTab {
                path: t.path.clone(),
                time: if self.active == Some(i) {
                    live_time
                } else {
                    t.saved.time
                },
            })
            .collect();
        (tabs, self.active)
    }

    /// Rebuilds the workspace from saved tabs, dropping what the document no
    /// longer has (a composition that is gone, a duplicate).
    pub fn from_saved(saved: Vec<SavedTab>, active: Option<usize>, doc: &Document) -> Workspace {
        let mut w = Workspace::default();
        for t in saved {
            if t.path.is_empty() {
                continue;
            }
            let len = t.path.len();
            w.tabs.push(CompTab {
                path: t.path,
                trail: (1..len).map(|_| TabView::default()).collect(),
                saved: TabView {
                    time: t.time,
                    ..TabView::default()
                },
            });
        }
        w.active = active
            .filter(|a| *a < w.tabs.len())
            .or(if w.tabs.is_empty() { None } else { Some(0) });
        w.reconcile(doc);
        w
    }

    /// The index of the tab showing `comp`, if any (one tab per composition).
    pub fn tab_of(&self, comp: CompId) -> Option<usize> {
        self.tabs.iter().position(|t| t.comp() == comp)
    }

    pub fn active_comp(&self) -> Option<CompId> {
        self.active
            .and_then(|i| self.tabs.get(i))
            .map(CompTab::comp)
    }

    /// Opens `comp` in its own tab with a fresh breadcrumb: switches to the
    /// tab that already shows it, or appends one.
    pub fn open(&mut self, comp: CompId) {
        let i = self.tab_of(comp).unwrap_or_else(|| {
            self.tabs.push(CompTab::new(comp));
            self.tabs.len() - 1
        });
        self.active = Some(i);
    }

    /// Makes tab `i` the active one.
    pub fn activate(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.active = Some(i);
        }
    }

    /// Closes tab `i`. The tab to its left (or the new first one) becomes
    /// active when the active tab is closed. Never touches the document.
    pub fn close(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > i => Some(a - 1),
            Some(a) if a == i => Some(i.saturating_sub(1).min(self.tabs.len() - 1)),
            other => other,
        };
    }

    /// Enters `comp` through `via_layer` in the active tab: the breadcrumb
    /// grows. If `comp` already has its own tab, that tab is activated
    /// instead (one tab per composition) and `false` is returned.
    pub fn navigate_into(
        &mut self,
        comp: CompId,
        via_layer: LayerId,
        parent_view: TabView,
    ) -> bool {
        if self.tab_of(comp).is_some_and(|i| Some(i) != self.active) {
            self.active = self.tab_of(comp);
            return false;
        }
        let Some(tab) = self.active.and_then(|i| self.tabs.get_mut(i)) else {
            return false;
        };
        tab.trail.push(parent_view);
        tab.path.push(NavStep {
            comp,
            via_layer: Some(via_layer),
        });
        true
    }

    /// Goes up to breadcrumb entry `index` in the active tab. Returns the
    /// view of that entry if the tab moved, so the caller can restore it.
    ///
    /// If another tab already shows the composition reached, the active tab
    /// would duplicate it (one tab per composition), so it closes and that
    /// tab becomes active instead; `None` is returned and the caller loads
    /// the other tab's own view.
    pub fn go_to_crumb(&mut self, index: usize) -> Option<TabView> {
        let a = self.active?;
        let tab = self.tabs.get_mut(a)?;
        if index + 1 >= tab.path.len() {
            return None;
        }
        tab.path.truncate(index + 1);
        let mut restored = None;
        while tab.trail.len() > index {
            restored = tab.trail.pop();
        }
        let comp = tab.comp();
        if let Some(other) = self
            .tabs
            .iter()
            .enumerate()
            .position(|(i, t)| i != a && t.comp() == comp)
        {
            self.tabs.remove(a);
            self.active = Some(if other > a { other - 1 } else { other });
            return None;
        }
        restored
    }

    /// One step up in the active tab, if it has one.
    #[cfg(test)]
    pub fn go_up(&mut self) -> Option<TabView> {
        let len = self.active.and_then(|i| self.tabs.get(i))?.path.len();
        if len < 2 {
            return None;
        }
        self.go_to_crumb(len - 2)
    }

    /// Whether the active tab has a step to go up to.
    pub fn can_go_up(&self) -> bool {
        self.active
            .and_then(|i| self.tabs.get(i))
            .is_some_and(|t| t.path.len() > 1)
    }

    /// Brings the tabs in line with the document after it changed (a
    /// composition was removed, an undo brought one back). Tabs whose
    /// composition is gone close; a breadcrumb is cut at its first missing
    /// composition; two tabs that now show the same composition keep the
    /// first. Returns `true` if the active tab changed.
    pub fn reconcile(&mut self, doc: &Document) -> bool {
        let before = self.active_comp();
        let was_active = self.active;
        let mut keep_active: Option<usize> = None;
        let mut kept: Vec<CompTab> = Vec::new();
        for (i, mut tab) in std::mem::take(&mut self.tabs).into_iter().enumerate() {
            let first_missing = tab.path.iter().position(|s| doc.comp(s.comp).is_none());
            match first_missing {
                Some(0) => continue,
                Some(n) => {
                    tab.path.truncate(n);
                    tab.trail.truncate(n - 1);
                }
                None => {}
            }
            // A step entered through a layer that no longer exists is kept:
            // the composition is still there, only the way in is gone.
            let comp = tab.comp();
            if kept.iter().any(|t| t.comp() == comp) {
                continue;
            }
            if was_active == Some(i) {
                keep_active = Some(kept.len());
            }
            kept.push(tab);
        }
        self.tabs = kept;
        self.active = match (keep_active, self.tabs.is_empty()) {
            (_, true) => None,
            (Some(a), _) => Some(a),
            (None, false) => Some(0),
        };
        self.active_comp() != before
    }
}

#[cfg(test)]
mod tests {
    use voxy_core::command::{AddSourceLayer, CreateComposition};
    use voxy_core::{CompSettings, History, SourceRef};

    use super::*;

    fn comps(n: usize) -> (History, Vec<CompId>) {
        let mut h = History::new(Document::empty("w"));
        let ids = (0..n)
            .map(|i| {
                h.execute_reported(CreateComposition {
                    settings: CompSettings {
                        name: format!("C{i}"),
                        ..CompSettings::default()
                    },
                    folder: None,
                })
                .expect("test setup")
                .created_comps[0]
            })
            .collect();
        (h, ids)
    }

    #[test]
    fn opening_switches_to_an_existing_tab_and_never_duplicates() {
        let (_, c) = comps(3);
        let mut w = Workspace::default();
        assert_eq!(w.active_comp(), None);
        w.open(c[0]);
        w.open(c[1]);
        w.open(c[0]);
        assert_eq!(w.tabs.len(), 2, "one tab per composition");
        assert_eq!(w.active_comp(), Some(c[0]));
        assert_eq!(w.tab_of(c[1]), Some(1));
        w.open(c[2]);
        assert_eq!((w.tabs.len(), w.active), (3, Some(2)));
    }

    #[test]
    fn closing_a_tab_picks_a_neighbour_and_the_last_close_empties_the_workspace() {
        let (_, c) = comps(3);
        let mut w = Workspace::default();
        for &id in &c {
            w.open(id);
        }
        w.activate(1);
        w.close(1);
        assert_eq!(w.active_comp(), Some(c[0]), "the tab to the left");
        w.close(1);
        assert_eq!(
            w.active_comp(),
            Some(c[0]),
            "closing another keeps the active one"
        );
        w.activate(0);
        w.close(0);
        assert!(w.tabs.is_empty() && w.active.is_none());
        w.close(5); // out of range: nothing
        // Closing a tab before the active one keeps the same tab active.
        let mut w = Workspace::default();
        for &id in &c {
            w.open(id);
        }
        w.close(0);
        assert_eq!(w.active_comp(), Some(c[2]));
    }

    #[test]
    fn navigating_grows_the_breadcrumb_and_going_up_restores_the_parent_view() {
        let (_, c) = comps(3);
        let mut w = Workspace::default();
        w.open(c[0]);
        let parent_view = TabView {
            time: Time::from_secs(3),
            selection: Some(LayerId(7)),
            ..TabView::default()
        };
        assert!(w.navigate_into(c[1], LayerId(7), parent_view));
        assert!(w.navigate_into(c[2], LayerId(8), TabView::default()));
        let tab = &w.tabs[0];
        assert_eq!(tab.comp(), c[2]);
        assert_eq!(
            tab.path.iter().map(|s| s.comp).collect::<Vec<_>>(),
            [c[0], c[1], c[2]]
        );
        assert_eq!(tab.path[1].via_layer, Some(LayerId(7)));
        assert!(w.can_go_up());
        let back = w.go_up().expect("one step up");
        assert_eq!(w.active_comp(), Some(c[1]));
        assert_eq!(back.time, Time::ZERO, "the view C1 had when C2 was entered");
        let back = w.go_to_crumb(0).expect("to the root");
        assert_eq!(back.time, Time::from_secs(3), "the root's own view");
        assert_eq!(back.selection, Some(LayerId(7)));
        assert_eq!(w.active_comp(), Some(c[0]));
        assert!(!w.can_go_up());
        assert!(w.go_up().is_none() && w.go_to_crumb(0).is_none());
    }

    #[test]
    fn navigating_into_a_composition_that_has_its_own_tab_switches_to_it() {
        let (_, c) = comps(2);
        let mut w = Workspace::default();
        w.open(c[0]);
        w.open(c[1]);
        w.activate(0);
        assert!(!w.navigate_into(c[1], LayerId(1), TabView::default()));
        assert_eq!(w.active_comp(), Some(c[1]), "switched to its tab");
        assert_eq!(w.tabs[0].path.len(), 1, "the first tab did not move");
        assert_eq!(w.tabs.len(), 2);
    }

    #[test]
    fn going_up_into_a_composition_with_its_own_tab_merges_into_it() {
        let (_, c) = comps(2);
        let mut w = Workspace::default();
        w.open(c[0]);
        w.open(c[1]);
        w.activate(1);
        // Tab 1 (C1) navigates into... C0 has its own tab, so use a fresh
        // comp path built directly: tab 1 = [C0 via root? no] -> emulate.
        w.tabs[1].path.insert(
            0,
            NavStep {
                comp: c[0],
                via_layer: None,
            },
        );
        w.tabs[1].trail.push(TabView::default());
        w.tabs[1].path[1].via_layer = Some(LayerId(3));
        assert!(w.go_up().is_none(), "no view to restore: the tab merged");
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.active_comp(), Some(c[0]));
    }

    #[test]
    fn navigation_never_runs_a_command() {
        // The workspace has no access to a History at all; this documents
        // that a long random walk leaves a document and its revision alone.
        let (mut h, c) = comps(4);
        h.execute(AddSourceLayer {
            comp: c[0],
            source: SourceRef::Comp(c[1]),
            index: 0,
            in_point: Time::ZERO,
        })
        .expect("test setup");
        let rev = h.revision();
        let doc = h.current().clone();
        let mut w = Workspace::default();
        let mut seed = 12345u64;
        let mut next = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (seed >> 33) as usize
        };
        for _ in 0..500 {
            match next() % 7 {
                0 => w.open(c[next() % 4]),
                1 => {
                    let _ =
                        w.navigate_into(c[next() % 4], LayerId(next() as u64), TabView::default());
                }
                2 => {
                    let _ = w.go_up();
                }
                3 => {
                    let _ = w.go_to_crumb(next() % 3);
                }
                4 => w.close(next() % 5),
                5 => w.activate(next() % 5),
                _ => {
                    let _ = w.reconcile(&doc);
                }
            }
            assert!(
                w.tabs.iter().all(|t| !t.path.is_empty()),
                "paths are never empty"
            );
            match w.active {
                Some(a) => assert!(a < w.tabs.len()),
                None => assert!(w.tabs.is_empty()),
            }
            let mut shown: Vec<_> = w.tabs.iter().map(CompTab::comp).collect();
            shown.sort_unstable();
            let n = shown.len();
            shown.dedup();
            assert_eq!(shown.len(), n, "one tab per composition");
            assert!(w.tabs.iter().all(|t| t.trail.len() + 1 == t.path.len()));
        }
        assert_eq!(h.revision(), rev);
        assert!(
            std::sync::Arc::ptr_eq(&doc, h.current()),
            "the document is untouched"
        );
    }

    #[test]
    fn reconcile_closes_tabs_for_removed_compositions_and_cuts_breadcrumbs() {
        let (h, c) = comps(3);
        let mut w = Workspace::default();
        w.open(c[0]);
        w.open(c[1]);
        w.activate(0);
        assert!(w.navigate_into(c[2], LayerId(1), TabView::default()));
        // Remove C2 and C1.
        let mut doc = voxy_core::Document::clone(h.current());
        std::sync::Arc::make_mut(&mut doc.comps).remove(&c[2]);
        let changed = w.reconcile(&doc);
        assert_eq!(w.tabs[0].path.len(), 1, "cut at the missing composition");
        assert_eq!(w.active_comp(), Some(c[0]));
        assert!(changed, "the active tab now shows C0");
        std::sync::Arc::make_mut(&mut doc.comps).remove(&c[1]);
        let _ = w.reconcile(&doc);
        assert_eq!(w.tabs.len(), 1, "the tab of a removed composition closed");
        std::sync::Arc::make_mut(&mut doc.comps).remove(&c[0]);
        let _ = w.reconcile(&doc);
        assert!(w.tabs.is_empty() && w.active.is_none());
        assert!(!w.reconcile(&doc), "nothing to change");
    }

    #[test]
    fn reconcile_keeps_one_tab_per_composition() {
        let (h, c) = comps(2);
        let mut w = Workspace::default();
        w.open(c[0]);
        w.open(c[1]);
        // Make the first tab navigate to C1 behind the workspace's back, as
        // an undo that restored a precomp layer could.
        w.tabs[0].path.push(NavStep {
            comp: c[1],
            via_layer: Some(LayerId(1)),
        });
        w.tabs[0].trail.push(TabView::default());
        w.activate(1);
        let _ = w.reconcile(h.current());
        assert_eq!(w.tabs.len(), 1, "duplicates collapse to the first");
        assert!(w.active.is_some());
    }

    #[test]
    fn tabs_round_trip_through_their_saved_form_and_lose_what_is_gone() {
        let (h, c) = comps(3);
        let mut w = Workspace::default();
        w.open(c[0]);
        assert!(w.navigate_into(c[1], LayerId(4), TabView::default()));
        w.open(c[2]);
        w.tabs[0].saved.time = Time::from_secs(2);
        w.activate(0);
        let (saved, active) = w.to_saved(Time::from_secs(7));
        assert_eq!(active, Some(0));
        assert_eq!(
            saved[0].time,
            Time::from_secs(7),
            "the active tab's live time"
        );
        assert_eq!(saved[1].time, Time::ZERO);
        assert_eq!(saved[0].path.len(), 2);
        let back = Workspace::from_saved(saved.clone(), active, h.current());
        assert_eq!(back.tabs.len(), 2);
        assert_eq!(back.active_comp(), Some(c[1]), "the navigated-into comp");
        assert_eq!(back.tabs[0].path, w.tabs[0].path);
        assert_eq!(back.tabs[0].trail.len(), 1);
        // A composition that no longer exists is dropped.
        let mut doc = voxy_core::Document::clone(h.current());
        std::sync::Arc::make_mut(&mut doc.comps).remove(&c[2]);
        let back = Workspace::from_saved(saved, Some(1), &doc);
        assert_eq!(back.tabs.len(), 1);
        assert_eq!(back.active, Some(0), "the active index follows");
        // Nothing saved is an empty workspace.
        let none = Workspace::from_saved(Vec::new(), None, h.current());
        assert!(none.tabs.is_empty() && none.active.is_none());
    }

    #[test]
    fn reconcile_keeps_the_active_tab_when_nothing_changed() {
        let (h, c) = comps(2);
        let mut w = Workspace::default();
        w.open(c[0]);
        w.open(c[1]);
        assert!(!w.reconcile(h.current()));
        assert_eq!(w.active_comp(), Some(c[1]));
    }
}
