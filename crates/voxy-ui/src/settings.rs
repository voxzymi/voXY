//! The Settings dialog (Edit > Settings…).
//!
//! A searchable list of per-user preferences, grouped in sections. Changes are
//! written to the preferences file at once. Preferences are UI state, never
//! document state, so they do not go through the command/undo system.
//!
//! This is the dialog shell plus the Appearance section; later U-E steps apply
//! the appearance live and add a Behavior section.

use egui::{Align, Context, Id, Layout, Modal};

use crate::prefs::{FontSize, PrecompOpen, Theme, TimelineDensity, UiScale};
use crate::state::EditorState;

/// The dialog's own state.
#[derive(Debug, Default, Clone)]
pub struct Settings {
    /// The search box text; a row shows only when its label matches.
    pub filter: String,
}

/// Whether a row labelled `label` shows under `filter` (case-insensitive).
pub fn matches(filter: &str, label: &str) -> bool {
    let f = filter.trim().to_lowercase();
    f.is_empty() || label.to_lowercase().contains(&f)
}

/// `#RRGGBB` → sRGB bytes, falling back to the voXY violet on anything else.
fn hex_to_rgb(hex: &str) -> [u8; 3] {
    let byte = |r: std::ops::Range<usize>| hex.get(r).and_then(|s| u8::from_str_radix(s, 16).ok());
    match (hex.len(), byte(1..3), byte(3..5), byte(5..7)) {
        (7, Some(r), Some(g), Some(b)) => [r, g, b],
        _ => [0x7C, 0x5C, 0xFF],
    }
}

/// sRGB bytes → `#RRGGBB`.
fn rgb_to_hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Draws the dialog if it is open, saving any change immediately.
pub fn show(ctx: &Context, state: &mut EditorState) {
    let Some(mut dialog) = state.settings.take() else {
        return;
    };
    let mut close = false;
    let width = 560.0_f32.min((ctx.content_rect().width() - 32.0).max(320.0));
    let modal = Modal::new(Id::new("voxy-settings")).show(ctx, |ui| {
        ui.set_width(width);
        ui.heading("Settings");
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut dialog.filter);
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height((ctx.content_rect().height() - 200.0).clamp(160.0, 460.0))
            .show(ui, |ui| {
                let mut changed = appearance_section(ui, state, &dialog.filter);
                changed |= behavior_section(ui, state, &dialog.filter);
                if changed {
                    state.save_prefs();
                }
            });
        ui.add_space(6.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Done").clicked() {
                close = true;
            }
        });
    });
    if modal.should_close() {
        close = true;
    }
    if !close {
        state.settings = Some(dialog);
    }
}

/// Draws the Appearance rows that match `filter`. Returns `true` if the user
/// changed a value (the caller then saves). The section heading is hidden when
/// no row matches.
fn appearance_section(ui: &mut egui::Ui, state: &mut EditorState, filter: &str) -> bool {
    const THEME: &str = "Theme";
    const ACCENT: &str = "Accent colour";
    const SCALE: &str = "Interface scale";
    const FONT: &str = "Font size";
    const DENSITY: &str = "Timeline density";

    if ![THEME, ACCENT, SCALE, FONT, DENSITY]
        .iter()
        .any(|label| matches(filter, label))
    {
        return false;
    }

    ui.label(egui::RichText::new("Appearance").strong());
    ui.add_space(2.0);

    let mut changed = false;
    let a = &mut state.prefs.appearance;

    if matches(filter, THEME) {
        ui.horizontal(|ui| {
            ui.label(THEME);
            egui::ComboBox::from_id_salt("settings-theme")
                .selected_text(a.theme.label())
                .show_ui(ui, |ui| {
                    for theme in Theme::ALL {
                        changed |= ui
                            .selectable_value(&mut a.theme, theme, theme.label())
                            .changed();
                    }
                });
        });
    }

    if matches(filter, ACCENT) {
        ui.horizontal(|ui| {
            ui.label(ACCENT);
            let mut rgb = hex_to_rgb(&a.accent);
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                a.accent = rgb_to_hex(rgb);
                changed = true;
            }
            ui.weak(&a.accent);
        });
    }

    if matches(filter, SCALE) {
        ui.horizontal(|ui| {
            ui.label(SCALE);
            egui::ComboBox::from_id_salt("settings-scale")
                .selected_text(a.ui_scale.label())
                .show_ui(ui, |ui| {
                    for scale in UiScale::ALL {
                        changed |= ui
                            .selectable_value(&mut a.ui_scale, scale, scale.label())
                            .changed();
                    }
                });
        });
    }

    if matches(filter, FONT) {
        ui.horizontal(|ui| {
            ui.label(FONT);
            egui::ComboBox::from_id_salt("settings-font")
                .selected_text(a.font_size.label())
                .show_ui(ui, |ui| {
                    for size in FontSize::ALL {
                        changed |= ui
                            .selectable_value(&mut a.font_size, size, size.label())
                            .changed();
                    }
                });
        });
    }

    if matches(filter, DENSITY) {
        ui.horizontal(|ui| {
            ui.label(DENSITY);
            egui::ComboBox::from_id_salt("settings-density")
                .selected_text(a.timeline_density.label())
                .show_ui(ui, |ui| {
                    for density in TimelineDensity::ALL {
                        changed |= ui
                            .selectable_value(&mut a.timeline_density, density, density.label())
                            .changed();
                    }
                });
        });
    }

    changed
}

