//! Milestone A: the project pool, compositions and precomps
//! (docs/design-media-comps.md).

#![allow(clippy::many_single_char_names)] // a, b, c, h, l, k read best short in these scenarios.

use voxy_core::command::{
    AddImageLayer, AddSolidLayer, AddSourceLayer, AddVideoLayer, Anchor, CreateComposition,
    CreateFolder, DuplicateComposition, MoveToFolder, PrecomposeLayers, PrecomposeMode,
    RemoveItems, RemoveMode, RenameComposition, RenameFolder, RenameMedia, ReplaceLayerSource,
    ReplaceMedia, ResizeMode, SetAnimated, SetCompositionSettings, SetLayerTiming,
    SetPrecompTiming, SetProperty, SetWorkArea, UnprecomposeLayer, nearest_frame_time, off_grid,
    validate_name,
};
use voxy_core::glam::DVec2;
use voxy_core::{
    AssetId, CommandError, CompId, CompSettings, Document, FolderId, FrameRate, History, Layer,
    LayerId, LayerKind, PoolItem, SourceRef, Time, Value, WorkArea, persist,
};

fn settings(name: &str) -> CompSettings {
    CompSettings {
        name: name.into(),
        width: 640,
        height: 360,
        ..CompSettings::default()
    }
}

fn history() -> History {
    History::new(Document::new("Pool", settings("Main")))
}

fn main_id(h: &History) -> CompId {
    h.current().main_comp.expect("test setup")
}

fn comp(h: &History, id: CompId) -> &voxy_core::Composition {
    h.current().comp(id).expect("test setup")
}

fn new_comp(h: &mut History, name: &str) -> CompId {
    h.execute_reported(CreateComposition {
        settings: settings(name),
        folder: None,
    })
    .expect("test setup")
    .created_comps[0]
}

fn add_solid(h: &mut History, c: CompId) -> LayerId {
    h.execute(AddSolidLayer {
        comp: c,
        name: None,
        color: voxy_core::LinearRgba::WHITE,
        index: 0,
    })
    .expect("test setup");
    comp(h, c).layers[0].id
}

fn add_image(h: &mut History, c: CompId) -> (AssetId, LayerId) {
    h.execute(AddImageLayer {
        comp: c,
        path: "media/a.png".into(),
        pixel_size: [100, 50],
        index: 0,
    })
    .expect("test setup");
    let l = &comp(h, c).layers[0];
    (l.kind.asset().expect("test setup"), l.id)
}

fn add_precomp(h: &mut History, into: CompId, child: CompId) -> Result<LayerId, CommandError> {
    let r = h.execute_reported(AddSourceLayer {
        comp: into,
        source: SourceRef::Comp(child),
        index: 0,
        in_point: Time::ZERO,
    })?;
    Ok(r.created_layers[0])
}

fn layer(h: &History, id: LayerId) -> Layer {
    h.current().find_layer(id).expect("test setup").1.clone()
}

/// Animates a layer's position with keys at `times` (milliseconds).
fn animate(h: &mut History, l: LayerId, times: &[i64]) {
    let pos = layer(h, l).transform.position.id;
    for (i, &ms) in times.iter().enumerate() {
        h.execute(SetAnimated {
            prop: pos,
            time: Time::from_millis(ms),
            animated: true,
        })
        .expect("test setup");
        h.execute(SetProperty {
            prop: pos,
            time: Time::from_millis(ms),
            value: Value::Vec2(DVec2::new(i as f64 * 10.0, 5.0)),
        })
        .expect("test setup");
    }
}

fn key_times(l: &Layer) -> Vec<Vec<Time>> {
    l.properties()
        .iter()
        .map(|p| {
            p.track()
                .map(|t| t.keys().iter().map(|k| k.time).collect())
                .unwrap_or_default()
        })
        .collect()
}

/// Everything about a layer except its IDs.
fn signature(l: &Layer) -> String {
    let mut l = l.clone();
    l.id = LayerId(0);
    l.parent = l.parent.map(|_| LayerId(0));
    for p in l.properties_mut() {
        p.id = 0.into();
    }
    format!("{l:?}")
}

// -------------------------------------------------------------------- names

#[test]
fn names_are_trimmed_and_validated() {
    assert_eq!(validate_name("  Intro  ").unwrap(), "Intro");
    for bad in ["", "   ", "a\nb", "tab\there"] {
        assert!(
            matches!(validate_name(bad), Err(CommandError::Invalid(_))),
            "{bad:?}"
        );
    }
    assert!(validate_name(&"x".repeat(256)).is_err());
    assert!(validate_name(&"é".repeat(255)).is_ok());
}

#[test]
fn renames_apply_and_duplicates_are_allowed() {
    let mut h = history();
    let main = main_id(&h);
    let (asset, _) = add_image(&mut h, main);
    let other = new_comp(&mut h, "Other");
    h.execute(RenameComposition {
        comp: other,
        name: " Main ".into(),
    })
    .unwrap();
    assert_eq!(comp(&h, other).name, "Main");
    h.execute(RenameMedia {
        asset,
        name: "Logo".into(),
    })
    .unwrap();
    assert_eq!(h.current().assets.get(asset).unwrap().name, "Logo");
    assert_eq!(h.current().assets.get(asset).unwrap().path, "media/a.png");
    let before = h.current().clone();
    assert!(
        h.execute(RenameMedia {
            asset,
            name: " ".into()
        })
        .is_err()
    );
    assert_eq!(h.current(), &before);
}

// ------------------------------------------------------------------ folders

