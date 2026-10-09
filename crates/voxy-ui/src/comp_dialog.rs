//! The Composition Settings dialog (drawing). Its rules live in
//! `comp_draft`; this file lays the fields out and reports OK or Cancel.

use egui::{
    Align, Color32, ComboBox, Context, DragValue, Id, Key, Layout, Modal, Response, RichText,
    Sense, Stroke, TextEdit, Ui, vec2,
};
use voxy_core::{FrameRate, LinearRgba, PreviewResolution};

use crate::comp_draft::{
    Draft, Field, Mode, PIXEL_ASPECTS, Preset, Stage, Tab, builtin_presets, color_name,
    frame_aspect_text, memory_text, pixel_aspect_name, timecode_readout, validate_preset_name,
};
use crate::state::EditorState;
use crate::theme::ACCENT;

/// Frame rates in the dropdown next to the field.
const FPS_CHOICES: [&str; 8] = ["23.976", "24", "25", "29.97", "30", "50", "59.94", "60"];

/// Dialog width in points. Narrower windows shrink it to fit.
const WIDTH: f32 = 560.0;

enum Outcome {
    None,
    Ok,
    Cancel,
}

/// Draws the dialog if one is open and acts on OK and Cancel.
pub fn show(ctx: &Context, state: &mut EditorState) {
    let Some(mut draft) = state.comp_dialog.take() else {
        return;
    };
    let outcome = draw(ctx, state, &mut draft);
    state.comp_dialog = Some(draft);
    match outcome {
        Outcome::Ok => state.confirm_comp_dialog(),
        Outcome::Cancel => state.cancel_comp_dialog(),
        Outcome::None => state.update_comp_preview(),
    }
}

fn presets(state: &EditorState) -> Vec<Preset> {
    let mut all = builtin_presets();
    all.extend(state.prefs.user_presets.iter().map(Preset::from));
    all
}

fn draw(ctx: &Context, state: &mut EditorState, d: &mut Draft) -> Outcome {
    let mut outcome = Outcome::None;
    let avail = ctx.content_rect().width();
    let width = WIDTH.min((avail - 32.0).max(280.0));
    let modal = Modal::new(Id::new("voxy-comp-settings")).show(ctx, |ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.heading("Composition Settings");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Close (Esc)").clicked() {
                    outcome = Outcome::Cancel;
                }
            });
        });
        ui.add_space(4.0);
        match d.stage {
            Stage::Editing => editing(ui, state, d, &mut outcome),
            Stage::Confirm => confirm(ui, state, d, &mut outcome),
        }
    });
    // Keyboard: Escape cancels (or backs out of naming a preset); Enter
    // confirms from any single-line field.
    let (esc, enter) = ctx.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter)));
    if matches!(outcome, Outcome::None) {
        if (esc || modal.should_close()) && d.saving_preset.is_some() {
            d.saving_preset = None;
        } else if esc || modal.should_close() {
            if d.stage == Stage::Confirm {
                d.stage = Stage::Editing;
            } else {
                outcome = Outcome::Cancel;
            }
        } else if enter && d.saving_preset.is_none() && !d.has_errors() {
            outcome = Outcome::Ok;
        }
    }
    outcome
}

// ------------------------------------------------------------ editing

fn editing(ui: &mut Ui, state: &mut EditorState, d: &mut Draft, outcome: &mut Outcome) {
    // Name.
    ui.horizontal(|ui| {
        ui.label("Composition Name:");
        let out = TextEdit::singleline(&mut d.name)
            .desired_width(ui.available_width())
            .show(ui);
        if d.fresh {
            out.response.request_focus();
            let mut st = out.state.clone();
            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(d.name.chars().count()),
            )));
            st.store(ui.ctx(), out.response.id);
            d.fresh = false;
        }
        outline(ui, &out.response, d.error_for(Field::Name).is_some());
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut d.tab, Tab::Basic, "Basic");
        ui.selectable_value(&mut d.tab, Tab::Advanced, "Advanced");
    });
    ui.separator();
    match d.tab {
        Tab::Basic => basic(ui, state, d),
        Tab::Advanced => advanced(ui, d),
    }
    ui.separator();
    issues(ui, d);
    if let Mode::Edit(id) = d.mode
        && let Some(comp) = state.history.current().comp(id)
    {
        ui.weak(d.summary(comp));
    }
    ui.add_space(4.0);
    footer(ui, d, outcome);
}

