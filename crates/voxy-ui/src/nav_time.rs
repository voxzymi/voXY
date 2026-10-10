//! Time mapping between a precomp layer's composition and the composition it
//! shows (`docs/design-media-comps.md` §7.2 and §7.3). Pure functions over
//! [`Layer::child_time`], the single mapping the renderer uses.

use voxy_core::{FrameRate, Layer, LayerKind, Time};

/// The child times a precomp layer shows while it is active, for shading the
/// child's ruler outside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildRange {
    /// The layer shows child times from `first` to `last` (inclusive).
    Bounded { first: Time, last: Time },
    /// The layer can show any time of the child (a loop that wraps), so
    /// nothing is shaded.
    Everything,
}

/// Most parent frames sampled; longer layers are sampled with a stride (the
/// ends are always included).
const MAX_SAMPLES: i64 = 4000;

/// The range of child times `layer` shows between its in- and out-points,
/// found by sampling `child_time` on the parent's frames (so remaps are
/// followed). `None` if `layer` is not a precomp layer.
pub fn child_range(
    layer: &Layer,
    child_duration: Time,
    parent_rate: FrameRate,
) -> Option<ChildRange> {
    let LayerKind::Precomp { looping, .. } = &layer.kind else {
        return None;
    };
    let frame = parent_rate.frame_to_time(1).max(Time(1));
    let end = (layer.out_point - Time(1)).max(layer.in_point);
    let count = ((end - layer.in_point).flicks() / frame.flicks().max(1)).max(0) + 1;
    let stride = (count / MAX_SAMPLES).max(1);
    let at = |t: Time| layer.child_time(t, child_duration);
    let mut first = at(layer.in_point)?;
    let mut last = first;
    let mut hit = |c: Time| {
        first = first.min(c);
        last = last.max(c);
    };
    let mut i = 0;
    while i < count {
        if let Some(c) = at(layer.in_point + Time::from_flicks(i * frame.flicks())) {
            hit(c);
        }
        i += stride;
    }
    if let Some(c) = at(end) {
        hit(c);
    }
    // A loop that wrapped shows the whole child.
    if *looping && child_duration > Time::ZERO && last - first >= child_duration - frame {
        return Some(ChildRange::Everything);
    }
    Some(ChildRange::Bounded { first, last })
}

/// The parent time at which `layer` shows child time `child_t`, if the
/// mapping can be inverted: no time remap, no loop, a speed that is not
/// zero. Otherwise `None` (scrubbing the child does not move the parent).
pub fn parent_time_of(layer: &Layer, child_t: Time) -> Option<Time> {
    let LayerKind::Precomp {
        speed,
        looping,
        time_remap,
        ..
    } = &layer.kind
    else {
        return None;
    };
    if time_remap.is_some() || *looping || *speed == 0.0 || !speed.is_finite() {
        return None;
    }
    let local = child_t - layer.start_offset;
    let parent = if *speed == 1.0 {
        local
    } else {
        Time::from_flicks((local.flicks() as f64 / speed).round() as i64)
    };
    Some(layer.in_point + parent)
}

#[cfg(test)]
mod tests {
    use voxy_core::command::{AddSourceLayer, CreateComposition, SetLayerTiming, SetPrecompTiming};
    use voxy_core::{CompId, CompSettings, Document, History, LayerId, SourceRef};

    use super::*;

    fn setup() -> (History, CompId, CompId, LayerId) {
        let mut h = History::new(Document::empty("n"));
        let mut comp = |name: &str| {
            h.execute_reported(CreateComposition {
                settings: CompSettings {
                    name: name.into(),
                    duration: Time::from_secs(10),
                    ..CompSettings::default()
                },
                folder: None,
            })
            .expect("test setup")
            .created_comps[0]
        };
        let (parent, child) = (comp("P"), comp("C"));
        let layer = h
            .execute_reported(AddSourceLayer {
                comp: parent,
                source: SourceRef::Comp(child),
                index: 0,
                in_point: Time::ZERO,
            })
            .expect("test setup")
            .created_layers[0];
        (h, parent, child, layer)
    }

    fn layer(h: &History, id: LayerId) -> Layer {
        h.current().find_layer(id).expect("layer").1.clone()
    }

    fn timing(h: &mut History, l: LayerId, secs: (i64, i64), offset_ms: i64) {
        h.execute(SetLayerTiming {
            layer: l,
            in_point: Time::from_secs(secs.0),
            out_point: Time::from_secs(secs.1),
            start_offset: Time::from_millis(offset_ms),
        })
        .expect("test setup");
    }