#[test]
fn folders_nest_move_and_rename() {
    let mut h = history();
    let main = main_id(&h);
    let a = h
        .execute_reported(CreateFolder {
            name: "A".into(),
            parent: None,
        })
        .unwrap()
        .created_folders[0];
    let b = h
        .execute_reported(CreateFolder {
            name: "B".into(),
            parent: Some(a),
        })
        .unwrap()
        .created_folders[0];
    h.execute(MoveToFolder {
        items: vec![PoolItem::Comp(main)],
        folder: Some(b),
    })
    .unwrap();
    assert_eq!(comp(&h, main).folder, Some(b));
    // A folder cannot move into itself or below itself.
    for dest in [a, b] {
        assert!(
            h.execute(MoveToFolder {
                items: vec![PoolItem::Folder(a)],
                folder: Some(dest)
            })
            .is_err()
        );
    }
    h.execute(RenameFolder {
        folder: b,
        name: "Renders".into(),
    })
    .unwrap();
    assert_eq!(h.current().folder(b).unwrap().name, "Renders");
    assert!(h.current().folder_within(b, a));
    assert!(
        h.execute(CreateFolder {
            name: "C".into(),
            parent: Some(FolderId(9999))
        })
        .is_err()
    );
}

// ------------------------------------------------------------------ removal

#[test]
fn removing_used_media_leaves_placeholders_and_undo_restores_everything() {
    let mut h = history();
    let main = main_id(&h);
    let f = h
        .execute_reported(CreateFolder {
            name: "Media".into(),
            parent: None,
        })
        .unwrap()
        .created_folders[0];
    let (asset, l) = add_image(&mut h, main);
    h.execute(MoveToFolder {
        items: vec![PoolItem::Asset(asset)],
        folder: Some(f),
    })
    .unwrap();
    let before = h.current().clone();

    let impact = h.current().removal_impact(&[PoolItem::Asset(asset)]);
    assert_eq!(impact.per_comp, vec![(main, vec![l])]);

    h.execute(RemoveItems {
        items: vec![PoolItem::Asset(asset)],
        mode: RemoveMode::PlaceholderLayers,
    })
    .unwrap();
    assert!(h.current().assets.get(asset).is_none());
    let LayerKind::Placeholder { was } = &layer(&h, l).kind else {
        panic!("expected a placeholder");
    };
    assert_eq!(was.pixel_size, Some([100, 50]));
    assert_eq!(was.name, "a.png");

    h.undo();
    assert_eq!(
        h.current(),
        &before,
        "item, folder position and layer restored"
    );
}

#[test]
fn removing_with_delete_unparents_children() {
    let mut h = history();
    let main = main_id(&h);
    let (asset, img) = add_image(&mut h, main);
    let solid = add_solid(&mut h, main);
    let mut d = (**h.current()).clone();
    d.layer_mut(solid).unwrap().parent = Some(img);
    h.reset(d);
    h.execute(RemoveItems {
        items: vec![PoolItem::Asset(asset)],
        mode: RemoveMode::DeleteLayers,
    })
    .unwrap();
    assert!(h.current().find_layer(img).is_none());
    assert_eq!(layer(&h, solid).parent, None);
}

#[test]
fn removing_a_folder_removes_its_contents_and_the_main_comp_is_replaced() {
    let mut h = history();
    let main = main_id(&h);
    let other = new_comp(&mut h, "Other");
    let p = add_precomp(&mut h, other, main).unwrap();
    let f = h
        .execute_reported(CreateFolder {
            name: "F".into(),
            parent: None,
        })
        .unwrap()
        .created_folders[0];
    let g = h
        .execute_reported(CreateFolder {
            name: "G".into(),
            parent: Some(f),
        })
        .unwrap()
        .created_folders[0];
    h.execute(MoveToFolder {
        items: vec![PoolItem::Comp(main)],
        folder: Some(g),
    })
    .unwrap();
    h.execute(RemoveItems {
        items: vec![PoolItem::Folder(f)],
        mode: RemoveMode::PlaceholderLayers,
    })
    .unwrap();
    let doc = h.current();
    assert!(doc.comp(main).is_none());
    assert!(doc.folder(f).is_none() && doc.folder(g).is_none());
    assert_eq!(doc.main_comp, Some(other));
    assert!(matches!(layer(&h, p).kind, LayerKind::Placeholder { .. }));
    // Zero compositions are allowed.
    h.execute(RemoveItems {
        items: vec![PoolItem::Comp(other)],
        mode: RemoveMode::DeleteLayers,
    })
    .unwrap();
    assert_eq!(h.current().main_comp, None);
    assert!(h.current().comps.is_empty());
    persist::from_json(&persist::to_json(h.current()).unwrap()).unwrap();
}

// ---------------------------------------------------------- usage, cycles

#[test]
fn usage_queries_group_by_composition_in_name_order() {
    let mut h = history();
    let main = main_id(&h);
    let (asset, l1) = add_image(&mut h, main);
    let b = new_comp(&mut h, "b-roll");
    let a = new_comp(&mut h, "Alpha");
    for c in [b, a] {
        h.execute(AddSourceLayer {
            comp: c,
            source: SourceRef::Asset(asset),
            index: 0,
            in_point: Time::ZERO,
        })
        .unwrap();
    }
    let l2 = h
        .execute_reported(AddSourceLayer {
            comp: b,
            source: SourceRef::Asset(asset),
            index: 0,
            in_point: Time::ZERO,
        })
        .unwrap()
        .created_layers[0];
    let u = h.current().usages(SourceRef::Asset(asset));
    let comps: Vec<CompId> = u.per_comp.iter().map(|(c, _)| *c).collect();
    assert_eq!(comps, vec![a, b, main], "Alpha, b-roll, Main");
    assert_eq!(u.total(), 4);
    assert_eq!(u.comp_count(), 3);
    assert_eq!(u.per_comp[1].1[0], l2, "stack order, top first");
    assert_eq!(u.primary(), Some(b), "most layers");
    assert_eq!(u.per_comp[2].1, vec![l1]);
    assert!(h.current().usages(SourceRef::Comp(main)).is_empty());
}