fn basic(ui: &mut Ui, state: &mut EditorState, d: &mut Draft) {
    let all = presets(state);
    egui::Grid::new("comp-basic")
        .num_columns(2)
        .spacing([10.0, 7.0])
        .show(ui, |ui| {
            // Preset.
            ui.label("Preset:");
            ui.horizontal_wrapped(|ui| {
                let matched = d.matching_preset(&all);
                ComboBox::from_id_salt("comp-preset")
                    .selected_text(matched.map_or("Custom", |i| all[i].name.as_str()))
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        let _ = ui.selectable_label(matched.is_none(), "Custom");
                        for (i, p) in all.iter().enumerate() {
                            if ui.selectable_label(matched == Some(i), &p.name).clicked() {
                                d.apply_preset(p);
                            }
                        }
                    });
                if ui.button("Save preset").clicked() && d.saving_preset.is_none() {
                    d.saving_preset = Some(String::new());
                }
                let deletable = matched.filter(|&i| !all[i].builtin);
                let del = ui.add_enabled(deletable.is_some(), egui::Button::new("Delete preset"));
                let del = if deletable.is_none() {
                    del.on_disabled_hover_text(if matched.is_some() {
                        "Built-in presets cannot be deleted."
                    } else {
                        "Choose one of your own presets to delete it."
                    })
                } else {
                    del
                };
                if del.clicked()
                    && let Some(i) = deletable
                {
                    state.prefs.remove_preset(&all[i].name);
                    state.save_prefs();
                }
            });
            ui.end_row();

            // Width and height.
            ui.label("");
            ui.vertical(|ui| {
                let mut w = d.width;
                ui.horizontal(|ui| {
                    if number(
                        ui,
                        "Width:",
                        &mut w,
                        1,
                        100_000,
                        "px",
                        d.error_for(Field::Width).is_some(),
                    ) {
                        d.set_width(w);
                    }
                });
                let mut h = d.height;
                ui.horizontal(|ui| {
                    if number(
                        ui,
                        "Height:",
                        &mut h,
                        1,
                        100_000,
                        "px",
                        d.error_for(Field::Height).is_some(),
                    ) {
                        d.set_height(h);
                    }
                });
                let mut lock = d.lock;
                if ui.checkbox(&mut lock, d.lock_label()).changed() {
                    d.set_lock(lock);
                }
            });
            ui.end_row();

            // Pixel aspect.
            ui.label("Pixel Aspect Ratio:");
            ui.horizontal_wrapped(|ui| {
                let name = pixel_aspect_name(d.pixel_aspect);
                ComboBox::from_id_salt("comp-pa")
                    .selected_text(name)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for (n, pa) in PIXEL_ASPECTS {
                            if ui.selectable_label(d.pixel_aspect == pa, n).clicked() {
                                d.set_pixel_aspect(pa);
                            }
                        }
                        if ui.selectable_label(name == "Custom", "Custom").clicked() {
                            d.set_pa_text(d.pa_text.clone());
                        }
                    });
                if name == "Custom" {
                    let mut t = d.pa_text.clone();
                    let r = ui.add(TextEdit::singleline(&mut t).desired_width(54.0));
                    if r.changed() {
                        d.set_pa_text(t);
                    }
                    outline(ui, &r, d.error_for(Field::PixelAspect).is_some());
                }
                ui.weak("Frame Aspect Ratio:");
                ui.label(frame_aspect_text(d.width, d.height, d.pixel_aspect));
            });
            ui.end_row();

            // Frame rate.
            ui.label("Frame Rate:");
            ui.horizontal_wrapped(|ui| {
                let mut t = d.fps_text.clone();
                let r = ui.add(TextEdit::singleline(&mut t).desired_width(60.0));
                if r.changed() {
                    d.set_fps_text(t);
                }
                outline(ui, &r, d.error_for(Field::FrameRate).is_some());
                ComboBox::from_id_salt("comp-fps")
                    .selected_text("")
                    .width(14.0)
                    .show_ui(ui, |ui| {
                        for c in FPS_CHOICES {
                            if ui.selectable_label(d.fps_text == c, c).clicked() {
                                d.set_fps_text(c.to_owned());
                            }
                        }
                    })
                    .response
                    .on_hover_text("Common frame rates");
                ui.label("frames per second");
                let enabled = d.drop_frame_enabled();
                ui.add_enabled_ui(enabled, |ui| {
                    let mut drop = d.drop_frame;
                    ComboBox::from_id_salt("comp-drop")
                        .selected_text(if drop { "Drop Frame" } else { "Non-Drop Frame" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut drop, true, "Drop Frame");
                            ui.selectable_value(&mut drop, false, "Non-Drop Frame");
                        })
                        .response
                        .on_disabled_hover_text(
                            "Drop-frame timecode exists only for 29.97 and 59.94 fps.",
                        );
                    if drop != d.drop_frame {
                        d.set_drop_frame(drop);
                    }
                });
            });
            ui.end_row();
        });
    if let Ok(r) = d.frame_rate()
        && r.den() != 1
        && r != FrameRate::FPS_23_976
        && !r.supports_drop_frame()
    {
        ui.weak(format!("Stored as {}/{} fps.", r.num(), r.den()));
    }
    ui.separator();
    egui::Grid::new("comp-basic-2")
        .num_columns(2)
        .spacing([10.0, 7.0])
        .show(ui, |ui| {
            // Preview resolution.
            ui.label("Resolution:");
            ui.horizontal_wrapped(|ui| {
                let tip = "Preview resolution: how large the viewport renders this composition. \
                           It never changes the composition's size and never affects export, \
                           which renders at full size.";
                ComboBox::from_id_salt("comp-res")
                    .selected_text(d.resolution.label())
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for r in [
                            PreviewResolution::Full,
                            PreviewResolution::Half,
                            PreviewResolution::Third,
                            PreviewResolution::Quarter,
                        ] {
                            ui.selectable_value(&mut d.resolution, r, r.label());
                        }
                        let custom = matches!(d.resolution, PreviewResolution::Custom(_));
                        if ui.selectable_label(custom, "Custom").clicked() && !custom {
                            d.resolution = PreviewResolution::Custom(5);
                        }
                    })
                    .response
                    .on_hover_text(tip);
                if let PreviewResolution::Custom(n) = &mut d.resolution {
                    let r = ui.add(DragValue::new(n).range(0..=200).prefix("1/"));
                    outline(ui, &r, d.error_for(Field::Preview).is_some());
                }
                ui.weak(memory_text(
                    d.width,
                    d.height,
                    d.resolution,
                    state.history.current().meta.working_space,
                ))
                .on_hover_text(tip);
            });
            ui.end_row();

            // Start timecode and duration.
            ui.label("Start Timecode:");
            timecode_row(ui, d, true);
            ui.end_row();
            ui.label("Duration:");
            timecode_row(ui, d, false);
            ui.end_row();
        });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Background Color:");
        let c = d.background.to_srgb();
        let mut rgb = [
            (c[0] * 255.0).round() as u8,
            (c[1] * 255.0).round() as u8,
            (c[2] * 255.0).round() as u8,
        ];
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            d.background = LinearRgba::from_srgb8(rgb[0], rgb[1], rgb[2], 255);
        }
        ui.add_enabled(false, egui::Button::new("Pick"))
            .on_disabled_hover_text(
                "Sampling a color from anywhere on screen is not available here. \
                 Use the swatch to pick or type a color.",
            );
        ui.label(color_name(d.background));
    });
    save_preset_row(ui, state, d, &all);
}

