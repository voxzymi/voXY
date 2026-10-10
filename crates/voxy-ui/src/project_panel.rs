//! The Project panel: footage, compositions and folders in one tree.
//!
//! Drawing only. What the rows are, how they sort and filter, and every
//! edit live in `project_view` and `EditorState`, where they are tested.
//! Icons are painted from strokes, so there are no image files and the
//! artwork is voXY's own.

use std::collections::HashMap;

use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Modifiers, Painter, Pos2, Rect, Sense, Shape, Stroke,
    Ui, pos2, vec2,
};
use voxy_core::command::RemoveMode;
use voxy_core::{AssetId, Document, FolderId, PoolItem};

use crate::actions::Action;
use crate::menu::{action_item, action_item_labelled};
use crate::project_view::{
    Column, ItemKind, Row, SortKey, TypeFilter, duration_text, fitting_columns, folder_len,
    item_raw, rate_text, settings_summary, size_text,
};
use crate::state::EditorState;
use crate::theme::ACCENT;

const ROW_H: f32 = 22.0;
const INDENT: f32 = 14.0;
const FONT: f32 = 12.5;

/// Draws the panel. `problems` are decode failures the viewport knows
/// about, by asset.
pub fn show(ui: &mut Ui, state: &mut EditorState, problems: &HashMap<AssetId, String>) {
    state.refresh_missing();
    let doc = state.history.current().clone();
    let rev = state.history.revision();
    let panel = ui.max_rect();

    os_drops(ui, state, panel);
    toolbar(ui, state);
    ui.add_space(2.0);
    let rows = state.project.rows(&doc, rev);
    info_header(ui, state, &doc);

    let footer_h = 48.0;
    let list_h = (ui.available_height() - footer_h).max(60.0);
    let width = ui.available_width();
    ui.allocate_ui(vec2(width, list_h), |ui| {
        list(ui, state, &doc, &rows, problems);
    });
    footer(ui, state, &doc);
    remove_dialog(ui.ctx(), state);
}

// ------------------------------------------------------------ toolbar

fn tip(state: &EditorState, ui: &Ui, action: Action, text: &str) -> String {
    match state.keymap.bindings(action).first() {
        Some(s) => format!("{text} ({})", ui.ctx().format_shortcut(s)),
        None => text.to_owned(),
    }
}

fn toolbar(ui: &mut Ui, state: &mut EditorState) {
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 28.0).max(60.0);
        let search = &mut state.project.view.search;
        ui.add(
            egui::TextEdit::singleline(search)
                .hint_text("Search")
                .desired_width(w),
        );
        if !search.is_empty() && ui.small_button("×").on_hover_text("Clear search").clicked() {
            search.clear();
        }
    });
    ui.horizontal_wrapped(|ui| {
        let hint = tip(
            state,
            ui,
            Action::FileNewComposition,
            "Create a new composition",
        );
        if ui.button("New Composition").on_hover_text(hint).clicked() {
            state.perform(Action::FileNewComposition);
        }
        let hint = tip(state, ui, Action::ProjectNewFolder, "Create a new folder");
        if ui.button("New Folder").on_hover_text(hint).clicked() {
            state.new_folder();
        }
        let hint = tip(
            state,
            ui,
            Action::FileImport,
            "Import footage into the project",
        );
        if ui.button("Import…").on_hover_text(hint).clicked() {
            state.import_dialog();
        }
    });
    ui.horizontal_wrapped(|ui| {
        let view = &mut state.project.view;
        egui::ComboBox::from_id_salt("project-filter")
            .selected_text(view.filter.label())
            .show_ui(ui, |ui| {
                for f in TypeFilter::ALL {
                    ui.selectable_value(&mut view.filter, f, f.label());
                }
            })
            .response
            .on_hover_text("Show only one kind of item");
        ui.menu_button(format!("Sort: {}", view.sort.label()), |ui| {
            for k in SortKey::ALL {
                ui.radio_value(&mut view.sort, k, k.label());
            }
            ui.separator();
            ui.checkbox(&mut view.descending, "Descending");
        })
        .response
        .on_hover_text("Sort by Type to group compositions together");
        ui.menu_button("Columns", |ui| columns_menu(ui, view));
    });
}

