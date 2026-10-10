//! The action table: every command reachable from a menu or a shortcut.
//!
//! Menus, the shortcut dispatcher, tooltips and (later) the shortcut editor
//! and the Tip of the Day validator all read this one table, so a command
//! cannot be in a menu and missing from the table. Each action says what it
//! needs ([`Requires`]); an action whose need is not met is disabled with a
//! tooltip and never runs, which is how the app stays panic-free with no
//! composition open.
//!
//! Ids are stable strings (`file.new_composition`). Old ids keep working as
//! `aliases`, so saved shortcut maps survive a rename.

use std::collections::BTreeMap;

use egui::{InputState, Key, KeyboardShortcut, Modifiers};

/// What an action needs before it can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requires {
    Nothing,
    /// An open (active) composition.
    Composition,
    /// A selected layer in the active composition.
    Layer,
    /// At least one selected item in the Project panel.
    ProjectSelection,
    /// At least one selected keyframe that starts a segment (any keyframe
    /// but the last of its property).
    Keyframe,
}

/// Tooltip for an action that needs a composition.
pub const NEEDS_COMP: &str = "Create or open a composition first.";

/// Where a shortcut applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Everywhere.
    Global,
    /// Everywhere except while the Project panel has focus.
    Editor,
    /// Only while the Project panel has focus.
    Project,
}

impl Scope {
    /// Whether two scopes can be active at the same time, so a shortcut
    /// bound in both is ambiguous.
    fn overlaps(self, other: Scope) -> bool {
        self == other || self == Scope::Global || other == Scope::Global
    }
}

/// The application state an action's availability depends on.
#[derive(Debug, Clone, Copy, Default)]
pub struct ActionContext {
    pub has_comp: bool,
    pub has_layer: bool,
    pub project_selection: usize,
    /// Selected keyframes that start a segment.
    pub selected_segments: usize,
}

/// A command with a stable id, a label and default shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    FileNewProject,
    FileNewComposition,
    FileNewCompositionFromFootage,
    FileOpen,
    FileSave,
    FileSaveAs,
    FileImport,
    FileQuit,
    CompositionSettings,
    CompositionGoToParent,
    EditUndo,
    EditRedo,
    EditKeyboardShortcuts,
    SelectionClear,
    LayerNewSolid,
    LayerImportImage,
    LayerImportVideo,
    LayerDelete,
    TransportPlayPause,
    TransportStart,
    TransportStepForward,
    TransportStepBack,
    ProjectNewFolder,
    ProjectRename,
    ProjectRemove,
    ProjectDuplicate,
    ProjectSelectAll,
    RevealSourceInProject,
    RevealInComposition,
    RevealInFileManager,
    EaseCopy,
    EasePaste,
    EaseApplySpring,
    EaseApplyBounce,
    EaseApplyOscillation,
    EaseApplySteps,
    EaseApplySteppedRamp,
    EffectApplyLast,
    EffectCopy,
    EffectPaste,
    EffectDuplicate,
    EffectRemove,
    EffectRemoveAll,
    EffectToggle,
    EffectMoveUp,
    EffectMoveDown,
    EffectReset,
    EffectSolo,
    EffectSplit,
}

/// Static description of an [`Action`].
#[allow(
    dead_code,
    reason = "read by saved shortcuts and the shortcut editor (Preferences, P-D); covered by tests"
)]
#[derive(Debug, Clone, Copy)]
pub struct ActionDef {
    pub id: &'static str,
    /// Older ids that still resolve to this action.
    pub aliases: &'static [&'static str],
    /// Menu label.
    pub label: &'static str,
    pub requires: Requires,
    pub scope: Scope,
    pub defaults: &'static [KeyboardShortcut],
    /// Runs even while a text field has focus (file and app commands).
    pub fires_while_typing: bool,
    /// One firing per key repeat (frame stepping).
    pub repeats: bool,
}

const fn key(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::NONE, key)
}
const fn cmd(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND, key)
}
const fn cmd_shift(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), key)
}
const fn cmd_alt(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), key)
}
const fn cmd_alt_shift(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(
        Modifiers::COMMAND
            .plus(Modifiers::ALT)
            .plus(Modifiers::SHIFT),
        key,
    )
}

