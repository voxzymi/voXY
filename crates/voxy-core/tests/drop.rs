//! Milestone D (core): resolving and applying drops
//! (docs/design-media-comps.md §5).

#![allow(clippy::many_single_char_names)] // a, b, c, h, l, p read best short in these scenarios.

use voxy_core::command::{
    AddSourceLayer, CreateComposition, CreateFolder, DropItem, DropItems, DropTarget, Placement,
    Plan, resolve_drop,
};
use voxy_core::glam::DVec2;
use voxy_core::{
    Asset, AssetId, AssetKind, CommandError, CompId, CompSettings, Document, FrameRate, History,
    LayerKind, SourceRef, Time,
};

fn settings(name: &str, w: u32, h: u32) -> CompSettings {
    CompSettings {
        name: name.into(),
        width: w,
        height: h,
        frame_rate: FrameRate::new(30, 1).expect("test setup"),
        duration: Time::from_secs(10),
        ..CompSettings::default()
    }
}

fn history() -> History {
    History::new(Document::with_comp("Drops", settings("Main", 640, 360)))
}

fn main_id(h: &History) -> CompId {
    h.current().main_comp.expect("test setup")
}

fn video(name: &str, w: u32, h: u32, secs: i64) -> Asset {
    Asset {
        pixel_size: Some([w, h]),
        duration: Some(Time::from_secs(secs)),
        frame_rate: Some(FrameRate::new(24, 1).expect("test setup")),
        ..Asset::new(AssetKind::Video, name.to_owned(), format!("media/{name}"))
    }
}

fn still(name: &str, w: u32, h: u32) -> Asset {
    Asset {
        pixel_size: Some([w, h]),
        ..Asset::new(AssetKind::Image, name.to_owned(), format!("media/{name}"))
    }
}

fn import(h: &mut History, assets: Vec<Asset>) -> Vec<AssetId> {
    h.execute_reported(voxy_core::command::ImportAssets {
        assets,
        folder: None,
    })
    .expect("test setup")
    .created_assets
}

fn timeline(comp: CompId, secs: f64, index: usize) -> DropTarget {
    DropTarget::Timeline {
        comp,
        time: Time::from_secs_f64(secs),
        index,
    }
}

fn layers_of(plan: &Plan) -> &[voxy_core::command::PlannedLayer] {
    match plan {
        Plan::Layers { layers, .. } => layers,
        other => panic!("expected layers, got {other:?}"),
    }
}

#[test]
fn several_items_stack_at_the_drop_time_with_the_first_on_top() {
    let mut h = history();
    let c = main_id(&h);
    let ids = import(
        &mut h,
        vec![
            video("a.mov", 64, 64, 2),
            still("b.png", 64, 64),
            video("c.mov", 64, 64, 3),
        ],
    );
    let items: Vec<_> = ids.iter().map(|&i| DropItem::Asset(i)).collect();
    let plan = resolve_drop(
        h.current(),
        &timeline(c, 1.0, 0),
        &items,
        Placement::default(),
    );
    let l = layers_of(&plan);
    assert_eq!(l.len(), 3);
    for (k, p) in l.iter().enumerate() {
        assert_eq!(p.item, k);
        assert_eq!(p.in_point, Time::from_secs(1), "all start together");
        assert_eq!(p.index, k, "first item is the topmost of the group");
    }
}

#[test]
fn ctrl_sequences_each_item_after_the_previous_one() {
    let mut h = history();
    let c = main_id(&h);
    let ids = import(
        &mut h,
        vec![video("a.mov", 64, 64, 2), video("b.mov", 64, 64, 3)],
    );
    let items: Vec<_> = ids.iter().map(|&i| DropItem::Asset(i)).collect();
    let plan = resolve_drop(
        h.current(),
        &timeline(c, 1.0, 0),
        &items,
        Placement { sequence: true },
    );
    let l = layers_of(&plan);
    assert_eq!(l[0].in_point, Time::from_secs(1));
    assert_eq!(
        l[1].in_point,
        Time::from_secs(3),
        "starts where the first ends"
    );
    assert_eq!(l[1].length, Time::from_secs(3));
}

