//! The Keyboard Shortcuts dialog (Edit > Keyboard Shortcuts…).
//!
//! Every action in the table is listed with its shortcuts. Add captures the
//! next key press; a shortcut another action already uses (in a scope that
//! can be active at the same time) moves to this action, and the dialog says
//! so. Changes are saved to the preferences file at once, as only what differs
//! from the defaults.

use egui::{Align, Context, Event, Id, Key, KeyboardShortcut, Layout, Modal, Modifiers, Ui};

use crate::actions::{Action, Scope, shortcut_to_text};
use crate::state::EditorState;

/// The dialog's own state.
#[derive(Debug, Default, Clone)]
pub struct Editor {
    /// The action waiting for a key press.
    pub capturing: Option<Action>,
    pub filter: String,
    /// What the last change did, e.g. that it took a shortcut from another
    /// action.
    pub notice: Option<String>,
}

/// What a key press means while capturing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Captured {
    Cancel,
    Shortcut(KeyboardShortcut),
}

/// The first key press among `events`: Escape cancels, anything else becomes
/// a shortcut of its command, Alt and Shift modifiers. A key held with
/// Control on a Mac (where the command key is Cmd) is ignored.
pub fn captured(events: &[Event]) -> Option<Captured> {
    for event in events {
        if let Event::Key {
            key,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } = event
        {
            if *key == Key::Escape && !modifiers.any() {
                return Some(Captured::Cancel);
            }
            if modifiers.ctrl && !modifiers.command {
                continue;
            }
            let mods = Modifiers {
                alt: modifiers.alt,
                shift: modifiers.shift,
                ..if modifiers.command {
                    Modifiers::COMMAND
                } else {
                    Modifiers::NONE
                }
            };
            return Some(Captured::Shortcut(KeyboardShortcut::new(mods, *key)));
        }
    }
    None
}

/// Whether the row for `action` matches the filter text (label or id).
pub fn matches(action: Action, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    f.is_empty()
        || action.label().to_lowercase().contains(&f)
        || action.id().to_lowercase().contains(&f)
}

/// Draws the dialog if it is open.
pub fn show(ctx: &Context, state: &mut EditorState) {
    let Some(mut editor) = state.shortcut_editor.take() else {
        return;
    };
    // While waiting for a key, the key press belongs to the dialog, and an
    // Escape that cancels the capture must not also close the dialog.
    let was_capturing = editor.capturing.is_some();
    if let Some(action) = editor.capturing {
        let events = ctx.input(|i| i.events.clone());
        match captured(&events) {
            Some(Captured::Cancel) => editor.capturing = None,
            Some(Captured::Shortcut(s)) => {
                let taken = state.keymap.assign(action, s);
                state.save_shortcuts();
                editor.notice = Some(if taken.is_empty() {
                    format!("{}: {}", action.label(), ctx.format_shortcut(&s))
                } else {
                    let names: Vec<&str> = taken.iter().map(|a| a.label()).collect();
                    format!(
                        "{} now uses {}; it was taken from {}.",
                        action.label(),
                        ctx.format_shortcut(&s),
                        names.join(", ")
                    )
                });
                editor.capturing = None;
            }
            None => {}
        }
    }
    let mut close = false;
    let width = 620.0_f32.min((ctx.content_rect().width() - 32.0).max(320.0));
    let modal = Modal::new(Id::new("voxy-shortcuts")).show(ctx, |ui| {
        ui.set_width(width);
        ui.heading("Keyboard Shortcuts");
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut editor.filter);
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height((ctx.content_rect().height() - 220.0).clamp(120.0, 420.0))
            .show(ui, |ui| rows(ui, state, &mut editor));
        ui.add_space(4.0);
        if let Some(n) = &editor.notice {
            ui.weak(n);
        }
        if editor.capturing.is_some() {
            ui.weak("Press the new shortcut, or Esc to cancel.");
        }
        ui.horizontal(|ui| {
            if ui.button("Reset All").clicked() {
                state.keymap.reset_all();
                state.save_shortcuts();
                editor.notice = Some("Every shortcut is back to its default.".to_owned());
                editor.capturing = None;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Done").clicked() {
                    close = true;
                }
            });
        });
    });
    if modal.should_close() && !was_capturing {
        close = true;
    }
    if !close {
        state.shortcut_editor = Some(editor);
    }
}

/// Width of the action-name column, in points.
const NAME_WIDTH: f32 = 230.0;