const fn def(
    id: &'static str,
    label: &'static str,
    requires: Requires,
    scope: Scope,
    defaults: &'static [KeyboardShortcut],
) -> ActionDef {
    ActionDef {
        id,
        aliases: &[],
        label,
        requires,
        scope,
        defaults,
        fires_while_typing: false,
        repeats: false,
    }
}

impl Action {
    pub const ALL: &'static [Action] = &[
        Action::FileNewProject,
        Action::FileNewComposition,
        Action::FileNewCompositionFromFootage,
        Action::FileOpen,
        Action::FileSave,
        Action::FileSaveAs,
        Action::FileImport,
        Action::FileQuit,
        Action::CompositionSettings,
        Action::CompositionGoToParent,
        Action::EditUndo,
        Action::EditRedo,
        Action::EditKeyboardShortcuts,
        Action::SelectionClear,
        Action::LayerNewSolid,
        Action::LayerImportImage,
        Action::LayerImportVideo,
        Action::LayerDelete,
        Action::TransportPlayPause,
        Action::TransportStart,
        Action::TransportStepForward,
        Action::TransportStepBack,
        Action::ProjectNewFolder,
        Action::ProjectRename,
        Action::ProjectRemove,
        Action::ProjectDuplicate,
        Action::ProjectSelectAll,
        Action::RevealSourceInProject,
        Action::RevealInComposition,
        Action::RevealInFileManager,
        Action::EaseCopy,
        Action::EasePaste,
        Action::EaseApplySpring,
        Action::EaseApplyBounce,
        Action::EaseApplyOscillation,
        Action::EaseApplySteps,
        Action::EaseApplySteppedRamp,
        Action::EffectApplyLast,
        Action::EffectCopy,
        Action::EffectPaste,
        Action::EffectDuplicate,
        Action::EffectRemove,
        Action::EffectRemoveAll,
        Action::EffectToggle,
        Action::EffectMoveUp,
        Action::EffectMoveDown,
        Action::EffectReset,
        Action::EffectSolo,
        Action::EffectSplit,
    ];

    pub fn def(self) -> ActionDef {
        use Requires::{Composition, Keyframe, Layer, Nothing, ProjectSelection};
        use Scope::{Editor, Global, Project};
        match self {
            // New Composition takes Ctrl/Cmd+N and New Project moves to
            // Ctrl/Cmd+Alt+N, as in After Effects.
            Action::FileNewProject => ActionDef {
                fires_while_typing: true,
                ..def(
                    "file.new_project",
                    "New Project",
                    Nothing,
                    Global,
                    const { &[cmd_alt(Key::N)] },
                )
            },
            Action::FileNewComposition => ActionDef {
                aliases: &[
                    "project_panel.new_composition",
                    "media_panel.new_composition",
                ],
                fires_while_typing: true,
                ..def(
                    "file.new_composition",
                    "Composition",
                    Nothing,
                    Global,
                    const { &[cmd(Key::N)] },
                )
            },
            Action::FileNewCompositionFromFootage => def(
                "file.new_composition_from_footage",
                "Composition From Footage…",
                Nothing,
                Global,
                &[],
            ),
            Action::FileOpen => ActionDef {
                fires_while_typing: true,
                ..def(
                    "file.open",
                    "Open…",
                    Nothing,
                    Global,
                    const { &[cmd(Key::O)] },
                )
            },
            Action::FileSave => ActionDef {
                fires_while_typing: true,
                ..def(
                    "file.save",
                    "Save",
                    Nothing,
                    Global,
                    const { &[cmd(Key::S)] },
                )
            },
            Action::FileSaveAs => ActionDef {
                fires_while_typing: true,
                ..def(
                    "file.save_as",
                    "Save As…",
                    Nothing,
                    Global,
                    const { &[cmd_shift(Key::S)] },
                )
            },
            Action::FileImport => ActionDef {
                aliases: &["project_panel.import", "media_panel.import", "media.import"],
                fires_while_typing: true,
                ..def(
                    "file.import",
                    "Import…",
                    Nothing,
                    Global,
                    const { &[cmd(Key::I)] },
                )
            },
            Action::FileQuit => ActionDef {
                fires_while_typing: true,
                ..def(
                    "file.quit",
                    "Quit",
                    Nothing,
                    Global,
                    const { &[cmd(Key::Q)] },
                )
            },
            // Alt+Up conflicts with the window manager on some Linux desktops.
            Action::CompositionGoToParent => def(
                "composition.go_to_parent",
                "Go to Parent Composition",
                Composition,
                Global,
                const { &[cmd_shift(Key::ArrowUp)] },
            ),
            Action::CompositionSettings => ActionDef {
                fires_while_typing: true,
                ..def(
                    "composition.settings",
                    "Composition Settings…",
                    Composition,
                    Global,
                    const { &[cmd(Key::K)] },
                )
            },
            Action::EditUndo => def(
                "edit.undo",
                "Undo",
                Nothing,
                Global,
                const { &[cmd(Key::Z)] },
            ),
            Action::EditRedo => def(
                "edit.redo",
                "Redo",
                Nothing,
                Global,
                const { &[cmd_shift(Key::Z), cmd(Key::Y)] },
            ),
            Action::EditKeyboardShortcuts => def(
                "edit.keyboard_shortcuts",
                "Keyboard Shortcuts…",
                Nothing,
                Global,
                &[],
            ),
            Action::SelectionClear => def(
                "selection.clear",
                "Deselect",
                Nothing,
                Global,
                const { &[key(Key::Escape)] },
            ),
            Action::LayerNewSolid => def("layer.new_solid", "New Solid", Composition, Global, &[]),
            Action::LayerImportImage => def(
                "layer.import_image",
                "Import Image…",
                Composition,
                Global,
                &[],
            ),
            Action::LayerImportVideo => def(
                "layer.import_video",
                "Import Video…",
                Composition,
                Global,
                &[],
            ),
            Action::LayerDelete => def(
                "layer.delete",
                "Delete",
                Layer,
                Editor,
                const { &[key(Key::Delete), key(Key::Backspace)] },
            ),
            Action::TransportPlayPause => def(
                "transport.play_pause",
                "Play/Pause",
                Composition,
                Global,
                const { &[key(Key::Space)] },
            ),
            Action::TransportStart => def(
                "transport.start",
                "Go to Start",
                Composition,
                Global,
                const { &[key(Key::Home)] },
            ),
            Action::TransportStepForward => ActionDef {
                repeats: true,
                ..def(
                    "transport.step_forward",
                    "Next Frame",
                    Composition,
                    Global,
                    const { &[key(Key::ArrowRight)] },
                )
            },
            Action::TransportStepBack => ActionDef {
                repeats: true,
                ..def(
                    "transport.step_back",
                    "Previous Frame",
                    Composition,
                    Global,
                    const { &[key(Key::ArrowLeft)] },
                )
            },
            Action::ProjectNewFolder => ActionDef {
                aliases: &["media_panel.new_folder", "media.new_folder"],
                ..def(
                    "project_panel.new_folder",
                    "Folder",
                    Nothing,
                    Global,
                    const { &[cmd_alt_shift(Key::N)] },
                )
            },
            Action::ProjectRename => ActionDef {
                aliases: &["media_panel.rename", "media.rename"],
                ..def(
                    "project_panel.rename",
                    "Rename",
                    ProjectSelection,
                    Project,
                    const { &[key(Key::Enter)] },
                )
            },
            Action::ProjectRemove => ActionDef {
                aliases: &["media_panel.remove", "media.remove"],
                ..def(
                    "project_panel.remove",
                    "Remove from Project",
                    ProjectSelection,
                    Project,
                    const { &[key(Key::Delete), key(Key::Backspace)] },
                )
            },
            Action::ProjectDuplicate => ActionDef {
                aliases: &["media_panel.duplicate", "media.duplicate"],
                ..def(
                    "project_panel.duplicate",
                    "Duplicate",
                    ProjectSelection,
                    Project,
                    const { &[cmd(Key::D)] },
                )
            },
            Action::ProjectSelectAll => ActionDef {
                aliases: &["media_panel.select_all", "media.select_all"],
                ..def(
                    "project_panel.select_all",
                    "Select All",
                    Nothing,
                    Project,
                    const { &[cmd(Key::A)] },
                )
            },
            Action::RevealSourceInProject => ActionDef {
                aliases: &["reveal.source_in_media"],
                ..def(
                    "reveal.source_in_project",
                    "Reveal Layer Source in Project",
                    Layer,
                    Global,
                    &[],
                )
            },
            Action::RevealInComposition => def(
                "reveal.in_composition",
                "Reveal in Composition",
                ProjectSelection,
                Global,
                &[],
            ),
            Action::RevealInFileManager => def(
                "reveal.in_file_manager",
                "Reveal in File Manager",
                ProjectSelection,
                Global,
                &[],
            ),
            // Keyframe easing. No default shortcuts: they exist so the
            // shortcut editor can bind them.
            Action::EaseCopy => def("ease.copy", "Copy Easing", Keyframe, Editor, &[]),
            Action::EasePaste => def("ease.paste", "Paste Easing", Keyframe, Editor, &[]),
            Action::EaseApplySpring => {
                def("ease.apply_spring", "Apply Spring", Keyframe, Editor, &[])
            }
            Action::EaseApplyBounce => {
                def("ease.apply_bounce", "Apply Bounce", Keyframe, Editor, &[])
            }
            Action::EaseApplyOscillation => def(
                "ease.apply_oscillation",
                "Apply Oscillation",
                Keyframe,
                Editor,
                &[],
            ),
            Action::EaseApplySteps => def("ease.apply_steps", "Apply Steps", Keyframe, Editor, &[]),
            Action::EaseApplySteppedRamp => def(
                "ease.apply_stepped_ramp",
                "Apply Stepped Ramp",
                Keyframe,
                Editor,
                &[],
            ),
            // Effects (Milestone E-C). No default shortcuts: they exist so
            // the shortcut editor can bind them.
            Action::EffectApplyLast => {
                def("effect.apply_last", "Apply Last Effect", Layer, Editor, &[])
            }
            Action::EffectCopy => def("effect.copy", "Copy Effect", Layer, Editor, &[]),
            Action::EffectPaste => def("effect.paste", "Paste Effect", Layer, Editor, &[]),
            Action::EffectDuplicate => {
                def("effect.duplicate", "Duplicate Effect", Layer, Editor, &[])
            }
            Action::EffectRemove => def("effect.remove", "Remove Effect", Layer, Editor, &[]),
            Action::EffectRemoveAll => def(
                "effect.remove_all",
                "Remove All Effects",
                Layer,
                Editor,
                &[],
            ),
            Action::EffectToggle => {
                def("effect.toggle", "Turn Effect On or Off", Layer, Editor, &[])
            }
            Action::EffectMoveUp => def("effect.move_up", "Move Effect Up", Layer, Editor, &[]),
            Action::EffectMoveDown => {
                def("effect.move_down", "Move Effect Down", Layer, Editor, &[])
            }
            Action::EffectReset => def("effect.reset", "Reset Effect", Layer, Editor, &[]),
            // Viewport only: neither is an edit, so neither is undone.
            Action::EffectSolo => def("effect.solo", "Solo Effect", Layer, Editor, &[]),
            Action::EffectSplit => def(
                "effect.before_after",
                "Before/After Split",
                Layer,
                Editor,
                &[],
            ),
        }
    }

    #[allow(
        dead_code,
        reason = "read by saved shortcuts and the shortcut editor (Preferences, P-D); covered by tests"
    )]
    pub fn id(self) -> &'static str {
        self.def().id
    }

    pub fn label(self) -> &'static str {
        self.def().label
    }

    /// Looks an action up by id or by one of its aliases.
    #[allow(
        dead_code,
        reason = "read by saved shortcuts and the shortcut editor (Preferences, P-D); covered by tests"
    )]
    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| {
            let d = a.def();
            d.id == id || d.aliases.contains(&id)
        })
    }

    /// `Ok` if the action can run now, otherwise the tooltip saying why not.
    pub fn availability(self, cx: &ActionContext) -> Result<(), &'static str> {
        match self.def().requires {
            Requires::Nothing => Ok(()),
            Requires::Composition if cx.has_comp => Ok(()),
            Requires::Composition => Err(NEEDS_COMP),
            Requires::Layer if !cx.has_comp => Err(NEEDS_COMP),
            Requires::Layer if cx.has_layer => Ok(()),
            Requires::Layer => Err("Select a layer first."),
            Requires::ProjectSelection if cx.project_selection > 0 => Ok(()),
            Requires::ProjectSelection => Err("Select an item in the Project panel first."),
            Requires::Keyframe if cx.selected_segments > 0 => Ok(()),
            Requires::Keyframe => {
                Err("Select a keyframe that is followed by another keyframe first.")
            }
        }
    }
}