#[test]
fn lengths_follow_the_rules_and_are_never_under_a_frame() {
    let mut h = history();
    let c = main_id(&h);
    let ids = import(
        &mut h,
        vec![video("long.mov", 64, 64, 20), still("s.png", 64, 64)],
    );
    // A video longer than the composition is not trimmed.
    let v = resolve_drop(
        h.current(),
        &timeline(c, 5.0, 0),
        &[DropItem::Asset(ids[0])],
        Placement::default(),
    );
    assert_eq!(layers_of(&v)[0].length, Time::from_secs(20));
    // A still lasts until the composition ends...
    let s = resolve_drop(
        h.current(),
        &timeline(c, 4.0, 0),
        &[DropItem::Asset(ids[1])],
        Placement::default(),
    );
    assert_eq!(layers_of(&s)[0].length, Time::from_secs(6));
    // ...or the project's default still duration, when there is one.
    let mut doc = Document::clone(h.current());
    doc.meta.settings.default_still_duration = Some(Time::from_secs(2));
    let s = resolve_drop(
        &doc,
        &timeline(c, 4.0, 0),
        &[DropItem::Asset(ids[1])],
        Placement::default(),
    );
    assert_eq!(layers_of(&s)[0].length, Time::from_secs(2));
    // Dropped at the very end: still a whole layer.
    let end = resolve_drop(
        h.current(),
        &timeline(c, 99.0, 0),
        &[DropItem::Asset(ids[1])],
        Placement::default(),
    );
    let p = layers_of(&end)[0];
    assert_eq!(
        p.in_point,
        Time::from_secs(10) - FrameRate::new(30, 1).expect("test setup").frame_to_time(1)
    );
    assert!(p.length >= FrameRate::new(30, 1).expect("test setup").frame_to_time(1));
}

#[test]
fn the_drop_time_snaps_to_a_frame_and_stays_inside() {
    let mut h = history();
    let c = main_id(&h);
    let ids = import(&mut h, vec![still("s.png", 8, 8)]);
    let rate = FrameRate::new(30, 1).expect("test setup");
    let at = |secs: f64| {
        let plan = resolve_drop(
            h.current(),
            &timeline(c, secs, 0),
            &[DropItem::Asset(ids[0])],
            Placement::default(),
        );
        layers_of(&plan)[0].in_point
    };
    assert_eq!(at(1.0 / 30.0 * 4.4), rate.frame_to_time(4));
    assert_eq!(at(1.0 / 30.0 * 4.6), rate.frame_to_time(5));
    assert_eq!(at(-3.0), Time::ZERO);
}

#[test]
fn the_index_is_clamped_and_inserts_in_the_middle() {
    let mut h = history();
    let c = main_id(&h);
    let ids = import(&mut h, vec![still("s.png", 8, 8)]);
    for _ in 0..3 {
        h.execute(voxy_core::command::AddSolidLayer {
            comp: c,
            name: None,
            color: voxy_core::LinearRgba::WHITE,
            index: 0,
        })
        .expect("test setup");
    }
    let item = [DropItem::Asset(ids[0])];
    let far = resolve_drop(
        h.current(),
        &timeline(c, 0.0, 99),
        &item,
        Placement::default(),
    );
    assert_eq!(layers_of(&far)[0].index, 3, "appended below the last layer");
    h.execute(DropItems {
        target: timeline(c, 0.0, 1),
        items: item.to_vec(),
        placement: Placement::default(),
        folder: None,
    })
    .expect("test setup");
    let layers = &h.current().comp(c).expect("test setup").layers;
    assert_eq!(layers.len(), 4, "three solids plus the drop");
    assert!(
        matches!(layers[1].kind, LayerKind::Image { .. }),
        "between rows"
    );
}