#[test]
fn compositions_cannot_contain_themselves() {
    let mut h = history();
    let a = main_id(&h);
    let b = new_comp(&mut h, "B");
    let c = new_comp(&mut h, "C");
    // Direct.
    let e = add_precomp(&mut h, a, a).unwrap_err();
    assert!(e.to_string().contains("inside itself"), "{e}");
    // Indirect: A ⊃ B ⊃ C, then C ⊃ A would close the loop.
    add_precomp(&mut h, a, b).unwrap();
    add_precomp(&mut h, b, c).unwrap();
    let before = h.current().clone();
    let e = add_precomp(&mut h, c, a).unwrap_err();
    let msg = e.to_string();
    assert!(
        msg.contains("“Main” already contains it") && msg.contains("“B”"),
        "{msg}"
    );
    assert_eq!(h.current(), &before);
    assert!(h.current().comp_contains(a, c));
    assert!(!h.current().comp_contains(c, a));
    // Replacing a source checks too.
    let solid = add_solid(&mut h, c);
    assert!(
        h.execute(ReplaceLayerSource {
            layers: vec![solid],
            source: SourceRef::Comp(a)
        })
        .is_err()
    );
    // A hand-made cycle fails validation.
    let mut d = (**h.current()).clone();
    d.layer_mut(solid).unwrap().kind = LayerKind::Precomp {
        comp: a,
        speed: 1.0,
        looping: false,
        time_remap: None,
        collapse: false,
    };
    assert!(d.validate().is_err());
}

// -------------------------------------------------------------- duplicate

#[test]
fn duplicate_composition_deep_copies_with_new_ids() {
    let mut h = history();
    let main = main_id(&h);
    let child = new_comp(&mut h, "Child");
    let parent = add_solid(&mut h, main);
    let kid = add_solid(&mut h, main);
    let pre = add_precomp(&mut h, main, child).unwrap();
    let mut d = (**h.current()).clone();
    d.layer_mut(kid).unwrap().parent = Some(parent);
    h.reset(d);
    let copy = h
        .execute_reported(DuplicateComposition { comp: main })
        .unwrap()
        .created_comps[0];
    let doc = h.current();
    doc.validate().unwrap();
    let (orig, dup) = (doc.comp(main).unwrap(), doc.comp(copy).unwrap());
    assert_eq!(dup.name, "Main 2");
    assert_ne!(dup.background.id, orig.background.id);
    for (o, n) in orig.layers.iter().zip(&dup.layers) {
        assert_ne!(o.id, n.id);
        assert_eq!(signature(o), signature(n));
    }
    let dup_kid = &dup.layers[orig.layer_index(kid).unwrap()];
    assert_eq!(
        dup_kid.parent,
        Some(dup.layers[orig.layer_index(parent).unwrap()].id)
    );
    let dup_pre = &dup.layers[orig.layer_index(pre).unwrap()];
    assert_eq!(
        dup_pre.kind.source(),
        Some(SourceRef::Comp(child)),
        "precomps stay shared"
    );
    // Duplicating again picks the next free number.
    let third = h
        .execute_reported(DuplicateComposition { comp: main })
        .unwrap()
        .created_comps[0];
    assert_eq!(comp(&h, third).name, "Main 3");
}

// ------------------------------------------------- resolution, frame rate

#[test]
fn frame_rate_changes_keep_real_time_positions() {
    let mut h = history();
    let main = main_id(&h);
    let l = add_solid(&mut h, main);
    animate(&mut h, l, &[0, 100, 1000]);
    h.execute(SetLayerTiming {
        layer: l,
        in_point: Time::from_millis(100),
        out_point: Time::from_millis(2000),
        start_offset: Time::ZERO,
    })
    .unwrap();
    let before = layer(&h, l);
    let mut s = CompSettings::of(comp(&h, main));
    s.frame_rate = FrameRate::FPS_24;
    // 100 ms is frame 3 at 30 fps but 2.4 frames at 24 fps.
    let og = off_grid(comp(&h, main), FrameRate::FPS_24);
    assert_eq!((og.keyframes, og.layer_edges), (1, 1));
    let r = h
        .execute_reported(SetCompositionSettings {
            comp: main,
            settings: s.clone(),
            resize: ResizeMode::default(),
            snap_to_frames: false,
        })
        .unwrap();
    assert_eq!(layer(&h, l), before, "nothing moves in time");
    assert_eq!(comp(&h, main).frame_rate, FrameRate::FPS_24);
    assert!(
        r.warnings[0].contains("1 keyframes and 1 layer edges"),
        "{:?}",
        r.warnings
    );

    h.undo();
    let r = h
        .execute_reported(SetCompositionSettings {
            comp: main,
            settings: s,
            resize: ResizeMode::default(),
            snap_to_frames: true,
        })
        .unwrap();
    let after = layer(&h, l);
    let snapped = nearest_frame_time(FrameRate::FPS_24, Time::from_millis(100));
    assert_eq!(snapped, FrameRate::FPS_24.frame_to_time(2));
    assert_eq!(
        key_times(&after)[1],
        vec![Time::ZERO, snapped, Time::from_millis(1000)]
    );
    assert_eq!(after.in_point, snapped);
    assert_eq!(
        after.start_offset,
        snapped - Time::from_millis(100),
        "a trim keeps the source in place"
    );
    assert!(
        r.warnings[0].contains("Moved 1 keyframes and 1 layer edges"),
        "{:?}",
        r.warnings
    );
}

