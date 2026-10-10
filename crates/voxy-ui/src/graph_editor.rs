//! The graph editor.
//!
//! - **Value / speed graph** (left): the animated properties of the selected
//!   layer over time, one curve per component (X red, Y green, Z blue).
//!   In value mode, keyframes can be dragged in time (frame-snapped) and in
//!   value. Speed mode shows how fast the value changes (for position: speed
//!   along the motion path).
//! - **Ease curve** (right): the normalized cubic-bezier of the selected
//!   segment, exactly what the model stores (`k0.ease_out`, `k1.ease_in`).
//!   Dragging a handle turns the segment into a Bézier segment. Presets and
//!   a speed preview sit below it.
//!
//! The segment is the one starting at the keyframe selected in the timeline
//! or the graph; without a selection, the one under the playhead.
//!
//! Like the timeline, each drag is one history transaction re-applied from
//! the original values every frame, so it is a single undo step.

use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2,
};
use voxy_core::command::{
    ApplyEasePreset, MoveKeyframes, SetEase, SetEasing, SetInterpolation, SetProperty,
};
use voxy_core::glam::{DVec2, DVec3};
use voxy_core::{
    CubicEase, EaseHandle, EasePreset, Interp, PropId, Property, SegmentEase, Time, Unit, Value,
    ValueKind,
};

use crate::ease_params::{
    HandleRole, drag_handle, handle_layout, handle_position, handle_readout, handle_tooltip,
    reset_handle,
};
use crate::ease_ui;
use crate::state::EditorState;
use crate::theme::ACCENT;

const PAD: f32 = 28.0;
const COMPONENT_COLORS: [Color32; 3] = [
    Color32::from_rgb(232, 96, 96),
    Color32::from_rgb(110, 205, 120),
    Color32::from_rgb(100, 150, 245),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphMode {
    #[default]
    Value,
    Speed,
}

/// Persistent view state of the graph editor.
#[derive(Debug, Default)]
pub struct GraphView {
    pub mode: GraphMode,
    /// The property shown, if the user picked one.
    pub prop: Option<PropId>,
    /// Height of the ease inspector's panel last frame (sizes its square).
    panel_height: f32,
    /// The name being typed for "save as preset".
    pub preset_name: String,
    drag: Option<GraphDrag>,
}

#[derive(Debug, Clone)]
enum GraphDrag {
    Key {
        prop: PropId,
        time: Time,
        value: Value,
        component: usize,
        origin: Pos2,
        secs_per_px: f64,
        units_per_px: f64,
        y_range: (f64, f64),
        last: (i64, f64),
    },
    Handle {
        prop: PropId,
        t0: Time,
        t1: Time,
        outgoing: bool,
        /// The opposite handle as shown when the drag began; written back
        /// so converting a linear segment keeps that side unchanged.
        other: EaseHandle,
        square: Rect,
        y_range: (f64, f64),
        last: Option<(f64, f64)>,
    },
    /// A handle of a parametric easing; `handle` indexes
    /// [`handle_layout`] for the easing's kind.
    Param {
        prop: PropId,
        t0: Time,
        handle: usize,
        /// The easing when the drag began; every frame re-derives from it.
        base: Interp,
        square: Rect,
        y_range: (f64, f64),
        last: Option<(f64, f64)>,
    },
}

/// Where a step easing jumps, in comp time, for drawing.
///
/// `before` and `after` are flick-level samples either side of the jump (a
/// couple of flicks apart) so the plateau values can be read exactly; `at`
/// is the ideal instant, where the speed marker goes. A jump at the left
/// key (jump-start) is bracketed by the key itself and `t0 + 2`; one at the
/// right key (jump-end) by `t1 − 1` and the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct JumpWindow {
    pub before: Time,
    pub after: Time,
    pub at: Time,
}

/// The step jumps of every segment of `track` that starts at or before
/// `end`, in time order.
pub(crate) fn jump_windows(track: &voxy_core::KeyframeTrack, end: Time) -> Vec<JumpWindow> {
    let keys = track.keys();
    let mut out = Vec::new();
    for i in 0..keys.len().saturating_sub(1) {
        let (t0, t1) = (keys[i].time.0, keys[i + 1].time.0);
        if t0 > end.0 {
            break;
        }
        let span = (t1 - t0) as f64;
        for jump in track.segment_jumps(i) {
            let ideal = (t0 + (jump.at * span).round() as i64).clamp(t0, t1);
            let (before, after) = match ideal {
                t if t == t0 => (t0, (t0 + 2).min(t1)),
                t if t == t1 => ((t1 - 1).max(t0), t1),
                t => ((t - 1).max(t0), (t + 2).min(t1)),
            };
            out.push(JumpWindow {
                before: Time(before),
                after: Time(after),
                at: Time(ideal),
            });
        }
    }
    out
}

/// The runs of consecutive points to draw as one polyline. A point with
/// `valid[i] == false` is dropped and ends the run; `break_after[i]` ends the
/// run after point `i` (the line is not continued across a jump).
pub(crate) fn split_runs(valid: &[bool], break_after: &[bool]) -> Vec<std::ops::Range<usize>> {
    let mut runs = Vec::new();
    let mut start = None;
    for (i, &ok) in valid.iter().enumerate() {
        if !ok {
            if let Some(s) = start.take() {
                runs.push(s..i);
            }
            continue;
        }
        let s = *start.get_or_insert(i);
        if break_after.get(i).copied().unwrap_or(false) {
            runs.push(s..i + 1);
            start = None;
        }
    }
    if let Some(s) = start {
        runs.push(s..valid.len());
    }
    runs
}

