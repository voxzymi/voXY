//! voXY's original look: a dark theme with a violet accent.

use egui::{Color32, Context, Visuals};

/// voXY violet.
pub const ACCENT: Color32 = Color32::from_rgb(124, 92, 255);

pub fn apply(ctx: &Context) {
    let mut visuals = Visuals::dark();
    visuals.selection.bg_fill = ACCENT.linear_multiply(0.6);
    visuals.selection.stroke.color = ACCENT;
    visuals.hyperlink_color = ACCENT;
    visuals.widgets.hovered.bg_stroke.color = ACCENT.linear_multiply(0.8);
    ctx.set_visuals(visuals);
}