#[test]
fn snapping_merges_colliding_keyframes() {
    let mut h = history();
    let main = main_id(&h);
    let l = add_solid(&mut h, main);
    // 10 ms and 20 ms both snap to frame 0 at 24 fps; the later one wins.
    animate(&mut h, l, &[10, 20, 1000]);
    let mut s = CompSettings::of(comp(&h, main));
    s.frame_rate = FrameRate::FPS_24;
    let r = h
        .execute_reported(SetCompositionSettings {
            comp: main,
            settings: s,
            resize: ResizeMode::default(),
            snap_to_frames: true,
        })
        .unwrap();
    let after = layer(&h, l);
    let keys = after.transform.position.track().unwrap().keys().to_vec();
    assert_eq!(keys.len(), 2);
    assert_eq!(
        keys[0].value,
        Value::Vec2(DVec2::new(10.0, 5.0)),
        "the later key won"
    );
    assert!(r.warnings.iter().any(|w| w.contains("1 keyframes landed")));
}

#[test]
fn resizing_keeps_layers_or_scales_them() {
    let mut h = history();
    let main = main_id(&h);
    let top = add_solid(&mut h, main);
    let child = add_solid(&mut h, main);
    animate(&mut h, top, &[0, 500]);
    let mut d = (**h.current()).clone();
    d.layer_mut(child).unwrap().parent = Some(top);
    h.reset(d);
    let pos = |h: &History, l| {
        layer(h, l)
            .transform
            .position
            .value_at(Time::ZERO)
            .as_vec2()
            .unwrap()
    };
    let scale = |h: &History, l| {
        layer(h, l)
            .transform
            .scale
            .value_at(Time::ZERO)
            .as_vec2()
            .unwrap()
    };
    let (p0, c0) = (pos(&h, top), pos(&h, child));
    let mut s = CompSettings::of(comp(&h, main));
    s.width = 1280;
    s.height = 720;

    // Keep layers, centered: top-level layers move by half the growth.
    h.execute(SetCompositionSettings {
        comp: main,
        settings: s.clone(),
        resize: ResizeMode::KeepLayers(Anchor::CENTER),
        snap_to_frames: false,
    })
    .unwrap();
    assert_eq!(pos(&h, top), p0 + DVec2::new(320.0, 180.0));
    assert_eq!(pos(&h, child), c0, "children follow their parent");
    assert_eq!(scale(&h, top), DVec2::ONE);
    let k = layer(&h, top).transform.position.track().unwrap().keys()[1]
        .value
        .clone();
    assert_eq!(
        k,
        Value::Vec2(DVec2::new(10.0 + 320.0, 5.0 + 180.0)),
        "keyframes move too"
    );
    h.undo();

    // Top-left anchor: nothing moves.
    h.execute(SetCompositionSettings {
        comp: main,
        settings: s.clone(),
        resize: ResizeMode::KeepLayers(Anchor::grid(0, 0)),
        snap_to_frames: false,
    })
    .unwrap();
    assert_eq!(pos(&h, top), p0);
    h.undo();

    // Scale to fit: 640×360 → 1280×720 doubles.
    h.execute(SetCompositionSettings {
        comp: main,
        settings: s,
        resize: ResizeMode::ScaleToFit,
        snap_to_frames: false,
    })
    .unwrap();
    assert_eq!(pos(&h, top), p0 * 2.0);
    assert_eq!(scale(&h, top), DVec2::splat(2.0));
    assert_eq!(scale(&h, child), DVec2::ONE);
    assert_eq!((comp(&h, main).width, comp(&h, main).height), (1280, 720));
}

#[test]
fn settings_edits_merge_and_validate() {
    let mut h = history();
    let main = main_id(&h);
    let steps = h.can_undo();
    assert!(!steps);
    for w in [700, 800, 900] {
        let mut s = CompSettings::of(comp(&h, main));
        s.width = w;
        h.execute(SetCompositionSettings {
            comp: main,
            settings: s,
            resize: ResizeMode::default(),
            snap_to_frames: false,
        })
        .unwrap();
    }
    h.end_interaction();
    assert_eq!(comp(&h, main).width, 900);
    h.undo();
    assert_eq!(comp(&h, main).width, 640, "one undo step");
    assert!(!h.can_undo());
    for bad in [
        CompSettings {
            width: 8,
            ..settings("x")
        },
        CompSettings {
            height: 20_000,
            ..settings("x")
        },
        CompSettings {
            duration: Time::ZERO,
            ..settings("x")
        },
        CompSettings {
            frame_rate: FrameRate::new(1000, 1).unwrap(),
            ..settings("x")
        },
        CompSettings {
            name: String::new(),
            ..settings("x")
        },
    ] {
        assert!(
            h.execute(SetCompositionSettings {
                comp: main,
                settings: bad,
                resize: ResizeMode::default(),
                snap_to_frames: false
            })
            .is_err()
        );
    }
}

