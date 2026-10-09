//! Property-based tests: random edit sequences against undo/redo and
//! save/load.

#![allow(clippy::many_single_char_names)] // r, g, b, x, y, i in generators read best short.

use proptest::prelude::*;
use std::collections::BTreeSet;

use voxy_core::command::{
    AddImageLayer, AddSolidLayer, AddSourceLayer, Anchor, ApplyEasePreset, CreateComposition,
    CreateFolder, DuplicateComposition, MoveKeyframes, MoveLayer, MoveToFolder, PrecomposeLayers,
    PrecomposeMode, RemoveItems, RemoveKeyframes, RemoveLayers, RemoveMode, RenameComposition,
    RenameLayer, ResizeMode, SetAnimated, SetCompositionSettings, SetEase, SetInterpolation,
    SetLayerTiming, SetLayerVisible, SetPrecompTiming, SetProperty, SetSpatialTangents,
    SetWorkingSpace, UnprecomposeLayer,
};
use voxy_core::glam::{DVec2, DVec3};
use voxy_core::{
    CompId, CompSettings, Document, EaseHandle, EasePreset, FrameRate, History, Interp, Layer,
    LinearRgba, PoolItem, PropId, SourceRef, SpatialTangents, Time, Value, WorkingSpace, persist,
};

/// An edit described independently of the IDs that exist at run time; layer
/// and property choices are indices resolved against the current document.
#[derive(Debug, Clone)]
enum Edit {
    AddSolid(f32, f32, f32),
    AddImage(u32, u32),
    Remove(usize),
    Rename(usize, String),
    Move(usize, usize),
    Visible(usize, bool),
    Position(usize, f64, f64),
    Rotation(usize, f64),
    Opacity(usize, f64),
    WorkingSpace(bool),
    // Keyframe edits: (layer, property selector, ...). Times are milliseconds.
    Animate(usize, u8, i64, bool),
    SetAt(usize, u8, i64, f64),
    RemoveKey(usize, u8, usize),
    MoveKey(usize, u8, usize, i64),
    Interp(usize, u8, usize, u8),
    Preset(usize, u8, usize, usize),
    Ease(usize, u8, usize, [f64; 4]),
    Tangent(usize, usize, f64, f64),
    // Pool and precomp edits (Milestone A).
    NewComp,
    Switch(usize),
    AddPrecomp(usize, i64),
    Precompose(usize, usize, bool),
    Unprecompose(usize),
    RemoveItem(usize, bool),
    Duplicate,
    RenameComp(String),
    NewFolder,
    MoveItem(usize, usize),
    Resize(u32, u32, bool),
    Rate(usize, bool),
    PrecompTiming(usize, u8, bool, bool),
    Timing(usize, i64, i64, i64),
    Undo,
    Redo,
    EndInteraction,
}

