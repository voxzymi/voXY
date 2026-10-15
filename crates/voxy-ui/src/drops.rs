//! Dropping project items and files onto the timeline, the viewport and
//! layers (`docs/design-media-comps.md` §5).
//!
//! What a drop does is decided by `voxy_core::command::resolve_drop`; this
//! file only turns pointer positions and modifier keys into a target, draws
//! the preview of that plan, and runs the command on release.
//!
//! - **Ctrl/Cmd** sequences several items instead of stacking them.
//! - **Alt/Option** over a layer replaces that layer's source.
//! - **Shift** turns snapping off (it is on by default).
//! - **Esc** cancels the drag.

use std::path::PathBuf;

use egui::{
    Color32, Context, CursorIcon, DragAndDrop, FontId, Modifiers, Painter, Pos2, Rect, Stroke,
    StrokeKind, Ui, pos2, vec2,
};
use voxy_core::command::{DropItem, DropItems, DropTarget, Placement, Plan, resolve_drop};
use voxy_core::{Document, PoolItem, Time};

use crate::state::EditorState;
use crate::theme::ACCENT;

/// Distance within which a dropped item's start snaps to an edge or the
/// playhead, in screen points.
pub const SNAP_PX: f32 = 8.0;

/// What the modifier keys ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mods {
    /// Ctrl/Cmd: put several items one after another.
    pub sequence: bool,
    /// Alt/Option: replace a layer's source instead of adding layers.
    pub replace: bool,
    /// Shift inverts this: snapping is on unless Shift is held.
    pub snap: bool,
}

impl Mods {
    pub fn from(m: Modifiers) -> Mods {
        Mods {
            sequence: m.command,
            replace: m.alt,
            snap: !m.shift,
        }
    }

    pub fn placement(self) -> Placement {
        Placement {
            sequence: self.sequence,
        }
    }
}

/// The items a Project-panel drag stands for, in the order dragged. A folder
/// stands for what it contains (footage first, then compositions).
pub fn items_from_pool(doc: &Document, items: &[PoolItem]) -> Vec<DropItem> {
    let mut out: Vec<DropItem> = Vec::new();
    let mut push = |item: DropItem| {
        let same = |a: &DropItem| match (a, &item) {
            (DropItem::Asset(x), DropItem::Asset(y)) => x == y,
            (DropItem::Comp(x), DropItem::Comp(y)) => x == y,
            _ => false,
        };
        if !out.iter().any(same) {
            out.push(item);
        }
    };
    for &item in items {
        match item {
            PoolItem::Asset(a) => push(DropItem::Asset(a)),
            PoolItem::Comp(c) => push(DropItem::Comp(c)),
            PoolItem::Folder(_) => {
                let inside = doc.expand_items(&[item]);
                for a in doc.assets.iter().filter(|a| inside.assets.contains(&a.id)) {
                    push(DropItem::Asset(a.id));
                }
                for &c in &inside.comps {
                    push(DropItem::Comp(c));
                }
            }
        }
    }
    out
}

/// `t`, moved to the nearest of `candidates` that is within `px` screen
/// points at `pps` points per second.
pub fn snap_time(t: Time, candidates: &[Time], pps: f64, px: f32) -> Time {
    let limit = f64::from(px) / pps.max(f64::EPSILON);
    candidates
        .iter()
        .copied()
        .filter(|c| (c.as_secs_f64() - t.as_secs_f64()).abs() <= limit)
        .min_by(|a, b| {
            let da = (a.as_secs_f64() - t.as_secs_f64()).abs();
            let db = (b.as_secs_f64() - t.as_secs_f64()).abs();
            da.total_cmp(&db)
        })
        .unwrap_or(t)
}

/// Where in the layer stack a drop at screen `y` inserts, given the vertical
/// centres of the layer rows from top to bottom: above the first row goes
/// first, between two rows goes between them, below the last goes last.
pub fn insert_index(row_centers: &[f32], y: f32) -> usize {
    row_centers.iter().take_while(|&&c| c < y).count()
}