#[test]
fn work_area_is_validated_and_clamped_by_duration_changes() {
    let mut h = history();
    let main = main_id(&h);
    let area = WorkArea {
        start: Time::from_secs(2),
        end: Time::from_secs(8),
    };
    h.execute(SetWorkArea {
        comp: main,
        area: Some(area),
    })
    .unwrap();
    assert!(
        h.execute(SetWorkArea {
            comp: main,
            area: Some(WorkArea {
                start: Time::from_secs(5),
                end: Time::from_secs(11)
            })
        })
        .is_err()
    );
    let mut s = CompSettings::of(comp(&h, main));
    s.duration = Time::from_secs(5);
    h.execute(SetCompositionSettings {
        comp: main,
        settings: s.clone(),
        resize: ResizeMode::default(),
        snap_to_frames: false,
    })
    .unwrap();
    assert_eq!(
        comp(&h, main).work_area,
        Some(WorkArea {
            start: Time::from_secs(2),
            end: Time::from_secs(5)
        })
    );
    s.duration = Time::from_secs(1);
    h.execute(SetCompositionSettings {
        comp: main,
        settings: s,
        resize: ResizeMode::default(),
        snap_to_frames: false,
    })
    .unwrap();
    assert_eq!(comp(&h, main).work_area, None);
}

// ------------------------------------------------------- sources, layers

#[test]
fn source_layer_lengths_follow_the_rules() {
    let mut h = history();
    let main = main_id(&h);
    // A 30 s video in a 10 s composition keeps its full length.
    h.execute(AddVideoLayer {
        comp: main,
        path: "v.mp4".into(),
        pixel_size: [320, 240],
        duration: Some(Time::from_secs(30)),
        frame_rate: Some(FrameRate::FPS_30),
        index: 0,
    })
    .unwrap();
    let video = comp(&h, main).layers[0].kind.asset().unwrap();
    let (still, _) = add_image(&mut h, main);
    let child = new_comp(&mut h, "Child");
    let len = |h: &mut History, s: SourceRef, at: i64| {
        let id = h
            .execute_reported(AddSourceLayer {
                comp: main,
                source: s,
                index: 0,
                in_point: Time::from_secs(at),
            })
            .unwrap()
            .created_layers[0];
        let l = layer(h, id);
        (l.in_point, l.out_point - l.in_point)
    };
    assert_eq!(
        len(&mut h, SourceRef::Asset(video), 4),
        (Time::from_secs(4), Time::from_secs(30))
    );
    assert_eq!(
        len(&mut h, SourceRef::Asset(still), 4).1,
        Time::from_secs(6),
        "until the end"
    );
    assert_eq!(
        len(&mut h, SourceRef::Comp(child), 4).1,
        Time::from_secs(10),
        "its own duration"
    );
    assert_eq!(
        len(&mut h, SourceRef::Asset(still), 10).1,
        FrameRate::FPS_30.frame_to_time(1),
        "at least one frame"
    );
    let mut ps = h.current().meta.settings.clone();
    ps.default_still_duration = Some(Time::from_secs(3));
    h.execute(voxy_core::command::SetProjectSettings(ps))
        .unwrap();
    assert_eq!(
        len(&mut h, SourceRef::Asset(still), 0).1,
        Time::from_secs(3)
    );
    let added = comp(&h, main).layers[0].clone();
    assert_eq!(added.name, "a.png");
    assert_eq!(
        added.transform.position.value_at(Time::ZERO).as_vec2(),
        Some(DVec2::new(320.0, 180.0))
    );
    assert_eq!(
        added.transform.anchor.value_at(Time::ZERO).as_vec2(),
        Some(DVec2::new(50.0, 25.0))
    );
}

#[test]
fn replacing_a_layer_source_keeps_everything_else() {
    let mut h = history();
    let main = main_id(&h);
    let (asset, l) = add_image(&mut h, main);
    animate(&mut h, l, &[0, 500]);
    let child = new_comp(&mut h, "Child");
    let before = layer(&h, l);
    h.execute(ReplaceLayerSource {
        layers: vec![l],
        source: SourceRef::Comp(child),
    })
    .unwrap();
    let after = layer(&h, l);
    assert_eq!(after.kind.source(), Some(SourceRef::Comp(child)));
    assert_eq!(
        (
            after.transform.clone(),
            after.in_point,
            after.out_point,
            after.name.clone()
        ),
        (
            before.transform.clone(),
            before.in_point,
            before.out_point,
            before.name.clone()
        )
    );
    // A placeholder becomes a real layer again.
    h.execute(RemoveItems {
        items: vec![PoolItem::Comp(child)],
        mode: RemoveMode::PlaceholderLayers,
    })
    .unwrap();
    assert!(matches!(layer(&h, l).kind, LayerKind::Placeholder { .. }));
    h.execute(ReplaceLayerSource {
        layers: vec![l],
        source: SourceRef::Asset(asset),
    })
    .unwrap();
    assert_eq!(layer(&h, l).kind, LayerKind::Image { asset });
    assert_eq!(layer(&h, l).transform, before.transform);
}

#[test]
fn replace_media_relinks_and_warns_on_mismatch() {
    let mut h = history();
    let main = main_id(&h);
    let (asset, l) = add_image(&mut h, main);
    let r = h
        .execute_reported(ReplaceMedia {
            asset,
            path: "media/b.png".into(),
            pixel_size: Some([200, 50]),
            duration: None,
            frame_rate: None,
            audio: None,
            content_hash: None,
        })
        .unwrap();
    assert_eq!(h.current().assets.get(asset).unwrap().path, "media/b.png");
    assert_eq!(layer(&h, l).kind, LayerKind::Image { asset });
    assert_eq!(r.warnings.len(), 1);
    assert!(r.warnings[0].contains("200×50"));
}

// ------------------------------------------------------------ time mapping