#[test]
fn a_viewport_drop_centers_on_the_pointer_on_top_at_the_current_time() {
    let mut h = history();
    let c = main_id(&h);
    let ids = import(&mut h, vec![still("s.png", 100, 50)]);
    let target = DropTarget::Viewport {
        comp: c,
        time: Time::from_secs(2),
        pos: DVec2::new(120.0, 80.0),
    };
    let r = h
        .execute_reported(DropItems {
            target,
            items: vec![DropItem::Asset(ids[0])],
            placement: Placement::default(),
            folder: None,
        })
        .expect("test setup");
    let l = h
        .current()
        .find_layer(r.created_layers[0])
        .expect("test setup")
        .1
        .clone();
    assert_eq!(
        h.current().comp(c).expect("test setup").layers[0].id,
        l.id,
        "on top"
    );
    assert_eq!(l.in_point, Time::from_secs(2));
    assert_eq!(
        l.transform.position.value_at(Time::ZERO).as_vec2(),
        Some(DVec2::new(120.0, 80.0))
    );
    assert_eq!(
        l.transform.anchor.value_at(Time::ZERO).as_vec2(),
        Some(DVec2::new(50.0, 25.0)),
        "anchored at the layer's own center"
    );
}

#[test]
fn a_composition_that_would_contain_itself_is_blocked_with_the_reason() {
    let mut h = history();
    let a = main_id(&h);
    let b = h
        .execute_reported(CreateComposition {
            settings: settings("B", 64, 64),
            folder: None,
        })
        .expect("test setup")
        .created_comps[0];
    h.execute(AddSourceLayer {
        comp: b,
        source: SourceRef::Comp(a),
        index: 0,
        in_point: Time::ZERO,
    })
    .expect("test setup");
    // B contains A, so A can't go into B... and A can't go into itself.
    for (target, item) in [(a, a), (a, b)] {
        let plan = resolve_drop(
            h.current(),
            &timeline(target, 0.0, 0),
            &[DropItem::Comp(item)],
            Placement::default(),
        );
        let Plan::Blocked(why) = plan else {
            panic!("expected a block for {target:?} <- {item:?}")
        };
        assert!(why.contains("can't be placed"), "{why}");
    }
    let rev = h.revision();
    let err = h.execute(DropItems {
        target: timeline(a, 0.0, 0),
        items: vec![DropItem::Comp(b)],
        placement: Placement::default(),
        folder: None,
    });
    assert!(matches!(err, Err(CommandError::Invalid(_))));
    assert_eq!(h.revision(), rev, "nothing changed");
}

#[test]
fn replace_keeps_timing_and_transform_and_uses_only_the_first_item() {
    let mut h = history();
    let c = main_id(&h);
    let ids = import(&mut h, vec![still("a.png", 10, 10), still("b.png", 20, 20)]);
    h.execute(AddSourceLayer {
        comp: c,
        source: SourceRef::Asset(ids[0]),
        index: 0,
        in_point: Time::from_secs(1),
    })
    .expect("test setup");
    let before = h.current().comp(c).expect("test setup").layers[0].clone();
    let plan = resolve_drop(
        h.current(),
        &DropTarget::ReplaceLayer { layer: before.id },
        &[DropItem::Asset(ids[1]), DropItem::Asset(ids[0])],
        Placement::default(),
    );
    let Plan::Replace { warnings, item, .. } = &plan else {
        panic!("{plan:?}")
    };
    assert_eq!(*item, 0);
    assert_eq!(warnings.len(), 1, "says only the first was used");
    let r = h
        .execute_reported(DropItems {
            target: DropTarget::ReplaceLayer { layer: before.id },
            items: vec![DropItem::Asset(ids[1]), DropItem::Asset(ids[0])],
            placement: Placement::default(),
            folder: None,
        })
        .expect("test setup");
    assert_eq!(r.warnings.len(), 1);
    let after = h.current().comp(c).expect("test setup").layers[0].clone();
    assert_eq!(after.kind.asset(), Some(ids[1]));
    assert_eq!(
        (after.in_point, after.out_point),
        (before.in_point, before.out_point)
    );
    assert_eq!(after.transform, before.transform, "transform and keys kept");
}