fn rows(ui: &mut Ui, state: &mut EditorState, editor: &mut Editor) {
    for &action in Action::ALL {
        if !matches(action, &editor.filter) {
            continue;
        }
        let name = if action.def().scope == Scope::Project {
            format!("{} (Project panel)", action.label())
        } else {
            action.label().to_owned()
        };
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(NAME_WIDTH, ui.spacing().interact_size.y),
                Layout::left_to_right(Align::Center),
                |ui| ui.add(egui::Label::new(name).truncate()),
            );
            let list = state.keymap.bindings(action);
            if list.is_empty() {
                ui.weak("none");
            }
            for (i, s) in list.iter().enumerate() {
                let text = ui.ctx().format_shortcut(s);
                if ui
                    .small_button(format!("{text} ×"))
                    .on_hover_text(format!("Remove this shortcut ({})", shortcut_to_text(*s)))
                    .clicked()
                {
                    state.keymap.remove_binding(action, i);
                    state.save_shortcuts();
                }
            }
            let waiting = editor.capturing == Some(action);
            if ui
                .selectable_label(waiting, if waiting { "Press keys…" } else { "+ Add" })
                .clicked()
            {
                editor.capturing = if waiting { None } else { Some(action) };
            }
            ui.add_enabled_ui(!state.keymap.is_default(action), |ui| {
                if ui.small_button("Reset").clicked() {
                    state.keymap.reset(action);
                    state.save_shortcuts();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{Keymap, shortcut_from_text};
    use egui::{RawInput, Rect};

    fn key_event(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn shortcuts_round_trip_through_text() {
        for &a in Action::ALL {
            for s in a.def().defaults {
                let text = shortcut_to_text(*s);
                assert_eq!(shortcut_from_text(&text), Some(*s), "{a:?}: {text}");
            }
        }
        assert_eq!(
            shortcut_to_text(KeyboardShortcut::new(
                Modifiers::COMMAND.plus(Modifiers::SHIFT),
                Key::Z
            )),
            "Ctrl+Shift+Z"
        );
    }

    #[test]
    fn text_is_read_forgivingly_and_rejected_when_wrong() {
        let cmd_k = Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::K));
        for t in ["Ctrl+K", "ctrl + k", "Cmd+K", "Command+K", "Control+K"] {
            assert_eq!(shortcut_from_text(t), cmd_k, "{t}");
        }
        assert_eq!(
            shortcut_from_text("Delete"),
            Some(KeyboardShortcut::new(Modifiers::NONE, Key::Delete))
        );
        assert_eq!(
            shortcut_from_text("Alt+Option+Plus").map(|s| s.logical_key),
            Some(Key::Plus)
        );
        for bad in ["", "Ctrl+", "Hyper+K", "Ctrl+Nonsense", "+"] {
            let r = shortcut_from_text(bad);
            // "+" alone is the Plus key; the rest are not shortcuts.
            if bad == "+" {
                assert_eq!(r.map(|s| s.logical_key), Some(Key::Plus));
            } else {
                assert_eq!(r, None, "{bad:?}");
            }
        }
    }

    #[test]
    fn only_changes_are_saved_and_they_load_back() {
        let mut km = Keymap::default();
        assert!(km.to_saved().is_empty(), "defaults save nothing");
        let new = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::K);
        km.assign(Action::CompositionSettings, new);
        km.remove_binding(Action::FileQuit, 0);
        let saved = km.to_saved();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved["file.quit"], Vec::<String>::new(), "unbound saves []");
        let mut loaded = Keymap::default();
        assert_eq!(loaded.load_saved(&saved), 2);
        for a in [
            Action::CompositionSettings,
            Action::FileQuit,
            Action::FileSave,
        ] {
            assert_eq!(loaded.bindings(a), km.bindings(a), "{a:?}");
        }
        // Back to the default saves nothing again.
        km.reset(Action::CompositionSettings);
        km.reset(Action::FileQuit);
        assert!(km.to_saved().is_empty());
    }

    #[test]
    fn unreadable_saved_entries_are_dropped_alone() {
        let mut km = Keymap::default();
        let saved = [
            (
                "file.save".to_owned(),
                vec!["Ctrl+Alt+S".to_owned(), "Bogus+".to_owned()],
            ),
            ("gone.forever".to_owned(), vec!["Ctrl+Q".to_owned()]),
        ]
        .into_iter()
        .collect();
        assert_eq!(km.load_saved(&saved), 1, "the unknown action is skipped");
        assert_eq!(
            km.bindings(Action::FileSave),
            vec![shortcut_from_text("Ctrl+Alt+S").unwrap()],
            "the readable one is kept"
        );
    }

    #[test]
    fn assigning_a_used_shortcut_moves_it_and_never_leaves_a_conflict() {
        let mut km = Keymap::default();
        let ctrl_s = shortcut_from_text("Ctrl+S").unwrap();
        let taken = km.assign(Action::FileOpen, ctrl_s);
        assert_eq!(taken, vec![Action::FileSave]);
        assert_eq!(km.bindings(Action::FileSave), vec![]);
        assert!(km.bindings(Action::FileOpen).contains(&ctrl_s));
        assert!(km.conflicts().is_empty(), "{:?}", km.conflicts());
        // Assigning what an action already has changes nothing.
        assert_eq!(km.assign(Action::FileOpen, ctrl_s), vec![]);
        assert_eq!(
            km.bindings(Action::FileOpen)
                .iter()
                .filter(|s| **s == ctrl_s)
                .count(),
            1
        );
    }

    #[test]
    fn scopes_that_cannot_clash_do_not_steal() {
        // Delete is Layer Delete (editor scope) and Remove from Project
        // (project scope): both keep it.
        let km = Keymap::default();
        assert_ne!(km.bindings(Action::LayerDelete), vec![]);
        assert_ne!(km.bindings(Action::ProjectRemove), vec![]);
        let mut km = Keymap::default();
        let del = shortcut_from_text("Delete").unwrap();
        let taken = km.assign(Action::ProjectRename, del);
        assert_eq!(taken, vec![Action::ProjectRemove], "same scope: moved");
        assert!(
            km.bindings(Action::LayerDelete).contains(&del),
            "other scope: kept"
        );
    }

    #[test]
    fn capturing_reads_the_first_press() {
        assert_eq!(
            captured(&[key_event(Key::Escape, Modifiers::NONE)]),
            Some(Captured::Cancel)
        );
        assert_eq!(
            captured(&[
                Event::PointerGone,
                key_event(Key::K, Modifiers::COMMAND.plus(Modifiers::SHIFT)),
                key_event(Key::L, Modifiers::NONE),
            ]),
            Some(Captured::Shortcut(KeyboardShortcut::new(
                Modifiers::COMMAND.plus(Modifiers::SHIFT),
                Key::K
            )))
        );
        // Repeats and releases are not presses.
        let release = Event::Key {
            key: Key::A,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        assert_eq!(captured(&[release]), None);
        assert_eq!(captured(&[]), None);
        // Ctrl+Esc is a shortcut, bare Esc cancels.
        assert!(matches!(
            captured(&[key_event(Key::Escape, Modifiers::COMMAND)]),
            Some(Captured::Shortcut(_))
        ));
    }

    #[test]
    fn the_filter_matches_label_or_id_without_regard_to_case() {
        assert!(matches(Action::FileSave, ""));
        assert!(matches(Action::FileSave, "SAVE"));
        assert!(matches(Action::FileSaveAs, "file.save_as"));
        assert!(!matches(Action::FileQuit, "save"));
    }

    /// One egui pass over the whole screen with `events`, then the dialog.
    fn pass(ctx: &Context, state: &mut EditorState, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 700.0),
            )),
            events,
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |_| {});
        out.textures_delta.clear();
        show(ctx, state);
    }

    #[test]
    fn the_dialog_draws_captures_saves_and_closes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.toml");
        let mut state = EditorState::new(voxy_core::Document::empty("t"));
        state.prefs_path = Some(path.clone());
        let ctx = Context::default();
        state.perform(Action::EditKeyboardShortcuts);
        assert!(state.shortcut_editor.is_some());
        for _ in 0..3 {
            pass(&ctx, &mut state, vec![]);
        }
        // Start capturing for Composition Settings and press Ctrl+Alt+J.
        state.shortcut_editor.as_mut().unwrap().capturing = Some(Action::CompositionSettings);
        let press = key_event(Key::J, Modifiers::COMMAND.plus(Modifiers::ALT));
        pass(&ctx, &mut state, vec![press]);
        let want = shortcut_from_text("Ctrl+Alt+J").unwrap();
        assert!(
            state
                .keymap
                .bindings(Action::CompositionSettings)
                .contains(&want)
        );
        assert!(state.shortcut_editor.as_ref().unwrap().capturing.is_none());
        let saved = crate::prefs::Preferences::load_from(&path);
        assert!(
            saved.shortcuts["composition.settings"]
                .iter()
                .any(|t| t == "Ctrl+Alt+J")
        );
        // Escape while capturing only cancels the capture.
        state.shortcut_editor.as_mut().unwrap().capturing = Some(Action::FileSave);
        pass(
            &ctx,
            &mut state,
            vec![key_event(Key::Escape, Modifiers::NONE)],
        );
        let editor = state.shortcut_editor.as_ref().expect("still open");
        assert!(editor.capturing.is_none());
        assert!(state.keymap.is_default(Action::FileSave));
    }
}
