//! Menu items driven by the action table.

use egui::{Button, Ui};

use crate::actions::Action;
use crate::state::{AppRequest, EditorState};

/// One menu item for `action`: labelled and with its first shortcut from
/// the table, disabled with a tooltip when the action cannot run. Clicking
/// runs it and closes the menu. Returns the request the application shell
/// has to confirm (New Project, Open, Quit).
pub fn action_item(ui: &mut Ui, state: &mut EditorState, action: Action) -> Option<AppRequest> {
    action_item_labelled(ui, state, action, action.label())
}

/// [`action_item`] with a different label (menus spell some actions
/// differently, e.g. "Remove from Project…").
pub fn action_item_labelled(
    ui: &mut Ui,
    state: &mut EditorState,
    action: Action,
    label: &str,
) -> Option<AppRequest> {
    let availability = state.availability(action);
    let mut button = Button::new(label);
    if let Some(shortcut) = state.keymap.bindings(action).first() {
        button = button.shortcut_text(ui.ctx().format_shortcut(shortcut));
    }
    let mut response = ui.add_enabled(availability.is_ok(), button);
    if let Err(why) = &availability {
        response = response.on_disabled_hover_text(why);
    }
    if response.clicked() {
        ui.close();
        return state.perform(action);
    }
    None
}