fn columns_menu(ui: &mut Ui, view: &mut crate::project_view::ViewSettings) {
    ui.add_enabled(false, egui::Checkbox::new(&mut true, "Name"));
    for c in Column::ALL {
        let mut on = view.columns.contains(&c);
        if ui.checkbox(&mut on, c.label()).changed() {
            if on {
                view.columns.insert(c);
            } else {
                view.columns.remove(&c);
            }
        }
    }
}

// -------------------------------------------------------- info header

fn info_header(ui: &mut Ui, state: &mut EditorState, doc: &Document) {
    let sel = &state.project.selection.items;
    if sel.len() != 1 {
        return;
    }
    let item = *sel.iter().next().expect("one item");
    let rev = state.history.revision();
    let Some(row) = state.project.row_for(doc, rev, item) else {
        return;
    };
    let weak = ui.visuals().weak_text_color();
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(4.0)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Thumbnails of footage and compositions arrive with the
                // preview renderer; until then the large icon stands in.
                let (rect, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
                ui.painter()
                    .rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
                paint_icon(
                    ui.painter(),
                    Rect::from_center_size(rect.center(), vec2(30.0, 30.0)),
                    row.kind,
                    ui.visuals().text_color(),
                    row.missing,
                );
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&row.name).strong());
                    let summary = settings_summary(&row);
                    ui.colored_label(
                        weak,
                        if summary.is_empty() {
                            row.kind.label().to_owned()
                        } else {
                            format!("{} · {summary}", row.kind.label())
                        },
                    );
                    match item {
                        PoolItem::Folder(f) => {
                            ui.colored_label(weak, format!("{} items", folder_len(doc, f)));
                        }
                        PoolItem::Asset(a) => {
                            if let Some(a) = doc.assets.get(a) {
                                ui.colored_label(weak, &a.path);
                            }
                            if row.missing {
                                ui.colored_label(ui.visuals().warn_fg_color, "File missing");
                            }
                        }
                        PoolItem::Comp(_) => {}
                    }
                    if row.duration.is_some() {
                        ui.colored_label(
                            weak,
                            format!(
                                "Duration {} · {}",
                                duration_text(row.duration, row.rate),
                                used_text(row.used)
                            ),
                        );
                    } else if row.used.is_some() {
                        ui.colored_label(weak, used_text(row.used));
                    }
                });
            });
        });
}

fn used_text(used: Option<usize>) -> String {
    match used {
        Some(0) | None => "Not used".to_owned(),
        Some(1) => "Used in 1 layer".to_owned(),
        Some(n) => format!("Used in {n} layers"),
    }
}

// --------------------------------------------------------------- list

struct ListCx<'a> {
    doc: &'a Document,
    rows: &'a [Row],
    cols: Vec<Column>,
    name_w: f32,
    problems: &'a HashMap<AssetId, String>,
    mods: Modifiers,
}

fn list(
    ui: &mut Ui,
    state: &mut EditorState,
    doc: &Document,
    rows: &[Row],
    problems: &HashMap<AssetId, String>,
) {
    let rect = ui.available_rect_before_wrap();
    // The background goes down first so rows, added later, win the pointer.
    let bg = ui.interact(rect, Id::new("project-list-bg"), Sense::click());
    if bg.clicked() {
        state.project.focused = true;
        if !ui.input(|i| i.modifiers.command || i.modifiers.shift) {
            state.project.selection.clear();
        }
    }
    if bg.double_clicked() {
        state.import_dialog();
    }
    bg.context_menu(|ui| empty_menu(ui, state));

    let width = rect.width();
    let cols = fitting_columns(&state.project.view.columns, width);
    let name_w = width - cols.iter().map(|c| c.width()).sum::<f32>();
    let cx = ListCx {
        doc,
        rows,
        cols,
        name_w,
        problems,
        mods: ui.input(|i| i.modifiers),
    };
    header_row(ui, state, &cx);

    if doc.assets.iter().next().is_none() && doc.comps.is_empty() && doc.folders.is_empty() {
        empty_hint(ui, state);
    } else if rows.is_empty() {
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.weak("No items match.");
            if ui.small_button("Clear search and filter").clicked() {
                state.project.view.search.clear();
                state.project.view.filter = TypeFilter::All;
            }
        });
    }

    let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
    if let Some(item) = state.project.scroll_to.take()
        && let Some(i) = rows.iter().position(|r| r.item == item)
    {
        let offset = (i as f32 * ROW_H - ui.available_height() / 2.0 + ROW_H / 2.0).max(0.0);
        area = area.vertical_scroll_offset(offset);
    }
    area.show_rows(ui, ROW_H, rows.len(), |ui, range| {
        for i in range {
            draw_row(ui, state, &cx, i);
        }
    });

    // Dropped on empty space: back to the top level.
    if let Some(items) = bg.dnd_release_payload::<Vec<PoolItem>>() {
        state.move_items((*items).clone(), None);
    }
    drag_label(ui.ctx(), ui.max_rect());
}

