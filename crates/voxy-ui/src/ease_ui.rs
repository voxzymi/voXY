//! Widgets of the segment inspector for the parametric easings: the type
//! selector, numeric fields, preset thumbnails and the speed strip.
//!
//! These only *describe* edits (they return the new [`Interp`]); the graph
//! editor turns them into undoable commands.

use egui::{Align2, Color32, FontId, Sense, Shape, Stroke, Ui, pos2, vec2};
use voxy_core::{Interp, InterpKind, Jump, JumpMode};

use crate::ease_params::{self, ParamDesc};
use crate::theme::ACCENT;

/// Label of a jump mode in the inspector. `None` is an addition beyond the
/// five graph types and is marked as such.
pub fn jump_label(mode: JumpMode) -> &'static str {
    match mode {
        JumpMode::Start => "Jump at start",
        JumpMode::End => "Jump at end",
        JumpMode::Both => "Jump at both ends",
        JumpMode::None => "No end jumps (extra)",
    }
}

fn jump_tooltip(mode: JumpMode) -> &'static str {
    match mode {
        JumpMode::Start => {
            "Each step begins with its jump (CSS jump-start). The first jump happens right after the first keyframe."
        }
        JumpMode::End => {
            "Each step ends with its jump (CSS jump-end). The last jump happens at the second keyframe."
        }
        JumpMode::Both => "Jumps at both ends: one more jump and level than steps (CSS jump-both).",
        JumpMode::None => {
            "An extra mode beyond the five graph types, from CSS steps(): no jump at either end, so there is one fewer jump than steps and the value reaches the end before the segment does. Needs at least 2 steps."
        }
    }
}

/// The type selector. Returns the kind chosen this frame, if it changed.
pub fn type_selector(
    ui: &mut Ui,
    id: impl std::fmt::Debug + std::hash::Hash,
    current: &Interp,
) -> Option<InterpKind> {
    let mut chosen = None;
    egui::ComboBox::from_id_salt(id)
        .selected_text(current.label())
        .width(150.0)
        .show_ui(ui, |ui| {
            for kind in InterpKind::ALL {
                if ui.selectable_label(current.kind() == kind, kind.label()).clicked()
                    && current.kind() != kind
                {
                    chosen = Some(kind);
                }
            }
        })
        .response
        .on_hover_text(
            "How the value moves between the two keyframes. Switching between related types keeps the parameters that mean the same thing.",
        );
    chosen
}

fn field(ui: &mut Ui, d: &ParamDesc, interp: &Interp) -> Option<Interp> {
    let value = ease_params::get(interp, d.id)?;
    let mut shown = value * d.scale;
    let speed = if d.integer {
        0.1
    } else {
        (d.max - d.min) * d.scale / 400.0
    };
    ui.label(d.label).on_hover_text(d.tooltip);
    let response = ui
        .add(
            egui::DragValue::new(&mut shown)
                .range(d.min * d.scale..=d.max * d.scale)
                .speed(speed)
                .fixed_decimals(d.decimals)
                .suffix(d.suffix),
        )
        .on_hover_text(d.tooltip);
    ui.end_row();
    response
        .changed()
        .then(|| ease_params::with(*interp, d.id, shown / d.scale))
}

/// The numeric fields of a parametric easing, plus the jump-mode selector
/// and the derived read-outs. Returns the edited easing when something
/// changed. Every value is clamped to its documented range.
pub fn param_fields(ui: &mut Ui, interp: &Interp) -> Option<Interp> {
    let mut edited = None;
    let mut current = *interp;
    egui::Grid::new("ease-params")
        .num_columns(2)
        .spacing([8.0, 3.0])
        .show(ui, |ui| {
            for &id in ease_params::fields(interp.kind()) {
                if let Some(next) = field(ui, &ease_params::desc(id), &current) {
                    current = next;
                    edited = Some(next);
                }
            }
            let jump = match &mut current {
                Interp::Steps(p) => Some(&mut p.jump),
                Interp::SteppedRamp(p) => Some(&mut p.jump),
                _ => None,
            };
            if let Some(jump) = jump {
                let before = *jump;
                ui.label("Jump position")
                    .on_hover_text("Where each step's jump falls, as in CSS steps().");
                egui::ComboBox::from_id_salt("ease-jump")
                    .selected_text(jump_label(*jump))
                    .show_ui(ui, |ui| {
                        for mode in JumpMode::ALL {
                            ui.selectable_value(jump, mode, jump_label(mode))
                                .on_hover_text(jump_tooltip(mode));
                        }
                    });
                ui.end_row();
                if *jump != before {
                    edited = Some(current.sanitized());
                }
            }
        });
    if let Interp::Spring(p) = interp {
        ui.label(
            egui::RichText::new(format!(
                "Peak overshoot: {:.0}%",
                p.peak_overshoot() * 100.0
            ))
            .small(),
        )
        .on_hover_text(
            "How far the first swing goes past the target, as a share of the change. It follows from frequency, damping and overshoot; it is shown here but not edited.",
        );
    }
    edited
}

