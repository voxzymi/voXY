//! The empty-state screen (`docs/design-project-panel.md` §4): what the
//! timeline area shows while no composition is open, and the viewport's
//! one-line message.
//!
//! Two tiles, New Composition and New Composition From Footage. Their layout
//! is a pure function of the available size ([`empty_state_layout`]) so it is
//! tested without a window.

use egui::{
    Context, CornerRadius, Id, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, UiBuilder,
    Vec2, WidgetInfo, WidgetType, pos2, vec2,
};

use crate::actions::Action;
use crate::state::EditorState;
use crate::theme::ACCENT;

/// Gap between tiles side by side, and stacked.
const GAP_ROW: f32 = 16.0;
const GAP_COLUMN: f32 = 12.0;
/// Below this width the tiles stack.
const STACK_BELOW: f32 = 560.0;
/// Tile height as a fraction of its width.
const ASPECT: f32 = 0.7;
const TILE_MIN: Vec2 = vec2(120.0, 84.0);
/// Room the Tip of the Day needs under the tiles, and the height under which
/// it is hidden.
const TIP_HEIGHT: f32 = 72.0;
const TIP_MIN_HEIGHT: f32 = 340.0;
/// Under this height the tiles become two compact buttons.
const COMPACT_BELOW: f32 = 200.0;
/// Space kept around everything.
const MARGIN: f32 = 8.0;

/// How the tiles are arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrangement {
    Row,
    Column,
}

/// Where everything goes, for an available size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub arrangement: Arrangement,
    /// Size of one tile.
    pub tile: Vec2,
    pub gap: f32,
    /// Compact buttons (a label, no icon) instead of tiles.
    pub compact: bool,
    pub show_tip: bool,
}

impl Layout {
    /// The size of both tiles and the gap between them.
    pub fn tiles_size(&self) -> Vec2 {
        match self.arrangement {
            Arrangement::Row => vec2(self.tile.x * 2.0 + self.gap, self.tile.y),
            Arrangement::Column => vec2(self.tile.x, self.tile.y * 2.0 + self.gap),
        }
    }

    /// The whole content: tiles, and the tip under them when it shows.
    pub fn content_size(&self) -> Vec2 {
        let tiles = self.tiles_size();
        if self.show_tip {
            vec2(tiles.x, tiles.y + MARGIN + TIP_HEIGHT)
        } else {
            tiles
        }
    }
}

/// The layout for an available `width` × `height` in points.
///
/// - Wide (>= 560): two tiles side by side; narrower: stacked.
/// - A tile is `clamp(available / 2 - gap, 220, 320)` wide (stacked:
///   `clamp(available - 32, 160, 320)`) and 0.7 of that high.
/// - Short areas shrink the tiles (never below 120 × 84), then hide the tip
///   (under 340), then replace the tiles by two compact buttons (under 200).
pub fn empty_state_layout(width: f32, height: f32) -> Layout {
    let arrangement = if width >= STACK_BELOW {
        Arrangement::Row
    } else {
        Arrangement::Column
    };
    let gap = match arrangement {
        Arrangement::Row => GAP_ROW,
        Arrangement::Column => GAP_COLUMN,
    };
    if height < COMPACT_BELOW {
        let tile = vec2(160.0_f32.min((width - 2.0 * MARGIN).max(0.0)), 32.0);
        // Two buttons only fit side by side if the width allows it.
        let arrangement = if width >= tile.x * 2.0 + GAP_ROW + 2.0 * MARGIN {
            Arrangement::Row
        } else {
            Arrangement::Column
        };
        return Layout {
            arrangement,
            tile,
            gap: GAP_COLUMN,
            compact: true,
            show_tip: false,
        };
    }
    let show_tip = height >= TIP_MIN_HEIGHT;
    let mut w = match arrangement {
        Arrangement::Row => (width / 2.0 - gap).clamp(220.0, 320.0),
        Arrangement::Column => (width - 32.0).clamp(160.0, 320.0),
    };
    let mut h = w * ASPECT;
    // Shrink until the tiles (and the tip) fit the height.
    let room = height - 2.0 * MARGIN - if show_tip { TIP_HEIGHT + MARGIN } else { 0.0 };
    let tiles_h = |h: f32| match arrangement {
        Arrangement::Row => h,
        Arrangement::Column => h * 2.0 + gap,
    };
    if tiles_h(h) > room {
        h = match arrangement {
            Arrangement::Row => room,
            Arrangement::Column => (room - gap) / 2.0,
        }
        .max(TILE_MIN.y);
        w = (h / ASPECT).clamp(TILE_MIN.x, w);
    }
    // Never wider than the area.
    let fit_w = match arrangement {
        Arrangement::Row => (width - 2.0 * MARGIN - gap) / 2.0,
        Arrangement::Column => width - 2.0 * MARGIN,
    };
    if w > fit_w {
        w = fit_w.max(0.0);
    }
    Layout {
        arrangement,
        tile: vec2(w, h),
        gap,
        compact: false,
        show_tip,
    }
}