fn header_row(ui: &mut Ui, state: &mut EditorState, cx: &ListCx<'_>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    let weak = ui.visuals().weak_text_color();
    let view = &mut state.project.view;
    let font = FontId::proportional(FONT);
    // The sort direction is a small painted triangle (the default font has
    // no arrow glyphs).
    let mark = |center: Pos2, descending: bool| {
        let s = 3.0;
        let pts = if descending {
            vec![
                center + vec2(-s, -s * 0.6),
                center + vec2(s, -s * 0.6),
                center + vec2(0.0, s * 0.7),
            ]
        } else {
            vec![
                center + vec2(-s, s * 0.6),
                center + vec2(s, s * 0.6),
                center + vec2(0.0, -s * 0.7),
            ]
        };
        painter.add(Shape::convex_polygon(pts, weak, Stroke::NONE));
    };
    let mut headings: Vec<(SortKey, &str, f32, f32)> =
        vec![(SortKey::Name, "Name", rect.left() + 6.0, cx.name_w - 6.0)];
    let mut x = rect.left() + cx.name_w;
    for c in &cx.cols {
        headings.push((c.sort_key(), c.label(), x + 4.0, c.width() - 4.0));
        x += c.width();
    }
    let mut hit = None;
    for (key, name, left, width) in headings {
        let col = Rect::from_min_size(pos2(left - 4.0, rect.top()), vec2(width + 4.0, ROW_H));
        let galley = painter.with_clip_rect(col).text(
            pos2(left, rect.center().y),
            Align2::LEFT_CENTER,
            name,
            font.clone(),
            weak,
        );
        if view.sort == key {
            mark(
                pos2(left + galley.width() + 9.0, rect.center().y),
                view.descending,
            );
        }
        if resp.interact_pointer_pos().is_some_and(|p| col.contains(p)) {
            hit = Some(key);
        }
    }
    if resp.clicked() {
        let key = hit.unwrap_or(SortKey::Name);
        if view.sort == key {
            view.descending = !view.descending;
        } else {
            view.sort = key;
            view.descending = false;
        }
    }
    resp.on_hover_text("Click a heading to sort");
}

fn empty_hint(ui: &mut Ui, state: &mut EditorState) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.weak("Double-click or drag files here to import");
        ui.add_space(6.0);
        if ui.button("Import…").clicked() {
            state.import_dialog();
        }
    });
}