/// The shortcut as saved in the preferences file: modifiers then the key,
/// joined by `+` (`Ctrl+Shift+Z`, `Delete`). `Ctrl` is the platform's command
/// key: Ctrl on Windows and Linux, Cmd on macOS.
pub fn shortcut_to_text(s: KeyboardShortcut) -> String {
    let m = s.modifiers;
    let mut out = String::new();
    for (on, name) in [(m.command, "Ctrl"), (m.alt, "Alt"), (m.shift, "Shift")] {
        if on {
            out.push_str(name);
            out.push('+');
        }
    }
    out.push_str(s.logical_key.name());
    out
}

/// Reads [`shortcut_to_text`] back. Modifier names are case-insensitive and
/// `Cmd`, `Command`, `Control` also mean the command key. Anything else
/// (an unknown key or modifier, no key) is `None`.
pub fn shortcut_from_text(text: &str) -> Option<KeyboardShortcut> {
    let mut mods = Modifiers::NONE;
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    // A saved "+" key is spelled "Plus", but accept "Ctrl++" too.
    if text.trim_end().ends_with("++") || text.trim() == "+" {
        parts.retain(|p| !p.is_empty());
        parts.push("Plus");
    }
    let key_name = parts.pop()?;
    for part in parts {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" | "command" | "control" => mods = mods.plus(Modifiers::COMMAND),
            "alt" | "option" => mods = mods.plus(Modifiers::ALT),
            "shift" => mods = mods.plus(Modifiers::SHIFT),
            _ => return None,
        }
    }
    Some(KeyboardShortcut::new(mods, Key::from_name(key_name)?))
}