// ------------------------------------------------------------- drawing

/// What a tile draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Icon {
    Composition,
    Footage,
}

/// The timeline area while no composition is open.
pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let avail = ui.available_rect_before_wrap();
    let layout = empty_state_layout(avail.width(), avail.height());
    let content = layout.content_size();
    let origin = pos2(
        avail.center().x - content.x / 2.0,
        (avail.center().y - content.y / 2.0).max(avail.top() + MARGIN),
    );
    let rects = tile_rects(&layout, origin);

    let new_comp = tile(
        ui,
        Id::new("empty-new-comp"),
        rects[0],
        &layout,
        "New Composition",
        Icon::Composition,
        "Create a composition from your project's default settings.",
    );
    let footage = tile(
        ui,
        Id::new("empty-from-footage"),
        rects[1],
        &layout,
        "New Composition From Footage",
        Icon::Footage,
        "Uses the footage selected in the Project panel. With nothing selected, opens a file picker.",
    );
    // Allocate the space so the parent knows it was used.
    ui.allocate_rect(avail, Sense::hover());

    if new_comp.clicked() {
        state.perform(Action::FileNewComposition);
    }
    let dropped = dropped_on(ui.ctx(), footage.rect);
    if !dropped.is_empty() {
        state.new_comp_from_files(&dropped);
    } else if footage.clicked() {
        state.perform(Action::FileNewCompositionFromFootage);
    }
}

fn tile_rects(layout: &Layout, origin: Pos2) -> [Rect; 2] {
    let first = Rect::from_min_size(origin, layout.tile);
    let step = match layout.arrangement {
        Arrangement::Row => vec2(layout.tile.x + layout.gap, 0.0),
        Arrangement::Column => vec2(0.0, layout.tile.y + layout.gap),
    };
    [first, first.translate(step)]
}

/// Files dropped on `rect` this frame.
fn dropped_on(ctx: &Context, rect: Rect) -> Vec<std::path::PathBuf> {
    ctx.input(|i| {
        if i.pointer.hover_pos().is_some_and(|p| rect.contains(p)) {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        } else {
            Vec::new()
        }
    })
}

fn tile(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    layout: &Layout,
    label: &str,
    icon: Icon,
    hint: &str,
) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    let response = response.on_hover_text(hint);
    let hovering_files = ui.input(|i| {
        !i.raw.hovered_files.is_empty() && i.pointer.hover_pos().is_some_and(|p| rect.contains(p))
    }) && icon == Icon::Footage;
    let visuals = ui.visuals();
    let hot = response.hovered() || response.is_pointer_button_down_on() || hovering_files;
    let stroke_color = if hot {
        visuals.strong_text_color()
    } else {
        visuals.weak_text_color()
    };
    let fill = if response.is_pointer_button_down_on() {
        visuals.widgets.active.bg_fill
    } else if hot {
        visuals.widgets.hovered.bg_fill
    } else {
        visuals.extreme_bg_color
    };
    let painter = ui.painter();
    let radius = CornerRadius::same(6);
    painter.rect(
        rect,
        radius,
        fill,
        Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        StrokeKind::Inside,
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(1.0),
            radius,
            Stroke::new(2.0, ACCENT),
            StrokeKind::Outside,
        );
    }
    let text_color = visuals.text_color();
    if layout.compact {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            compact_label(icon),
            egui::FontId::proportional(13.0),
            text_color,
        );
    } else {
        let art = Rect::from_center_size(
            pos2(rect.center().x, rect.top() + rect.height() * 0.38),
            vec2(rect.width() * 0.4, rect.height() * 0.34),
        );
        draw_icon(painter, art, icon, Stroke::new(1.5, stroke_color));
        painter.text(
            pos2(rect.center().x, rect.bottom() - rect.height() * 0.2),
            egui::Align2::CENTER_CENTER,
            wrap_label(label, rect.width() - 16.0),
            egui::FontId::proportional(13.0),
            text_color,
        );
    }
    // Enter and Space activate a focused tile: egui turns them into a click.
    response
}

fn compact_label(icon: Icon) -> &'static str {
    match icon {
        Icon::Composition => "New Composition",
        Icon::Footage => "From Footage",
    }
}

/// The long label on two lines when it would not fit.
fn wrap_label(label: &str, width: f32) -> String {
    // 13 pt text is about 7 points per character.
    let fits = (width / 7.0).floor() as usize;
    if label.chars().count() <= fits {
        return label.to_owned();
    }
    match label.rsplit_once(' ') {
        Some(_) if label == "New Composition From Footage" => {
            "New Composition\nFrom Footage".to_owned()
        }
        _ => label.to_owned(),
    }
}

