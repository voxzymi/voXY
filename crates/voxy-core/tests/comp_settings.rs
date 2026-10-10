//! Milestone C: composition settings model (docs/design-comp-settings.md).

use voxy_core::command::{
    CreateComposition, ResizeMode, SetCompositionSettings, validate_comp_settings,
};
use voxy_core::{
    CompSettings, Document, FrameRate, History, PreviewResolution, Time, evaluate, persist,
};

fn rate(x: f64) -> FrameRate {
    FrameRate::from_decimal(x).expect("test input")
}

#[test]
fn typed_decimals_become_exact_rates() {
    assert_eq!(rate(23.976), FrameRate::FPS_23_976);
    assert_eq!(rate(23.976_023_976), FrameRate::FPS_23_976);
    assert_eq!(rate(29.97), FrameRate::FPS_29_97);
    assert_eq!(rate(59.94), FrameRate::FPS_59_94);
    assert_eq!(rate(24.0), FrameRate::FPS_24);
    assert_eq!(rate(30.00001), FrameRate::FPS_30);
    assert_eq!(rate(60.0), FrameRate::FPS_60);
    // Not NTSC: 24.0 must not turn into 24000/1001.
    assert_ne!(rate(24.0), FrameRate::FPS_23_976);
}

#[test]
fn arbitrary_decimals_use_the_nearest_small_fraction() {
    let r = rate(12.5);
    assert_eq!((r.num(), r.den()), (25, 2));
    let r = rate(7.123);
    assert_eq!((r.num(), r.den()), (7123, 1000));
    let r = rate(29.9);
    assert_eq!((r.num(), r.den()), (299, 10));
    let r = rate(0.5);
    assert_eq!((r.num(), r.den()), (1, 2));
    // Always close, always a small denominator.
    for x in [1.37, 18.2, 33.333, 47.95, 99.1, 144.0001, 239.999] {
        let r = rate(x);
        assert!((r.as_f64() - x).abs() < 0.002, "{x} -> {r:?}");
        assert!(r.den() <= 1001);
    }
}

#[test]
fn bad_decimals_are_rejected() {
    for x in [0.0, -5.0, f64::NAN, f64::INFINITY, 1e9] {
        assert!(FrameRate::from_decimal(x).is_none(), "{x}");
    }
}

#[test]
fn drop_frame_rules_and_timecode_base() {
    assert!(FrameRate::FPS_29_97.supports_drop_frame());
    assert!(FrameRate::FPS_59_94.supports_drop_frame());
    for r in [
        FrameRate::FPS_23_976,
        FrameRate::FPS_24,
        FrameRate::FPS_25,
        FrameRate::FPS_30,
        FrameRate::FPS_50,
        FrameRate::FPS_60,
    ] {
        assert!(!r.supports_drop_frame(), "{r}");
    }
    let bases: Vec<u32> = [
        FrameRate::FPS_23_976,
        FrameRate::FPS_24,
        FrameRate::FPS_25,
        FrameRate::FPS_29_97,
        FrameRate::FPS_30,
        FrameRate::FPS_50,
        FrameRate::FPS_59_94,
        FrameRate::FPS_60,
    ]
    .iter()
    .map(|r| r.timecode_base())
    .collect();
    assert_eq!(bases, vec![24, 24, 25, 30, 30, 50, 60, 60]);
}

#[test]
fn durations_in_flicks_for_common_rates() {
    // 10 seconds of frames at each rate converts back to whole frames, and
    // exact-integer rates land on exact seconds.
    for (r, frames) in [
        (FrameRate::FPS_24, 240),
        (FrameRate::FPS_25, 250),
        (FrameRate::FPS_30, 300),
        (FrameRate::FPS_60, 600),
    ] {
        assert_eq!(r.frame_to_time(frames), Time::from_secs(10), "{r}");
        assert_eq!(r.time_to_frame(Time::from_secs(10)), frames);
    }
    for r in [
        FrameRate::FPS_23_976,
        FrameRate::FPS_29_97,
        FrameRate::FPS_59_94,
    ] {
        let t = r.frame_to_time(1000);
        assert_eq!(r.time_to_frame(t), 1000, "{r} round trip");
        assert!((t.as_secs_f64() - 1000.0 / r.as_f64()).abs() < 1e-6, "{r}");
    }
}