fn edit() -> impl Strategy<Value = Edit> {
    let unit = 0.0f32..=1.0;
    prop_oneof![
        (unit.clone(), unit.clone(), unit).prop_map(|(r, g, b)| Edit::AddSolid(r, g, b)),
        (1u32..4096, 1u32..4096).prop_map(|(w, h)| Edit::AddImage(w, h)),
        any::<usize>().prop_map(Edit::Remove),
        (any::<usize>(), "[a-zA-Z0-9 ]{0,12}").prop_map(|(i, s)| Edit::Rename(i, s)),
        (any::<usize>(), 0usize..8).prop_map(|(i, to)| Edit::Move(i, to)),
        (any::<usize>(), any::<bool>()).prop_map(|(i, v)| Edit::Visible(i, v)),
        (any::<usize>(), -5000.0f64..5000.0, -5000.0f64..5000.0)
            .prop_map(|(i, x, y)| Edit::Position(i, x, y)),
        (any::<usize>(), -720.0f64..720.0).prop_map(|(i, r)| Edit::Rotation(i, r)),
        (any::<usize>(), -1.0f64..2.0).prop_map(|(i, o)| Edit::Opacity(i, o)),
        any::<bool>().prop_map(Edit::WorkingSpace),
        (any::<usize>(), 0u8..4, -2000i64..12_000, any::<bool>())
            .prop_map(|(i, p, t, on)| Edit::Animate(i, p, t, on)),
        (any::<usize>(), 0u8..4, -2000i64..12_000, -1000.0f64..1000.0)
            .prop_map(|(i, p, t, v)| Edit::SetAt(i, p, t, v)),
        (any::<usize>(), 0u8..4, any::<usize>()).prop_map(|(i, p, k)| Edit::RemoveKey(i, p, k)),
        (any::<usize>(), 0u8..4, any::<usize>(), -3000i64..3000)
            .prop_map(|(i, p, k, d)| Edit::MoveKey(i, p, k, d)),
        (any::<usize>(), 0u8..4, any::<usize>(), 0u8..3)
            .prop_map(|(i, p, k, m)| Edit::Interp(i, p, k, m)),
        (
            any::<usize>(),
            0u8..4,
            any::<usize>(),
            0usize..EasePreset::ALL.len()
        )
            .prop_map(|(i, p, k, n)| Edit::Preset(i, p, k, n)),
        (
            any::<usize>(),
            0u8..4,
            any::<usize>(),
            prop::array::uniform4(-1.0f64..2.0)
        )
            .prop_map(|(i, p, k, e)| Edit::Ease(i, p, k, e)),
        (
            any::<usize>(),
            any::<usize>(),
            -500.0f64..500.0,
            -500.0f64..500.0
        )
            .prop_map(|(i, k, x, y)| Edit::Tangent(i, k, x, y)),
        Just(Edit::Undo),
        Just(Edit::Redo),
        Just(Edit::EndInteraction),
    ]
}

fn pool_edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        Just(Edit::NewComp),
        any::<usize>().prop_map(Edit::Switch),
        (any::<usize>(), 0i64..12_000).prop_map(|(i, t)| Edit::AddPrecomp(i, t)),
        (any::<usize>(), 1usize..4, any::<bool>())
            .prop_map(|(i, n, leave)| Edit::Precompose(i, n, leave)),
        any::<usize>().prop_map(Edit::Unprecompose),
        (any::<usize>(), any::<bool>()).prop_map(|(i, p)| Edit::RemoveItem(i, p)),
        Just(Edit::Duplicate),
        "[a-zA-Z0-9 ]{0,8}".prop_map(Edit::RenameComp),
        Just(Edit::NewFolder),
        (any::<usize>(), any::<usize>()).prop_map(|(i, f)| Edit::MoveItem(i, f)),
        (16u32..2000, 16u32..2000, any::<bool>()).prop_map(|(w, h, s)| Edit::Resize(w, h, s)),
        (0usize..8, any::<bool>()).prop_map(|(r, s)| Edit::Rate(r, s)),
        (any::<usize>(), 1u8..8, any::<bool>(), any::<bool>())
            .prop_map(|(i, sp, l, r)| Edit::PrecompTiming(i, sp, l, r)),
        (
            any::<usize>(),
            -2000i64..12_000,
            0i64..12_000,
            -2000i64..4000
        )
            .prop_map(|(i, a, len, o)| Edit::Timing(i, a, len, o)),
    ]
}

/// Edits of every kind, pool edits included.
fn any_edit() -> impl Strategy<Value = Edit> {
    prop_oneof![3 => edit(), 2 => pool_edit()]
}

/// Every ID in a document.
fn all_ids(doc: &Document) -> BTreeSet<u64> {
    let mut ids: BTreeSet<u64> = doc.assets.iter().map(|a| a.id.raw()).collect();
    ids.extend(doc.folders.keys().map(|f| f.raw()));
    for c in doc.comps.values() {
        ids.insert(c.id.raw());
        ids.insert(c.background.id.raw());
        for l in &c.layers {
            ids.insert(l.id.raw());
            ids.extend(l.properties().iter().map(|p| p.id.raw()));
        }
    }
    ids
}

/// Property selector → (property ID, a value of its kind derived from `v`).
fn target(l: &Layer, sel: u8, v: f64) -> (PropId, Value) {
    let t = &l.transform;
    match sel {
        0 => (t.position.id, Value::Vec2(DVec2::new(v, -v))),
        1 => (t.rotation.id, Value::Scalar(v)),
        2 => (t.opacity.id, Value::Scalar(v / 1000.0)),
        _ => (t.scale.id, Value::Vec2(DVec2::splat(v / 100.0))),
    }
}