#[test]
fn child_time_follows_offset_speed_loop_and_remap() {
    let mut h = history();
    let main = main_id(&h);
    let child = new_comp(&mut h, "Child"); // 10 s
    let p = add_precomp(&mut h, main, child).unwrap();
    let ten = Time::from_secs(10);
    let ct = |h: &History, t: i64| layer(h, p).child_time(Time::from_millis(t), ten).unwrap();
    h.execute(SetLayerTiming {
        layer: p,
        in_point: Time::from_secs(2),
        out_point: Time::from_secs(9),
        start_offset: Time::from_secs(1),
    })
    .unwrap();
    assert_eq!(ct(&h, 2000), Time::from_secs(1));
    assert_eq!(ct(&h, 5500), Time::from_millis(4500));
    h.execute(SetPrecompTiming {
        layer: p,
        speed: 2.0,
        looping: false,
        time_remap: false,
    })
    .unwrap();
    assert_eq!(ct(&h, 5500), Time::from_secs(1 + 7));
    h.execute(SetPrecompTiming {
        layer: p,
        speed: 2.0,
        looping: true,
        time_remap: false,
    })
    .unwrap();
    assert_eq!(ct(&h, 8000), Time::from_secs(3), "13 s wraps to 3 s");
    // Remap starts by matching what is shown now, then overrides speed.
    h.execute(SetPrecompTiming {
        layer: p,
        speed: 1.0,
        looping: false,
        time_remap: true,
    })
    .unwrap();
    let l = layer(&h, p);
    let LayerKind::Precomp {
        time_remap: Some(remap),
        ..
    } = &l.kind
    else {
        panic!()
    };
    assert_eq!(remap.track().unwrap().len(), 2);
    assert_eq!(ct(&h, 2000), Time::from_secs(1));
    assert_eq!(ct(&h, 9000), Time::from_secs(8));
    let solid = add_solid(&mut h, main);
    assert!(layer(&h, solid).child_time(Time::ZERO, ten).is_none());
    assert!(
        h.execute(SetPrecompTiming {
            layer: p,
            speed: 0.0,
            looping: false,
            time_remap: false
        })
        .is_err()
    );
}

// -------------------------------------------------- precompose round trips

#[test]
fn precompose_keeps_times_and_unprecompose_restores_the_layers() {
    let mut h = history();
    let main = main_id(&h);
    let a = add_solid(&mut h, main);
    let b = add_solid(&mut h, main);
    let c = add_solid(&mut h, main);
    animate(&mut h, a, &[0, 250, 4000]);
    animate(&mut h, c, &[100, 900]);
    h.execute(SetLayerTiming {
        layer: a,
        in_point: Time::from_millis(500),
        out_point: Time::from_secs(6),
        start_offset: Time::ZERO,
    })
    .unwrap();
    let start = h.current().clone();
    let original: Vec<String> = comp(&h, main).layers.iter().map(|l| signature(l)).collect();

    let r = h
        .execute_reported(PrecomposeLayers {
            comp: main,
            layers: vec![a, b],
            mode: PrecomposeMode::MoveAttributes,
            name: "Group".into(),
            time: Time::ZERO,
        })
        .unwrap();
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let (child, pre) = (r.created_comps[0], r.created_layers[0]);
    let m = comp(&h, main);
    assert_eq!(m.layers.len(), 2);
    assert_eq!(m.layers[0].id, c);
    assert_eq!(m.layers[1].id, pre, "at the topmost selected layer");
    let k = comp(&h, child);
    assert_eq!(
        (k.width, k.height, k.duration),
        (640, 360, Time::from_secs(10))
    );
    let inside: Vec<LayerId> = k.layers.iter().map(|l| l.id).collect();
    assert_eq!(inside, vec![b, a], "stack order, same layers");
    assert_eq!(
        key_times(&layer(&h, a))[1],
        vec![Time::ZERO, Time::from_millis(250), Time::from_secs(4)]
    );

    let r = h
        .execute_reported(UnprecomposeLayer { layer: pre })
        .unwrap();
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let back: Vec<String> = comp(&h, main).layers.iter().map(|l| signature(l)).collect();
    assert_eq!(back, original, "same layers, times and keyframes, new IDs");
    assert!(
        h.current().comp(child).is_some(),
        "the composition stays in the project"
    );

    h.undo();
    h.undo();
    assert_eq!(h.current(), &start, "two undo steps");
}

#[test]
fn unprecompose_shifts_keyframes_by_the_layer_offset() {
    let mut h = history();
    let main = main_id(&h);
    let child = new_comp(&mut h, "Child");
    let inner = add_solid(&mut h, child);
    animate(&mut h, inner, &[0, 1000, 3000]);
    let pre = add_precomp(&mut h, main, child).unwrap();
    // Shown from child time 0.5 s, starting at 2 s in Main, until 5 s.
    h.execute(SetLayerTiming {
        layer: pre,
        in_point: Time::from_secs(2),
        out_point: Time::from_secs(5),
        start_offset: Time::from_millis(500),
    })
    .unwrap();
    let r = h
        .execute_reported(UnprecomposeLayer { layer: pre })
        .unwrap();
    let copy = layer(&h, comp(&h, main).layers[0].id);
    let shift = Time::from_millis(1500);
    assert_eq!(
        key_times(&copy)[1],
        vec![
            shift,
            Time::from_millis(1000) + shift,
            Time::from_millis(3000) + shift
        ]
    );
    // The child layer spanned 0–10 s: clipped to the precomp's 2–5 s, and
    // its in-point trim moves its source offset along.
    assert_eq!(
        (copy.in_point, copy.out_point),
        (Time::from_secs(2), Time::from_secs(5))
    );
    assert_eq!(copy.start_offset, Time::from_millis(500));
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    // Precompose the copy again: times are unchanged, and unprecompose
    // returns the same layer.
    let sig = signature(&copy);
    let r = h
        .execute_reported(PrecomposeLayers {
            comp: main,
            layers: vec![copy.id],
            mode: PrecomposeMode::MoveAttributes,
            name: "Again".into(),
            time: Time::ZERO,
        })
        .unwrap();
    assert_eq!(key_times(&layer(&h, copy.id)), key_times(&copy));
    h.execute(UnprecomposeLayer {
        layer: r.created_layers[0],
    })
    .unwrap();
    assert_eq!(signature(&comp(&h, main).layers[0]), sig);
}