#[allow(clippy::too_many_lines)] // one row: icon, name or rename field, columns, menu, drag
fn draw_row(ui: &mut Ui, state: &mut EditorState, cx: &ListCx<'_>, i: usize) {
    let row = &cx.rows[i];
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(
        rect,
        Id::new(("project-row", row.item)),
        Sense::click_and_drag(),
    );
    let selected = state.project.selection.items.contains(&row.item);
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let warn = ui.visuals().warn_fg_color;
    let problem = match row.item {
        PoolItem::Asset(a) => cx.problems.get(&a),
        _ => None,
    };
    let bad = row.missing || problem.is_some();

    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 0.0, ACCENT.linear_multiply(0.28));
    } else if resp.hovered() {
        painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    }

    // Folders accept drops.
    if let PoolItem::Folder(f) = row.item {
        if let Some(p) = resp.dnd_hover_payload::<Vec<PoolItem>>() {
            let ok = can_drop(cx.doc, &p, Some(f));
            let color = if ok { ACCENT } else { warn };
            painter.rect_stroke(
                rect.shrink(1.0),
                3.0,
                Stroke::new(1.5, color),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(p) = resp.dnd_release_payload::<Vec<PoolItem>>()
            && can_drop(cx.doc, &p, Some(f))
        {
            state.move_items((*p).clone(), Some(f));
        }
    } else if let Some(p) = resp.dnd_release_payload::<Vec<PoolItem>>() {
        // Dropped on an item: into the folder that item is in.
        let into = cx.doc.item_folder(row.item);
        if can_drop(cx.doc, &p, into) {
            state.move_items((*p).clone(), into);
        }
    }

    // Disclosure arrow, icon, name.
    let mut x = rect.left() + 6.0 + row.depth as f32 * INDENT;
    if row.expandable {
        let arrow = Rect::from_min_size(pos2(x - 2.0, rect.top()), vec2(14.0, ROW_H));
        let r = ui.interact(arrow, Id::new(("project-arrow", row.item)), Sense::click());
        let c = if r.hovered() { text } else { weak };
        paint_arrow(&painter, arrow.center(), row.expanded, c);
        if r.clicked()
            && let PoolItem::Folder(f) = row.item
        {
            toggle_folder(state, f);
        }
    }
    x += 14.0;
    let icon = Rect::from_min_size(pos2(x, rect.center().y - 8.0), vec2(16.0, 16.0));
    paint_icon(&painter, icon, row.kind, if bad { warn } else { text }, bad);
    x += 22.0;
    let name_rect = Rect::from_min_max(
        pos2(x, rect.top()),
        pos2(rect.left() + cx.name_w - 4.0, rect.bottom()),
    );

    let renaming = state
        .project
        .rename
        .as_ref()
        .is_some_and(|r| r.item == row.item);
    if renaming {
        rename_field(ui, state, name_rect);
    } else {
        painter.with_clip_rect(name_rect).text(
            pos2(name_rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            &row.name,
            FontId::proportional(FONT),
            if bad { warn } else { text },
        );
    }

    let mut cx_left = rect.left() + cx.name_w;
    for c in &cx.cols {
        let value = match c {
            Column::Type => row.kind.label().to_owned(),
            Column::Size => size_text(row.size),
            Column::FrameRate => rate_text(row.rate),
            Column::Duration => duration_text(row.duration, row.rate),
            Column::Used => row.used.map_or_else(String::new, |n| n.to_string()),
            Column::DateAdded => format!("#{}", item_raw(row.item)),
        };
        let col = Rect::from_min_size(pos2(cx_left, rect.top()), vec2(c.width(), ROW_H));
        painter.with_clip_rect(col).text(
            pos2(cx_left + 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            value,
            FontId::proportional(FONT),
            weak,
        );
        cx_left += c.width();
    }

    // Interaction.
    let resp = if renaming {
        resp
    } else {
        resp.on_hover_text(row_tooltip(cx.doc, row, problem.map(String::as_str)))
    };
    if resp.clicked() {
        state.project.focused = true;
        state
            .project
            .selection
            .click(cx.rows, i, cx.mods.command, cx.mods.shift);
    }
    if resp.double_clicked() {
        match row.item {
            PoolItem::Comp(c) => state.open_comp(c),
            PoolItem::Folder(f) => toggle_folder(state, f),
            PoolItem::Asset(_) => {}
        }
    }
    if resp.secondary_clicked() {
        state.project.focused = true;
        if !selected {
            state.project.selection.set_one(row.item);
        }
    }
    if resp.drag_started() {
        let items = if selected {
            state.project.selection.in_display_order(cx.rows)
        } else {
            state.project.selection.set_one(row.item);
            vec![row.item]
        };
        resp.dnd_set_drag_payload(items);
    }
    if resp.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    resp.context_menu(|ui| item_menu(ui, state, row));
}

fn toggle_folder(state: &mut EditorState, f: FolderId) {
    let expanded = &mut state.project.view.expanded;
    if !expanded.remove(&f) {
        expanded.insert(f);
    }
}

/// Whether dropping `items` into `folder` is allowed: not onto themselves
/// or into their own subfolders, and something would actually move.
fn can_drop(doc: &Document, items: &[PoolItem], folder: Option<FolderId>) -> bool {
    let cycles = |i: &PoolItem| match (*i, folder) {
        (PoolItem::Folder(g), Some(f)) => doc.folder_within(f, g),
        _ => false,
    };
    !items.iter().any(cycles) && items.iter().any(|&i| doc.item_folder(i) != folder)
}

fn row_tooltip(doc: &Document, row: &Row, problem: Option<&str>) -> String {
    use std::fmt::Write as _;
    let mut tip = String::new();
    match row.item {
        PoolItem::Asset(a) => {
            if let Some(a) = doc.assets.get(a) {
                tip.push_str(&a.path);
                if let Some([w, h]) = a.pixel_size {
                    let _ = write!(tip, "\n{w} × {h} px");
                }
                if let Some(d) = a.duration {
                    let _ = write!(tip, ", {:.2} s", d.as_secs_f64());
                }
                if let Some(r) = a.frame_rate {
                    let _ = write!(tip, ", {r}");
                }
            }
            if row.missing {
                tip.push_str("\nMissing: the file could not be found. Use Relink… to point to it.");
            }
            if let Some(e) = problem {
                let _ = write!(tip, "\nCannot play: {e}");
            }
        }
        PoolItem::Comp(_) => {
            let _ = write!(
                tip,
                "{}\nDuration {}",
                settings_summary(row),
                duration_text(row.duration, row.rate)
            );
            let _ = write!(tip, "\n{}", used_text(row.used));
        }
        PoolItem::Folder(f) => {
            let _ = write!(tip, "{} items", folder_len(doc, f));
        }
    }
    tip
}

fn rename_field(ui: &mut Ui, state: &mut EditorState, rect: Rect) {
    let Some(mut rn) = state.project.rename.take() else {
        return;
    };
    let mut te = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let out = egui::TextEdit::singleline(&mut rn.text)
        .desired_width(rect.width())
        .show(&mut te);
    if rn.fresh {
        out.response.request_focus();
        let mut st = out.state.clone();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(rn.text.chars().count()),
        )));
        st.store(ui.ctx(), out.response.id);
        rn.fresh = false;
    }
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if out.response.lost_focus() {
        if !escape {
            state.rename_item(rn.item, rn.text);
        }
        // Done (committed or cancelled): leave `rename` cleared.
    } else {
        state.project.rename = Some(rn);
    }
}