/// The row under screen `y`, if any (rows are `[top, bottom)`).
pub fn row_at(rows: &[Rect], y: f32) -> Option<usize> {
    rows.iter().position(|r| y >= r.top() && y < r.bottom())
}

impl EditorState {
    /// Probes dropped or picked files for a drop. Files that cannot be read
    /// are named in the status bar; the rest are returned.
    /// Imports files dropped from the operating system and adds them with a
    /// drop, as one undo step. The slow per-file probing runs in the
    /// background (see [`crate::import`]); a dropped folder contributes its
    /// supported files and counts the rest as skipped, and unreadable or
    /// corrupt files are named. Never a silent no-op.
    pub fn drop_files(&mut self, target: DropTarget, paths: &[PathBuf], placement: Placement) {
        self.begin_import(paths, crate::import::ImportDest::Drop { target, placement });
    }

    /// Runs a drop of already-resolved items (a Project-panel drag) as one
    /// undo step and reports what happened.
    pub fn perform_drop(&mut self, target: DropTarget, items: Vec<DropItem>, placement: Placement) {
        self.perform_drop_inner(target, items, placement, 0, &[]);
    }

    /// Shared drop execution. `skipped` and `failures` describe files left
    /// out while gathering an OS-file drop, so the single status line can
    /// mention them alongside what was placed.
    pub(crate) fn perform_drop_inner(
        &mut self,
        target: DropTarget,
        items: Vec<DropItem>,
        placement: Placement,
        skipped: usize,
        failures: &[String],
    ) {
        if items.is_empty() {
            return;
        }
        if let Plan::Blocked(why) = resolve_drop(self.history.current(), &target, &items, placement)
        {
            self.error(why);
            return;
        }
        let folder = self.target_folder();
        let opens_comp = matches!(target, DropTarget::NewComp { .. });
        let Some(report) = self.execute_reported(DropItems {
            target,
            items,
            placement,
            folder,
        }) else {
            return;
        };
        self.project.missing.invalidate();
        self.show_created(&report);
        if let Some(&layer) = report.created_layers.first()
            && !opens_comp
        {
            self.selection = Some(layer);
        }
        if let Some(&comp) = report.created_comps.first() {
            self.open_comp(comp);
        }
        let n = report.created_layers.len();
        let mut text = if report.created_comps.is_empty() {
            if n == 0 {
                "Replaced the layer's source".to_owned()
            } else {
                format!("Added {n} {}", if n == 1 { "layer" } else { "layers" })
            }
        } else {
            format!(
                "Created {} from the footage",
                if report.created_comps.len() == 1 {
                    "a composition".to_owned()
                } else {
                    format!("{} compositions", report.created_comps.len())
                }
            )
        };
        let reused = report.reused_assets.len();
        if reused > 0 {
            text = format!("{text} (reused {reused} already in the project)");
        }
        if skipped > 0 {
            let files = if skipped == 1 { "file" } else { "files" };
            text = format!("{text}; skipped {skipped} unsupported {files}");
        }
        if !failures.is_empty() {
            text = format!("{text}; could not import {}", failures.join("; "));
        }
        for w in &report.warnings {
            text.push(' ');
            text.push_str(w);
        }
        self.info(text);
    }
}

/// The Project-panel drag in progress, as drop items.
pub fn dragged_items(ctx: &Context, doc: &Document) -> Option<Vec<DropItem>> {
    let payload = DragAndDrop::payload::<Vec<PoolItem>>(ctx)?;
    let items = items_from_pool(doc, &payload);
    (!items.is_empty()).then_some(items)
}

/// Files being dropped from the operating system this frame.
pub fn dropped_files(ctx: &Context) -> Vec<PathBuf> {
    ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_path_buf())
            .collect()
    })
}