/// The time of the `k`-th keyframe (modulo count) of an animated property.
fn key_time(doc: &Document, prop: PropId, k: usize) -> Option<Time> {
    let track = doc.property(prop)?.track()?;
    Some(track.keys()[k % track.len()].time)
}

/// The composition edits go to: the one `focus` names if it still exists,
/// else the main one, else none.
fn target_comp(doc: &Document, focus: Option<CompId>) -> Option<CompId> {
    focus.filter(|c| doc.comp(*c).is_some()).or(doc.main_comp)
}

/// Pool items in a stable order.
fn pool_items(doc: &Document) -> Vec<PoolItem> {
    let mut v: Vec<PoolItem> = doc.assets.iter().map(|a| PoolItem::Asset(a.id)).collect();
    v.extend(doc.comps.keys().map(|&c| PoolItem::Comp(c)));
    v.extend(doc.folders.keys().map(|&f| PoolItem::Folder(f)));
    v
}

fn apply(h: &mut History, e: &Edit, focus: &mut Option<CompId>) {
    let doc = h.current().clone();
    let Some(comp) = target_comp(&doc, *focus) else {
        // No compositions left: only edits that need none apply.
        match e {
            Edit::NewComp => apply_pool(h, e, &doc, None, focus),
            Edit::Undo => drop(h.undo()),
            Edit::Redo => drop(h.redo()),
            _ => {}
        }
        return;
    };
    if matches!(
        e,
        Edit::NewComp
            | Edit::Switch(_)
            | Edit::AddPrecomp(..)
            | Edit::Precompose(..)
            | Edit::Unprecompose(_)
            | Edit::RemoveItem(..)
            | Edit::Duplicate
            | Edit::RenameComp(_)
            | Edit::NewFolder
            | Edit::MoveItem(..)
            | Edit::Resize(..)
            | Edit::Rate(..)
            | Edit::PrecompTiming(..)
            | Edit::Timing(..)
    ) {
        apply_pool(h, e, &doc, Some(comp), focus);
        return;
    }
    let layers = &doc.comp(comp).expect("exists").layers;
    let pick = |i: usize| (!layers.is_empty()).then(|| &layers[i % layers.len()]);
    // Edits on missing targets are skipped; invalid values must be rejected
    // without changing anything, so errors are ignored on purpose.
    let _ = match e {
        Edit::AddSolid(r, g, b) => h.execute(AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::new(*r, *g, *b, 1.0),
            index: 0,
        }),
        Edit::AddImage(w, hh) => h.execute(AddImageLayer {
            comp,
            path: "media/img.png".into(),
            pixel_size: [*w, *hh],
            index: 0,
        }),
        Edit::Remove(i) => match pick(*i) {
            Some(l) => h.execute(RemoveLayers { layers: vec![l.id] }),
            None => Ok(()),
        },
        Edit::Rename(i, name) => match pick(*i) {
            Some(l) => h.execute(RenameLayer {
                layer: l.id,
                name: name.clone(),
            }),
            None => Ok(()),
        },
        Edit::Move(i, to) => match pick(*i) {
            Some(l) => h.execute(MoveLayer {
                layer: l.id,
                to_index: *to,
            }),
            None => Ok(()),
        },
        Edit::Visible(i, v) => match pick(*i) {
            Some(l) => h.execute(SetLayerVisible {
                layer: l.id,
                visible: *v,
            }),
            None => Ok(()),
        },
        Edit::Position(i, x, y) => match pick(*i) {
            Some(l) => h.execute(SetProperty {
                time: Time::ZERO,
                prop: l.transform.position.id,
                value: Value::Vec2(DVec2::new(*x, *y)),
            }),
            None => Ok(()),
        },
        Edit::Rotation(i, r) => match pick(*i) {
            Some(l) => h.execute(SetProperty {
                time: Time::ZERO,
                prop: l.transform.rotation.id,
                value: Value::Scalar(*r),
            }),
            None => Ok(()),
        },
        Edit::Opacity(i, o) => match pick(*i) {
            Some(l) => h.execute(SetProperty {
                time: Time::ZERO,
                prop: l.transform.opacity.id,
                value: Value::Scalar(*o),
            }),
            None => Ok(()),
        },
        Edit::WorkingSpace(linear) => h.execute(SetWorkingSpace(if *linear {
            WorkingSpace::Linear
        } else {
            WorkingSpace::SrgbCompat
        })),
        Edit::Undo => {
            h.undo();
            Ok(())
        }
        Edit::Redo => {
            h.redo();
            Ok(())
        }
        Edit::EndInteraction => {
            h.end_interaction();
            Ok(())
        }
        Edit::Animate(i, sel, ms, on) => match pick(*i) {
            Some(l) => h.execute(SetAnimated {
                prop: target(l, *sel, 0.0).0,
                time: Time::from_millis(*ms),
                animated: *on,
            }),
            None => Ok(()),
        },
        Edit::SetAt(i, sel, ms, v) => match pick(*i) {
            Some(l) => {
                let (prop, value) = target(l, *sel, *v);
                h.execute(SetProperty {
                    prop,
                    time: Time::from_millis(*ms),
                    value,
                })
            }
            None => Ok(()),
        },
        Edit::RemoveKey(i, sel, k) => match pick(*i).map(|l| target(l, *sel, 0.0).0) {
            Some(prop) => match key_time(&doc, prop, *k) {
                Some(t) => h.execute(RemoveKeyframes {
                    prop,
                    times: vec![t],
                }),
                None => Ok(()),
            },
            None => Ok(()),
        },
        Edit::MoveKey(i, sel, k, d) => match pick(*i).map(|l| target(l, *sel, 0.0).0) {
            Some(prop) => match key_time(&doc, prop, *k) {
                Some(t) => h.execute(MoveKeyframes {
                    prop,
                    times: vec![t],
                    delta: Time::from_millis(*d),
                }),
                None => Ok(()),
            },
            None => Ok(()),
        },
        Edit::Interp(i, sel, k, m) => match pick(*i).map(|l| target(l, *sel, 0.0).0) {
            Some(prop) => match key_time(&doc, prop, *k) {
                Some(t) => h.execute(SetInterpolation {
                    prop,
                    times: vec![t],
                    interp: [Interp::Hold, Interp::Linear, Interp::Bezier][usize::from(*m)],
                }),
                None => Ok(()),
            },
            None => Ok(()),
        },
        Edit::Preset(i, sel, k, n) => match pick(*i).map(|l| target(l, *sel, 0.0).0) {
            Some(prop) => match key_time(&doc, prop, *k) {
                Some(t) => h.execute(ApplyEasePreset {
                    prop,
                    times: vec![t],
                    preset: EasePreset::ALL[*n],
                }),
                None => Ok(()),
            },
            None => Ok(()),
        },
        Edit::Ease(i, sel, k, [x1, y1, x2, y2]) => match pick(*i).map(|l| target(l, *sel, 0.0).0) {
            // Out-of-range x values produce no handle: exercises "None" sides.
            Some(prop) => match key_time(&doc, prop, *k) {
                Some(t) => h.execute(SetEase {
                    prop,
                    time: t,
                    ease_in: EaseHandle::new(*x1, *y1),
                    ease_out: EaseHandle::new(*x2, *y2),
                }),
                None => Ok(()),
            },
            None => Ok(()),
        },
        Edit::Tangent(i, k, x, y) => match pick(*i).map(|l| l.transform.position.id) {
            Some(prop) => match key_time(&doc, prop, *k) {
                Some(t) => h.execute(SetSpatialTangents {
                    prop,
                    time: t,
                    tangents: Some(SpatialTangents {
                        incoming: DVec3::new(-x, -y, 0.0),
                        outgoing: DVec3::new(*x, *y, 0.0),
                    }),
                }),
                None => Ok(()),
            },
            None => Ok(()),
        },
        _ => Ok(()),
    };
}