/// Two actions bound to the same shortcut in scopes that can be active at
/// once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub shortcut: KeyboardShortcut,
    pub actions: Vec<Action>,
}

/// The shortcut bindings in effect: the defaults from the table plus the
/// user's overrides.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    overrides: BTreeMap<Action, Vec<KeyboardShortcut>>,
}

impl Keymap {
    pub fn bindings(&self, action: Action) -> Vec<KeyboardShortcut> {
        self.overrides
            .get(&action)
            .cloned()
            .unwrap_or_else(|| action.def().defaults.to_vec())
    }

    /// Rebinds an action (an empty list unbinds it).
    pub fn set(&mut self, action: Action, shortcuts: Vec<KeyboardShortcut>) {
        self.overrides.insert(action, shortcuts);
    }

    pub fn reset(&mut self, action: Action) {
        self.overrides.remove(&action);
    }

    pub fn reset_all(&mut self) {
        self.overrides.clear();
    }

    pub fn is_default(&self, action: Action) -> bool {
        !self.overrides.contains_key(&action)
    }

    /// Adds `shortcut` to `action`. Another action that uses it in a scope
    /// that can be active at the same time loses it (so there are never two
    /// meanings for one key); those actions are returned so the editor can
    /// say so. Adding a shortcut the action already has changes nothing.
    pub fn assign(&mut self, action: Action, shortcut: KeyboardShortcut) -> Vec<Action> {
        let mut taken_from = Vec::new();
        for &other in Action::ALL {
            if other == action || !other.def().scope.overlaps(action.def().scope) {
                continue;
            }
            let mut list = self.bindings(other);
            if let Some(i) = list.iter().position(|s| *s == shortcut) {
                list.remove(i);
                self.set(other, list);
                taken_from.push(other);
            }
        }
        let mut mine = self.bindings(action);
        if !mine.contains(&shortcut) {
            mine.push(shortcut);
            self.set(action, mine);
        }
        taken_from
    }