#[test]
fn a_new_composition_takes_its_settings_from_the_chosen_item() {
    let mut h = History::new(Document::empty("New"));
    let ids = import(
        &mut h,
        vec![video("a.mov", 1280, 720, 4), still("b.png", 640, 480)],
    );
    let items: Vec<_> = ids.iter().map(|&i| DropItem::Asset(i)).collect();
    // Single composition from the first item.
    let r = h
        .execute_reported(DropItems {
            target: DropTarget::NewComp {
                per_item: false,
                source: 0,
            },
            items: items.clone(),
            placement: Placement::default(),
            folder: None,
        })
        .expect("test setup");
    let c = h
        .current()
        .comp(r.created_comps[0])
        .expect("test setup")
        .clone();
    assert_eq!((c.width, c.height), (1280, 720));
    assert_eq!(c.frame_rate, FrameRate::new(24, 1).expect("test setup"));
    assert_eq!(c.duration, Time::from_secs(4));
    assert_eq!(c.name, "a");
    assert_eq!(c.layers.len(), 2);
    assert_eq!(
        h.current().main_comp,
        Some(c.id),
        "the first one is the main composition"
    );
    // From the second (a still): size from it, project defaults for rate and length.
    let r = h
        .execute_reported(DropItems {
            target: DropTarget::NewComp {
                per_item: false,
                source: 1,
            },
            items,
            placement: Placement::default(),
            folder: None,
        })
        .expect("test setup");
    let c2 = h.current().comp(r.created_comps[0]).expect("test setup");
    let defaults = &h.current().meta.settings.new_comp;
    assert_eq!((c2.width, c2.height), (640, 480));
    assert_eq!(c2.frame_rate, defaults.frame_rate);
    assert_eq!(c2.duration, defaults.duration);
    assert_eq!(c2.name, "b");
}

#[test]
fn one_composition_per_item_with_numbered_names() {
    let mut h = History::new(Document::empty("New"));
    let ids = import(
        &mut h,
        vec![still("pic.png", 32, 32), still("pic.jpg", 48, 48)],
    );
    let r = h
        .execute_reported(DropItems {
            target: DropTarget::NewComp {
                per_item: true,
                source: 0,
            },
            items: ids.iter().map(|&i| DropItem::Asset(i)).collect(),
            placement: Placement::default(),
            folder: None,
        })
        .expect("test setup");
    assert_eq!(r.created_comps.len(), 2);
    let names: Vec<_> = r
        .created_comps
        .iter()
        .map(|&c| h.current().comp(c).expect("test setup").name.clone())
        .collect();
    assert_eq!(names, ["pic", "pic 2"]);
    let sizes: Vec<_> = r
        .created_comps
        .iter()
        .map(|&c| {
            let c = h.current().comp(c).expect("test setup");
            (c.width, c.layers.len())
        })
        .collect();
    assert_eq!(sizes, [(32, 1), (48, 1)]);
    h.undo();
    assert!(h.current().comps.is_empty(), "one undo removes both");
}

#[test]
fn files_are_imported_created_and_placed_in_one_undo_step() {
    let mut h = History::new(Document::empty("New"));
    let folder = h
        .execute_reported(CreateFolder {
            name: "Footage".into(),
            parent: None,
        })
        .expect("test setup")
        .created_folders[0];
    let rev = h.revision();
    let r = h
        .execute_reported(DropItems {
            target: DropTarget::NewComp {
                per_item: false,
                source: 0,
            },
            items: vec![
                DropItem::File(video("clip.mov", 320, 180, 3)),
                DropItem::File(still("pic.png", 320, 180)),
            ],
            placement: Placement::default(),
            folder: Some(folder),
        })
        .expect("test setup");
    assert_eq!(r.created_assets.len(), 2);
    assert_eq!(r.created_comps.len(), 1);
    assert_eq!(r.created_layers.len(), 2);
    let doc = h.current();
    assert!(doc.assets.iter().all(|a| a.folder == Some(folder)));
    assert_eq!(
        doc.comp(r.created_comps[0]).expect("test setup").folder,
        Some(folder)
    );
    h.undo();
    assert_eq!(h.revision(), rev);
    assert!(
        h.current().assets.iter().next().is_none(),
        "imports undone too"
    );
    assert!(!h.can_undo() || h.current().comps.is_empty());
}