#[test]
fn validation_of_the_new_fields() {
    let ok = CompSettings::default();
    assert!(validate_comp_settings(&ok).is_ok());
    let drop_on_30 = CompSettings {
        drop_frame: true,
        ..ok.clone()
    };
    assert!(validate_comp_settings(&drop_on_30).is_err());
    let drop_on_ntsc = CompSettings {
        drop_frame: true,
        frame_rate: FrameRate::FPS_29_97,
        ..ok.clone()
    };
    assert!(validate_comp_settings(&drop_on_ntsc).is_ok());
    for n in [0, 1, 65, 1000] {
        let s = CompSettings {
            preview_resolution: PreviewResolution::Custom(n),
            ..ok.clone()
        };
        assert!(validate_comp_settings(&s).is_err(), "custom {n}");
    }
    let s = CompSettings {
        preview_resolution: PreviewResolution::Custom(8),
        ..ok.clone()
    };
    assert!(validate_comp_settings(&s).is_ok());
    let far = CompSettings {
        start_offset: Time::from_secs(48 * 3600),
        ..ok.clone()
    };
    assert!(validate_comp_settings(&far).is_err());
    let before_zero = CompSettings {
        start_offset: Time::from_secs(-3600),
        ..ok
    };
    assert!(validate_comp_settings(&before_zero).is_ok());
}

#[test]
fn preview_resolution_math() {
    assert_eq!(PreviewResolution::Full.size(1080, 1080), (1080, 1080));
    assert_eq!(PreviewResolution::Half.size(1080, 1080), (540, 540));
    assert_eq!(PreviewResolution::Third.size(1920, 1080), (640, 360));
    assert_eq!(PreviewResolution::Custom(10).size(1920, 1080), (192, 108));
    assert_eq!(PreviewResolution::Quarter.size(16, 16), (4, 4));
    assert_eq!(
        PreviewResolution::Custom(64).size(16, 16),
        (1, 1),
        "never zero"
    );
    assert_eq!(PreviewResolution::from_divisor(3), PreviewResolution::Third);
    assert_eq!(
        PreviewResolution::from_divisor(7),
        PreviewResolution::Custom(7)
    );
    assert_eq!(PreviewResolution::Quarter.scale(), 0.25);
}

fn history() -> History {
    History::new(Document::with_comp("T", CompSettings::default()))
}

#[test]
fn settings_command_sets_the_new_fields_in_one_undo_step() {
    let mut h = history();
    let id = h.current().main_comp.unwrap();
    let before = h.current().clone();
    let s = CompSettings {
        frame_rate: FrameRate::FPS_29_97,
        drop_frame: true,
        start_offset: FrameRate::FPS_29_97.frame_to_time(108_000),
        preview_resolution: PreviewResolution::Half,
        ..CompSettings::of(h.current().comp(id).unwrap())
    };
    h.execute(SetCompositionSettings {
        comp: id,
        settings: s.clone(),
        resize: ResizeMode::default(),
        snap_to_frames: false,
    })
    .unwrap();
    let c = h.current().comp(id).unwrap();
    assert_eq!(
        (c.drop_frame, c.preview_resolution, c.start_offset),
        (true, PreviewResolution::Half, s.start_offset)
    );
    h.undo();
    assert_eq!(h.current(), &before, "one undo restores everything");
}