/// Curve samples of `interp` over `0..=1` for a thumbnail. Jumps are drawn
/// as vertical steps because consecutive samples straddle them.
fn samples(interp: &Interp, count: usize) -> Vec<(f64, f64)> {
    (0..=count)
        .map(|i| {
            let u = i as f64 / count as f64;
            (u, interp.shape_progress(u).unwrap_or(u))
        })
        .collect()
}

/// A small picture of an easing, drawn from the easing itself.
fn thumbnail(ui: &mut Ui, interp: &Interp, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(42.0, 28.0), Sense::click());
    let painter = ui.painter_at(rect);
    let hovered = response.hovered();
    painter.rect_filled(
        rect,
        3.0,
        if selected {
            Color32::from_rgb(52, 58, 76)
        } else {
            ui.visuals().extreme_bg_color
        },
    );
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(
            if selected || hovered { 1.5 } else { 1.0 },
            if selected {
                ACCENT
            } else {
                Color32::from_white_alpha(if hovered { 90 } else { 40 })
            },
        ),
        egui::StrokeKind::Inside,
    );
    let pts = samples(interp, 40);
    let (lo, hi) = pts
        .iter()
        .fold((0.0f64, 1.0f64), |(lo, hi), &(_, p)| (lo.min(p), hi.max(p)));
    let inner = rect.shrink2(vec2(6.0, 5.0));
    let to_screen = |u: f64, p: f64| {
        pos2(
            inner.left() + u as f32 * inner.width(),
            inner.bottom() - ((p - lo) / (hi - lo)) as f32 * inner.height(),
        )
    };
    painter.add(Shape::line(
        pts.iter().map(|&(u, p)| to_screen(u, p)).collect(),
        Stroke::new(1.6, if selected { Color32::WHITE } else { ACCENT }),
    ));
    response
}

/// The strip of parametric easings with default parameters. Returns the
/// easing to apply when one is clicked.
pub fn preset_strip(ui: &mut Ui, current: &Interp) -> Option<Interp> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        for kind in InterpKind::PARAMETRIC {
            let interp = kind.default_interp();
            let response = thumbnail(ui, &interp, current.kind() == kind)
                .on_hover_text(format!("{} (default parameters)", kind.label()));
            if response.clicked() {
                chosen = Some(current.convert_to(kind));
            }
        }
    });
    chosen
}

/// A strip showing speed across the segment. `slope(u)` is the speed on the
/// continuous parts; `jumps` are drawn as markers (speed is infinite there,
/// so no curve passes through them).
pub fn speed_strip(ui: &mut Ui, width: f32, slope: impl Fn(f64) -> f64, jumps: &[Jump]) {
    const POINTS: usize = 64;
    let slopes: Vec<f64> = (0..=POINTS)
        .map(|i| slope(i as f64 / POINTS as f64).abs().min(20.0))
        .collect();
    let max = slopes.iter().copied().fold(1e-9, f64::max);
    let (strip, response) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
    let painter = ui.painter_at(strip);
    painter.rect_filled(strip, 2.0, ui.visuals().extreme_bg_color);
    let inner = strip.shrink(4.0);
    let line = Color32::from_rgb(240, 190, 80);
    painter.add(Shape::line(
        slopes
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                pos2(
                    inner.left() + inner.width() * i as f32 / POINTS as f32,
                    inner.bottom() - (s / max) as f32 * inner.height(),
                )
            })
            .collect(),
        Stroke::new(1.5, line),
    ));
    for jump in jumps {
        let x = inner.left() + inner.width() * jump.at as f32;
        painter.line_segment(
            [pos2(x, inner.top() + 7.0), pos2(x, inner.bottom())],
            Stroke::new(1.0, Color32::from_white_alpha(90)),
        );
        painter.text(
            pos2(x, inner.top()),
            Align2::CENTER_TOP,
            "∞",
            FontId::proportional(10.0),
            line,
        );
    }
    painter.text(
        inner.left_top(),
        Align2::LEFT_TOP,
        "speed",
        FontId::proportional(9.0),
        ui.visuals().weak_text_color(),
    );
    if !jumps.is_empty() {
        response.on_hover_text(
            "Speed is infinite at each jump, so jumps are marked (∞) instead of drawn as a curve.",
        );
    }
}