fn timecode_row(ui: &mut Ui, d: &mut Draft, start: bool) {
    let (rate, drop) = (d.frame_rate().unwrap_or(FrameRate::FPS_30), d.drop_frame);
    ui.horizontal_wrapped(|ui| {
        let mut t = if start {
            d.start_text.clone()
        } else {
            d.duration_text.clone()
        };
        let r = ui.add(TextEdit::singleline(&mut t).desired_width(96.0));
        if r.changed() {
            if start {
                d.set_start_text(t);
            } else {
                d.set_duration_text(t);
            }
        }
        // Tidy the text once the user leaves the field.
        if r.lost_focus() {
            d.reformat_times();
        }
        let field = if start { Field::Start } else { Field::Duration };
        outline(ui, &r, d.error_for(field).is_some());
        let time = if start { d.start } else { d.duration };
        ui.weak(timecode_readout(time, rate, drop));
    });
}

fn save_preset_row(ui: &mut Ui, state: &mut EditorState, d: &mut Draft, all: &[Preset]) {
    let Some(mut name) = d.saving_preset.take() else {
        return;
    };
    let mut keep = true;
    ui.horizontal(|ui| {
        ui.label("Preset name:");
        let r = ui.add(TextEdit::singleline(&mut name).desired_width(180.0));
        r.request_focus();
        let verdict = validate_preset_name(&name, &builtin_presets());
        let save = ui.add_enabled(verdict.is_ok(), egui::Button::new("Save"));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if (save.clicked() || enter)
            && let Ok(n) = &verdict
        {
            match d.as_user_preset(n.clone()) {
                Ok(p) => {
                    state.prefs.upsert_preset(p);
                    state.save_prefs();
                    keep = false;
                }
                Err(e) => state.error(e),
            }
        }
        if ui.button("Cancel").clicked() {
            keep = false;
        }
        if let Err(e) = verdict
            && !name.is_empty()
        {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    });
    let _ = all;
    if keep {
        d.saving_preset = Some(name);
    }
}

fn advanced(ui: &mut Ui, d: &mut Draft) {
    ui.label(RichText::new("When the size of an existing composition changes").strong());
    ui.radio_value(&mut d.keep_layers, true, "Keep layers as they are");
    ui.add_enabled_ui(d.keep_layers, |ui| {
        ui.horizontal(|ui| {
            ui.label("Place the old canvas at:");
            anchor_grid(ui, &mut d.anchor);
        });
    });
    ui.radio_value(
        &mut d.keep_layers,
        false,
        "Scale layers to fit the new size",
    );
    ui.add_space(6.0);
    ui.label(RichText::new("When the frame rate of an existing composition changes").strong());
    ui.checkbox(
        &mut d.snap_keys,
        "Snap keyframes and layer edges to the nearest frame",
    );
    ui.weak("Keyframes never move in time on their own; this only moves ones that would fall between frames.");
}

/// The 3×3 anchor grid: where the old canvas sits in the new one.
fn anchor_grid(ui: &mut Ui, anchor: &mut (u8, u8)) {
    egui::Grid::new("comp-anchor")
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            for row in 0..3u8 {
                for col in 0..3u8 {
                    let on = *anchor == (col, row);
                    let (rect, r) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                    let p = ui.painter();
                    p.rect_filled(
                        rect,
                        3.0,
                        if on {
                            ACCENT
                        } else {
                            ui.visuals().widgets.inactive.bg_fill
                        },
                    );
                    if r.hovered() {
                        p.rect_stroke(
                            rect,
                            3.0,
                            Stroke::new(1.0, ACCENT),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if r.clicked() {
                        *anchor = (col, row);
                    }
                    r.on_hover_text("Where the old canvas sits in the resized one");
                }
                ui.end_row();
            }
        });
}

fn issues(ui: &mut Ui, d: &Draft) {
    let err = ui.visuals().error_fg_color;
    let warn = ui.visuals().warn_fg_color;
    for i in d.issues() {
        ui.colored_label(if i.is_error { err } else { warn }, i.message);
    }
}

fn footer(ui: &mut Ui, d: &mut Draft, outcome: &mut Outcome) {
    ui.horizontal(|ui| {
        if d.is_edit() {
            ui.checkbox(&mut d.live_preview, "Preview").on_hover_text(
                "Show the changes in the viewport while you edit. Cancel reverts them.",
            );
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let ok = ui.add_enabled(
                !d.has_errors(),
                egui::Button::new(RichText::new("OK").strong()).fill(ACCENT.linear_multiply(0.7)),
            );
            if ok.clicked() {
                *outcome = Outcome::Ok;
            }
            if ui.button("Cancel").clicked() {
                *outcome = Outcome::Cancel;
            }
        });
    });
}