/// Esc cancels a Project-panel drag. Returns `true` if it did.
pub fn cancel_on_escape(ctx: &Context) -> bool {
    if DragAndDrop::has_payload_of_type::<Vec<PoolItem>>(ctx)
        && ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        DragAndDrop::clear_payload(ctx);
        return true;
    }
    false
}

/// Whether the Project-panel drag ends this frame (the button was released).
pub fn released(ctx: &Context) -> bool {
    ctx.input(|i| i.pointer.any_released())
}

/// A short description of what the drop will do, for the pointer label.
pub fn summary(plan: &Plan) -> String {
    match plan {
        Plan::Blocked(why) => why.clone(),
        Plan::Layers { layers, .. } => {
            let n = layers.len();
            format!("Add {n} {}", if n == 1 { "layer" } else { "layers" })
        }
        Plan::Replace { .. } => "Replace source".to_owned(),
        Plan::NewComps { comps, .. } => {
            if comps.len() == 1 {
                "New composition".to_owned()
            } else {
                format!("{} new compositions", comps.len())
            }
        }
    }
}

/// A label at the pointer: the plan's summary, red when blocked.
pub fn pointer_label(ctx: &Context, painter: &Painter, at: Pos2, plan: &Plan) {
    let blocked = matches!(plan, Plan::Blocked(_));
    if blocked {
        ctx.set_cursor_icon(CursorIcon::NotAllowed);
    }
    let color = if blocked {
        ctx.global_style().visuals.error_fg_color
    } else {
        ctx.global_style().visuals.text_color()
    };
    let text = summary(plan);
    let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), color);
    let rect = Rect::from_min_size(at + vec2(14.0, 14.0), galley.size()).expand2(vec2(5.0, 3.0));
    painter.rect_filled(rect, 3.0, Color32::from_black_alpha(200));
    painter.galley(rect.min + vec2(5.0, 3.0), galley, color);
}

/// A translucent preview bar.
pub fn ghost_bar(painter: &Painter, rect: Rect) {
    painter.rect(
        rect,
        2.0,
        ACCENT.linear_multiply(0.35),
        Stroke::new(1.0, ACCENT),
        StrokeKind::Inside,
    );
}

/// The line showing where in the stack the layers will be inserted.
pub fn insert_line(painter: &Painter, y: f32, x0: f32, x1: f32) {
    painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(2.0, ACCENT));
}

/// The cross-hair drawn on the viewport at the drop point.
pub fn drop_marker(painter: &Painter, at: Pos2) {
    let s = Stroke::new(1.5, ACCENT);
    painter.circle_stroke(at, 7.0, s);
    painter.line_segment([at - vec2(12.0, 0.0), at + vec2(12.0, 0.0)], s);
    painter.line_segment([at - vec2(0.0, 12.0), at + vec2(0.0, 12.0)], s);
}

/// The composition pixel under screen point `p`, when the composition of
/// `size` is fitted into `panel` (centered, aspect kept, as the viewport draws
/// it). Points outside the picture still map (a drop there is allowed).
pub fn view_to_comp(panel: Rect, size: [u32; 2], p: Pos2) -> Option<voxy_core::glam::DVec2> {
    let (w, h) = (size[0] as f32, size[1] as f32);
    if w <= 0.0 || h <= 0.0 || panel.width() <= 0.0 || panel.height() <= 0.0 {
        return None;
    }
    let scale = (panel.width() / w).min(panel.height() / h);
    let image = Rect::from_center_size(panel.center(), vec2(w, h) * scale);
    let local = (p - image.min) / scale;
    Some(voxy_core::glam::DVec2::new(
        f64::from(local.x),
        f64::from(local.y),
    ))
}

/// The screen point of composition pixel `point`, the inverse of
/// [`view_to_comp`] (same fit). Used to outline a layer over the viewport.
pub fn comp_to_view(panel: Rect, size: [u32; 2], point: voxy_core::glam::DVec2) -> Pos2 {
    let (w, h) = (size[0] as f32, size[1] as f32);
    let scale = (panel.width() / w).min(panel.height() / h);
    let image = Rect::from_center_size(panel.center(), vec2(w, h) * scale);
    image.min + vec2(point.x as f32, point.y as f32) * scale
}

