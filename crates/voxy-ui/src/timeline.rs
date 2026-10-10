//! The timeline panel: a time ruler, one bar per layer, and expandable
//! per-property keyframe lanes.
//!
//! - Click or drag the ruler to move the playhead (snaps to frames).
//! - Drag a layer bar to move it; drag its edges to trim.
//! - Click a keyframe to select it (Shift/Ctrl adds), drag to move the
//!   selection; Delete removes it.
//! - Ctrl + scroll zooms around the pointer, plain horizontal scroll pans.
//!
//! Every drag runs inside one history transaction, re-applied from the
//! original values each frame, so a whole drag is a single undo step and a
//! rejected position (a key landing on another key) simply holds the last
//! good one.

use std::collections::{BTreeMap, HashSet};

use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2,
    vec2,
};
use voxy_core::command::{MoveKeyframes, RemoveKeyframes, SetLayerTiming, SetLayerVisible};
use voxy_core::{Composition, FrameRate, Layer, LayerId, LayerKind, PropId, Property, Time, Value};

use crate::panels::timecode_at;
use crate::state::EditorState;
use crate::theme::ACCENT;

const ROW_H: f32 = 24.0;
const HEADER_W: f32 = 230.0;
const RULER_H: f32 = 24.0;
const HANDLE_W: f32 = 7.0;
const MIN_PPS: f64 = 2.0;
const MAX_PPS: f64 = 20_000.0;

/// Persistent view state of the timeline.
#[derive(Debug, Default)]
pub struct TimelineView {
    /// Pixels per second; `None` fits the whole composition.
    pps: Option<f64>,
    /// Time at the left edge of the lanes, in seconds.
    scroll: f64,
    pub expanded: HashSet<LayerId>,
    pub selected_keys: HashSet<(PropId, Time)>,
    /// Scroll this layer into view on the next frame (set by Reveal).
    pub reveal: Option<LayerId>,
    drag: Option<Drag>,
    /// Where each layer's row was drawn this frame, top to bottom, for
    /// dropping items between them.
    pub(crate) rows: Vec<(LayerId, Rect)>,
    /// Top of the first row and bottom of the last (including expanded
    /// property rows) this frame.
    rows_span: Option<(f32, f32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragKind {
    MoveLayer,
    TrimIn,
    TrimOut,
    Keys,
}

#[derive(Debug, Clone)]
struct Drag {
    kind: DragKind,
    layer: LayerId,
    /// The layer's `(in_point, out_point, start_offset)` when the drag began.
    orig: (Time, Time, Time),
    /// Selected keys when the drag began.
    keys: Vec<(PropId, Time)>,
    origin_x: f32,
    last_frames: i64,
}

impl Drag {
    fn label(&self) -> &'static str {
        match self.kind {
            DragKind::MoveLayer => "Move Layer",
            DragKind::TrimIn | DragKind::TrimOut => "Trim Layer",
            DragKind::Keys => "Move Keyframes",
        }
    }
}

/// Maps between time and screen x.
#[derive(Debug, Clone, Copy)]
struct Axis {
    left: f32,
    pps: f64,
    scroll: f64,
}

impl Axis {
    fn x(self, t: Time) -> f32 {
        self.left + ((t.as_secs_f64() - self.scroll) * self.pps) as f32
    }

    fn secs(self, x: f32) -> f64 {
        f64::from(x - self.left) / self.pps + self.scroll
    }
}

/// Everything a row needs besides the layer itself.
#[derive(Clone, Copy)]
struct Cx {
    axis: Axis,
    rate: FrameRate,
    time: Time,
    /// The composition's start timecode and drop-frame display.
    start: Time,
    drop: bool,
}

pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let Some(comp) = state.comp() else {
        crate::empty_state::show(ui, state);
        return;
    };
    let comp = &*comp;
    let rate = comp.frame_rate;
    let duration = comp.duration.as_secs_f64().max(0.001);