/// Line art with painter strokes only: a frame, and inside it a small shape
/// trio (composition) or a film strip (footage).
fn draw_icon(painter: &egui::Painter, art: Rect, icon: Icon, stroke: Stroke) {
    painter.rect_stroke(art, CornerRadius::same(3), stroke, StrokeKind::Inside);
    let inner = art.shrink(art.height() * 0.2);
    match icon {
        Icon::Composition => {
            let r = inner.height() * 0.5;
            let step = inner.width() / 3.0;
            let y = inner.center().y;
            // circle, square, triangle
            painter.circle_stroke(pos2(inner.left() + step * 0.5, y), r * 0.8, stroke);
            painter.rect_stroke(
                Rect::from_center_size(pos2(inner.left() + step * 1.5, y), Vec2::splat(r * 1.5)),
                CornerRadius::ZERO,
                stroke,
                StrokeKind::Middle,
            );
            let c = pos2(inner.left() + step * 2.5, y);
            painter.add(egui::Shape::closed_line(
                vec![
                    pos2(c.x, c.y - r * 0.85),
                    pos2(c.x + r * 0.85, c.y + r * 0.7),
                    pos2(c.x - r * 0.85, c.y + r * 0.7),
                ],
                stroke,
            ));
        }
        Icon::Footage => {
            // Sprocket holes along the top and bottom, one frame between.
            let hole = art.height() * 0.09;
            let n = 5;
            for i in 0..n {
                let x = art.left() + art.width() * (i as f32 + 0.5) / n as f32;
                for y in [art.top() + hole * 2.0, art.bottom() - hole * 2.0] {
                    painter.rect_stroke(
                        Rect::from_center_size(pos2(x, y), Vec2::splat(hole)),
                        CornerRadius::ZERO,
                        stroke,
                        StrokeKind::Middle,
                    );
                }
            }
            painter.rect_stroke(
                Rect::from_center_size(art.center(), vec2(art.width() * 0.5, art.height() * 0.34)),
                CornerRadius::same(2),
                stroke,
                StrokeKind::Middle,
            );
        }
    }
}

/// The viewport while no composition is open: a message and a hint, no
/// buttons (the tiles live in the timeline area).
pub fn show_viewport_message(ui: &mut Ui) {
    let avail = ui.available_rect_before_wrap();
    ui.scope_builder(UiBuilder::new().max_rect(avail), |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((avail.height() / 2.0 - 24.0).max(0.0));
            ui.label("No composition open");
            ui.weak("Use the timeline area to create one");
        });
    });
}