/// Number of graphable components of a property, if it can be graphed.
fn dims(kind: ValueKind) -> Option<usize> {
    match kind {
        ValueKind::Scalar => Some(1),
        ValueKind::Vec2 => Some(2),
        ValueKind::Vec3 => Some(3),
        _ => None,
    }
}

fn components(v: &Value) -> [f64; 3] {
    match *v {
        Value::Scalar(s) => [s, 0.0, 0.0],
        Value::Vec2(v) => [v.x, v.y, 0.0],
        Value::Vec3(v) => [v.x, v.y, v.z],
        _ => [0.0; 3],
    }
}

fn with_component(v: &Value, i: usize, x: f64) -> Value {
    match *v {
        Value::Scalar(_) => Value::Scalar(x),
        Value::Vec2(mut p) => {
            p[i] = x;
            Value::Vec2(DVec2::new(p.x, p.y))
        }
        Value::Vec3(mut p) => {
            p[i] = x;
            Value::Vec3(DVec3::new(p.x, p.y, p.z))
        }
        ref other => other.clone(),
    }
}

/// Factor and suffix used to display a unit (factors and normalized values
/// are shown as percentages).
fn display(unit: Unit) -> (f64, &'static str) {
    match unit {
        Unit::Factor | Unit::Normalized => (100.0, "%"),
        Unit::Degrees => (1.0, "°"),
        Unit::Pixels => (1.0, "px"),
        Unit::Seconds => (1.0, "s"),
        Unit::None => (1.0, ""),
    }
}

/// A "nice" step (1, 2 or 5 × 10ⁿ) giving about `count` intervals.
pub(crate) fn nice_step(range: f64, count: f64) -> f64 {
    let raw = (range / count).max(1e-12);
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let nice = if norm < 1.5 {
        1.0
    } else if norm < 3.5 {
        2.0
    } else if norm < 7.5 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Pads a data range by 10% and gives empty ranges a usable height.
pub(crate) fn fit_range(min: f64, max: f64) -> (f64, f64) {
    if !(min.is_finite() && max.is_finite()) || min > max {
        return (0.0, 1.0);
    }
    let span = max - min;
    if span < 1e-9 {
        let pad = (min.abs() * 0.1).max(1.0);
        return (min - pad, max + pad);
    }
    (min - span * 0.1, max + span * 0.1)
}

/// Rate of change of the property at `t` (units per second), by central
/// difference over a quarter frame. Vectors give their magnitude, so for
/// spatial properties this is the speed along the motion path.
pub(crate) fn speed_at(prop: &Property, time: Time, step: Time) -> f64 {
    let before = components(&prop.value_at(time - step));
    let after = components(&prop.value_at(time + step));
    let dt = 2.0 * step.as_secs_f64();
    before
        .iter()
        .zip(after)
        .map(|(b, a)| ((a - b) / dt).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// Maps the ease square (normalized time × progress) to the screen.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EaseSquare {
    pub rect: Rect,
    pub y_range: (f64, f64),
}

impl EaseSquare {
    pub fn project(self, u: f64, p: f64) -> Pos2 {
        let (lo, hi) = self.y_range;
        pos2(
            self.rect.left() + (u as f32) * self.rect.width(),
            self.rect.bottom() - ((p - lo) / (hi - lo)) as f32 * self.rect.height(),
        )
    }

    pub fn unproject(self, pos: Pos2) -> (f64, f64) {
        let (lo, hi) = self.y_range;
        let u = f64::from((pos.x - self.rect.left()) / self.rect.width());
        let p = lo + f64::from((self.rect.bottom() - pos.y) / self.rect.height()) * (hi - lo);
        (u, p)
    }
}

/// The vertical range of the ease square: always 0..1 plus a margin, widened
/// to include overshooting handles.
fn ease_y_range(h1: EaseHandle, h2: EaseHandle) -> (f64, f64) {
    let lo = h1.y().min(h2.y()).min(0.0) - 0.15;
    let hi = h1.y().max(h2.y()).max(1.0) + 0.15;
    (lo, hi)
}

pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let doc = state.history.current().clone();
    let Some((_, layer)) = state.selection.and_then(|id| doc.find_layer(id)) else {
        ui.weak("Select a layer to see its animation curves.");
        return;
    };
    let graphable: Vec<&Property> = layer
        .properties()
        .into_iter()
        .filter(|p| p.is_animated() && dims(p.desc.kind).is_some())
        .collect();
    if graphable.is_empty() {
        ui.weak("This layer has no animated properties. Turn on the stopwatch (⏱) next to a property in the Properties panel.");
        return;
    }

    // Which property: the one with a selected key, else the user's pick,
    // else the first.
    let selected_key = (state.timeline.selected_keys.len() == 1)
        .then(|| state.timeline.selected_keys.iter().next().copied())
        .flatten();
    let prop = selected_key
        .and_then(|(p, _)| graphable.iter().find(|g| g.id == p))
        .or_else(|| {
            state
                .graph
                .prop
                .and_then(|id| graphable.iter().find(|g| g.id == id))
        })
        .unwrap_or(&graphable[0]);
    let prop: &Property = prop;

    ui.horizontal(|ui| {
        for p in &graphable {
            if ui
                .selectable_label(p.id == prop.id, p.desc.label.as_ref())
                .clicked()
            {
                state.graph.prop = Some(p.id);
                state.timeline.selected_keys.clear();
            }
        }
        ui.separator();
        ui.selectable_value(&mut state.graph.mode, GraphMode::Value, "Value");
        ui.selectable_value(&mut state.graph.mode, GraphMode::Speed, "Speed");
    });

    let Some(comp) = state.comp() else {
        crate::empty_state::weak_notice(ui);
        return;
    };
    let comp = &*comp;
    let avail = ui.available_size();
    let ease_w = 250f32.min(avail.x * 0.4);
    ui.horizontal(|ui| {
        value_graph(
            ui,
            state,
            prop,
            comp,
            vec2((avail.x - ease_w - 12.0).max(120.0), avail.y),
        );
        ui.separator();
        state.graph.panel_height = avail.y;
        ui.vertical(|ui| {
            ui.set_width(ease_w);
            egui::ScrollArea::vertical()
                .id_salt("ease-inspector")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ease_editor(ui, state, prop, selected_key, ease_w - 14.0);
                });
        });
    });

    drive_drag(ui, state, comp.frame_rate);
}

