//! Property-based tests: random edit sequences against undo/redo and
//! save/load.

#![allow(clippy::many_single_char_names)] // r, g, b, x, y, i in generators read best short.

use proptest::prelude::*;
use voxy_core::command::{
    AddImageLayer, AddSolidLayer, MoveLayer, RemoveLayers, RenameLayer, SetLayerVisible,
    SetProperty, SetWorkingSpace,
};
use voxy_core::glam::DVec2;
use voxy_core::{CompSettings, Document, History, LinearRgba, Time, Value, WorkingSpace, persist};

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
        Just(Edit::Undo),
        Just(Edit::Redo),
        Just(Edit::EndInteraction),
    ]
}

fn apply(h: &mut History, e: &Edit) {
    let doc = h.current().clone();
    let comp = doc.root_comp;
    let layers = &doc.root().layers;
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
                prop: l.transform.position.id,
                value: Value::Vec2(DVec2::new(*x, *y)),
            }),
            None => Ok(()),
        },
        Edit::Rotation(i, r) => match pick(*i) {
            Some(l) => h.execute(SetProperty {
                prop: l.transform.rotation.id,
                value: Value::Scalar(*r),
            }),
            None => Ok(()),
        },
        Edit::Opacity(i, o) => match pick(*i) {
            Some(l) => h.execute(SetProperty {
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
    fn undo_redo_round_trip(edits in prop::collection::vec(edit(), 0..40)) {
        let mut h = History::new(small_doc());
        let initial = h.current().clone();
        for e in &edits {
            apply(&mut h, e);
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
    fn save_load_round_trip(edits in prop::collection::vec(edit(), 0..40)) {
        let mut h = History::new(small_doc());
        for e in &edits {
            apply(&mut h, e);
        }
        let json = persist::to_json(h.current()).unwrap();
        let loaded = persist::from_json(&json).unwrap();
        prop_assert_eq!(&loaded, &**h.current());
        prop_assert_eq!(persist::to_json(&loaded).unwrap(), json);
    }

    /// Evaluation never panics and keeps opacity in range.
    #[test]
    fn evaluation_is_total(edits in prop::collection::vec(edit(), 0..30), t in -10_000i64..20_000) {
        let mut h = History::new(small_doc());
        for e in &edits {
            apply(&mut h, e);
        }
        let doc = h.current();
        let eval = voxy_core::evaluate(doc, doc.root_comp, Time::from_millis(t)).unwrap();
        for l in &eval.layers {
            prop_assert!((0.0..=1.0).contains(&l.opacity));
            prop_assert!(l.world.is_finite());
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
        let comp = h.current().root_comp;
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