#[test]
fn unprecompose_at_double_speed_halves_child_times() {
    let mut h = history();
    let main = main_id(&h);
    let child = new_comp(&mut h, "Child");
    let inner = add_solid(&mut h, child);
    animate(&mut h, inner, &[0, 2000]);
    let pre = add_precomp(&mut h, main, child).unwrap();
    h.execute(SetPrecompTiming {
        layer: pre,
        speed: 2.0,
        looping: false,
        time_remap: false,
    })
    .unwrap();
    h.execute(UnprecomposeLayer { layer: pre }).unwrap();
    let copy = &comp(&h, main).layers[0];
    assert_eq!(key_times(copy)[1], vec![Time::ZERO, Time::from_secs(1)]);
    assert_eq!(copy.out_point, Time::from_secs(5));
}

#[test]
fn unprecompose_keeps_the_precomp_transform_through_a_parent() {
    let mut h = history();
    let main = main_id(&h);
    let child = new_comp(&mut h, "Child");
    add_solid(&mut h, child);
    let pre = add_precomp(&mut h, main, child).unwrap();
    let rot = layer(&h, pre).transform.rotation.id;
    h.execute(SetProperty {
        prop: rot,
        time: Time::ZERO,
        value: Value::Scalar(30.0),
    })
    .unwrap();
    let r = h
        .execute_reported(UnprecomposeLayer { layer: pre })
        .unwrap();
    let m = comp(&h, main);
    assert_eq!(m.layers.len(), 2);
    let holder = &m.layers[1];
    assert_eq!(holder.name, "Child transform");
    assert_eq!(m.layers[0].parent, Some(holder.id));
    assert_eq!(
        holder.transform.rotation.value_at(Time::ZERO),
        Value::Scalar(30.0)
    );
    // The world placement of the content is unchanged.
    let before = voxy_core::layer_world(
        h.current().comp(child).unwrap(),
        h.current().comp(child).unwrap().layers[0].id,
        Time::ZERO,
    );
    let world = voxy_core::layer_world(m, m.layers[0].id, Time::ZERO);
    let precomp_m = voxy_core::local_matrix(&holder.transform, Time::ZERO);
    assert!(world.abs_diff_eq(precomp_m * before, 1e-9));
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn unprecompose_reports_what_it_cannot_keep() {
    let mut h = history();
    let main = main_id(&h);
    let child = new_comp(&mut h, "Child");
    let inner = add_solid(&mut h, child);
    animate(&mut h, inner, &[0, 1000]);
    let pre = add_precomp(&mut h, main, child).unwrap();
    h.execute(SetPrecompTiming {
        layer: pre,
        speed: 1.0,
        looping: false,
        time_remap: true,
    })
    .unwrap();
    let op = layer(&h, pre).transform.opacity.id;
    h.execute(SetProperty {
        prop: op,
        time: Time::ZERO,
        value: Value::Scalar(0.5),
    })
    .unwrap();
    let r = h
        .execute_reported(UnprecomposeLayer { layer: pre })
        .unwrap();
    let all = r.warnings.join("\n");
    assert!(all.contains("opacity"), "{all}");
    assert!(all.contains("sampling"), "{all}");
    let copy = &comp(&h, main).layers[0];
    // Sampled once per frame over the 10 s layer.
    assert_eq!(copy.transform.position.track().unwrap().len(), 300);
}

#[test]
fn precompose_reports_broken_parent_links() {
    let mut h = history();
    let main = main_id(&h);
    let parent = add_solid(&mut h, main);
    let kid = add_solid(&mut h, main);
    let rot = layer(&h, parent).transform.rotation.id;
    h.execute(SetProperty {
        prop: rot,
        time: Time::ZERO,
        value: Value::Scalar(90.0),
    })
    .unwrap();
    let mut d = (**h.current()).clone();
    d.layer_mut(kid).unwrap().parent = Some(parent);
    h.reset(d);
    let world_before = voxy_core::layer_world(comp(&h, main), kid, Time::ZERO);
    let r = h
        .execute_reported(PrecomposeLayers {
            comp: main,
            layers: vec![kid],
            mode: PrecomposeMode::MoveAttributes,
            name: "K".into(),
            time: Time::ZERO,
        })
        .unwrap();
    assert_eq!(r.warnings.len(), 1);
    assert!(
        r.warnings[0].contains("was parented to"),
        "{}",
        r.warnings[0]
    );
    let k = comp(&h, r.created_comps[0]);
    assert_eq!(k.layers[0].parent, None);
    let world_after = voxy_core::layer_world(k, kid, Time::ZERO);
    assert!(world_after.abs_diff_eq(world_before, 1e-9));
}

#[test]
fn precompose_leave_attributes_keeps_the_layer() {
    let mut h = history();
    let main = main_id(&h);
    let (asset, l) = add_image(&mut h, main);
    animate(&mut h, l, &[0, 700]);
    let before = layer(&h, l);
    let r = h
        .execute_reported(PrecomposeLayers {
            comp: main,
            layers: vec![l],
            mode: PrecomposeMode::LeaveAttributes,
            name: "Logo".into(),
            time: Time::ZERO,
        })
        .unwrap();
    let child = r.created_comps[0];
    let after = layer(&h, l);
    assert_eq!(after.kind.source(), Some(SourceRef::Comp(child)));
    assert_eq!(after.transform, before.transform);
    assert_eq!(
        (after.in_point, after.out_point),
        (before.in_point, before.out_point)
    );
    let k = comp(&h, child);
    assert_eq!((k.width, k.height), (100, 50));
    assert_eq!(k.layers[0].kind, LayerKind::Image { asset });
    // Local coordinates are unchanged: the source fills the new canvas.
    assert!(
        voxy_core::layer_world(k, k.layers[0].id, Time::ZERO)
            .abs_diff_eq(voxy_core::glam::DAffine2::IDENTITY, 1e-12)
    );
    // Solids have no source to leave behind.
    let s = add_solid(&mut h, main);
    assert!(
        h.execute(PrecomposeLayers {
            comp: main,
            layers: vec![s],
            mode: PrecomposeMode::LeaveAttributes,
            name: "x".into(),
            time: Time::ZERO
        })
        .is_err()
    );
    assert!(
        h.execute(PrecomposeLayers {
            comp: main,
            layers: vec![s, l],
            mode: PrecomposeMode::LeaveAttributes,
            name: "x".into(),
            time: Time::ZERO
        })
        .is_err()
    );
}

// ------------------------------------------------------------------ IDs

#[test]
fn ids_are_never_reused_after_undo() {
    let mut h = history();
    let main = main_id(&h);
    let first = add_solid(&mut h, main);
    let before_undo = h.current().clone();
    h.undo();
    let undone = h.current().clone();
    let second = add_solid(&mut h, main);
    assert_ne!(first, second);
    assert!(second.raw() > first.raw());
    // Undo and redo still give identical documents.
    h.undo();
    assert_eq!(h.current(), &undone);
    h.redo();
    assert_ne!(h.current(), &before_undo, "a different edit");
    let comp_id = new_comp(&mut h, "X");
    h.undo();
    let again = new_comp(&mut h, "Y");
    assert_ne!(comp_id, again);
}

// --------------------------------------------------------- persistence

#[test]
fn format_0_3_0_projects_open_as_one_main_composition() {
    let legacy = r#"{
      "format": "voxy-project",
      "format_version": "0.3.0",
      "generator": "voXY 0.1.0",
      "project": {
        "meta": { "name": "Old", "working_space": "linear" },
        "assets": [ { "id": 5, "name": "a.png", "kind": "image", "path": "a.png", "pixel_size": [10, 10] } ],
        "comps": [ {
          "id": 1, "name": "Main", "width": 320, "height": 180,
          "frame_rate": { "num": 30, "den": 1 }, "duration": 7056000000,
          "background": { "id": 2, "key": "comp.background", "static": { "type": "color", "value": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 } } },
          "layers": []
        } ],
        "root_comp": 1,
        "next_id": 6
      }
    }"#;
    let doc = persist::from_json(legacy).unwrap();
    assert_eq!(doc.main_comp, Some(CompId(1)));
    assert!(doc.folders.is_empty());
    assert_eq!(doc.assets.get(AssetId(5)).unwrap().folder, None);
    assert_eq!(doc.main().unwrap().work_area, None);
    assert_eq!(doc.meta.settings, voxy_core::ProjectSettings::default());
    let resaved = persist::to_json(&doc).unwrap();
    assert!(resaved.contains("\"format_version\": \"0.4.0\""));
    assert!(resaved.contains("\"main_comp\": 1") && !resaved.contains("root_comp"));
}