    /// Removes the shortcut at `index` of `action`'s list (an empty list
    /// unbinds the action).
    pub fn remove_binding(&mut self, action: Action, index: usize) {
        let mut list = self.bindings(action);
        if index < list.len() {
            list.remove(index);
            self.set(action, list);
        }
    }

    /// Applies saved bindings keyed by action id. Ids are resolved through
    /// the aliases, so a map saved under an old name still loads; unknown
    /// ids are dropped. Returns how many were applied.
    #[allow(
        dead_code,
        reason = "read by saved shortcuts and the shortcut editor (Preferences, P-D); covered by tests"
    )]
    pub fn apply_saved<'a>(
        &mut self,
        saved: impl IntoIterator<Item = (&'a str, Vec<KeyboardShortcut>)>,
    ) -> usize {
        let mut applied = 0;
        for (id, shortcuts) in saved {
            if let Some(action) = Action::from_id(id) {
                self.set(action, shortcuts);
                applied += 1;
            }
        }
        applied
    }

    /// What differs from the defaults, by action id, ready to save. An
    /// action the user unbound saves an empty list.
    pub fn to_saved(&self) -> BTreeMap<String, Vec<String>> {
        self.overrides
            .iter()
            .filter(|(a, shortcuts)| shortcuts.as_slice() != a.def().defaults)
            .map(|(a, shortcuts)| {
                (
                    a.id().to_owned(),
                    shortcuts.iter().copied().map(shortcut_to_text).collect(),
                )
            })
            .collect()
    }

    /// Loads saved overrides ([`Keymap::to_saved`]): ids resolve through
    /// aliases, unknown ids are dropped, and a shortcut that cannot be read
    /// is dropped alone (the rest of that action's list is kept). Returns
    /// how many actions were rebound.
    pub fn load_saved(&mut self, saved: &BTreeMap<String, Vec<String>>) -> usize {
        let parsed: Vec<(&str, Vec<KeyboardShortcut>)> = saved
            .iter()
            .map(|(id, list)| {
                (
                    id.as_str(),
                    list.iter()
                        .filter_map(|t| {
                            let s = shortcut_from_text(t);
                            if s.is_none() {
                                tracing::warn!("ignoring unreadable shortcut {t:?} for {id}");
                            }
                            s
                        })
                        .collect(),
                )
            })
            .collect();
        self.apply_saved(parsed)
    }

    /// Shortcuts bound twice in scopes that overlap.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut by_shortcut: BTreeMap<String, (KeyboardShortcut, Vec<Action>)> = BTreeMap::new();
        for &a in Action::ALL {
            for s in self.bindings(a) {
                by_shortcut
                    .entry(format!("{s:?}"))
                    .or_insert_with(|| (s, Vec::new()))
                    .1
                    .push(a);
            }
        }
        let mut out = Vec::new();
        for (shortcut, actions) in by_shortcut.into_values() {
            let clash = actions.iter().enumerate().any(|(i, a)| {
                actions[i + 1..]
                    .iter()
                    .any(|b| a.def().scope.overlaps(b.def().scope))
            });
            if clash {
                out.push(Conflict { shortcut, actions });
            }
        }
        out
    }

    /// Consumes the shortcuts pressed this frame and returns the actions they
    /// trigger, in press order. `project_focused` selects the Project panel
    /// scope; `typing` suppresses everything except file and app commands.
    ///
    /// More specific shortcuts (more modifiers) are matched first, so
    /// Ctrl+Alt+N is never taken by Ctrl+N.
    pub fn dispatch(
        &self,
        input: &mut InputState,
        project_focused: bool,
        typing: bool,
    ) -> Vec<Action> {
        let mut candidates: Vec<(Action, KeyboardShortcut)> = Action::ALL
            .iter()
            .flat_map(|&a| self.bindings(a).into_iter().map(move |s| (a, s)))
            .filter(|(a, _)| {
                let d = a.def();
                let in_scope = match d.scope {
                    Scope::Global => true,
                    Scope::Editor => !project_focused,
                    Scope::Project => project_focused,
                };
                in_scope && (!typing || d.fires_while_typing)
            })
            .collect();
        candidates.sort_by_key(|(_, s)| std::cmp::Reverse(specificity(s.modifiers)));
        let mut fired = Vec::new();
        for (action, shortcut) in candidates {
            if action.def().repeats {
                while input.consume_shortcut(&shortcut) {
                    fired.push(action);
                }
            } else if input.consume_shortcut(&shortcut) {
                fired.push(action);
            }
        }
        fired
    }
}