    fn precomp(h: &mut History, l: LayerId, speed: f64, looping: bool) {
        h.execute(SetPrecompTiming {
            layer: l,
            speed,
            looping,
            time_remap: false,
        })
        .expect("test setup");
    }

    const RATE: FrameRate = FrameRate::FPS_30;

    #[test]
    fn a_plain_precomp_shows_its_offset_range() {
        let (mut h, _, _, l) = setup();
        timing(&mut h, l, (2, 6), 1000);
        let r = child_range(&layer(&h, l), Time::from_secs(10), RATE).expect("precomp");
        let ChildRange::Bounded { first, last } = r else {
            panic!("{r:?}")
        };
        assert_eq!(first, Time::from_secs(1), "child time at the in-point");
        // The last parent frame is one flick before the out-point.
        assert!((last - Time::from_secs(5)).flicks().abs() < RATE.frame_to_time(1).flicks());
    }

    #[test]
    fn speed_scales_the_range() {
        let (mut h, _, _, l) = setup();
        timing(&mut h, l, (0, 4), 0);
        precomp(&mut h, l, 2.0, false);
        let ChildRange::Bounded { first, last } =
            child_range(&layer(&h, l), Time::from_secs(10), RATE).expect("precomp")
        else {
            panic!()
        };
        assert_eq!(first, Time::ZERO);
        assert!((last - Time::from_secs(8)).flicks().abs() < 2 * RATE.frame_to_time(1).flicks());
    }

    #[test]
    fn a_loop_that_wraps_shades_nothing_and_one_that_does_not_is_bounded() {
        let (mut h, _, _, l) = setup();
        timing(&mut h, l, (0, 10), 0);
        // Child is 10 s; the layer is 25 s long once stretched... keep within
        // the parent: speed 3 makes 10 parent seconds cover 30 child seconds.
        precomp(&mut h, l, 3.0, true);
        assert_eq!(
            child_range(&layer(&h, l), Time::from_secs(10), RATE),
            Some(ChildRange::Everything)
        );
        precomp(&mut h, l, 0.5, true);
        let r = child_range(&layer(&h, l), Time::from_secs(10), RATE).expect("precomp");
        assert!(
            matches!(r, ChildRange::Bounded { .. }),
            "5 s of a 10 s child: {r:?}"
        );
    }

    #[test]
    fn other_layers_have_no_range() {
        let mut h = History::new(Document::empty("n"));
        let comp = h
            .execute_reported(CreateComposition {
                settings: CompSettings::default(),
                folder: None,
            })
            .expect("test setup")
            .created_comps[0];
        h.execute(voxy_core::command::AddSolidLayer {
            comp,
            name: None,
            color: voxy_core::LinearRgba::WHITE,
            index: 0,
        })
        .expect("test setup");
        let solid = h.current().comp(comp).expect("comp").layers[0]
            .as_ref()
            .clone();
        assert!(child_range(&solid, Time::from_secs(10), RATE).is_none());
        assert!(parent_time_of(&solid, Time::ZERO).is_none());
    }

    #[test]
    fn the_inverse_mapping_round_trips_where_it_exists() {
        let (mut h, _, _, l) = setup();
        timing(&mut h, l, (2, 8), 500);
        for speed in [1.0, 2.0, 0.5] {
            precomp(&mut h, l, speed, false);
            let lay = layer(&h, l);
            for parent_ms in [2000, 3333, 5000, 7999] {
                let parent = Time::from_millis(parent_ms);
                let child = lay
                    .child_time(parent, Time::from_secs(10))
                    .expect("precomp");
                let back = parent_time_of(&lay, child).expect("invertible");
                assert!(
                    (back - parent).flicks().abs() <= 2,
                    "speed {speed}: {parent:?} -> {child:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn remaps_loops_and_zero_speed_cannot_be_inverted() {
        let (mut h, _, _, l) = setup();
        precomp(&mut h, l, 1.0, true);
        assert!(
            parent_time_of(&layer(&h, l), Time::from_secs(1)).is_none(),
            "loop"
        );
        // Commands refuse a speed of 0, but a hand-edited file could have one.
        let mut zero = layer(&h, l);
        if let LayerKind::Precomp { speed, looping, .. } = &mut zero.kind {
            (*speed, *looping) = (0.0, false);
        }
        assert!(
            parent_time_of(&zero, Time::from_secs(1)).is_none(),
            "speed 0"
        );
        h.execute(SetPrecompTiming {
            layer: l,
            speed: 1.0,
            looping: false,
            time_remap: true,
        })
        .expect("test setup");
        assert!(
            parent_time_of(&layer(&h, l), Time::from_secs(1)).is_none(),
            "remap"
        );
    }
}