// ------------------------------------------------------------- confirm

fn confirm(ui: &mut Ui, state: &mut EditorState, d: &mut Draft, outcome: &mut Outcome) {
    let Mode::Edit(id) = d.mode else { return };
    let Some(comp) = state.history.current().comp(id).cloned() else {
        *outcome = Outcome::Cancel;
        return;
    };
    ui.label(RichText::new("How should this change be applied?").strong());
    if d.size_changed(&comp) && !comp.layers.is_empty() {
        ui.add_space(4.0);
        ui.label(format!(
            "The size changes from {} × {} to {} × {}.",
            comp.width, comp.height, d.width, d.height
        ));
        ui.radio_value(
            &mut d.keep_layers,
            true,
            "Keep layers as they are (default)",
        );
        ui.add_enabled_ui(d.keep_layers, |ui| {
            ui.horizontal(|ui| {
                ui.label("Place the old canvas at:");
                anchor_grid(ui, &mut d.anchor);
            });
        });
        ui.radio_value(
            &mut d.keep_layers,
            false,
            "Scale layers to fit the new size",
        );
    }
    if let Ok(rate) = d.frame_rate()
        && rate != comp.frame_rate
    {
        let og = voxy_core::command::off_grid(&comp, rate);
        if og.keyframes + og.layer_edges > 0 {
            ui.add_space(6.0);
            ui.label(format!(
                "{} keyframes and {} layer edges fall between frames at {}.",
                og.keyframes, og.layer_edges, rate
            ));
            ui.radio_value(&mut d.snap_keys, true, "Snap them to the nearest frame");
            ui.radio_value(
                &mut d.snap_keys,
                false,
                "Leave them off the frame grid (default)",
            );
        }
    }
    ui.add_space(6.0);
    ui.weak(d.summary(&comp));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("Apply").strong())
                        .fill(ACCENT.linear_multiply(0.7)),
                )
                .clicked()
            {
                // Stage stays `Confirm`, so confirming applies for real.
                d.stage = Stage::Confirm;
                *outcome = Outcome::Ok;
            }
            if ui.button("Back").clicked() {
                d.stage = Stage::Editing;
            }
            if ui.button("Cancel").clicked() {
                *outcome = Outcome::Cancel;
            }
        });
    });
}

