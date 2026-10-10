//! Evaluating nested compositions (docs/design-media-comps.md §8.1).

#![allow(clippy::many_single_char_names)] // a, b, c, h, l read best short in these scenarios.

use voxy_core::command::{
    AddSolidLayer, AddSourceLayer, Command, CreateComposition, SetCollapse, SetLayerTiming,
    SetPrecompTiming,
};
use voxy_core::{
    CompId, CompSettings, Document, EvaluatedContent, History, LayerId, MAX_NESTING, Nested,
    SourceRef, Time, evaluate,
};

fn settings(name: &str, secs: i64) -> CompSettings {
    CompSettings {
        name: name.into(),
        width: 100,
        height: 50,
        duration: Time::from_secs(secs),
        ..CompSettings::default()
    }
}

fn new_comp(h: &mut History, name: &str, secs: i64) -> CompId {
    h.execute_reported(CreateComposition {
        settings: settings(name, secs),
        folder: None,
    })
    .expect("test setup")
    .created_comps[0]
}

fn nest(h: &mut History, into: CompId, child: CompId) -> LayerId {
    h.execute_reported(AddSourceLayer {
        comp: into,
        source: SourceRef::Comp(child),
        index: 0,
        in_point: Time::ZERO,
    })
    .expect("test setup")
    .created_layers[0]
}