fn value_graph(
    ui: &mut Ui,
    state: &mut EditorState,
    prop: &Property,
    comp: &voxy_core::Composition,
    size: egui::Vec2,
) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    let Some(dim_count) = dims(prop.desc.kind) else {
        return;
    };
    let Some(track) = prop.track() else { return };
    let rate = comp.frame_rate;
    let duration = comp.duration.as_secs_f64().max(0.001);
    let mode = state.graph.mode;
    let (factor, suffix) = display(prop.desc.unit);
    let plot = rect.shrink2(vec2(PAD + 18.0, PAD * 0.6));
    let plot = Rect::from_min_max(
        pos2(rect.left() + PAD + 26.0, plot.top()),
        pos2(rect.right() - 10.0, plot.bottom()),
    );

    // Sample the curves (or speeds) across the composition.
    let samples = (plot.width() / 2.0).max(8.0) as usize;
    // Central-difference step for speeds: a quarter frame.
    let h = Time((rate.frame_to_time(1).flicks() / 4).max(1));
    let mut times: Vec<Time> = (0..=samples)
        .map(|i| Time::from_secs_f64(duration * i as f64 / samples as f64))
        .collect();
    // Step jumps: sample either side of each one exactly, so the plateaus
    // end where they do, and remember where to break the line.
    let windows = jump_windows(track, Time::from_secs_f64(duration));
    times.extend(windows.iter().flat_map(|w| [w.before, w.after]));
    times.sort();
    times.dedup();
    // Speed is infinite at a jump: leave the samples around one out (a
    // central difference across it is a meaningless spike) and mark it.
    let valid: Vec<bool> = times
        .iter()
        .map(|&t| {
            mode == GraphMode::Value
                || !windows
                    .iter()
                    .any(|w| t + h >= w.before && t - h <= w.after)
        })
        .collect();
    let break_after: Vec<bool> = times
        .iter()
        .map(|&t| windows.iter().any(|w| w.before == t))
        .collect();
    let series: Vec<Vec<f64>> = match mode {
        GraphMode::Value => (0..dim_count)
            .map(|c| {
                times
                    .iter()
                    .map(|&t| components(&prop.value_at(t))[c])
                    .collect()
            })
            .collect(),
        GraphMode::Speed => vec![
            times
                .iter()
                .zip(&valid)
                .map(|(&t, &ok)| if ok { speed_at(prop, t, h) } else { 0.0 })
                .collect(),
        ],
    };

    let y_range = match &state.graph.drag {
        Some(GraphDrag::Key {
            y_range, prop: p, ..
        }) if *p == prop.id => *y_range,
        _ => {
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for s in &series {
                for (&x, &ok) in s.iter().zip(&valid) {
                    if ok {
                        lo = lo.min(x);
                        hi = hi.max(x);
                    }
                }
            }
            if mode == GraphMode::Speed {
                lo = 0.0;
            }
            fit_range(lo, hi)
        }
    };
    let x_of = |t: f64| plot.left() + (t / duration) as f32 * plot.width();
    let y_of = |val: f64| {
        plot.bottom() - ((val - y_range.0) / (y_range.1 - y_range.0)) as f32 * plot.height()
    };

    // Grid and labels.
    let grid = Stroke::new(1.0, Color32::from_white_alpha(14));
    let label_color = visuals.weak_text_color();
    let ystep = nice_step(y_range.1 - y_range.0, 5.0);
    let mut yv = (y_range.0 / ystep).ceil() * ystep;
    while yv <= y_range.1 {
        let y = y_of(yv);
        painter.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], grid);
        let unit = if mode == GraphMode::Speed {
            format!("{suffix}/s")
        } else {
            suffix.to_owned()
        };
        painter.text(
            pos2(plot.left() - 4.0, y),
            Align2::RIGHT_CENTER,
            format!("{}{unit}", trim_number(yv * factor)),
            FontId::proportional(10.0),
            label_color,
        );
        yv += ystep;
    }
    let tstep = nice_step(duration, (plot.width() / 90.0).max(2.0).into()).max(1.0 / rate.as_f64());
    let mut tv = 0.0;
    while tv <= duration + 1e-9 {
        let x = x_of(tv);
        painter.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], grid);
        painter.text(
            pos2(x, plot.bottom() + 3.0),
            Align2::CENTER_TOP,
            format!("{}s", trim_number(tv)),
            FontId::proportional(10.0),
            label_color,
        );
        tv += tstep;
    }

    // Curves.
    for (c, s) in series.iter().enumerate() {
        let color = if dim_count == 1 || mode == GraphMode::Speed {
            ACCENT
        } else {
            COMPONENT_COLORS[c]
        };
        let point = |i: usize| pos2(x_of(times[i].as_secs_f64()), y_of(s[i]));
        for run in split_runs(&valid, &break_after) {
            if run.len() > 1 {
                painter.add(Shape::line(
                    run.map(point).collect(),
                    Stroke::new(1.8, color),
                ));
            }
        }
        // A jump is not part of the curve: a thin dashed connector between
        // the two plateaus.
        if mode == GraphMode::Value {
            for w in &windows {
                if let (Ok(a), Ok(b)) = (
                    times.binary_search(&w.before),
                    times.binary_search(&w.after),
                ) {
                    painter.extend(Shape::dashed_line(
                        &[point(a), point(b)],
                        Stroke::new(1.0, color.gamma_multiply(0.5)),
                        3.0,
                        3.0,
                    ));
                    if c == 0 {
                        let (from, to) = (
                            components(&prop.value_at(w.before)),
                            components(&prop.value_at(w.after)),
                        );
                        let change: Vec<String> = (0..dim_count)
                            .filter(|&k| (to[k] - from[k]).abs() > 1e-9)
                            .map(|k| {
                                let delta = (to[k] - from[k]) * factor;
                                let axis = if dim_count == 1 {
                                    ""
                                } else {
                                    ["X ", "Y ", "Z "][k]
                                };
                                format!("{axis}{delta:+.2}{suffix}")
                            })
                            .collect();
                        let hit = Rect::from_two_pos(point(a), point(b)).expand(5.0);
                        ui.interact(
                            hit,
                            Id::new(("graph-jump", prop.id, w.at.0)),
                            Sense::hover(),
                        )
                        .on_hover_text(format!("Step jump: {}", change.join(", ")));
                    }
                }
            }
        }
    }
    // Speed marks every jump with an infinity cap instead of a curve.
    if mode == GraphMode::Speed {
        for w in &windows {
            let x = x_of(w.at.as_secs_f64());
            painter.line_segment(
                [pos2(x, plot.top() + 12.0), pos2(x, plot.bottom())],
                Stroke::new(1.0, Color32::from_white_alpha(50)),
            );
            painter.text(
                pos2(x, plot.top()),
                Align2::CENTER_TOP,
                "∞",
                FontId::proportional(12.0),
                ACCENT,
            );
        }
        if !windows.is_empty() {
            painter.text(
                pos2(plot.right() - 4.0, plot.top() + 2.0),
                Align2::RIGHT_TOP,
                "∞ = step jump (speed is infinite)",
                FontId::proportional(9.0),
                label_color,
            );
        }
    }

    // Keyframes.
    for k in track.keys() {
        let x = x_of(k.time.as_secs_f64());
        let is_sel = state.timeline.selected_keys.contains(&(prop.id, k.time));
        if mode == GraphMode::Speed {
            painter.line_segment(
                [pos2(x, plot.bottom() - 6.0), pos2(x, plot.bottom())],
                Stroke::new(2.0, if is_sel { Color32::WHITE } else { label_color }),
            );
            continue;
        }
        let vals = components(&k.value);
        for c in 0..dim_count {
            let center = pos2(x, y_of(vals[c]));
            let hit = Rect::from_center_size(center, vec2(12.0, 12.0));
            if !plot.expand(6.0).contains(center) {
                continue;
            }
            let id = Id::new(("graph-key", prop.id, k.time.0, c));
            let r = ui
                .interact(hit, id, Sense::click_and_drag())
                .on_hover_cursor(CursorIcon::Move)
                .on_hover_text(format!(
                    "{} {}: {}{suffix}",
                    timecode_secs(k.time),
                    ["X", "Y", "Z"][c],
                    trim_number(vals[c] * factor)
                ));
            if r.clicked() || r.drag_started() {
                state.timeline.selected_keys = std::collections::HashSet::from([(prop.id, k.time)]);
            }
            if r.drag_started() {
                let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(center);
                state.history.begin("Edit Keyframe");
                state.graph.drag = Some(GraphDrag::Key {
                    prop: prop.id,
                    time: k.time,
                    value: k.value.clone(),
                    component: c,
                    origin,
                    secs_per_px: duration / f64::from(plot.width()),
                    units_per_px: (y_range.1 - y_range.0) / f64::from(plot.height()),
                    y_range,
                    last: (0, 0.0),
                });
            }
            let color = if dim_count == 1 {
                ACCENT
            } else {
                COMPONENT_COLORS[c]
            };
            painter.circle_filled(
                center,
                if is_sel { 5.0 } else { 4.0 },
                if is_sel { Color32::WHITE } else { color },
            );
            painter.circle_stroke(
                center,
                if is_sel { 5.0 } else { 4.0 },
                Stroke::new(1.0, Color32::from_black_alpha(160)),
            );
        }
    }

    // Playhead.
    let px = x_of(state.time.as_secs_f64());
    painter.line_segment(
        [pos2(px, plot.top()), pos2(px, plot.bottom())],
        Stroke::new(1.5, ACCENT),
    );

    let title = match mode {
        GraphMode::Value => format!("{} — value", prop.desc.label),
        GraphMode::Speed => format!("{} — speed", prop.desc.label),
    };
    painter.text(
        rect.left_top() + vec2(8.0, 4.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(11.0),
        label_color,
    );
}