// ------------------------------------------------------------- helpers

/// A red outline around a field with a problem.
fn outline(ui: &Ui, r: &Response, bad: bool) {
    if bad {
        ui.painter().rect_stroke(
            r.rect.expand(1.0),
            3.0,
            Stroke::new(1.5, Color32::from_rgb(230, 70, 70)),
            egui::StrokeKind::Outside,
        );
    }
}

/// How fast dragging changes a value: 1 per point, 10× with Shift, 0.1× with
/// Ctrl/Cmd.
pub fn scrub_speed(shift: bool, command: bool) -> f64 {
    if shift {
        10.0
    } else if command {
        0.1
    } else {
        1.0
    }
}

/// A numeric field: click the value to type, or drag the label or the
/// value to scrub. Returns whether the value changed.
fn number(
    ui: &mut Ui,
    label: &str,
    value: &mut u32,
    min: u32,
    max: u32,
    unit: &str,
    bad: bool,
) -> bool {
    let before = *value;
    let mods = ui.input(|i| i.modifiers);
    let speed = scrub_speed(mods.shift, mods.command);
    // egui divides the speed by 10 while Shift is the only modifier held,
    // so compensate to get the documented 10× instead.
    let drag_speed = if mods.shift_only() {
        speed * 10.0
    } else {
        speed
    };
    let id = Id::new(("scrub", label));
    let lab = ui.add(egui::Label::new(label).sense(Sense::drag()));
    if lab.hovered() || lab.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if lab.dragged() {
        let mut acc: f64 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
        acc += f64::from(lab.drag_delta().x) * speed;
        let whole = acc.trunc();
        acc -= whole;
        ui.data_mut(|d| d.insert_temp(id, acc));
        *value = (i64::from(*value) + whole as i64).clamp(i64::from(min), i64::from(max)) as u32;
    }
    let r = ui.add(
        DragValue::new(value)
            .range(min..=max)
            .speed(drag_speed)
            .suffix(format!(" {unit}")),
    );
    outline(ui, &r, bad);
    *value != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_speeds() {
        assert_eq!(scrub_speed(false, false), 1.0);
        assert_eq!(scrub_speed(true, false), 10.0);
        assert_eq!(scrub_speed(false, true), 0.1);
    }

    fn run(width: f32, height: f32, state: &mut EditorState) {
        let ctx = Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(width, height),
            )),
            ..Default::default()
        };
        for _ in 0..3 {
            ctx.run_ui(input.clone(), |ui| show(ui.ctx(), state))
                .textures_delta
                .clear();
        }
    }

    #[test]
    fn the_dialog_draws_at_two_window_sizes_in_both_modes_and_tabs() {
        for (w, h) in [(1440.0, 900.0), (640.0, 480.0)] {
            let mut s = EditorState::new(EditorState::untitled());
            s.open_new_comp_dialog();
            run(w, h, &mut s);
            s.comp_dialog.as_mut().unwrap().tab = Tab::Advanced;
            run(w, h, &mut s);
            s.cancel_comp_dialog();
            let id = {
                s.open_new_comp_dialog();
                s.confirm_comp_dialog();
                s.comp_id().unwrap()
            };
            s.open_comp_settings(id);
            run(w, h, &mut s);
            let d = s.comp_dialog.as_mut().unwrap();
            d.set_width(1280);
            d.live_preview = true;
            run(w, h, &mut s);
            assert!(s.comp_dialog.is_some(), "still open at {w}x{h}");
        }
    }

    #[test]
    fn the_dialog_fits_a_small_window() {
        let ctx = Context::default();
        let mut s = EditorState::new(EditorState::untitled());
        s.open_new_comp_dialog();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(600.0, 420.0),
            )),
            ..Default::default()
        };
        let mut width = 0.0;
        for _ in 0..3 {
            ctx.run_ui(input.clone(), |ui| {
                let ctx = ui.ctx().clone();
                show(&ctx, &mut s);
                width = ctx.globally_used_rect().width();
            })
            .textures_delta
            .clear();
        }
        assert!(width <= 600.0, "dialog used {width} of 600 points");
    }
}