// ------------------------------------------------------- context menus

fn item_menu(ui: &mut Ui, state: &mut EditorState, row: &Row) {
    if let PoolItem::Comp(c) = row.item
        && ui.button("Open Composition").clicked()
    {
        state.open_comp(c);
        ui.close();
    }
    if let PoolItem::Comp(c) = row.item
        && ui.button("Composition Settings…").clicked()
    {
        state.open_comp_settings(c);
        ui.close();
    }
    for a in [
        Action::ProjectRename,
        Action::ProjectDuplicate,
        Action::ProjectRemove,
    ] {
        action_item(ui, state, a);
    }
    ui.separator();
    reveal_in_composition_menu(ui, state);
    action_item(ui, state, Action::RevealInFileManager);
    if let PoolItem::Asset(a) = row.item
        && row.missing
        && ui.button("Relink…").clicked()
    {
        state.relink(a);
        ui.close();
    }
    ui.separator();
    empty_menu(ui, state);
}

/// "Reveal in Composition": a submenu when several compositions use the
/// selection, a single entry for one, a disabled entry for none.
pub(crate) fn reveal_in_composition_menu(ui: &mut Ui, state: &mut EditorState) {
    let doc = state.history.current().clone();
    let targets = state.comp_targets();
    match targets.len() {
        0 => {
            ui.add_enabled(
                false,
                egui::Button::new(Action::RevealInComposition.label()),
            )
            .on_disabled_hover_text(crate::reveal::NOT_USED);
        }
        1 => {
            action_item(ui, state, Action::RevealInComposition);
        }
        _ => {
            let labels = crate::reveal::menu_labels(&doc, &targets);
            ui.menu_button(Action::RevealInComposition.label(), |ui| {
                for (t, label) in targets.iter().zip(labels) {
                    if ui.button(label).clicked() {
                        state.reveal_in_composition(t);
                        ui.close();
                    }
                }
            });
        }
    }
}

/// The context menu of empty space, also the lower part of an item's menu.
fn empty_menu(ui: &mut Ui, state: &mut EditorState) {
    for (action, label) in [
        (Action::FileNewComposition, "New Composition"),
        (
            Action::FileNewCompositionFromFootage,
            "New Composition From Footage…",
        ),
        (Action::ProjectNewFolder, "New Folder"),
        (Action::FileImport, "Import…"),
        (Action::ProjectSelectAll, "Select All"),
    ] {
        action_item_labelled(ui, state, action, label);
    }
}

// ---------------------------------------------------- footer & dialogs