fn trim_number(x: f64) -> String {
    let s = format!("{x:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

fn timecode_secs(t: Time) -> String {
    format!("{}s", trim_number(t.as_secs_f64()))
}

fn ease_editor(
    ui: &mut Ui,
    state: &mut EditorState,
    prop: &Property,
    selected: Option<(PropId, Time)>,
    width: f32,
) {
    let Some(track) = prop.track() else { return };
    let keys = track.keys();
    if keys.len() < 2 {
        ui.weak("Add a second keyframe to edit the easing between them.");
        return;
    }
    // The segment: starting at the selected key, else under the playhead.
    let start = selected
        .filter(|(p, _)| *p == prop.id)
        .and_then(|(_, t)| track.search(t).ok())
        .filter(|&i| i + 1 < keys.len())
        .unwrap_or_else(|| {
            track.segment_at(state.time).map_or(
                if state.time < keys[0].time {
                    0
                } else {
                    keys.len() - 2
                },
                |(i, _)| i,
            )
        });
    let (k0, k1) = (&keys[start], &keys[start + 1]);

    ui.label(
        egui::RichText::new(format!(
            "Segment {} to {} ({})",
            timecode_secs(k0.time),
            timecode_secs(k1.time),
            k0.interp.label().to_lowercase()
        ))
        .small(),
    );
    ui.horizontal(|ui| {
        ui.label("Type");
        if let Some(kind) = ease_ui::type_selector(ui, ("ease-type", prop.id), &k0.interp) {
            state.execute(SetEasing {
                prop: prop.id,
                time: k0.time,
                interp: k0.interp.convert_to(kind),
            });
        }
        let default = k0.interp.kind().default_interp();
        if k0.interp.is_parametric()
            && ui
                .add_enabled(k0.interp != default, egui::Button::new("Reset").small())
                .on_hover_text("Put every parameter of this type back to its default.")
                .on_disabled_hover_text("Already at the defaults.")
                .clicked()
        {
            state.execute(SetEasing {
                prop: prop.id,
                time: k0.time,
                interp: default,
            });
        }
    });

    let (h1, h2) = match k0.interp {
        Interp::Bezier => (k0.ease_out, k1.ease_in),
        _ => (EaseHandle::LINEAR_OUT, EaseHandle::LINEAR_IN),
    };
    let y_range = match &state.graph.drag {
        Some(GraphDrag::Handle { y_range, .. } | GraphDrag::Param { y_range, .. }) => *y_range,
        _ if k0.interp.is_parametric() => {
            // Springs and oscillations leave 0..1: fit the sampled curve.
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            for i in 0..=96 {
                let p = k0.interp.shape_progress(f64::from(i) / 96.0).unwrap_or(0.0);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            (lo - 0.15, hi + 0.15)
        }
        _ => ease_y_range(h1, h2),
    };
    // The inspector below the square scrolls, so the square keeps a fixed shape.
    let height = (state.graph.panel_height * 0.5).min(width * 0.75).max(80.0);
    let (outer, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let rect = outer.shrink2(vec2(8.0, 6.0));
    let sq = EaseSquare { rect, y_range };
    let painter = ui.painter_at(outer);
    painter.rect_filled(outer, 2.0, ui.visuals().extreme_bg_color);
    let unit = Rect::from_two_pos(sq.project(0.0, 0.0), sq.project(1.0, 1.0));
    painter.rect_stroke(
        unit,
        0.0,
        Stroke::new(1.0, Color32::from_white_alpha(30)),
        egui::StrokeKind::Middle,
    );
    painter.line_segment(
        [sq.project(0.0, 0.0), sq.project(1.0, 1.0)],
        Stroke::new(1.0, Color32::from_white_alpha(18)),
    );

    // The curve.
    let pts: Vec<Pos2> = (0..=96)
        .map(|i| {
            let u = f64::from(i) / 96.0;
            let p = match k0.interp {
                Interp::Hold => {
                    if i == 96 {
                        1.0
                    } else {
                        0.0
                    }
                }
                Interp::Linear => u,
                Interp::Bezier => CubicEase { p1: h1, p2: h2 }.progress(u),
                shaped => shaped.shape_progress(u).unwrap_or(u),
            };
            sq.project(u, p)
        })
        .collect();
    painter.add(Shape::line(pts, Stroke::new(2.0, ACCENT)));

    // Bézier handles (drawn for hold, linear and Bézier segments: dragging
    // one makes the segment Bézier). Parametric easings have their own
    // parameters, so they get no Bézier handles.
    let parametric = k0.interp.is_parametric();
    let a = sq.project(0.0, 0.0);
    let b = sq.project(1.0, 1.0);
    let p1 = sq.project(h1.x(), h1.y());
    let p2 = sq.project(h2.x(), h2.y());
    let handle_stroke = Stroke::new(1.2, Color32::from_white_alpha(140));
    if !parametric {
        painter.line_segment([a, p1], handle_stroke);
        painter.line_segment([b, p2], handle_stroke);
    }
    for (pos, outgoing) in [(p1, true), (p2, false)]
        .into_iter()
        .filter(|_| !parametric)
    {
        let id = Id::new(("ease-handle", prop.id, outgoing));
        let r = ui
            .interact(
                Rect::from_center_size(pos, vec2(14.0, 14.0)),
                id,
                Sense::drag(),
            )
            .on_hover_cursor(CursorIcon::Grab);
        if r.drag_started() {
            state.history.begin("Edit Easing");
            state.graph.drag = Some(GraphDrag::Handle {
                prop: prop.id,
                t0: k0.time,
                t1: k1.time,
                outgoing,
                other: if outgoing { h2 } else { h1 },
                square: rect,
                y_range,
                last: None,
            });
        }
        let active = r.hovered() || r.dragged();
        painter.circle_filled(pos, if active { 6.0 } else { 5.0 }, Color32::WHITE);
        painter.circle_stroke(pos, 5.0, Stroke::new(1.5, ACCENT));
    }

    // Handles of a parametric easing: yellow = main, white = secondary.
    // (Which parameter each one drives is provisional; see `ease_params`.)
    if parametric {
        for (index, spec) in handle_layout(k0.interp.kind()).iter().enumerate() {
            let (hu, hp) = handle_position(&k0.interp, spec);
            let pos = sq.project(hu, hp);
            let r = ui
                .interact(
                    Rect::from_center_size(pos, vec2(16.0, 16.0)),
                    Id::new(("param-handle", prop.id, index)),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(CursorIcon::Grab)
                .on_hover_text(format!(
                    "{}\n{}",
                    handle_readout(&k0.interp, spec),
                    handle_tooltip(spec)
                ));
            if r.double_clicked() {
                state.execute(SetEasing {
                    prop: prop.id,
                    time: k0.time,
                    interp: reset_handle(k0.interp, spec),
                });
            }
            if r.drag_started() {
                state.history.begin("Edit Easing");
                state.graph.drag = Some(GraphDrag::Param {
                    prop: prop.id,
                    t0: k0.time,
                    handle: index,
                    base: k0.interp,
                    square: rect,
                    y_range,
                    last: None,
                });
            }
            let fill = match spec.role {
                HandleRole::Main => Color32::from_rgb(240, 190, 80),
                HandleRole::Secondary => Color32::WHITE,
            };
            let active = r.hovered() || r.dragged();
            painter.circle_filled(pos, if active { 6.5 } else { 5.5 }, fill);
            painter.circle_stroke(pos, 5.5, Stroke::new(1.5, Color32::from_black_alpha(190)));
            if r.dragged() {
                // Egui hides tooltips while dragging, so show the values the
                // handle is setting right beside it.
                let text = handle_readout(&k0.interp, spec);
                let galley =
                    painter.layout_no_wrap(text, FontId::proportional(11.0), Color32::WHITE);
                let at = pos2(
                    (pos.x - galley.size().x / 2.0)
                        .clamp(outer.left() + 2.0, outer.right() - galley.size().x - 2.0),
                    (pos.y - 24.0).max(outer.top() + 2.0),
                );
                painter.rect_filled(
                    Rect::from_min_size(at, galley.size()).expand(3.0),
                    3.0,
                    Color32::from_black_alpha(220),
                );
                painter.galley(at, galley, Color32::WHITE);
            }
        }
        if let Some(edited) = ease_ui::param_fields(ui, &k0.interp) {
            state.execute(SetEasing {
                prop: prop.id,
                time: k0.time,
                interp: edited,
            });
        }
    } else {
        ui.label(
            egui::RichText::new(format!(
                "cubic-bezier({}, {}, {}, {})",
                trim_number(h1.x()),
                trim_number(h1.y()),
                trim_number(h2.x()),
                trim_number(h2.y())
            ))
            .monospace()
            .small(),
        );
    }

    // Speed preview: d(progress)/du across the segment.
    if k0.interp == Interp::Bezier {
        let ease = CubicEase { p1: h1, p2: h2 };
        ease_ui::speed_strip(ui, width, |u| ease.slope(u), &[]);
    } else if parametric {
        let interp = k0.interp;
        let jumps = interp.shape_jumps();
        ease_ui::speed_strip(ui, width, |u| interp.shape_slope(u).unwrap_or(0.0), &jumps);
    }

    ui.horizontal_wrapped(|ui| {
        for preset in EasePreset::ALL {
            if ui.small_button(preset.label()).clicked() {
                state.execute(ApplyEasePreset {
                    prop: prop.id,
                    times: vec![k0.time],
                    preset,
                });
            }
        }
        if ui.small_button("Hold").clicked() {
            state.execute(SetInterpolation {
                prop: prop.id,
                times: vec![k0.time],
                interp: Interp::Hold,
            });
        }
    });
    let targets = state.easing_targets(prop.id, k0.time);
    let segments: usize = targets.iter().map(|(_, t)| t.len()).sum();
    let current = SegmentEase {
        interp: k0.interp,
        ease_out: k0.ease_out,
        ease_in: k1.ease_in,
    };
    if let Some(interp) = ease_ui::preset_strip(ui, &k0.interp) {
        if segments > 1 {
            state.apply_easing(targets.clone(), SegmentEase::new(interp));
        } else {
            state.execute(SetEasing {
                prop: prop.id,
                time: k0.time,
                interp,
            });
        }
    }
    easing_clipboard_and_presets(ui, state, &targets, segments, current);
}

/// Copy, paste, apply-to-selection and the user's saved easings. Everything
/// that writes goes through `ApplyEasing`, so applying to many segments is
/// one undo step.
fn easing_clipboard_and_presets(
    ui: &mut Ui,
    state: &mut EditorState,
    targets: &[(PropId, Vec<Time>)],
    segments: usize,
    current: SegmentEase,
) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .small_button("Copy")
            .on_hover_text("Copy this segment's easing: its type, parameters and curve.")
            .clicked()
        {
            state.copy_easing(current);
        }
        let paste = ui.add_enabled(
            state.ease_clipboard.is_some(),
            egui::Button::new("Paste").small(),
        );
        let paste = match state.ease_clipboard {
            Some(clip) => paste.on_hover_text(format!(
                "Paste the copied {} easing onto {}.",
                clip.label(),
                segment_count_text(segments)
            )),
            None => paste.on_disabled_hover_text("Copy an easing first."),
        };
        if paste.clicked()
            && let Some(clip) = state.ease_clipboard
        {
            state.apply_easing(targets.to_vec(), clip);
        }
        if segments > 1
            && ui
                .small_button(format!("Apply to {segments}"))
                .on_hover_text(format!(
                    "Give all {segments} selected segments this segment's easing (one undo step)."
                ))
                .clicked()
        {
            state.apply_easing(targets.to_vec(), current);
        }
    });
    if segments > 1 {
        ui.label(
            egui::RichText::new(format!(
                "Presets and Paste apply to {segments} selected segments."
            ))
            .small()
            .weak(),
        );
    }

    egui::CollapsingHeader::new("My easings")
        .default_open(!state.prefs.easing_presets.is_empty())
        .show(ui, |ui| {
            let saved = state.prefs.easing_presets.clone();
            if saved.is_empty() {
                ui.weak("Name the current easing below to keep it here.");
            }
            ui.horizontal_wrapped(|ui| {
                for preset in &saved {
                    let response = ui
                        .small_button(&preset.name)
                        .on_hover_text(format!(
                            "{} — click to apply to {}. Right-click to delete.",
                            preset.ease.label(),
                            segment_count_text(segments)
                        ));
                    if response.clicked() {
                        state.apply_easing(targets.to_vec(), preset.ease);
                    }
                    response.context_menu(|ui| {
                        if ui.button("Delete").clicked() {
                            state.prefs.remove_easing_preset(&preset.name);
                            state.save_prefs();
                            ui.close();
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut state.graph.preset_name)
                        .hint_text("Name")
                        .desired_width(110.0),
                );
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let name = state.graph.preset_name.trim().to_owned();
                let clicked = ui
                    .add_enabled(!name.is_empty(), egui::Button::new("Save").small())
                    .on_hover_text("Save this segment's easing under that name (replaces one with the same name). Enter does the same.")
                    .clicked();
                if (clicked || entered) && state.prefs.save_easing_preset(&name, current) {
                    state.save_prefs();
                    state.graph.preset_name.clear();
                }
            });
        });
}

fn segment_count_text(segments: usize) -> String {
    if segments == 1 {
        "this segment".to_owned()
    } else {
        format!("{segments} segments")
    }
}

fn drive_drag(ui: &Ui, state: &mut EditorState, rate: voxy_core::FrameRate) {
    let Some(drag) = state.graph.drag.clone() else {
        return;
    };
    let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
    if let Some(pos) = pos {
        match drag {
            GraphDrag::Key {
                prop,
                time,
                ref value,
                component,
                origin,
                secs_per_px,
                units_per_px,
                y_range,
                last,
            } => {
                let frames =
                    (f64::from(pos.x - origin.x) * secs_per_px * rate.as_f64()).round() as i64;
                let dv = f64::from(origin.y - pos.y) * units_per_px;
                if (frames, dv) != last {
                    state.history.rollback();
                    state.history.begin("Edit Keyframe");
                    let delta = rate.frame_to_time(frames);
                    let moved = frames != 0
                        && time + delta >= Time::ZERO
                        && state
                            .history
                            .execute(MoveKeyframes {
                                prop,
                                times: vec![time],
                                delta,
                            })
                            .is_ok();
                    let at = if moved { time + delta } else { time };
                    let new_value =
                        with_component(value, component, components(value)[component] + dv);
                    let _ = state.history.execute(SetProperty {
                        prop,
                        time: at,
                        value: new_value,
                    });
                    state.timeline.selected_keys = std::collections::HashSet::from([(prop, at)]);
                    state.graph.drag = Some(GraphDrag::Key {
                        prop,
                        time,
                        value: value.clone(),
                        component,
                        origin,
                        secs_per_px,
                        units_per_px,
                        y_range,
                        last: (frames, dv),
                    });
                }
            }
            GraphDrag::Handle {
                prop,
                t0,
                t1,
                outgoing,
                other,
                square,
                y_range,
                last,
            } => {
                let sq = EaseSquare {
                    rect: square,
                    y_range,
                };
                let (u, p) = sq.unproject(pos);
                let handle = (u.clamp(0.0, 1.0), p);
                if Some(handle) != last
                    && let Some(h) = EaseHandle::new(handle.0, handle.1)
                {
                    state.history.rollback();
                    state.history.begin("Edit Easing");
                    let _ = state.history.execute(SetInterpolation {
                        prop,
                        times: vec![t0],
                        interp: Interp::Bezier,
                    });
                    let (p1, p2) = if outgoing { (h, other) } else { (other, h) };
                    let _ = state.history.execute(SetEase {
                        prop,
                        time: t0,
                        ease_in: None,
                        ease_out: Some(p1),
                    });
                    let _ = state.history.execute(SetEase {
                        prop,
                        time: t1,
                        ease_in: Some(p2),
                        ease_out: None,
                    });
                    state.graph.drag = Some(GraphDrag::Handle {
                        prop,
                        t0,
                        t1,
                        outgoing,
                        other,
                        square,
                        y_range,
                        last: Some(handle),
                    });
                }
            }
            GraphDrag::Param {
                prop,
                t0,
                handle,
                base,
                square,
                y_range,
                last,
            } => {
                let sq = EaseSquare {
                    rect: square,
                    y_range,
                };
                let target = sq.unproject(pos);
                if Some(target) != last
                    && let Some(spec) = handle_layout(base.kind()).get(handle)
                {
                    state.history.rollback();
                    state.history.begin("Edit Easing");
                    let _ = state.history.execute(SetEasing {
                        prop,
                        time: t0,
                        interp: drag_handle(base, spec, target),
                    });
                    state.graph.drag = Some(GraphDrag::Param {
                        prop,
                        t0,
                        handle,
                        base,
                        square,
                        y_range,
                        last: Some(target),
                    });
                }
            }
        }
    }
    if down {
        ui.ctx().request_repaint();
    } else {
        state.history.commit();
        state.graph.drag = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxy_core::property::{builtin_desc, keys};
    use voxy_core::{FrameRate, PropId};

    #[test]
    fn nice_steps() {
        assert_eq!(nice_step(10.0, 5.0), 2.0);
        assert_eq!(nice_step(1000.0, 4.0), 200.0);
        assert!((nice_step(0.3, 3.0) - 0.1).abs() < 1e-12);
        assert_eq!(nice_step(70.0, 10.0), 5.0);
    }

    #[test]
    fn ranges_are_padded_and_never_empty() {
        assert_eq!(fit_range(0.0, 10.0), (-1.0, 11.0));
        let (lo, hi) = fit_range(5.0, 5.0);
        assert!(lo < 5.0 && hi > 5.0);
        assert_eq!(fit_range(f64::INFINITY, f64::NEG_INFINITY), (0.0, 1.0));
    }

    #[test]
    fn ease_square_round_trips() {
        let sq = EaseSquare {
            rect: Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 150.0)),
            y_range: (-0.25, 1.4),
        };
        for (u, p) in [(0.0, 0.0), (1.0, 1.0), (0.3, -0.2), (0.75, 1.3)] {
            let (u2, p2) = sq.unproject(sq.project(u, p));
            assert!((u - u2).abs() < 1e-5 && (p - p2).abs() < 1e-5);
        }
    }

    #[test]
    fn runs_split_at_jumps_and_invalid_points() {
        let t = true;
        let f = false;
        assert_eq!(split_runs(&[t; 4], &[f; 4]), vec![0..4]);
        // A break after point 1: the line does not continue across it.
        assert_eq!(split_runs(&[t; 4], &[f, t, f, f]), vec![0..2, 2..4]);
        // Invalid points are dropped and end the run.
        assert_eq!(split_runs(&[t, t, f, f, t], &[f; 5]), vec![0..2, 4..5]);
        // A break on the last point and an empty input.
        assert_eq!(split_runs(&[t; 3], &[f, f, t]), vec![0..3]);
        assert_eq!(split_runs(&[], &[]).len(), 0);
        assert_eq!(split_runs(&[f, f], &[f, f]).len(), 0);
    }

    fn two_key_scalar(interp: Interp, secs: i64, from: f64, to: f64) -> voxy_core::KeyframeTrack {
        let mut a = voxy_core::Keyframe::new(Time::ZERO, Value::Scalar(from));
        a.interp = interp;
        voxy_core::KeyframeTrack::try_from(vec![
            a,
            voxy_core::Keyframe::new(Time::from_secs(secs), Value::Scalar(to)),
        ])
        .unwrap()
    }

    #[test]
    fn jump_windows_bracket_every_step_jump() {
        use voxy_core::{JumpMode, StepsParams};
        let secs = 2;
        for jump in JumpMode::ALL {
            for count in [1u32, 2, 4, 7] {
                let interp = Interp::Steps(StepsParams {
                    count,
                    jump,
                    hold: 0.0,
                });
                let track = two_key_scalar(interp, secs, 10.0, 110.0);
                let windows = jump_windows(&track, Time::from_secs(secs));
                let jumps = track.segment_jumps(0);
                assert_eq!(windows.len(), jumps.len(), "{jump:?} {count}");
                let at = |t: Time| track.evaluate(t, false).as_scalar().unwrap();
                for (w, j) in windows.iter().zip(&jumps) {
                    assert!(w.before < w.after && w.before <= w.at && w.at <= w.after);
                    assert!(w.after.0 - w.before.0 <= 3, "a window is a few flicks wide");
                    // The value really steps by the jump's height across it.
                    let step = at(w.after) - at(w.before);
                    assert!(
                        (step - j.height * 100.0).abs() < 1e-6,
                        "{jump:?} {count}: {step} vs {}",
                        j.height * 100.0
                    );
                    // And it is flat on both sides of the window.
                    assert_eq!(at(w.before), at(Time(w.before.0 - 1).max(Time::ZERO)));
                }
                assert!(windows.windows(2).all(|p| p[0].at < p[1].at), "ascending");
            }
        }
    }

    #[test]
    fn windows_sit_at_the_segment_ends_for_start_and_end_jumps() {
        use voxy_core::{JumpMode, StepsParams};
        let make = |jump| {
            let interp = Interp::Steps(StepsParams {
                count: 4,
                jump,
                hold: 0.0,
            });
            jump_windows(&two_key_scalar(interp, 2, 0.0, 1.0), Time::from_secs(2))
        };
        let start = make(JumpMode::Start);
        assert_eq!((start[0].before, start[0].at), (Time::ZERO, Time::ZERO));
        let end = make(JumpMode::End);
        let last = end.last().unwrap();
        assert_eq!(
            (last.after, last.at),
            (Time::from_secs(2), Time::from_secs(2))
        );
        // Legacy and continuous easings have none.
        for interp in [Interp::Linear, Interp::Hold, Interp::Bezier] {
            assert_eq!(
                jump_windows(&two_key_scalar(interp, 2, 0.0, 1.0), Time::from_secs(2)).len(),
                0
            );
        }
        // Segments after `end` are not listed.
        let interp = Interp::Steps(StepsParams::default());
        assert_eq!(
            jump_windows(&two_key_scalar(interp, 2, 0.0, 1.0), Time(-1)).len(),
            0
        );
    }

    #[test]
    fn linear_animation_has_constant_speed() {
        let mut p = Property::new(
            PropId(1),
            builtin_desc(keys::ROTATION).unwrap(),
            Value::Scalar(0.0),
        )
        .unwrap();
        p.add_keyframe(Time::ZERO).unwrap();
        p.set_value_at(Time::from_secs(2), Value::Scalar(90.0))
            .unwrap();
        let h = Time(FrameRate::FPS_30.frame_to_time(1).flicks() / 4);
        for ms in [100, 500, 1000, 1900] {
            assert!((speed_at(&p, Time::from_millis(ms), h) - 45.0).abs() < 1e-6);
        }
        // After the last key the value holds: speed zero.
        assert!(speed_at(&p, Time::from_secs(3), h).abs() < 1e-9);
    }

    #[test]
    fn ease_range_includes_overshoot() {
        let (lo, hi) = ease_y_range(
            EaseHandle::new(0.3, 1.6).unwrap(),
            EaseHandle::new(0.6, -0.4).unwrap(),
        );
        assert!(lo < -0.4 && hi > 1.6);
    }
}