    ui.horizontal(|ui| {
        if ui
            .button("Fit")
            .on_hover_text("Fit the whole composition")
            .clicked()
        {
            state.timeline.pps = None;
            state.timeline.scroll = 0.0;
        }
        ui.weak("Ctrl+scroll zooms · scroll pans · drag bars to move/trim");
    });

    let area = ui.max_rect();
    let lane_left = area.left() + HEADER_W;
    let lane_w = (area.width() - HEADER_W).max(50.0);
    let mut pps = *state
        .timeline
        .pps
        .get_or_insert(f64::from(lane_w) / duration);
    let mut scroll = state.timeline.scroll;
    // Reveal: bring a layer that starts outside the visible time range into view.
    if let Some(id) = state.timeline.reveal
        && let Some(layer) = comp.layer(id)
    {
        let visible = f64::from(lane_w) / pps;
        let start = layer.in_point.as_secs_f64();
        if start < scroll || start > scroll + visible {
            scroll = (start - visible * 0.1).max(0.0);
            state.timeline.scroll = scroll;
        }
    }

    // Zoom and pan with the pointer over the lanes.
    if let Some(p) = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| area.contains(*p) && p.x > lane_left)
    {
        let (zoom, pan) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta.x));
        if zoom != 1.0 {
            let under = f64::from(p.x - lane_left) / pps + scroll;
            pps = (pps * f64::from(zoom)).clamp(MIN_PPS, MAX_PPS);
            scroll = under - f64::from(p.x - lane_left) / pps;
        }
        if pan != 0.0 {
            scroll -= f64::from(pan) / pps;
        }
        scroll = scroll.clamp(0.0, duration);
        state.timeline.pps = Some(pps);
        state.timeline.scroll = scroll;
    }

    let cx = Cx {
        axis: Axis {
            left: lane_left,
            pps,
            scroll,
        },
        rate,
        time: state.time,
        start: comp.start_offset,
        drop: comp.drop_frame,
    };

    ruler(ui, state, comp.duration, cx);

    state.timeline.rows.clear();
    state.timeline.rows_span = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let top = ui.cursor().top();
            for layer in &comp.layers {
                layer_rows(ui, state, layer, cx);
            }
            state.timeline.rows_span = Some((top, ui.cursor().top()));
            if comp.layers.is_empty() {
                ui.add_space(8.0);
                ui.weak("No layers. Use Layer ▸ New Solid or Layer ▸ Import Image…");
            }
        });

    drive_drag(ui, state, cx);
    drop_on_timeline(ui, state, comp, cx, area);
}