/// Draws the Behavior rows that match `filter`. Returns `true` on a change.
/// These surface preferences that already drive behaviour today: autosave,
/// the Tip of the Day, and what double-clicking a precomp layer does.
fn behavior_section(ui: &mut egui::Ui, state: &mut EditorState, filter: &str) -> bool {
    const AUTOSAVE: &str = "Autosave";
    const TIPS: &str = "Show Tip of the Day";
    const PRECOMP: &str = "Double-click a precomp layer";

    if ![AUTOSAVE, TIPS, PRECOMP]
        .iter()
        .any(|label| matches(filter, label))
    {
        return false;
    }

    ui.add_space(8.0);
    ui.label(egui::RichText::new("Behavior").strong());
    ui.add_space(2.0);

    let mut changed = false;
    let p = &mut state.prefs;

    if matches(filter, AUTOSAVE) {
        ui.horizontal(|ui| {
            let mut on = p.autosave_seconds > 0;
            if ui.checkbox(&mut on, "Autosave projects").changed() {
                p.autosave_seconds = if on {
                    crate::prefs::DEFAULT_AUTOSAVE_SECONDS
                } else {
                    0
                };
                changed = true;
            }
            if p.autosave_seconds > 0 {
                ui.label("every");
                let mut minutes = (f64::from(p.autosave_seconds) / 60.0).round().max(1.0) as u32;
                if ui
                    .add(
                        egui::DragValue::new(&mut minutes)
                            .range(1..=60)
                            .suffix(" min"),
                    )
                    .changed()
                {
                    p.autosave_seconds = minutes * 60;
                    changed = true;
                }
            }
        });
    }

    if matches(filter, TIPS)
        && ui
            .checkbox(
                &mut p.show_tips,
                "Show the Tip of the Day on the start screen",
            )
            .changed()
    {
        changed = true;
    }

    if matches(filter, PRECOMP) {
        ui.horizontal(|ui| {
            ui.label(PRECOMP);
            egui::ComboBox::from_id_salt("settings-precomp")
                .selected_text(p.precomp_double_click.label())
                .show_ui(ui, |ui| {
                    for mode in PrecompOpen::ALL {
                        changed |= ui
                            .selectable_value(&mut p.precomp_double_click, mode, mode.label())
                            .changed();
                    }
                });
        });
    }

    changed
}

#[cfg(test)]
mod tests {
    use egui::{Context, RawInput, Rect};

    use super::*;
    use crate::actions::Action;

    /// One egui pass over the whole screen, then the dialog.
    fn pass(ctx: &Context, state: &mut EditorState) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 700.0),
            )),
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |_| {});
        out.textures_delta.clear();
        show(ctx, state);
    }

    #[test]
    fn filter_matches_case_insensitively() {
        assert!(matches("", "Theme"));
        assert!(matches("THEME", "Theme"));
        assert!(matches("scale", "Interface scale"));
        assert!(!matches("theme", "Accent colour"));
    }

    #[test]
    fn hex_round_trips_and_bad_values_fall_back() {
        assert_eq!(rgb_to_hex([0x7C, 0x5C, 0xFF]), "#7C5CFF");
        assert_eq!(hex_to_rgb("#112233"), [0x11, 0x22, 0x33]);
        assert_eq!(hex_to_rgb("nonsense"), [0x7C, 0x5C, 0xFF]);
        assert_eq!(hex_to_rgb("#12"), [0x7C, 0x5C, 0xFF]);
    }

    #[test]
    fn behavior_rows_are_searchable() {
        assert!(matches("autosave", "Autosave"));
        assert!(matches("TIP", "Show Tip of the Day"));
        assert!(!matches("theme", "Autosave"));
    }

    #[test]
    fn a_behavior_change_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.toml");
        let mut state = EditorState::new(voxy_core::Document::empty("t"));
        state.prefs_path = Some(path.clone());
        state.prefs.autosave_seconds = 0;
        state.save_prefs();
        assert_eq!(
            crate::prefs::Preferences::load_from(&path).autosave_seconds,
            0,
            "autosave off round-trips through Behavior"
        );
    }

    #[test]
    fn the_dialog_opens_draws_and_a_change_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.toml");
        let mut state = EditorState::new(voxy_core::Document::empty("t"));
        state.prefs_path = Some(path.clone());
        let ctx = Context::default();

        state.perform(Action::EditPreferences);
        assert!(state.settings.is_some(), "the action opens the dialog");
        for _ in 0..3 {
            pass(&ctx, &mut state);
        }
        assert!(state.settings.is_some(), "it stays open until Done");

        // A change persists immediately, without the command/undo system.
        state.prefs.appearance.theme = Theme::Light;
        state.save_prefs();
        let saved = crate::prefs::Preferences::load_from(&path);
        assert_eq!(saved.appearance.theme, Theme::Light);
    }
}