#[allow(clippy::too_many_lines)]
fn apply_pool(
    h: &mut History,
    e: &Edit,
    doc: &Document,
    comp: Option<CompId>,
    focus: &mut Option<CompId>,
) {
    let layers: &[std::sync::Arc<Layer>] =
        comp.and_then(|c| doc.comp(c)).map_or(&[], |c| &c.layers);
    let pick = |i: usize| (!layers.is_empty()).then(|| &layers[i % layers.len()]);
    let comps: Vec<CompId> = doc.comps.keys().copied().collect();
    // Edits on missing targets are skipped and rejected edits must change
    // nothing, so errors are ignored on purpose.
    let _ = match (e, comp) {
        (Edit::NewComp, _) => h
            .execute_reported(CreateComposition {
                settings: CompSettings {
                    name: "Comp".into(),
                    width: 160,
                    height: 90,
                    ..CompSettings::default()
                },
                folder: None,
            })
            .map(|r| *focus = r.created_comps.first().copied()),
        (Edit::Switch(i), _) => {
            *focus = (!comps.is_empty()).then(|| comps[i % comps.len()]);
            Ok(())
        }
        (Edit::AddPrecomp(i, ms), Some(c)) if !comps.is_empty() => h.execute(AddSourceLayer {
            comp: c,
            source: SourceRef::Comp(comps[i % comps.len()]),
            index: 0,
            in_point: Time::from_millis(*ms),
        }),
        (Edit::Precompose(i, n, leave), Some(c)) if !layers.is_empty() => {
            let ids = (0..*n)
                .map(|k| layers[(i + k) % layers.len()].id)
                .collect::<BTreeSet<_>>();
            h.execute(PrecomposeLayers {
                comp: c,
                layers: ids.into_iter().collect(),
                mode: if *leave {
                    PrecomposeMode::LeaveAttributes
                } else {
                    PrecomposeMode::MoveAttributes
                },
                name: "Pre".into(),
                time: Time::from_millis(300),
            })
        }
        (Edit::Unprecompose(i), _) => match pick(*i) {
            Some(l) => h.execute(UnprecomposeLayer { layer: l.id }),
            None => Ok(()),
        },
        (Edit::RemoveItem(i, placeholder), _) => {
            let items = pool_items(doc);
            if items.is_empty() {
                Ok(())
            } else {
                h.execute(RemoveItems {
                    items: vec![items[i % items.len()]],
                    mode: if *placeholder {
                        RemoveMode::PlaceholderLayers
                    } else {
                        RemoveMode::DeleteLayers
                    },
                })
            }
        }
        (Edit::Duplicate, Some(c)) => h.execute(DuplicateComposition { comp: c }),
        (Edit::RenameComp(name), Some(c)) => h.execute(RenameComposition {
            comp: c,
            name: name.clone(),
        }),
        (Edit::NewFolder, _) => h.execute(CreateFolder {
            name: "F".into(),
            parent: doc.folders.keys().next_back().copied(),
        }),
        (Edit::MoveItem(i, f), _) => {
            let items = pool_items(doc);
            let folders: Vec<_> = doc.folders.keys().copied().collect();
            if items.is_empty() {
                Ok(())
            } else {
                h.execute(MoveToFolder {
                    items: vec![items[i % items.len()]],
                    folder: (!folders.is_empty() && f % 3 != 0).then(|| folders[f % folders.len()]),
                })
            }
        }
        (Edit::Resize(w, hh, scale), Some(c)) => {
            let mut s = CompSettings::of(doc.comp(c).expect("exists"));
            s.width = *w;
            s.height = *hh;
            h.execute(SetCompositionSettings {
                comp: c,
                settings: s,
                resize: if *scale {
                    ResizeMode::ScaleToFit
                } else {
                    ResizeMode::KeepLayers(Anchor::grid(2, 0))
                },
                snap_to_frames: false,
            })
        }
        (Edit::Rate(r, snap), Some(c)) => {
            let mut s = CompSettings::of(doc.comp(c).expect("exists"));
            s.frame_rate = [
                FrameRate::FPS_23_976,
                FrameRate::FPS_24,
                FrameRate::FPS_25,
                FrameRate::FPS_29_97,
                FrameRate::FPS_30,
                FrameRate::FPS_50,
                FrameRate::FPS_59_94,
                FrameRate::FPS_60,
            ][*r];
            h.execute(SetCompositionSettings {
                comp: c,
                settings: s,
                resize: ResizeMode::default(),
                snap_to_frames: *snap,
            })
        }
        (Edit::PrecompTiming(i, speed, looping, remap), _) => match pick(*i) {
            Some(l) => h.execute(SetPrecompTiming {
                layer: l.id,
                speed: f64::from(*speed) / 2.0,
                looping: *looping,
                time_remap: *remap,
            }),
            None => Ok(()),
        },
        (Edit::Timing(i, start, len, offset), _) => match pick(*i) {
            Some(l) => h.execute(SetLayerTiming {
                layer: l.id,
                in_point: Time::from_millis(*start),
                out_point: Time::from_millis(start + len),
                start_offset: Time::from_millis(*offset),
            }),
            None => Ok(()),
        },
        _ => Ok(()),
    };
}