/// Items dragged from the Project panel, or files from the operating system,
/// over the timeline: shows what dropping would do and does it on release.
fn drop_on_timeline(ui: &Ui, state: &mut EditorState, comp: &Composition, cx: Cx, area: Rect) {
    use voxy_core::command::DropTarget;

    use crate::drops::{
        Mods, SNAP_PX, cancel_on_escape, dragged_items, dropped_files, ghost_bar, insert_index,
        insert_line, pointer_label, released, row_at, snap_time,
    };

    let ctx = ui.ctx().clone();
    if cancel_on_escape(&ctx) {
        return;
    }
    let Some(p) = ctx
        .input(|i| i.pointer.hover_pos())
        .filter(|p| area.contains(*p))
    else {
        return;
    };
    let files = dropped_files(&ctx);
    let dragged = dragged_items(&ctx, state.history.current());
    if dragged.is_none() && files.is_empty() {
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            ui.painter().rect_stroke(
                area.shrink(2.0),
                4.0,
                Stroke::new(2.0, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        return;
    }
    let mods = Mods::from(ctx.input(|i| i.modifiers));
    let rows: Vec<Rect> = state.timeline.rows.iter().map(|(_, r)| *r).collect();
    let centers: Vec<f32> = rows.iter().map(|r| r.center().y).collect();
    let mut time = Time::from_secs_f64(cx.axis.secs(p.x.max(cx.axis.left)).max(0.0));
    if mods.snap {
        let mut edges = vec![Time::ZERO, comp.duration, cx.time];
        edges.extend(comp.layers.iter().flat_map(|l| [l.in_point, l.out_point]));
        time = snap_time(time, &edges, cx.axis.pps, SNAP_PX);
    }
    let replace_row = mods.replace.then(|| row_at(&rows, p.y)).flatten();
    let target = match replace_row {
        Some(i) => DropTarget::ReplaceLayer {
            layer: state.timeline.rows[i].0,
        },
        None => DropTarget::Timeline {
            comp: comp.id,
            time,
            index: insert_index(&centers, p.y),
        },
    };
    // Files arrive already dropped: probe and place them now.
    if !files.is_empty() {
        let items = state.drop_items_from_files(&files);
        state.perform_drop(target, items, mods.placement());
        return;
    }
    let Some(items) = dragged else { return };
    let plan = voxy_core::command::resolve_drop(
        state.history.current(),
        &target,
        &items,
        mods.placement(),
    );
    if released(&ctx) {
        egui::DragAndDrop::clear_payload(&ctx);
        state.perform_drop(target, items, mods.placement());
        return;
    }
    // Preview.
    let painter = ui.painter();
    match &plan {
        voxy_core::command::Plan::Layers { layers, .. } => {
            let (top, bottom) = state
                .timeline
                .rows_span
                .unwrap_or((area.top() + RULER_H, area.top() + RULER_H));
            let base = layers.first().map_or(0, |l| l.index);
            let y = rows.get(base).map_or(bottom, Rect::top).max(top);
            insert_line(painter, y, area.left(), area.right());
            for (k, l) in layers.iter().enumerate() {
                let x0 = cx.axis.x(l.in_point);
                let x1 = cx.axis.x(l.in_point + l.length).max(x0 + 2.0);
                let top = y + k as f32 * ROW_H + 3.0;
                ghost_bar(
                    painter,
                    Rect::from_min_max(pos2(x0, top), pos2(x1, top + ROW_H - 6.0)),
                );
            }
        }
        voxy_core::command::Plan::Replace { .. } => {
            if let Some(i) = replace_row {
                painter.rect_stroke(
                    rows[i],
                    0.0,
                    Stroke::new(2.0, ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
        }
        _ => {}
    }
    pointer_label(&ctx, painter, p, &plan);
}

fn ruler(ui: &mut Ui, state: &mut EditorState, duration: Time, cx: Cx) {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), RULER_H), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let v = ui.visuals();
    painter.rect_filled(rect, 0.0, v.faint_bg_color);
    let lane = Rect::from_min_max(pos2(cx.axis.left, rect.top()), rect.max);
    let lane_painter = ui.painter_at(lane.with_min_x(lane.left() - 6.0));

    // Time past the end of the composition.
    let end_x = cx.axis.x(duration);
    if end_x < lane.right() {
        lane_painter.rect_filled(
            Rect::from_min_max(pos2(end_x.max(lane.left()), lane.top()), lane.max),
            0.0,
            Color32::from_black_alpha(90),
        );
    }

    // Entered through a precomp layer: shade the child times that layer
    // never shows, with a tick where its range starts and ends (§7.3).
    if let Some(crate::nav_time::ChildRange::Bounded { first, last }) = state.active_child_range() {
        let shade = Color32::from_black_alpha(90);
        let (x0, x1) = (cx.axis.x(first), cx.axis.x(last));
        if x0 > lane.left() {
            lane_painter.rect_filled(
                Rect::from_min_max(lane.min, pos2(x0.min(lane.right()), lane.bottom())),
                0.0,
                shade,
            );
        }
        if x1 < lane.right() {
            lane_painter.rect_filled(
                Rect::from_min_max(pos2(x1.max(lane.left()), lane.top()), lane.max),
                0.0,
                shade,
            );
        }
        for x in [x0, x1] {
            if x >= lane.left() && x <= lane.right() {
                lane_painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(1.0, ACCENT),
                );
            }
        }
    }

    // Tick spacing: the smallest "nice" step at least 70 px wide.
    let frame = 1.0 / cx.rate.as_f64();
    let step = [
        frame,
        2.0 * frame,
        5.0 * frame,
        10.0 * frame,
        1.0,
        2.0,
        5.0,
        10.0,
        30.0,
        60.0,
        300.0,
        600.0,
    ]
    .into_iter()
    .find(|s| s * cx.axis.pps >= 70.0)
    .unwrap_or(3600.0);
    let first = (cx.axis.scroll / step).floor() as i64;
    let last = (cx.axis.secs(lane.right()) / step).ceil() as i64;
    for n in first..=last {
        let t = Time::from_secs_f64(n as f64 * step);
        let x = cx.axis.x(t);
        painter.line_segment(
            [pos2(x, rect.bottom() - 8.0), pos2(x, rect.bottom())],
            Stroke::new(1.0, v.weak_text_color()),
        );
        if x >= lane.left() {
            lane_painter.text(
                pos2(x + 3.0, rect.top() + 2.0),
                Align2::LEFT_TOP,
                timecode_at(t, cx.rate, cx.start, cx.drop),
                FontId::proportional(10.0),
                v.weak_text_color(),
            );
        }
    }
    painter.text(
        pos2(rect.left() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        timecode_at(state.time, cx.rate, cx.start, cx.drop),
        FontId::monospace(12.0),
        v.text_color(),
    );
    playhead(&lane_painter, cx, lane, true);

    if (resp.clicked() || resp.dragged())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let last_frame = (cx.rate.time_to_frame(duration) - 1).max(0);
        let frame = (cx.axis.secs(p.x) * cx.rate.as_f64()).round() as i64;
        state.time = cx.rate.frame_to_time(frame.clamp(0, last_frame));
    }
}

fn playhead(painter: &Painter, cx: Cx, lane: Rect, head: bool) {
    let x = cx.axis.x(cx.time);
    if x < lane.left() - 1.0 || x > lane.right() + 1.0 {
        return;
    }
    painter.line_segment(
        [pos2(x, lane.top()), pos2(x, lane.bottom())],
        Stroke::new(1.5, ACCENT),
    );
    if head {
        painter.add(Shape::convex_polygon(
            vec![
                pos2(x - 5.0, lane.top()),
                pos2(x + 5.0, lane.top()),
                pos2(x, lane.top() + 7.0),
            ],
            ACCENT,
            Stroke::NONE,
        ));
    }
}

fn diamond(painter: &Painter, c: Pos2, r: f32, fill: Color32, stroke: Stroke) {
    painter.add(Shape::convex_polygon(
        vec![
            c + vec2(0.0, -r),
            c + vec2(r, 0.0),
            c + vec2(0.0, r),
            c + vec2(-r, 0.0),
        ],
        fill,
        stroke,
    ));
}

fn bar_color(layer: &Layer, time: Time) -> Color32 {
    match &layer.kind {
        LayerKind::Solid { color, .. } => {
            let [r, g, b, _] = color
                .value_at(time)
                .as_color()
                .unwrap_or(voxy_core::LinearRgba::WHITE)
                .to_srgb();
            let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            Color32::from_rgb(c(r), c(g), c(b))
        }
        LayerKind::Image { .. } => Color32::from_rgb(60, 150, 160),
        LayerKind::Video { .. } => Color32::from_rgb(70, 110, 190),
        LayerKind::Precomp { .. } => Color32::from_rgb(170, 120, 60),
        LayerKind::Placeholder { .. } => Color32::from_rgb(110, 110, 110),
    }
}

fn layer_rows(ui: &mut Ui, state: &mut EditorState, layer: &Layer, cx: Cx) {
    let selected = state.selection == Some(layer.id);
    let expanded = state.timeline.expanded.contains(&layer.id);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    state.timeline.rows.push((layer.id, rect));
    if state.timeline.reveal == Some(layer.id) {
        ui.scroll_to_rect(rect, Some(egui::Align::Center));
        state.timeline.reveal = None;
    }
    let header = Rect::from_min_size(rect.min, vec2(HEADER_W, ROW_H));
    let lane = Rect::from_min_max(pos2(cx.axis.left, rect.top()), rect.max);
    let painter = ui.painter_at(rect);
    let lane_painter = ui.painter_at(lane.with_min_x(lane.left() - 6.0));
    let weak = ui.visuals().weak_text_color();
    let text = ui.visuals().text_color();

    if selected {
        painter.rect_filled(rect, 0.0, ACCENT.linear_multiply(0.18));
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, ui.visuals().faint_bg_color),
    );

    // Header: expand arrow, visibility dot, name.
    let name_id = Id::new(("tl-name", layer.id));
    let name_resp = ui.interact(header, name_id, Sense::click());
    if name_resp.clicked() || name_resp.secondary_clicked() {
        state.selection = Some(layer.id);
    }
    // Double-click on a precomp layer goes into it (Shift: the other way).
    let shift = ui.input(|i| i.modifiers.shift);
    if name_resp.double_clicked() && matches!(layer.kind, LayerKind::Precomp { .. }) {
        state.open_precomp_layer(layer.id, shift);
    }
    name_resp.context_menu(|ui| {
        if matches!(layer.kind, LayerKind::Precomp { .. }) {
            if ui.button("Open Composition").clicked() {
                state.navigate_into_layer(layer.id);
                ui.close();
            }
            if ui.button("Open in New Tab").clicked() {
                state.open_precomp_in_own_tab(layer.id);
                ui.close();
            }
            ui.separator();
        }
        for a in [
            crate::actions::Action::RevealSourceInProject,
            crate::actions::Action::RevealInFileManager,
            crate::actions::Action::LayerDelete,
        ] {
            crate::menu::action_item(ui, state, a);
        }
    });
    let arrow = Rect::from_min_size(header.min + vec2(2.0, 0.0), vec2(18.0, ROW_H));
    if ui
        .interact(arrow, Id::new(("tl-expand", layer.id)), Sense::click())
        .clicked()
    {
        if expanded {
            state.timeline.expanded.remove(&layer.id);
        } else {
            state.timeline.expanded.insert(layer.id);
        }
    }
    let c = arrow.center();
    let tri = if expanded {
        vec![
            c + vec2(-4.0, -2.5),
            c + vec2(4.0, -2.5),
            c + vec2(0.0, 3.5),
        ]
    } else {
        vec![
            c + vec2(-2.5, -4.0),
            c + vec2(3.5, 0.0),
            c + vec2(-2.5, 4.0),
        ]
    };
    painter.add(Shape::convex_polygon(tri, weak, Stroke::NONE));

    let eye = Rect::from_min_size(header.min + vec2(22.0, 0.0), vec2(18.0, ROW_H));
    let eye_resp = ui
        .interact(eye, Id::new(("tl-vis", layer.id)), Sense::click())
        .on_hover_text("Visible");
    if eye_resp.clicked() {
        state.execute(SetLayerVisible {
            layer: layer.id,
            visible: !layer.flags.visible,
        });
    }
    if layer.flags.visible {
        painter.circle_filled(eye.center(), 4.0, text);
    } else {
        painter.circle_stroke(eye.center(), 4.0, Stroke::new(1.0, weak));
    }
    painter.text(
        pos2(header.left() + 46.0, rect.center().y),
        Align2::LEFT_CENTER,
        &layer.name,
        FontId::proportional(13.0),
        if layer.flags.visible { text } else { weak },
    );

    // Lane: the layer bar with a summary of all its keyframes.
    let x0 = cx.axis.x(layer.in_point);
    let x1 = cx.axis.x(layer.out_point);
    let bar = Rect::from_min_max(
        pos2(x0, rect.top() + 3.0),
        pos2(x1.max(x0 + 1.0), rect.bottom() - 3.0),
    );
    let base = bar_color(layer, cx.time);
    let alpha = if layer.flags.visible { 1.0 } else { 0.35 };
    lane_painter.rect_filled(bar, 3.0, base.linear_multiply(0.55 * alpha));
    lane_painter.rect_stroke(
        bar,
        3.0,
        Stroke::new(
            if selected { 2.0 } else { 1.0 },
            base.linear_multiply(alpha),
        ),
        egui::StrokeKind::Inside,
    );
    lane_painter.text(
        pos2(bar.left().max(lane.left()) + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        layer.kind.type_name(),
        FontId::proportional(11.0),
        Color32::from_white_alpha(170),
    );
    let mut summary: Vec<Time> = layer
        .properties()
        .iter()
        .filter_map(|p| p.track())
        .flat_map(|t| t.keys().iter().map(|k| k.time))
        .collect();
    summary.sort();
    summary.dedup();
    for t in summary {
        let x = cx.axis.x(t);
        if x >= lane.left() - 6.0 && x <= lane.right() + 6.0 {
            diamond(
                &lane_painter,
                pos2(x, rect.bottom() - 8.0),
                3.5,
                Color32::WHITE,
                Stroke::NONE,
            );
        }
    }

    let visible_bar = bar.intersect(lane);
    if visible_bar.is_positive() {
        let body_id = Id::new(("tl-bar", layer.id));
        let body = ui
            .interact(visible_bar, body_id, Sense::click_and_drag())
            .on_hover_cursor(CursorIcon::Grab);
        if body.clicked() {
            state.selection = Some(layer.id);
        }
        if body.double_clicked() && matches!(layer.kind, LayerKind::Precomp { .. }) {
            state.open_precomp_layer(layer.id, shift);
        }
        if body.drag_started() {
            start_layer_drag(ui, state, layer, DragKind::MoveLayer, cx);
        }
        let hw = HANDLE_W.min(bar.width() / 3.0);
        let left = Rect::from_min_size(bar.min, vec2(hw, bar.height())).intersect(lane);
        let right = Rect::from_min_max(pos2(bar.right() - hw, bar.top()), bar.max).intersect(lane);
        for (r, kind, key) in [
            (left, DragKind::TrimIn, "tl-trim-in"),
            (right, DragKind::TrimOut, "tl-trim-out"),
        ] {
            if !r.is_positive() {
                continue;
            }
            let h = ui
                .interact(r, Id::new((key, layer.id)), Sense::click_and_drag())
                .on_hover_cursor(CursorIcon::ResizeHorizontal);
            if h.drag_started() {
                start_layer_drag(ui, state, layer, kind, cx);
            }
        }
    }
    playhead(&lane_painter, cx, lane, false);

    if expanded {
        for p in layer.properties() {
            property_row(ui, state, layer, p, cx);
        }
    }
}

fn start_layer_drag(ui: &Ui, state: &mut EditorState, layer: &Layer, kind: DragKind, cx: Cx) {
    let origin_x = ui.input(|i| i.pointer.press_origin()).map_or(0.0, |p| p.x);
    state.selection = Some(layer.id);
    let drag = Drag {
        kind,
        layer: layer.id,
        orig: (layer.in_point, layer.out_point, layer.start_offset),
        keys: Vec::new(),
        origin_x,
        last_frames: 0,
    };
    state.history.begin(drag.label());
    state.timeline.drag = Some(drag);
    let _ = cx;
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Scalar(x) => format!("{x:.2}"),
        Value::Vec2(v) => format!("{:.1}, {:.1}", v.x, v.y),
        Value::Vec3(v) => format!("{:.1}, {:.1}, {:.1}", v.x, v.y, v.z),
        Value::Int(i) => i.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Color(_) => String::new(),
        Value::Enum(e) => e.to_string(),
        Value::String(s) => s.to_string(),
        Value::Layer(Some(id)) => id.to_string(),
        Value::Asset(Some(id)) => id.to_string(),
        Value::Layer(None) | Value::Asset(None) => "none".to_owned(),
        Value::Curve(c) => format!("{} points", c.points().len()),
    }
}

fn property_row(ui: &mut Ui, state: &mut EditorState, layer: &Layer, p: &Property, cx: Cx) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H - 4.0), Sense::hover());
    let header = Rect::from_min_size(rect.min, vec2(HEADER_W, rect.height()));
    let lane = Rect::from_min_max(pos2(cx.axis.left, rect.top()), rect.max);
    let painter = ui.painter_at(rect);
    let lane_painter = ui.painter_at(lane.with_min_x(lane.left() - 6.0));
    let weak = ui.visuals().weak_text_color();
    painter.rect_filled(rect, 0.0, Color32::from_black_alpha(35));
    painter.text(
        pos2(header.left() + 46.0, rect.center().y),
        Align2::LEFT_CENTER,
        p.desc.label.as_ref(),
        FontId::proportional(11.5),
        if p.is_animated() {
            ui.visuals().text_color()
        } else {
            weak
        },
    );
    painter.text(
        pos2(header.right() - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        value_text(&p.value_at(cx.time)),
        FontId::monospace(10.5),
        weak,
    );

    // Clicking empty lane space clears the key selection.
    let empty = ui.interact(lane, Id::new(("tl-lane", p.id)), Sense::click());
    if empty.clicked() {
        state.timeline.selected_keys.clear();
    }
    lane_painter.line_segment(
        [
            pos2(lane.left(), rect.center().y),
            pos2(lane.right(), rect.center().y),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(14)),
    );

    if let Some(track) = p.track() {
        // A bar between consecutive keys that hold the same value hints at
        // "nothing changes here" in the usual way.
        for w in track.keys().windows(2) {
            if w[0].value == w[1].value {
                let (xa, xb) = (cx.axis.x(w[0].time), cx.axis.x(w[1].time));
                lane_painter.line_segment(
                    [pos2(xa, rect.center().y), pos2(xb, rect.center().y)],
                    Stroke::new(2.0, Color32::from_white_alpha(40)),
                );
            }
        }
        for k in track.keys() {
            let x = cx.axis.x(k.time);
            if x < lane.left() - 8.0 || x > lane.right() + 8.0 {
                continue;
            }
            let id = (p.id, k.time);
            let is_sel = state.timeline.selected_keys.contains(&id);
            let hit = Rect::from_center_size(pos2(x, rect.center().y), vec2(14.0, rect.height()))
                .intersect(lane);
            if hit.is_positive() {
                let r = ui
                    .interact(
                        hit,
                        Id::new(("tl-key", p.id, k.time.0)),
                        Sense::click_and_drag(),
                    )
                    .on_hover_cursor(CursorIcon::PointingHand);
                let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                if r.clicked() {
                    if additive {
                        if !state.timeline.selected_keys.remove(&id) {
                            state.timeline.selected_keys.insert(id);
                        }
                    } else {
                        state.timeline.selected_keys = HashSet::from([id]);
                    }
                    state.selection = Some(layer.id);
                }
                if r.drag_started() {
                    if !is_sel {
                        if !additive {
                            state.timeline.selected_keys.clear();
                        }
                        state.timeline.selected_keys.insert(id);
                    }
                    start_key_drag(ui, state, layer);
                }
            }
            let (fill, stroke) = if is_sel {
                (ACCENT, Stroke::new(1.5, Color32::WHITE))
            } else {
                (
                    Color32::from_gray(205),
                    Stroke::new(1.0, Color32::from_gray(40)),
                )
            };
            diamond(&lane_painter, pos2(x, rect.center().y), 5.0, fill, stroke);
        }
    }
    playhead(&lane_painter, cx, lane, false);
}

