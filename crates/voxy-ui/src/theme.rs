//! voXY's original look: a violet accent over a dark, light or high-contrast
//! base, chosen by the appearance preference.

use egui::{Color32, Context, Visuals};

use crate::prefs::Theme;

/// voXY violet.
pub const ACCENT: Color32 = Color32::from_rgb(124, 92, 255);

/// Applies the accent to a base `Visuals` (selection, links, hover).
fn accented(mut visuals: Visuals) -> Visuals {
    visuals.selection.bg_fill = ACCENT.linear_multiply(0.6);
    visuals.selection.stroke.color = ACCENT;
    visuals.hyperlink_color = ACCENT;
    visuals.widgets.hovered.bg_stroke.color = ACCENT.linear_multiply(0.8);
    visuals
}

/// Sets the interface visuals for `theme`. Called every frame from the
/// appearance preference, so switching theme takes effect without a restart.
pub fn apply(ctx: &Context, theme: Theme) {
    let visuals = match theme {
        Theme::Dark => accented(Visuals::dark()),
        Theme::Light => accented(Visuals::light()),
        Theme::HighContrast => high_contrast(),
    };
    ctx.set_visuals(visuals);
}

/// A dark base pushed to stronger contrast: near-black panels, white text and
/// brighter, heavier strokes so edges read clearly.
fn high_contrast() -> Visuals {
    let mut visuals = accented(Visuals::dark());
    visuals.override_text_color = Some(Color32::WHITE);
    visuals.panel_fill = Color32::from_gray(8);
    visuals.window_fill = Color32::from_gray(8);
    visuals.extreme_bg_color = Color32::BLACK;
    let widgets = &mut visuals.widgets;
    for style in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        style.bg_stroke.color = Color32::from_gray(200);
        style.bg_stroke.width = style.bg_stroke.width.max(1.0);
        style.fg_stroke.color = Color32::WHITE;
    }
    visuals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_applies_without_panic() {
        let ctx = Context::default();
        for theme in Theme::ALL {
            apply(&ctx, theme);
        }
    }
}