#[test]
fn a_rejected_settings_edit_changes_nothing() {
    let mut h = history();
    let id = h.current().main_comp.unwrap();
    let before = h.current().clone();
    let bad = CompSettings {
        drop_frame: true, // 30 fps cannot drop frames
        ..CompSettings::of(h.current().comp(id).unwrap())
    };
    assert!(
        h.execute(SetCompositionSettings {
            comp: id,
            settings: bad,
            resize: ResizeMode::default(),
            snap_to_frames: false,
        })
        .is_err()
    );
    assert_eq!(h.current(), &before);
    assert!(!h.can_undo() || h.undo_label() != Some("Composition Settings"));
}

#[test]
fn create_composition_keeps_the_dialog_values_and_undoes_in_one_step() {
    let mut h = History::new(Document::empty("T"));
    let start = h.current().clone();
    let s = CompSettings {
        name: "Vertical".into(),
        width: 1080,
        height: 1920,
        frame_rate: FrameRate::FPS_23_976,
        start_offset: Time::from_secs(3600),
        preview_resolution: PreviewResolution::Quarter,
        ..CompSettings::default()
    };
    let r = h
        .execute_reported(CreateComposition {
            settings: s.clone(),
            folder: None,
        })
        .unwrap();
    let c = h.current().comp(r.created_comps[0]).unwrap();
    assert_eq!(CompSettings::of(c), s);
    h.undo();
    assert_eq!(h.current(), &start);
}

#[test]
fn preview_resolution_never_changes_what_is_rendered() {
    // Evaluation (what export renders) ignores the preview setting, so an
    // export render can only be keyed on the composition's real content.
    let mut h = history();
    let id = h.current().main_comp.unwrap();
    let full = evaluate(h.current(), id, Time::ZERO).unwrap();
    let s = CompSettings {
        preview_resolution: PreviewResolution::Quarter,
        ..CompSettings::of(h.current().comp(id).unwrap())
    };
    h.execute(SetCompositionSettings {
        comp: id,
        settings: s,
        resize: ResizeMode::default(),
        snap_to_frames: false,
    })
    .unwrap();
    let quarter = evaluate(h.current(), id, Time::ZERO).unwrap();
    assert_eq!(full, quarter);
    // The size is the composition's own.
    assert_eq!((quarter.width, quarter.height), (1920, 1080));
}

#[test]
fn files_before_0_5_open_with_defaults_and_new_fields_round_trip() {
    let doc = Document::with_comp("T", CompSettings::default());
    let json = persist::to_json(&doc).unwrap();
    assert!(json.contains("\"format_version\": \"0.5.0\""));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    for comp in v["project"]["comps"].as_array().unwrap() {
        for field in ["start_offset", "drop_frame", "preview_resolution"] {
            assert!(comp.get(field).is_none(), "default not written: {field}");
        }
    }
    let old = json.replace(
        "\"format_version\": \"0.5.0\"",
        "\"format_version\": \"0.4.0\"",
    );
    let back = persist::from_json(&old).unwrap();
    assert_eq!(back, doc);

    let mut h = History::new(doc);
    let id = h.current().main_comp.unwrap();
    h.execute(SetCompositionSettings {
        comp: id,
        settings: CompSettings {
            frame_rate: FrameRate::FPS_59_94,
            drop_frame: true,
            start_offset: Time::from_secs(10),
            preview_resolution: PreviewResolution::Custom(5),
            ..CompSettings::of(h.current().comp(id).unwrap())
        },
        resize: ResizeMode::default(),
        snap_to_frames: false,
    })
    .unwrap();
    let json = persist::to_json(h.current()).unwrap();
    assert_eq!(&persist::from_json(&json).unwrap(), &**h.current());
    // A newer file is still refused.
    let newer = json.replace(
        "\"format_version\": \"0.5.0\"",
        &format!(
            "\"format_version\": \"0.{}.0\"",
            persist::FORMAT_VERSION.minor + 1
        ),
    );
    assert!(persist::from_json(&newer).is_err());
}