fn start_key_drag(ui: &Ui, state: &mut EditorState, layer: &Layer) {
    let origin_x = ui.input(|i| i.pointer.press_origin()).map_or(0.0, |p| p.x);
    let mut keys: Vec<(PropId, Time)> = state.timeline.selected_keys.iter().copied().collect();
    keys.sort();
    let drag = Drag {
        kind: DragKind::Keys,
        layer: layer.id,
        orig: (layer.in_point, layer.out_point, layer.start_offset),
        keys,
        origin_x,
        last_frames: 0,
    };
    state.history.begin(drag.label());
    state.timeline.drag = Some(drag);
}

/// Applies a drag of `frames` frames from the original values. Returns
/// `false` (leaving the last good state in place) if the edit is rejected.
fn apply_drag(state: &mut EditorState, drag: &Drag, rate: FrameRate, frames: i64) -> bool {
    let delta = rate.frame_to_time(frames);
    state.history.rollback();
    state.history.begin(drag.label());
    let ok = match drag.kind {
        DragKind::Keys => {
            let mut groups: BTreeMap<PropId, Vec<Time>> = BTreeMap::new();
            for &(prop, time) in &drag.keys {
                groups.entry(prop).or_default().push(time);
            }
            let moved = drag.keys.iter().all(|k| k.1 + delta >= Time::ZERO)
                && groups.into_iter().all(|(prop, times)| {
                    state
                        .history
                        .execute(MoveKeyframes { prop, times, delta })
                        .is_ok()
                });
            if moved {
                state.timeline.selected_keys =
                    drag.keys.iter().map(|&(p, t)| (p, t + delta)).collect();
            }
            moved
        }
        kind => {
            let (i, o, s) = drag.orig;
            let one = rate.frame_to_time(1);
            let (ni, no, ns) = match kind {
                DragKind::MoveLayer => (i + delta, o + delta, s),
                DragKind::TrimIn => {
                    let ni = (i + delta).min(o - one);
                    (ni, o, s + (ni - i))
                }
                _ => (i, (o + delta).max(i + one), s),
            };
            state
                .history
                .execute(SetLayerTiming {
                    layer: drag.layer,
                    in_point: ni,
                    out_point: no,
                    start_offset: ns,
                })
                .is_ok()
        }
    };
    if !ok {
        // Back to the pre-drag state; the caller re-applies the last good one.
        state.history.rollback();
        state.history.begin(drag.label());
    }
    ok
}