fn specificity(m: Modifiers) -> u8 {
    u8::from(m.alt) + u8::from(m.shift) + u8::from(m.command || m.ctrl || m.mac_cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_and_aliases_are_unique() {
        let mut seen = HashSet::new();
        for &a in Action::ALL {
            let d = a.def();
            assert!(seen.insert(d.id), "duplicate id {}", d.id);
            for alias in d.aliases {
                assert!(seen.insert(alias), "alias {alias} clashes with another id");
            }
        }
    }

    #[test]
    fn every_variant_is_in_the_table() {
        // `ALL` drives the menus and the no-panic test; a variant missing
        // from it would silently escape both.
        let ids: HashSet<_> = Action::ALL.iter().map(|a| a.id()).collect();
        assert_eq!(ids.len(), Action::ALL.len());
        assert!(Action::ALL.len() >= 26);
        for &a in Action::ALL {
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
    }

    #[test]
    fn ids_name_the_project_panel_and_never_the_media_panel() {
        for &a in Action::ALL {
            assert!(
                !a.id().starts_with("media"),
                "{} still uses the old name",
                a.id()
            );
        }
        assert_eq!(
            Action::from_id("media_panel.new_folder"),
            Some(Action::ProjectNewFolder)
        );
        assert_eq!(Action::from_id("media.remove"), Some(Action::ProjectRemove));
        assert_eq!(
            Action::from_id("project_panel.import"),
            Some(Action::FileImport)
        );
        assert_eq!(
            Action::from_id("reveal.source_in_media"),
            Some(Action::RevealSourceInProject)
        );
        assert_eq!(Action::from_id("no.such.action"), None);
    }

    #[test]
    fn default_shortcuts_do_not_collide() {
        let conflicts = Keymap::default().conflicts();
        assert!(conflicts.is_empty(), "{conflicts:?}");
    }

    #[test]
    fn new_composition_and_new_project_shortcuts() {
        let km = Keymap::default();
        assert_eq!(km.bindings(Action::FileNewComposition), vec![cmd(Key::N)]);
        assert_eq!(km.bindings(Action::FileNewProject), vec![cmd_alt(Key::N)]);
        // Ctrl+N used to be New Project; nothing else may use either shortcut.
        for s in [cmd(Key::N), cmd_alt(Key::N)] {
            let owners: Vec<_> = Action::ALL
                .iter()
                .filter(|a| km.bindings(**a).contains(&s))
                .collect();
            assert_eq!(owners.len(), 1, "{s:?} is bound to {owners:?}");
        }
    }

    #[test]
    fn rebinding_into_a_clash_is_reported() {
        let mut km = Keymap::default();
        km.set(Action::FileNewProject, vec![cmd(Key::N)]);
        let c = km.conflicts();
        assert_eq!(c.len(), 1);
        assert!(c[0].actions.contains(&Action::FileNewProject));
        assert!(c[0].actions.contains(&Action::FileNewComposition));
        km.reset(Action::FileNewProject);
        assert_eq!(km.conflicts().len(), 0);
    }

    #[test]
    fn project_and_editor_scopes_may_share_a_key() {
        // Delete removes layers in the editor and items in the Project panel.
        let km = Keymap::default();
        assert!(km.bindings(Action::LayerDelete).contains(&key(Key::Delete)));
        assert!(
            km.bindings(Action::ProjectRemove)
                .contains(&key(Key::Delete))
        );
        assert_eq!(km.conflicts().len(), 0);
        // ...but a global action on the same key would clash with both.
        let mut km = km;
        km.set(Action::FileSave, vec![key(Key::Delete)]);
        assert_eq!(km.conflicts().len(), 1);
    }

    #[test]
    fn saved_shortcuts_load_through_aliases() {
        let mut km = Keymap::default();
        let n = km.apply_saved([
            ("media_panel.new_folder", vec![cmd(Key::F)]),
            ("project_panel.remove", vec![key(Key::X)]),
            ("gone.forever", vec![key(Key::Q)]),
        ]);
        assert_eq!(n, 2, "unknown ids are dropped");
        assert_eq!(km.bindings(Action::ProjectNewFolder), vec![cmd(Key::F)]);
        assert_eq!(km.bindings(Action::ProjectRemove), vec![key(Key::X)]);
    }

    #[test]
    fn availability_explains_itself() {
        let none = ActionContext::default();
        for &a in Action::ALL {
            let r = a.availability(&none);
            match a.def().requires {
                Requires::Nothing => assert!(r.is_ok(), "{a:?}"),
                _ => assert!(r.is_err(), "{a:?} needs something"),
            }
        }
        assert_eq!(Action::LayerNewSolid.availability(&none), Err(NEEDS_COMP));
        let comp = ActionContext {
            has_comp: true,
            ..ActionContext::default()
        };
        assert!(Action::LayerNewSolid.availability(&comp).is_ok());
        assert_eq!(
            Action::LayerDelete.availability(&comp),
            Err("Select a layer first.")
        );
        assert!(Action::RevealInComposition.availability(&comp).is_err());
        let sel = ActionContext {
            project_selection: 2,
            ..comp
        };
        assert!(Action::RevealInComposition.availability(&sel).is_ok());
    }

    #[test]
    fn dispatch_prefers_the_more_specific_shortcut() {
        let km = Keymap::default();
        let fire = |mods: Modifiers, k: Key| {
            let mut input = InputState::default();
            input.events.push(egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: mods,
            });
            input.modifiers = mods;
            km.dispatch(&mut input, false, false)
        };
        let ctrl_alt = Modifiers::COMMAND.plus(Modifiers::ALT);
        assert_eq!(fire(ctrl_alt, Key::N), vec![Action::FileNewProject]);
        assert_eq!(
            fire(Modifiers::COMMAND, Key::N),
            vec![Action::FileNewComposition]
        );
        let all = Modifiers::COMMAND
            .plus(Modifiers::ALT)
            .plus(Modifiers::SHIFT);
        assert_eq!(fire(all, Key::N), vec![Action::ProjectNewFolder]);
    }

    #[test]
    fn dispatch_respects_scope_and_typing() {
        let km = Keymap::default();
        let press = |k: Key, mods: Modifiers, project: bool, typing: bool| {
            let mut input = InputState::default();
            input.events.push(egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: mods,
            });
            input.modifiers = mods;
            km.dispatch(&mut input, project, typing)
        };
        assert_eq!(
            press(Key::Delete, Modifiers::NONE, false, false),
            vec![Action::LayerDelete]
        );
        assert_eq!(
            press(Key::Delete, Modifiers::NONE, true, false),
            vec![Action::ProjectRemove]
        );
        assert!(
            press(Key::Delete, Modifiers::NONE, false, true).is_empty(),
            "typing"
        );
        assert_eq!(
            press(Key::N, Modifiers::COMMAND, false, true),
            vec![Action::FileNewComposition],
            "file commands still run while typing"
        );
    }
}