#[test]
fn new_projects_round_trip_with_ui_state() {
    let mut h = history();
    let main = main_id(&h);
    let child = new_comp(&mut h, "Child");
    let f = h
        .execute_reported(CreateFolder {
            name: "Comps".into(),
            parent: None,
        })
        .unwrap()
        .created_folders[0];
    h.execute(MoveToFolder {
        items: vec![PoolItem::Comp(child)],
        folder: Some(f),
    })
    .unwrap();
    let pre = add_precomp(&mut h, main, child).unwrap();
    h.execute(SetPrecompTiming {
        layer: pre,
        speed: 1.5,
        looping: true,
        time_remap: true,
    })
    .unwrap();
    let (asset, _) = add_image(&mut h, main);
    h.execute(RemoveItems {
        items: vec![PoolItem::Asset(asset)],
        mode: RemoveMode::PlaceholderLayers,
    })
    .unwrap();
    h.execute(SetWorkArea {
        comp: main,
        area: Some(WorkArea {
            start: Time::ZERO,
            end: Time::from_secs(1),
        }),
    })
    .unwrap();
    let ui = serde_json::json!({ "version": 1, "tabs": [ { "comp": 99 } ] });
    let json = persist::to_json_with_ui(h.current(), Some(&ui)).unwrap();
    let (doc, back) = persist::from_json_with_ui(&json).unwrap();
    assert_eq!(&doc, &**h.current());
    assert_eq!(back, Some(ui));
    // Files without UI state, and loaders that ignore it, still work.
    assert_eq!(persist::from_json(&json).unwrap(), doc);
    let (_, none) = persist::from_json_with_ui(&persist::to_json(&doc).unwrap()).unwrap();
    assert_eq!(none, None);
    assert_eq!(
        persist::to_json_with_ui(&doc, Some(&back.unwrap())).unwrap(),
        json
    );
}