fn small_doc() -> Document {
    Document::new(
        "Props",
        CompSettings {
            width: 320,
            height: 180,
            ..CompSettings::default()
        },
    )
}

proptest! {

    /// Undoing everything returns the original document; redoing everything
    /// returns the final one. Every intermediate state is valid.
    #[test]
    fn undo_redo_round_trip(edits in prop::collection::vec(any_edit(), 0..40)) {
        let mut h = History::new(small_doc());
        let initial = h.current().clone();
        let mut focus = None;
        for e in &edits {
            apply(&mut h, e, &mut focus);
            prop_assert!(h.current().validate().is_ok());
        }
        // The sequence may end mid-way through its own undo stack; the far
        // end of the redo chain is what "redo everything" must reach.
        while h.redo() {}
        let last = h.current().clone();
        while h.undo() {}
        prop_assert_eq!(&**h.current(), &*initial);
        while h.redo() {}
        prop_assert_eq!(&**h.current(), &*last);
    }

    /// Any reachable document survives save → load unchanged, and saving
    /// again is byte-identical.
    #[test]
    fn save_load_round_trip(edits in prop::collection::vec(any_edit(), 0..40)) {
        let mut h = History::new(small_doc());
        let mut focus = None;
        for e in &edits {
            apply(&mut h, e, &mut focus);
        }
        let json = persist::to_json(h.current()).unwrap();
        let loaded = persist::from_json(&json).unwrap();
        prop_assert_eq!(&loaded, &**h.current());
        prop_assert_eq!(persist::to_json(&loaded).unwrap(), json);
    }

    /// Evaluation never panics and keeps opacity in range.
    #[test]
    fn evaluation_is_total(edits in prop::collection::vec(any_edit(), 0..30), t in -10_000i64..20_000) {
        let mut h = History::new(small_doc());
        let mut focus = None;
        for e in &edits {
            apply(&mut h, e, &mut focus);
        }
        let doc = h.current();
        for &c in doc.comps.keys() {
            let eval = voxy_core::evaluate(doc, c, Time::from_millis(t)).unwrap();
            for l in &eval.layers {
                prop_assert!((0.0..=1.0).contains(&l.opacity));
                prop_assert!(l.world.is_finite());
            }
        }
    }

    /// An ID that existed in any earlier state is never given to a new
    /// node, even after undo.
    #[test]
    fn ids_are_never_reused(edits in prop::collection::vec(any_edit(), 0..40)) {
        let mut h = History::new(small_doc());
        let mut focus = None;
        let mut high = h.current().next_id();
        for e in &edits {
            let before = all_ids(h.current());
            let was_undo_redo = matches!(e, Edit::Undo | Edit::Redo);
            apply(&mut h, e, &mut focus);
            if !was_undo_redo {
                let now = all_ids(h.current());
                // Edits only move existing nodes or allocate new ones, so
                // every ID that appears is above everything handed out yet.
                for id in now.difference(&before) {
                    prop_assert!(*id >= high, "ID {id} reused");
                }
            }
            high = high.max(h.current().next_id());
        }
    }

    /// Arbitrary bytes never make the project loader panic.
    #[test]
    fn loader_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let text = String::from_utf8_lossy(&bytes);
        let _ = persist::from_json(&text);
    }
}

#[test]
fn colors_survive_json_exactly() {
    // f32 colors go through f64 JSON; check the full byte range of sRGB8.
    let mut h = History::new(small_doc());
    for v in 0..=255u8 {
        let comp = h.current().main_comp.unwrap();
        h.execute(AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::from_srgb8(v, v, 255 - v, v),
            index: 0,
        })
        .unwrap();
    }
    let json = persist::to_json(h.current()).unwrap();
    assert_eq!(&persist::from_json(&json).unwrap(), &**h.current());
}