fn footer(ui: &mut Ui, state: &mut EditorState, doc: &Document) {
    let (items, comps) = crate::project_view::ProjectState::counts(doc);
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!(
            "{items} {}, {comps} {}",
            if items == 1 { "item" } else { "items" },
            if comps == 1 {
                "composition"
            } else {
                "compositions"
            }
        ))
        .on_hover_text("Footage and compositions; folders are not counted");
        let hint = tip(
            state,
            ui,
            Action::FileNewComposition,
            "Create a new composition",
        );
        if ui
            .button("Create New Composition")
            .on_hover_text(hint)
            .clicked()
        {
            state.perform(Action::FileNewComposition);
        }
    });
}

fn remove_dialog(ctx: &egui::Context, state: &mut EditorState) {
    let Some(p) = state.project.pending_remove.clone() else {
        return;
    };
    let mut choice: Option<Option<RemoveMode>> = None;
    let modal = egui::Modal::new(Id::new("project-remove")).show(ctx, |ui| {
        ui.heading("Remove from Project");
        let n = p.items.len() + p.contained;
        ui.label(format!(
            "{n} {} will be removed from the project. Files on disk are not deleted.",
            if n == 1 { "item" } else { "items" }
        ));
        if p.layers > 0 {
            ui.label(format!(
                "{} {} in {} {} use {}.",
                p.layers,
                if p.layers == 1 { "layer" } else { "layers" },
                p.comps,
                if p.comps == 1 {
                    "composition"
                } else {
                    "compositions"
                },
                if n == 1 { "it" } else { "them" },
            ));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if p.layers > 0 {
                if ui.button("Replace Layers with Placeholders").clicked() {
                    choice = Some(Some(RemoveMode::PlaceholderLayers));
                }
                if ui.button("Delete Layers").clicked() {
                    choice = Some(Some(RemoveMode::DeleteLayers));
                }
            } else if ui.button("Remove").clicked() {
                choice = Some(Some(RemoveMode::PlaceholderLayers));
            }
            if ui.button("Cancel").clicked() {
                choice = Some(None);
            }
        });
    });
    if modal.should_close() {
        choice.get_or_insert(None);
    }
    match choice {
        Some(Some(mode)) => state.remove_items(p.items, mode),
        Some(None) => state.project.pending_remove = None,
        None => {}
    }
}