/// Runs the active drag from the pointer position until the button is released.
fn drive_drag(ui: &Ui, state: &mut EditorState, cx: Cx) {
    let Some(mut drag) = state.timeline.drag.clone() else {
        return;
    };
    let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
    if let Some(pos) = pos {
        let secs = f64::from(pos.x - drag.origin_x) / cx.axis.pps;
        let mut frames = (secs * cx.rate.as_f64()).round() as i64;
        if drag.kind == DragKind::Keys
            && let Some(first) = drag.keys.iter().map(|k| k.1).min()
        {
            // Keyframes never move before the start of the composition.
            frames = frames.max(-cx.rate.time_to_frame(first));
        }
        if frames != drag.last_frames {
            if apply_drag(state, &drag, cx.rate, frames) {
                drag.last_frames = frames;
            } else {
                apply_drag(state, &drag, cx.rate, drag.last_frames);
            }
            state.timeline.drag = Some(drag.clone());
        }
    }
    if down {
        ui.ctx().request_repaint();
    } else {
        state.history.commit();
        state.timeline.drag = None;
    }
}

/// Deletes the selected keyframes as one undo step. Returns whether there
/// was anything to delete.
pub fn delete_selected_keys(state: &mut EditorState) -> bool {
    let doc = state.history.current().clone();
    let mut groups: BTreeMap<PropId, Vec<Time>> = BTreeMap::new();
    for &(prop, time) in &state.timeline.selected_keys {
        // Ignore keys that no longer exist (after undo, or moved by a drag).
        if doc
            .property(prop)
            .and_then(Property::track)
            .is_some_and(|t| t.key_at(time).is_some())
        {
            groups.entry(prop).or_default().push(time);
        }
    }
    state.timeline.selected_keys.clear();
    if groups.is_empty() {
        return false;
    }
    state.history.begin("Delete Keyframes");
    for (prop, times) in groups {
        state.execute(RemoveKeyframes { prop, times });
    }
    state.history.commit();
    true
}
