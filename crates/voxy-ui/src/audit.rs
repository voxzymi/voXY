//! The "no active composition" audit (`docs/design-project-panel.md` §3).
//!
//! An empty project is where every new project starts, so nothing may assume a
//! composition, a layer or a keyframe exists. Two checks, run over several
//! situations:
//!
//! 1. **Every registered action**, run in each situation with the file pickers
//!    and dialogs stubbed to "cancel": no panic; the document is untouched
//!    except by the few actions allowed to change it; and no action creates a
//!    composition behind the user's back.
//! 2. **Every panel is drawn** headlessly (egui runs a frame without a window or
//!    GPU), several frames in a row so layout settles.

use egui::{Pos2, RawInput, Rect, vec2};
use egui_dock::TabViewer;
use voxy_core::command::{AddKeyframe, AddSolidLayer, SetEasing, SetInterpolation, SetProperty};
use voxy_core::{
    BounceParams, Interp, JumpMode, LinearRgba, PoolItem, SpringParams, StepsParams, Time, Value,
};

use crate::actions::Action;
use crate::dialogs::NoDialogs;
use crate::panels::{Panels, Tab};
use crate::state::EditorState;

const TABS: [Tab; 5] = [
    Tab::Viewport,
    Tab::Timeline,
    Tab::Graph,
    Tab::Properties,
    Tab::Project,
];

/// A new project: what the app starts with.
fn empty() -> EditorState {
    let mut s = EditorState::new(EditorState::untitled());
    s.dialogs = Box::new(NoDialogs);
    s
}

/// One composition and nothing in it.
fn with_comp() -> EditorState {
    let mut s = empty();
    let doc = s.history.current().clone();
    let mut settings = doc.meta.settings.new_comp.clone();
    settings.name = "Main".into();
    let folder = None;
    s.create_composition(settings, folder)
        .expect("a composition");
    s
}

/// A solid with keyed rotation and opacity (every kind of easing), the layer
/// selected, every key selected, and the graph editor pointed at rotation.
fn animated() -> EditorState {
    let mut s = with_comp();
    let comp = s.comp_id().unwrap();
    s.execute(AddSolidLayer {
        comp,
        name: None,
        color: LinearRgba::WHITE,
        index: 0,
    });
    let layer = s.history.current().comp(comp).unwrap().layers[0].clone();
    s.selection = Some(layer.id);
    let easings = [
        Interp::Spring(SpringParams::default()),
        Interp::Bounce(BounceParams::default()),
        Interp::Steps(StepsParams {
            count: 5,
            jump: JumpMode::Both,
            hold: 0.2,
        }),
        Interp::Linear,
    ];
    for prop in [layer.transform.rotation.id, layer.transform.opacity.id] {
        s.execute(AddKeyframe {
            prop,
            time: Time::ZERO,
        });
        for (i, secs) in [1i64, 2, 3, 4].into_iter().enumerate() {
            s.execute(SetProperty {
                prop,
                time: Time::from_secs(secs),
                value: Value::Scalar(i as f64 * 0.2),
            });
        }
        for (i, interp) in easings.into_iter().enumerate() {
            s.execute(SetInterpolation {
                prop,
                times: vec![Time::from_secs(i as i64)],
                interp: Interp::Linear,
            });
            s.execute(SetEasing {
                prop,
                time: Time::from_secs(i as i64),
                interp,
            });
        }
        s.timeline
            .selected_keys
            .extend((0..=4).map(|secs| (prop, Time::from_secs(secs))));
    }
    s.graph.prop = Some(layer.transform.rotation.id);
    s
}

/// Draws every panel for `frames` frames.
fn draw(state: &mut EditorState, frames: usize) {
    let ctx = egui::Context::default();
    for _ in 0..frames {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0))),
            ..RawInput::default()
        };
        // No renderer consumes the output, so the texture uploads are dropped
        // explicitly (egui panics on a silently dropped `TexturesDelta`).
        let mut out = ctx.run_ui(input, |ui| {
            let mut panels = Panels {
                state,
                viewport: None,
            };
            for mut tab in TABS {
                ui.scope(|ui| panels.ui(ui, &mut tab));
            }
        });
        out.textures_delta.clear();
        crate::comp_dialog::show(&ctx, state);
        crate::footage::show_prompt(&ctx, state);
    }
}

#[test]
fn every_panel_draws_with_no_composition() {
    draw(&mut empty(), 3);
}

#[test]
fn every_panel_draws_with_an_empty_composition() {
    draw(&mut with_comp(), 3);
}

#[test]
fn every_panel_draws_with_animated_layers_and_selected_keys() {
    let mut s = animated();
    draw(&mut s, 3);
    // And again with the playhead in the middle of a step segment and the
    // graph editor in speed mode (jump markers).
    s.time = Time::from_secs_f64(3.4);
    s.graph.mode = crate::graph_editor::GraphMode::Speed;
    draw(&mut s, 2);
}

#[test]
fn every_panel_draws_with_media_in_the_pool_and_no_composition() {
    use voxy_core::command::ImportAssets;
    use voxy_core::{Asset, AssetKind};
    let mut s = empty();
    s.execute(ImportAssets {
        assets: vec![Asset::new(
            AssetKind::Image,
            "a.png".to_owned(),
            "missing/a.png".to_owned(),
        )],
        folder: None,
    });
    let assets: Vec<_> = s.history.current().assets.iter().map(|a| a.id).collect();
    s.project
        .selection
        .items
        .extend(assets.into_iter().map(PoolItem::Asset));
    draw(&mut s, 3);
}

/// Actions that may change the document from an empty project. Everything
/// else must leave it exactly as it was.
fn may_edit(action: Action) -> bool {
    matches!(action, Action::ProjectNewFolder)
}

/// Actions that start something outside the app (a file manager) and so are
/// not run against a state that could satisfy them.
fn spawns_a_process(action: Action) -> bool {
    matches!(action, Action::RevealInFileManager)
}

#[test]
fn no_action_panics_or_creates_a_composition_in_an_empty_project() {
    for &action in Action::ALL {
        let mut s = empty();
        let revision = s.history.revision();
        let available = s.availability(action);
        let _ = s.perform(action);
        let doc = s.history.current();
        assert!(
            doc.comps.is_empty() && doc.main_comp.is_none(),
            "{action:?} created a composition (available: {available:?})"
        );
        if !may_edit(action) {
            assert_eq!(
                s.history.revision(),
                revision,
                "{action:?} changed an empty project"
            );
        }
        // An action that needs a composition says so instead of running.
        if available.is_err() {
            assert!(
                s.status.as_ref().is_some_and(|st| st.is_error),
                "{action:?} is unavailable but gave no reason"
            );
        }
    }
}

#[test]
fn no_action_panics_with_a_composition_and_no_selection() {
    for &action in Action::ALL {
        if spawns_a_process(action) {
            continue;
        }
        let mut s = with_comp();
        let _ = s.availability(action);
        let _ = s.perform(action);
        draw(&mut s, 1);
    }
}

#[test]
fn no_action_panics_with_animated_layers_and_selected_keys() {
    for &action in Action::ALL {
        if spawns_a_process(action) {
            continue;
        }
        let mut s = animated();
        let _ = s.availability(action);
        let _ = s.perform(action);
        // The state it leaves behind must still draw and still validate.
        draw(&mut s, 1);
        s.history
            .current()
            .validate()
            .expect("document stays valid");
    }
}

#[test]
fn the_audit_covers_every_action_and_every_tab() {
    assert!(Action::ALL.len() >= 33, "{}", Action::ALL.len());
    assert_eq!(TABS.len(), 5);
}