/// Files dragged in from the operating system.
fn os_drops(ui: &mut Ui, state: &mut EditorState, panel: Rect) {
    let (dropped, hovering, pointer) = ui.input(|i| {
        (
            i.raw.dropped_files.clone(),
            !i.raw.hovered_files.is_empty(),
            i.pointer.hover_pos(),
        )
    });
    let inside = pointer.is_some_and(|p| panel.contains(p));
    if hovering && inside {
        ui.painter().rect_stroke(
            panel.shrink(2.0),
            4.0,
            Stroke::new(2.0, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    if inside && !dropped.is_empty() {
        let paths: Vec<_> = dropped
            .into_iter()
            .map(|f| f.path().to_path_buf())
            .collect();
        if !paths.is_empty() {
            state.import_paths(&paths);
        }
    }
}

/// A small label at the pointer while items are dragged inside the panel.
/// Over the timeline and viewport those panels say what the drop will do.
fn drag_label(ctx: &egui::Context, panel: Rect) {
    let Some(items) = egui::DragAndDrop::payload::<Vec<PoolItem>>(ctx) else {
        return;
    };
    let Some(pos) = ctx.pointer_hover_pos().filter(|p| panel.contains(*p)) else {
        return;
    };
    egui::Area::new(Id::new("project-drag-label"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + vec2(14.0, 10.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(if items.len() == 1 {
                    "Move item".to_owned()
                } else {
                    format!("Move {} items", items.len())
                });
            });
        });
}

// ------------------------------------------------------------- icons

fn paint_arrow(p: &Painter, center: Pos2, open: bool, color: Color32) {
    let s = 3.5;
    let pts = if open {
        vec![
            center + vec2(-s, -s * 0.5),
            center + vec2(s, -s * 0.5),
            center + vec2(0.0, s * 0.8),
        ]
    } else {
        vec![
            center + vec2(-s * 0.5, -s),
            center + vec2(-s * 0.5, s),
            center + vec2(s * 0.8, 0.0),
        ]
    };
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// voXY's own project-item icons, drawn as outlines in `rect` (16 px or
/// larger). `badge` adds the warning mark for missing media.
fn paint_icon(p: &Painter, rect: Rect, kind: ItemKind, color: Color32, badge: bool) {
    let u = rect.width() / 16.0;
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * u;
    let stroke = Stroke::new(1.3 * u.max(0.8), color);
    match kind {
        ItemKind::Folder => {
            p.add(Shape::closed_line(
                vec![
                    at(1.5, 4.0),
                    at(6.0, 4.0),
                    at(7.5, 5.8),
                    at(14.5, 5.8),
                    at(14.5, 13.5),
                    at(1.5, 13.5),
                ],
                stroke,
            ));
        }
        ItemKind::Composition => {
            // Two stacked layers: the back one peeks out above and right.
            p.add(Shape::line(
                vec![at(4.5, 3.0), at(14.0, 3.0), at(14.0, 10.5)],
                stroke,
            ));
            p.rect_stroke(
                Rect::from_min_max(at(1.5, 5.0), at(11.5, 13.5)),
                1.5 * u,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.circle_stroke(at(5.0, 9.2), 1.4 * u, stroke);
            p.line_segment([at(7.5, 10.5), at(10.0, 8.0)], stroke);
        }
        ItemKind::Image => {
            p.rect_stroke(
                Rect::from_min_max(at(1.5, 2.5), at(14.5, 13.5)),
                1.5 * u,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.circle_stroke(at(5.0, 6.0), 1.3 * u, stroke);
            p.add(Shape::line(
                vec![
                    at(2.5, 12.5),
                    at(6.5, 8.5),
                    at(9.0, 11.0),
                    at(11.0, 9.0),
                    at(13.5, 12.5),
                ],
                stroke,
            ));
        }
        ItemKind::Video => {
            p.rect_stroke(
                Rect::from_min_max(at(1.5, 2.5), at(14.5, 13.5)),
                1.5 * u,
                stroke,
                egui::StrokeKind::Inside,
            );
            for y in [4.5, 7.5, 10.5] {
                p.circle_filled(at(3.4, y), 0.7 * u, color);
                p.circle_filled(at(12.6, y), 0.7 * u, color);
            }
            p.add(Shape::convex_polygon(
                vec![at(6.3, 5.3), at(6.3, 10.7), at(10.0, 8.0)],
                color,
                Stroke::NONE,
            ));
        }
    }
    if badge {
        let warn = Color32::from_rgb(240, 170, 40);
        p.add(Shape::convex_polygon(
            vec![at(9.0, 15.8), at(12.0, 9.8), at(15.0, 15.8)],
            warn,
            Stroke::NONE,
        ));
        p.line_segment(
            [at(12.0, 11.8), at(12.0, 13.6)],
            Stroke::new(1.1 * u.max(0.8), Color32::BLACK),
        );
        p.circle_filled(at(12.0, 14.9), 0.6 * u, Color32::BLACK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxy_core::command::CreateFolder;
    use voxy_core::{Document, History};

    #[test]
    fn dropping_respects_cycles_and_no_ops() {
        let mut h = History::new(Document::empty("T"));
        let outer = h
            .execute_reported(CreateFolder {
                name: "Outer".into(),
                parent: None,
            })
            .unwrap()
            .created_folders[0];
        let inner = h
            .execute_reported(CreateFolder {
                name: "Inner".into(),
                parent: Some(outer),
            })
            .unwrap()
            .created_folders[0];
        let doc = h.current();
        assert!(
            can_drop(doc, &[PoolItem::Folder(inner)], None),
            "out to the top level"
        );
        assert!(
            !can_drop(doc, &[PoolItem::Folder(outer)], Some(inner)),
            "into its own child"
        );
        assert!(
            !can_drop(doc, &[PoolItem::Folder(outer)], Some(outer)),
            "into itself"
        );
        assert!(
            !can_drop(doc, &[PoolItem::Folder(inner)], Some(outer)),
            "already there"
        );
        assert!(can_drop(
            doc,
            &[PoolItem::Folder(outer), PoolItem::Folder(inner)],
            None
        ));
    }

    #[test]
    fn used_text_reads_naturally() {
        assert_eq!(used_text(None), "Not used");
        assert_eq!(used_text(Some(0)), "Not used");
        assert_eq!(used_text(Some(1)), "Used in 1 layer");
        assert_eq!(used_text(Some(3)), "Used in 3 layers");
    }
}