fn nested_of(doc: &Document, comp: CompId, time: Time) -> (Time, Nested, bool) {
    let eval = evaluate(doc, comp, time).expect("comp");
    match &eval.layers[0].content {
        EvaluatedContent::Precomp {
            child_time,
            nested,
            collapse,
            ..
        } => (*child_time, nested.clone(), *collapse),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_child_is_evaluated_at_the_mapped_time_with_its_own_layers() {
    let mut h = History::new(Document::empty("N"));
    let parent = new_comp(&mut h, "Parent", 10);
    let child = new_comp(&mut h, "Child", 4);
    h.execute(AddSolidLayer {
        comp: child,
        name: None,
        color: voxy_core::LinearRgba::WHITE,
        index: 0,
    })
    .expect("test setup");
    let l = nest(&mut h, parent, child);
    h.execute(SetPrecompTiming {
        layer: l,
        speed: 2.0,
        looping: false,
        time_remap: false,
    })
    .expect("test setup");
    let (ct, nested, collapse) = nested_of(h.current(), parent, Time::from_secs(1));
    assert_eq!(
        ct,
        Time::from_secs(2),
        "speed 2: one parent second is two child seconds"
    );
    assert!(!collapse);
    let Nested::Shown(eval) = nested else {
        panic!("expected the child")
    };
    assert_eq!((eval.comp, eval.time, eval.layers.len()), (child, ct, 1));
    assert_eq!((eval.width, eval.height), (100, 50));
}

#[test]
fn past_the_end_of_the_child_nothing_is_shown_unless_it_loops() {
    let mut h = History::new(Document::empty("N"));
    let parent = new_comp(&mut h, "Parent", 10);
    let child = new_comp(&mut h, "Child", 4);
    let l = nest(&mut h, parent, child);
    // The precomp layer is as long as the child (4 s); stretch it to the
    // parent's whole length so it outlives the child.
    h.execute(SetLayerTiming {
        layer: l,
        in_point: Time::ZERO,
        out_point: Time::from_secs(10),
        start_offset: Time::ZERO,
    })
    .expect("test setup");
    let (_, nested, _) = nested_of(h.current(), parent, Time::from_secs(5));
    assert_eq!(nested, Nested::OutsideChild);
    h.execute(SetPrecompTiming {
        layer: l,
        speed: 1.0,
        looping: true,
        time_remap: false,
    })
    .expect("test setup");
    let (ct, nested, _) = nested_of(h.current(), parent, Time::from_secs(5));
    assert_eq!(ct, Time::from_secs(1), "5 s wraps in a 4 s child");
    assert!(matches!(nested, Nested::Shown(_)));
}

#[test]
fn the_collapse_flag_is_reported() {
    let mut h = History::new(Document::empty("N"));
    let parent = new_comp(&mut h, "Parent", 10);
    let child = new_comp(&mut h, "Child", 10);
    let l = nest(&mut h, parent, child);
    h.execute(SetCollapse {
        layer: l,
        collapse: true,
    })
    .expect("test setup");
    assert!(nested_of(h.current(), parent, Time::ZERO).2);
}

#[test]
fn a_cycle_that_got_past_validation_draws_nothing_and_does_not_loop() {
    let mut h = History::new(Document::empty("N"));
    let a = new_comp(&mut h, "A", 10);
    let b = new_comp(&mut h, "B", 10);
    nest(&mut h, a, b);
    // Commands refuse this, so corrupt the document directly.
    let mut doc = Document::clone(h.current());
    let layer = nest_raw(&mut doc, b, a);
    assert_ne!(layer, LayerId(0));
    let eval = evaluate(&doc, a, Time::ZERO).expect("a");
    let EvaluatedContent::Precomp { nested, .. } = &eval.layers[0].content else {
        panic!()
    };
    let Nested::Shown(b_eval) = nested else {
        panic!("B shows")
    };
    let EvaluatedContent::Precomp { nested, .. } = &b_eval.layers[0].content else {
        panic!()
    };
    assert_eq!(*nested, Nested::TooDeep, "A inside B inside A is cut");
}

/// Adds a precomp layer of `child` to `into` without the cycle check.
fn nest_raw(doc: &mut Document, into: CompId, child: CompId) -> LayerId {
    let mut scratch = Document::clone(doc);
    // Build the layer with a command on a copy where it is legal, then move it.
    let other = scratch
        .comps
        .keys()
        .copied()
        .find(|&c| c != into && c != child)
        .unwrap_or_else(|| {
            CreateComposition {
                settings: settings("Other", 10),
                folder: None,
            }
            .apply(&mut scratch)
            .expect("create");
            *scratch.comps.keys().last().expect("other")
        });
    AddSourceLayer {
        comp: other,
        source: SourceRef::Comp(child),
        index: 0,
        in_point: Time::ZERO,
    }
    .apply(&mut scratch)
    .expect("legal in the copy");
    let layer = scratch.comp(other).expect("other").layers[0].clone();
    let id = layer.id;
    doc.comp_mut(into).expect("into").layers.insert(0, layer);
    id
}

#[test]
fn a_chain_deeper_than_the_limit_is_cut_at_the_limit() {
    let mut h = History::new(Document::empty("N"));
    let comps: Vec<CompId> = (0..MAX_NESTING + 3)
        .map(|i| new_comp(&mut h, &format!("C{i}"), 10))
        .collect();
    for pair in comps.windows(2) {
        nest(&mut h, pair[0], pair[1]);
    }
    let mut depth = 1;
    let mut eval = evaluate(h.current(), comps[0], Time::ZERO).expect("root");
    loop {
        let EvaluatedContent::Precomp { nested, .. } = &eval.layers[0].content else {
            panic!()
        };
        match nested {
            Nested::Shown(inner) => {
                depth += 1;
                eval = (**inner).clone();
            }
            Nested::TooDeep => break,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(depth, MAX_NESTING, "the root plus 15 levels are evaluated");
}

#[test]
fn a_missing_child_is_reported_as_missing() {
    let mut h = History::new(Document::empty("N"));
    let parent = new_comp(&mut h, "Parent", 10);
    let child = new_comp(&mut h, "Child", 10);
    nest(&mut h, parent, child);
    let mut doc = Document::clone(h.current());
    std::sync::Arc::make_mut(&mut doc.comps).remove(&child);
    let (_, nested, _) = nested_of(&doc, parent, Time::ZERO);
    assert_eq!(nested, Nested::Missing);
}
