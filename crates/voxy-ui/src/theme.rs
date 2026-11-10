//! voXY's original look: a violet accent over a dark, light or high-contrast
//! base, chosen by the appearance preference.

use egui::{Color32, Context, Visuals};

use crate::prefs::Theme;

/// voXY violet: the default accent and the colour bespoke-painted accents use.
pub const ACCENT: Color32 = Color32::from_rgb(124, 92, 255);

/// Parses a `#RRGGBB` accent preference into a colour, falling back to the
/// voXY violet [`ACCENT`] on anything unexpected.
pub fn accent_color(hex: &str) -> Color32 {
    if hex.len() == 7
        && hex.starts_with('#')
        && let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[1..3], 16),
            u8::from_str_radix(&hex[3..5], 16),
            u8::from_str_radix(&hex[5..7], 16),
        )
    {
        return Color32::from_rgb(r, g, b);
    }
    ACCENT
}

/// Applies `accent` to a base `Visuals` (selection, links, hover).
fn accented(mut visuals: Visuals, accent: Color32) -> Visuals {
    visuals.selection.bg_fill = accent.linear_multiply(0.6);
    visuals.selection.stroke.color = accent;
    visuals.hyperlink_color = accent;
    visuals.widgets.hovered.bg_stroke.color = accent.linear_multiply(0.8);
    visuals
}

/// Sets the interface visuals for `theme`, tinting selection, links and hover
/// with `accent`. Called every frame from the appearance preferences, so a
/// theme or accent change takes effect without a restart. (Bespoke-painted
/// accents still use the brand [`ACCENT`]; unifying those is a larger change.)
pub fn apply(ctx: &Context, theme: Theme, accent: Color32) {
    let visuals = match theme {
        Theme::Dark => accented(Visuals::dark(), accent),
        Theme::Light => accented(Visuals::light(), accent),
        Theme::HighContrast => high_contrast(accent),
    };
    ctx.set_visuals(visuals);
}

/// A dark base pushed to stronger contrast: near-black panels, white text and
/// brighter, heavier strokes so edges read clearly.
fn high_contrast(accent: Color32) -> Visuals {
    let mut visuals = accented(Visuals::dark(), accent);
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
            apply(&ctx, theme, ACCENT);
        }
    }

    #[test]
    fn accent_parses_or_falls_back() {
        assert_eq!(accent_color("#112233"), Color32::from_rgb(0x11, 0x22, 0x33));
        assert_eq!(accent_color("not a colour"), ACCENT);
        assert_eq!(accent_color("#12"), ACCENT);
    }
}