/// The weak one-liner for panels that need a composition.
pub fn weak_notice(ui: &mut Ui) {
    ui.weak("No composition open");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(l: &Layout) -> [Rect; 2] {
        tile_rects(l, Pos2::ZERO)
    }

    #[test]
    fn side_by_side_from_560_points_and_stacked_below() {
        for w in [560.0, 800.0, 1600.0] {
            let l = empty_state_layout(w, 800.0);
            assert_eq!(l.arrangement, Arrangement::Row, "{w}");
            assert_eq!(l.gap, 16.0);
        }
        for w in [200.0, 359.0, 559.0] {
            let l = empty_state_layout(w, 800.0);
            assert_eq!(l.arrangement, Arrangement::Column, "{w}");
            assert_eq!(l.gap, 12.0);
        }
    }

    #[test]
    fn tile_sizes_follow_the_clamps() {
        let wide = empty_state_layout(1600.0, 800.0);
        assert_eq!(wide.tile, vec2(320.0, 224.0));
        let row560 = empty_state_layout(560.0, 800.0);
        assert_eq!(row560.tile.x, 264.0, "560 / 2 - 16");
        let stacked = empty_state_layout(359.0, 800.0);
        assert_eq!(stacked.tile.x, 320.0_f32.min(359.0 - 32.0));
        let narrow = empty_state_layout(200.0, 800.0);
        assert_eq!(narrow.tile.x, 168.0, "200 - 32");
        for l in [wide, row560, stacked, narrow] {
            assert!((l.tile.y - l.tile.x * 0.7).abs() < 0.01);
        }
    }

    #[test]
    fn tiles_never_overlap_and_always_fit() {
        for w in (136..=2000).step_by(8).map(|w| w as f32) {
            for h in (200..=1200).step_by(8).map(|h| h as f32) {
                let l = empty_state_layout(w, h);
                let [a, b] = rects(&l);
                assert!(!a.intersects(b), "overlap at {w}x{h}: {a:?} {b:?}");
                let content = l.content_size();
                assert!(content.x <= w + 0.01, "too wide at {w}x{h}: {content:?}");
                assert!(content.y <= h + 0.01, "too tall at {w}x{h}: {content:?}");
                if !l.compact {
                    assert!(l.tile.x >= 120.0_f32.min(w - 16.0) - 0.01, "{w}x{h}: {l:?}");
                }
            }
        }
    }

    #[test]
    fn the_tip_hides_when_short_and_tiles_become_buttons_when_shorter() {
        assert!(empty_state_layout(800.0, 340.0).show_tip);
        assert!(!empty_state_layout(800.0, 339.0).show_tip);
        assert!(!empty_state_layout(800.0, 200.0).compact);
        let c = empty_state_layout(800.0, 199.0);
        assert!(c.compact && !c.show_tip);
        assert_eq!(c.tile.y, 32.0);
    }

    #[test]
    fn short_areas_shrink_the_tiles_but_not_below_the_minimum() {
        let tall = empty_state_layout(359.0, 800.0);
        let short = empty_state_layout(359.0, 260.0);
        assert!(short.tile.y < tall.tile.y);
        assert!(short.tile.x >= 120.0 && short.tile.y >= 84.0 - 0.01);
        let l = empty_state_layout(359.0, 200.0);
        assert!(l.tile.y >= 84.0 - 0.01 && l.content_size().y <= 200.0);
    }

    #[test]
    fn compact_buttons_stack_when_the_width_cannot_hold_two() {
        assert_eq!(
            empty_state_layout(800.0, 150.0).arrangement,
            Arrangement::Row
        );
        assert_eq!(
            empty_state_layout(300.0, 150.0).arrangement,
            Arrangement::Column
        );
    }

    use egui::{Event, PointerButton, RawInput};
    use voxy_core::Document;

    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    /// A file the window system says was dropped.
    #[derive(Debug)]
    struct Dropped(PathBuf);

    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    const SCREEN: Vec2 = vec2(800.0, 500.0);

    /// Runs one frame of the empty state over the whole screen.
    fn frame(ctx: &Context, state: &mut EditorState, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            events,
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| show(ui, state));
        out.textures_delta.clear();
    }

    /// The centres of the two tiles for a screen of [`SCREEN`].
    fn centres() -> [Pos2; 2] {
        let l = empty_state_layout(SCREEN.x, SCREEN.y);
        let content = l.content_size();
        let origin = pos2(
            SCREEN.x / 2.0 - content.x / 2.0,
            (SCREEN.y / 2.0 - content.y / 2.0).max(MARGIN),
        );
        tile_rects(&l, origin).map(|r| r.center())
    }

    fn click(ctx: &Context, state: &mut EditorState, at: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(ctx, state, vec![Event::PointerMoved(at)]);
        frame(ctx, state, vec![button(true)]);
        frame(ctx, state, vec![button(false)]);
        frame(ctx, state, vec![]);
    }

    fn fresh() -> (Context, EditorState) {
        let mut state = EditorState::new(Document::empty("t"));
        state.dialogs = Box::new(crate::dialogs::NoDialogs);
        (Context::default(), state)
    }

    #[test]
    fn the_first_tile_opens_the_new_composition_dialog() {
        let (ctx, mut state) = fresh();
        frame(&ctx, &mut state, vec![]);
        click(&ctx, &mut state, centres()[0]);
        assert!(state.comp_dialog.is_some());
        assert!(state.history.current().comps.is_empty(), "nothing made yet");
    }

    #[test]
    fn the_second_tile_with_a_cancelled_picker_does_nothing() {
        let (ctx, mut state) = fresh();
        frame(&ctx, &mut state, vec![]);
        let rev = state.history.revision();
        click(&ctx, &mut state, centres()[1]);
        assert_eq!(state.history.revision(), rev);
        assert!(state.comp_dialog.is_none() && state.status.is_none());
    }

    #[test]
    fn files_dropped_on_the_second_tile_make_a_composition() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pic.png");
        image::RgbaImage::new(32, 24).save(&path).unwrap();
        let (ctx, mut state) = fresh();
        let at = centres()[1];
        frame(&ctx, &mut state, vec![Event::PointerMoved(at)]);
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            dropped_files: vec![Arc::new(Dropped(path))],
            events: vec![Event::PointerMoved(at)],
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| show(ui, &mut state));
        out.textures_delta.clear();
        let doc = state.history.current();
        assert_eq!(doc.comps.len(), 1);
        assert_eq!(doc.comps.values().next().unwrap().width, 32);
    }

    #[test]
    fn files_dropped_elsewhere_are_ignored() {
        let (ctx, mut state) = fresh();
        let at = centres()[0];
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            dropped_files: vec![Arc::new(Dropped("/nowhere/pic.png".into()))],
            events: vec![Event::PointerMoved(at)],
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| show(ui, &mut state));
        out.textures_delta.clear();
        assert!(state.history.current().comps.is_empty() && state.status.is_none());
    }
}