#[test]
fn dropping_files_on_a_timeline_imports_and_places_them() {
    let mut h = history();
    let c = main_id(&h);
    let r = h
        .execute_reported(DropItems {
            target: timeline(c, 0.0, 0),
            items: vec![DropItem::File(video("clip.mov", 64, 64, 3))],
            placement: Placement::default(),
            folder: None,
        })
        .expect("test setup");
    assert_eq!(r.created_assets.len(), 1);
    let l = &h.current().comp(c).expect("test setup").layers[0];
    assert!(matches!(l.kind, LayerKind::Video { .. }));
    assert_eq!(l.out_point - l.in_point, Time::from_secs(3));
}

#[test]
fn a_failing_drop_changes_nothing() {
    let mut h = history();
    let c = main_id(&h);
    let rev = h.revision();
    let mut bad = still("bad.png", 8, 8);
    bad.path = "  ".into();
    let err = h.execute(DropItems {
        target: timeline(c, 0.0, 0),
        items: vec![DropItem::File(still("ok.png", 8, 8)), DropItem::File(bad)],
        placement: Placement::default(),
        folder: None,
    });
    assert!(err.is_err());
    assert_eq!(h.revision(), rev);
    assert!(
        h.current().assets.iter().next().is_none(),
        "the first file was not kept"
    );
    // Unknown size, missing items, empty drops, missing targets.
    let unknown = Asset::new(AssetKind::Image, "x.png".to_owned(), "x.png".to_owned());
    let plans = [
        resolve_drop(
            h.current(),
            &DropTarget::NewComp {
                per_item: false,
                source: 0,
            },
            &[DropItem::File(unknown)],
            Placement::default(),
        ),
        resolve_drop(
            h.current(),
            &timeline(c, 0.0, 0),
            &[DropItem::Asset(AssetId::from(999_999u64))],
            Placement::default(),
        ),
        resolve_drop(h.current(), &timeline(c, 0.0, 0), &[], Placement::default()),
        resolve_drop(
            h.current(),
            &timeline(CompId::from(999_999u64), 0.0, 0),
            &[DropItem::File(still("a.png", 8, 8))],
            Placement::default(),
        ),
    ];
    for p in plans {
        assert!(matches!(p, Plan::Blocked(_)), "{p:?}");
    }
}

#[test]
fn the_preview_matches_the_result() {
    let mut h = history();
    let c = main_id(&h);
    let ids = import(
        &mut h,
        vec![
            video("a.mov", 64, 64, 2),
            still("b.png", 32, 32),
            video("c.mov", 16, 16, 12),
        ],
    );
    for sequence in [false, true] {
        for (secs, index) in [(0.0, 0), (2.5, 1), (9.9, 7)] {
            let items: Vec<_> = ids.iter().map(|&i| DropItem::Asset(i)).collect();
            let placement = Placement { sequence };
            let target = timeline(c, secs, index);
            let plan = resolve_drop(h.current(), &target, &items, placement);
            let planned = layers_of(&plan).to_vec();
            let before = h.current().comp(c).expect("test setup").layers.len();
            let r = h
                .execute_reported(DropItems {
                    target,
                    items,
                    placement,
                    folder: None,
                })
                .expect("test setup");
            let comp = h.current().comp(c).expect("test setup").clone();
            for (p, id) in planned.iter().zip(&r.created_layers) {
                let l = comp.layer(*id).expect("test setup");
                assert_eq!(l.in_point, p.in_point);
                assert_eq!(l.out_point, p.in_point + p.length);
                assert_eq!(comp.layers.iter().position(|x| x.id == *id), Some(p.index));
            }
            assert_eq!(comp.layers.len(), before + planned.len());
            h.undo();
        }
    }
}