/// Items dragged over the viewport (or files dropped on it): a marker where
/// the layers will be centered, and the drop on release. New layers go on
/// top at the playhead.
pub fn viewport_drop(
    ui: &Ui,
    state: &mut EditorState,
    comp: voxy_core::CompId,
    size: [u32; 2],
    panel: Rect,
) {
    let ctx = ui.ctx().clone();
    if cancel_on_escape(&ctx) {
        return;
    }
    let Some(p) = ctx
        .input(|i| i.pointer.hover_pos())
        .filter(|p| panel.contains(*p))
    else {
        return;
    };
    let files = dropped_files(&ctx);
    let dragged = dragged_items(&ctx, state.history.current());
    if dragged.is_none() && files.is_empty() {
        return;
    }
    let Some(pos) = view_to_comp(panel, size, p) else {
        return;
    };
    let mods = Mods::from(ctx.input(|i| i.modifiers));
    let target = DropTarget::Viewport {
        comp,
        time: state.time,
        pos,
    };
    if !files.is_empty() {
        state.drop_files(target, &files, mods.placement());
        return;
    }
    let Some(items) = dragged else { return };
    if released(&ctx) {
        DragAndDrop::clear_payload(&ctx);
        state.perform_drop(target, items, mods.placement());
        return;
    }
    let plan = resolve_drop(state.history.current(), &target, &items, mods.placement());
    // The viewport picture is drawn after this call, so the preview goes on
    // a layer above it.
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("viewport-drop-preview"),
    ));
    if !matches!(plan, Plan::Blocked(_)) {
        drop_marker(&painter, p);
    }
    pointer_label(&ctx, &painter, p, &plan);
}

#[cfg(test)]
mod tests {
    use voxy_core::command::{CreateFolder, ImportAssets, MoveToFolder};
    use voxy_core::{Asset, AssetKind, History};

    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            pixel_size: Some([32, 32]),
            ..Asset::new(AssetKind::Image, name.to_owned(), name.to_owned())
        }
    }

    #[test]
    fn modifiers_map_to_the_documented_options() {
        let none = Mods::from(Modifiers::NONE);
        assert_eq!(
            (none.sequence, none.replace, none.snap),
            (false, false, true)
        );
        let ctrl = Mods::from(Modifiers::COMMAND);
        assert!(ctrl.sequence && ctrl.placement().sequence);
        assert!(Mods::from(Modifiers::ALT).replace);
        assert!(!Mods::from(Modifiers::SHIFT).snap, "Shift inverts snapping");
    }

    #[test]
    fn a_pool_drag_keeps_order_drops_repeats_and_expands_folders() {
        let mut h = History::new(Document::empty("t"));
        let ids = h
            .execute_reported(ImportAssets {
                assets: vec![asset("a.png"), asset("b.png"), asset("c.png")],
                folder: None,
            })
            .unwrap()
            .created_assets;
        let folder = h
            .execute_reported(CreateFolder {
                name: "F".into(),
                parent: None,
            })
            .unwrap()
            .created_folders[0];
        h.execute(MoveToFolder {
            items: vec![PoolItem::Asset(ids[1]), PoolItem::Asset(ids[2])],
            folder: Some(folder),
        })
        .unwrap();
        let items = items_from_pool(
            h.current(),
            &[
                PoolItem::Asset(ids[0]),
                PoolItem::Folder(folder),
                PoolItem::Asset(ids[0]),
                PoolItem::Asset(ids[1]),
            ],
        );
        let got: Vec<_> = items
            .iter()
            .map(|i| match i {
                DropItem::Asset(a) => *a,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(got, ids, "dragged order, the folder's contents, no repeats");
        assert!(items_from_pool(h.current(), &[]).is_empty());
    }

    #[test]
    fn snapping_takes_the_nearest_edge_within_eight_points() {
        let edges = [
            Time::from_secs(1),
            Time::from_secs(2),
            Time::from_secs(2) + Time::from_millis(30),
        ];
        let pps = 100.0; // 8 points = 0.08 s
        let snap = |s: f64| snap_time(Time::from_secs_f64(s), &edges, pps, SNAP_PX);
        assert_eq!(snap(1.05), Time::from_secs(1));
        assert_eq!(snap(0.95), Time::from_secs(1));
        assert_eq!(snap(1.2), Time::from_secs_f64(1.2), "too far: unchanged");
        assert_eq!(
            snap(2.02),
            Time::from_secs(2) + Time::from_millis(30),
            "nearest wins"
        );
        assert_eq!(snap(2.0), Time::from_secs(2));
        assert_eq!(
            snap_time(Time::from_secs(5), &[], pps, SNAP_PX),
            Time::from_secs(5)
        );
        // Zooming in shrinks the time that 8 points cover.
        assert_eq!(
            snap_time(Time::from_secs_f64(1.05), &edges, 1000.0, SNAP_PX),
            Time::from_secs_f64(1.05)
        );
    }

    #[test]
    fn the_insert_index_is_above_between_or_below_the_rows() {
        let centers = [12.0, 36.0, 60.0];
        assert_eq!(insert_index(&centers, 0.0), 0, "above the first row");
        assert_eq!(insert_index(&centers, 24.0), 1, "between the first two");
        assert_eq!(insert_index(&centers, 40.0), 2);
        assert_eq!(insert_index(&centers, 500.0), 3, "below the last: appended");
        assert_eq!(insert_index(&[], 10.0), 0, "an empty stack");
    }

    #[test]
    fn rows_are_found_by_height() {
        let rows = [
            Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 24.0)),
            Rect::from_min_max(pos2(0.0, 24.0), pos2(100.0, 48.0)),
        ];
        assert_eq!(row_at(&rows, 0.0), Some(0));
        assert_eq!(row_at(&rows, 24.0), Some(1));
        assert_eq!(row_at(&rows, 48.0), None);
        assert_eq!(row_at(&rows, -1.0), None);
    }

    #[test]
    fn summaries_name_the_outcome() {
        assert_eq!(summary(&Plan::Blocked("no".into())), "no");
        assert_eq!(
            summary(&Plan::Replace {
                layer: voxy_core::LayerId(1),
                item: 0,
                warnings: vec![]
            }),
            "Replace source"
        );
    }

    #[test]
    fn the_viewport_maps_the_pointer_into_composition_pixels() {
        // A 200x100 composition in a 400x400 panel is drawn 400x200, centered.
        let panel = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 400.0));
        let at = |x: f32, y: f32| view_to_comp(panel, [200, 100], pos2(x, y)).unwrap();
        assert_eq!(at(200.0, 200.0), voxy_core::glam::DVec2::new(100.0, 50.0));
        assert_eq!(at(0.0, 100.0), voxy_core::glam::DVec2::new(0.0, 0.0));
        assert_eq!(at(400.0, 300.0), voxy_core::glam::DVec2::new(200.0, 100.0));
        // Outside the picture still maps.
        assert_eq!(at(200.0, 0.0).y, -50.0);
        assert!(view_to_comp(panel, [0, 100], pos2(1.0, 1.0)).is_none());
        let empty = Rect::from_min_max(pos2(0.0, 0.0), pos2(0.0, 0.0));
        assert!(view_to_comp(empty, [10, 10], pos2(0.0, 0.0)).is_none());
    }

    // ---- simulated drags through the real panels ----------------------

    use std::sync::Arc;

    use egui::{Event, PointerButton, RawInput};
    use voxy_core::command::AddSolidLayer;
    use voxy_core::{AssetId, CompId, CompSettings, LayerKind};

    const SCREEN: egui::Vec2 = egui::vec2(1400.0, 800.0);

    fn raw(mut events: Vec<Event>, modifiers: Modifiers) -> RawInput {
        events.insert(0, Event::ModifiersChanged(modifiers));
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            events,
            ..RawInput::default()
        }
    }

    fn release(at: Pos2) -> Event {
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }
    }

    /// A project with one composition holding `layers` solids, and `n`
    /// footage items.
    fn project(layers: usize, n: usize) -> (EditorState, CompId, Vec<AssetId>) {
        let settings = CompSettings {
            name: "Main".into(),
            width: 640,
            height: 360,
            ..CompSettings::default()
        };
        let mut s = EditorState::new(Document::with_comp("t", settings));
        let comp = s.history.current().main_comp.expect("a composition");
        for _ in 0..layers {
            s.history
                .execute(AddSolidLayer {
                    comp,
                    name: None,
                    color: voxy_core::LinearRgba::WHITE,
                    index: 0,
                })
                .expect("test setup");
        }
        let assets: Vec<Asset> = (0..n).map(|i| asset(&format!("item{i}.png"))).collect();
        let ids = if assets.is_empty() {
            Vec::new()
        } else {
            s.history
                .execute_reported(ImportAssets {
                    assets,
                    folder: None,
                })
                .expect("test setup")
                .created_assets
        };
        (s, comp, ids)
    }

    fn drag(ctx: &Context, items: Vec<PoolItem>) {
        DragAndDrop::set_payload(ctx, items);
    }

    /// Lays the timeline out once and returns it for geometry.
    fn layout_pass(ctx: &Context, s: &mut EditorState) {
        let mut out = ctx.run_ui(raw(vec![], Modifiers::NONE), |ui| {
            crate::timeline::show(ui, s);
        });
        out.textures_delta.clear();
    }

    /// Move over `at` and release, with `mods`, over the timeline.
    fn drop_at(ctx: &Context, s: &mut EditorState, at: Pos2, mods: Modifiers) {
        for events in [
            vec![Event::PointerMoved(at)],
            vec![Event::PointerMoved(at)],
            vec![release(at)],
        ] {
            let mut out = ctx.run_ui(raw(events, mods), |ui| crate::timeline::show(ui, s));
            out.textures_delta.clear();
        }
    }

    fn row_gap(s: &EditorState, between: usize) -> Pos2 {
        let rows = &s.timeline.rows;
        let y = f32::midpoint(rows[between - 1].1.bottom(), rows[between].1.top());
        pos2(700.0, y)
    }

    #[test]
    fn dropping_between_two_rows_inserts_there_in_one_undo_step() {
        let (mut s, comp, ids) = project(3, 1);
        let ctx = Context::default();
        layout_pass(&ctx, &mut s);
        let at = row_gap(&s, 1);
        let rev = s.history.revision();
        drag(&ctx, vec![PoolItem::Asset(ids[0])]);
        drop_at(&ctx, &mut s, at, Modifiers::NONE);
        let layers = &s.history.current().comp(comp).expect("comp").layers;
        assert_eq!(layers.len(), 4);
        assert!(
            matches!(layers[1].kind, LayerKind::Image { .. }),
            "between rows 0 and 1"
        );
        assert_eq!(s.selection, Some(layers[1].id), "the new layer is selected");
        assert!(!s.status.as_ref().expect("a status").is_error);
        s.history.undo();
        assert_eq!(s.history.revision(), rev);
        assert_eq!(
            s.history.current().comp(comp).expect("comp").layers.len(),
            3
        );
    }

    #[test]
    fn dropping_below_the_last_row_appends() {
        let (mut s, comp, ids) = project(2, 1);
        let ctx = Context::default();
        layout_pass(&ctx, &mut s);
        drag(&ctx, vec![PoolItem::Asset(ids[0])]);
        let at = row_gap_or_below(&s);
        drop_at(&ctx, &mut s, at, Modifiers::NONE);
        let layers = &s.history.current().comp(comp).expect("comp").layers;
        assert_eq!(layers.len(), 3);
        assert!(
            matches!(layers[2].kind, LayerKind::Image { .. }),
            "appended last"
        );
    }

    #[test]
    fn ctrl_sequences_the_items_and_the_first_is_on_top() {
        let (mut s, comp, ids) = project(1, 2);
        let ctx = Context::default();
        layout_pass(&ctx, &mut s);
        let rows = s.timeline.rows.clone();
        let above = pos2(700.0, rows[0].1.top() + 2.0);
        drag(&ctx, ids.iter().map(|&i| PoolItem::Asset(i)).collect());
        drop_at(&ctx, &mut s, above, Modifiers::COMMAND);
        let layers = &s.history.current().comp(comp).expect("comp").layers;
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0].kind.asset(), Some(ids[0]), "first item on top");
        assert_eq!(layers[1].kind.asset(), Some(ids[1]));
        assert_eq!(layers[1].in_point, layers[0].out_point, "in sequence");
    }

    #[test]
    fn alt_over_a_layer_replaces_its_source_keeping_its_timing() {
        let (mut s, comp, ids) = project(2, 1);
        let ctx = Context::default();
        layout_pass(&ctx, &mut s);
        let rows = s.timeline.rows.clone();
        let target = rows[1].0;
        let before = s
            .history
            .current()
            .find_layer(target)
            .expect("layer")
            .1
            .clone();
        drag(&ctx, vec![PoolItem::Asset(ids[0])]);
        drop_at(
            &ctx,
            &mut s,
            pos2(700.0, rows[1].1.center().y),
            Modifiers::ALT,
        );
        let doc = s.history.current();
        let after = doc.find_layer(target).expect("layer").1;
        assert_eq!(
            after.kind.asset(),
            Some(ids[0]),
            "now shows the dropped item"
        );
        assert_eq!(
            (after.in_point, after.out_point),
            (before.in_point, before.out_point)
        );
        assert_eq!(
            doc.comp(comp).expect("comp").layers.len(),
            2,
            "no layer added"
        );
    }

    #[test]
    fn a_composition_dropped_into_itself_is_refused_with_the_reason() {
        let (mut s, comp, _) = project(1, 0);
        let ctx = Context::default();
        layout_pass(&ctx, &mut s);
        let at = row_gap_or_below(&s);
        let rev = s.history.revision();
        drag(&ctx, vec![PoolItem::Comp(comp)]);
        drop_at(&ctx, &mut s, at, Modifiers::NONE);
        assert_eq!(s.history.revision(), rev, "nothing changed");
        let status = s.status.as_ref().expect("the reason is shown");
        assert!(
            status.is_error && status.text.contains("can't be placed"),
            "{status:?}"
        );
    }

    fn row_gap_or_below(s: &EditorState) -> Pos2 {
        pos2(
            700.0,
            s.timeline.rows.last().expect("rows").1.bottom() + 10.0,
        )
    }

    #[test]
    fn escape_cancels_a_drag() {
        let (mut s, comp, ids) = project(1, 1);
        let ctx = Context::default();
        layout_pass(&ctx, &mut s);
        let at = row_gap_or_below(&s);
        let rev = s.history.revision();
        drag(&ctx, vec![PoolItem::Asset(ids[0])]);
        let mut out = ctx.run_ui(raw(vec![Event::PointerMoved(at)], Modifiers::NONE), |ui| {
            crate::timeline::show(ui, &mut s);
        });
        out.textures_delta.clear();
        let esc = Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let mut out = ctx.run_ui(raw(vec![esc], Modifiers::NONE), |ui| {
            crate::timeline::show(ui, &mut s);
        });
        out.textures_delta.clear();
        assert!(!DragAndDrop::has_any_payload(&ctx), "the drag is over");
        let mut out = ctx.run_ui(raw(vec![release(at)], Modifiers::NONE), |ui| {
            crate::timeline::show(ui, &mut s);
        });
        out.textures_delta.clear();
        assert_eq!(
            s.history.revision(),
            rev,
            "releasing afterwards drops nothing"
        );
        assert_eq!(
            s.history.current().comp(comp).expect("comp").layers.len(),
            1
        );
    }

    #[test]
    fn a_viewport_drop_centers_the_layer_where_it_was_dropped() {
        let (mut s, comp, ids) = project(0, 1);
        let ctx = Context::default();
        let panel = Rect::from_min_size(pos2(100.0, 100.0), egui::vec2(640.0, 360.0));
        let at = pos2(100.0 + 320.0 + 50.0, 100.0 + 180.0 - 30.0);
        drag(&ctx, vec![PoolItem::Asset(ids[0])]);
        for events in [
            vec![Event::PointerMoved(at)],
            vec![Event::PointerMoved(at)],
            vec![release(at)],
        ] {
            let mut out = ctx.run_ui(raw(events, Modifiers::NONE), |ui| {
                viewport_drop(ui, &mut s, comp, [640, 360], panel);
            });
            out.textures_delta.clear();
        }
        let layers = &s.history.current().comp(comp).expect("comp").layers;
        assert_eq!(layers.len(), 1);
        assert_eq!(
            layers[0].transform.position.value_at(Time::ZERO).as_vec2(),
            Some(voxy_core::glam::DVec2::new(370.0, 150.0))
        );
    }

    #[test]
    fn files_dropped_on_the_timeline_are_imported_and_placed() {
        struct Dropped(std::path::PathBuf);
        impl std::fmt::Debug for Dropped {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{:?}", self.0)
            }
        }
        impl egui::DroppedFile for Dropped {
            fn path(&self) -> &std::path::Path {
                &self.0
            }
            fn bytes(&self) -> Result<Vec<u8>, String> {
                std::fs::read(&self.0).map_err(|e| e.to_string())
            }
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dropped.png");
        image::RgbaImage::new(40, 30).save(&path).expect("png");
        let (mut s, comp, _) = project(1, 0);
        let ctx = Context::default();
        layout_pass(&ctx, &mut s);
        let at = row_gap_or_below(&s);
        let mut input = raw(vec![Event::PointerMoved(at)], Modifiers::NONE);
        input.dropped_files = vec![Arc::new(Dropped(path))];
        let mut out = ctx.run_ui(input, |ui| crate::timeline::show(ui, &mut s));
        out.textures_delta.clear();
        // The drop imports in the background; pump it to completion.
        for _ in 0..2000 {
            s.poll_import(&ctx);
            if s.import_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let doc = s.history.current();
        assert_eq!(doc.assets.iter().count(), 1, "imported");
        assert_eq!(doc.comp(comp).expect("comp").layers.len(), 2, "and placed");
        s.history.undo();
        assert_eq!(
            s.history.current().assets.iter().count(),
            0,
            "one undo for both"
        );
    }

    #[test]
    fn footage_dropped_on_the_empty_timeline_makes_a_composition() {
        let mut s = EditorState::new(Document::empty("t"));
        let ids = s
            .history
            .execute_reported(ImportAssets {
                assets: vec![asset("clip.png")],
                folder: None,
            })
            .expect("test setup")
            .created_assets;
        let ctx = Context::default();
        let at = pos2(700.0, 400.0);
        DragAndDrop::set_payload(&ctx, vec![PoolItem::Asset(ids[0])]);
        for events in [
            vec![Event::PointerMoved(at)],
            vec![Event::PointerMoved(at)],
            vec![release(at)],
        ] {
            let mut out = ctx.run_ui(raw(events, Modifiers::NONE), |ui| {
                crate::timeline::show(ui, &mut s);
            });
            out.textures_delta.clear();
        }
        let doc = s.history.current();
        assert_eq!(doc.comps.len(), 1);
        let c = doc.comps.values().next().expect("comp");
        assert_eq!((c.width, c.height, c.layers.len()), (32, 32, 1));
        assert_eq!(c.name, "clip");
    }
}
