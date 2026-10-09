//! Decode tests against clips encoded at test time (see `common`), so no
//! binary fixtures are committed.

#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::many_single_char_names)]

mod common;

use common::{Clip, H, W, encode, index_of, soft};
use ffmpeg::codec;
use ffmpeg_next as ffmpeg;
use std::sync::atomic::AtomicBool;
use voxy_core::{FrameRate, Time};

use voxy_media::video::{DecodeOptions, SeekPlan, Step, VideoDecoder, probe, scan_keyframes};

const MP4_BFRAMES: Clip = Clip {
    container: "mp4",
    codec: codec::Id::MPEG4,
    rate: FrameRate::FPS_30,
    frames: 90,
    gop: 12,
    b_frames: 2,
};

#[test]
fn probe_reports_stream_facts() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "probe", &MP4_BFRAMES);
    let info = probe(&path).unwrap();
    let v = info.video.unwrap();
    assert_eq!((v.width, v.height), (W, H));
    assert_eq!(v.frame_rate, Some(FrameRate::FPS_30));
    assert_eq!(v.codec, "mpeg4");
    assert_eq!(info.audio_streams, 0);
    let d = info.duration.unwrap();
    assert!((d.as_secs_f64() - 3.0).abs() < 0.05, "{d}");
}

#[test]
fn sequential_decode_yields_every_frame_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "seq", &MP4_BFRAMES);
    let mut dec = VideoDecoder::open(&path, soft()).unwrap();
    let mut n = 0;
    while let Some(f) = dec.next_frame().unwrap() {
        assert_eq!(index_of(&f), n, "frame {n}");
        assert_eq!(
            f.time,
            FrameRate::FPS_30.frame_to_time(i64::from(n)),
            "time of frame {n}"
        );
        assert_eq!(
            (f.width, f.height, f.pixels.len()),
            (W, H, (W * H * 4) as usize)
        );
        n += 1;
    }
    assert_eq!(n, MP4_BFRAMES.frames);
}

#[test]
fn random_access_is_frame_accurate_with_b_frames() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "seek", &MP4_BFRAMES);
    let mut dec = VideoDecoder::open(&path, soft()).unwrap();
    let rate = FrameRate::FPS_30;
    let one = rate.frame_to_time(1);
    // Forward, backward, across GOPs, repeated, start and end.
    for i in [
        0, 1, 2, 11, 12, 13, 89, 45, 44, 46, 5, 5, 60, 23, 24, 0, 88, 47, 36, 35, 70, 3,
    ] {
        let start = rate.frame_to_time(i64::from(i));
        for t in [start, start + Time(one.0 / 2), start + one - Time(1)] {
            let f = dec.frame_at(t).unwrap();
            assert_eq!(index_of(&f), i, "frame_at({t}) wanted frame {i}");
            assert_eq!(f.time, start);
        }
    }
    // Before the start: the first frame. Past the end: the last.
    assert_eq!(index_of(&dec.frame_at(Time(-5)).unwrap()), 0);
    assert_eq!(index_of(&dec.frame_at(Time::from_secs(60)).unwrap()), 89);
}

#[test]
fn rounded_container_timestamps_and_ntsc_rates() {
    // Matroska stores milliseconds; 29.97 fps does not fall on them.
    let dir = tempfile::tempdir().unwrap();
    let clip = Clip {
        container: "mkv",
        codec: codec::Id::FFV1,
        rate: FrameRate::FPS_29_97,
        frames: 60,
        gop: 1,
        b_frames: 0,
    };
    let path = encode(dir.path(), "ntsc", &clip);
    let mut dec = VideoDecoder::open(&path, soft()).unwrap();
    assert_eq!(dec.info().frame_rate, Some(FrameRate::FPS_29_97));
    for i in [0u32, 2, 3, 59, 30, 1, 17, 58, 40] {
        let f = dec.frame_at_index(u64::from(i)).unwrap();
        assert_eq!(index_of(&f), i, "frame_at_index({i})");
        let nominal = FrameRate::FPS_29_97.frame_to_time(i64::from(i));
        assert!(
            (f.time - nominal).0.abs() <= Time::from_millis(1).0,
            "frame {i} time {} vs {}",
            f.time,
            nominal
        );
    }
}

#[test]
fn streams_with_a_nonzero_start_time() {
    // MPEG-TS streams start around 1.4 s; time zero is still the first frame.
    let dir = tempfile::tempdir().unwrap();
    let clip = Clip {
        container: "ts",
        codec: codec::Id::MPEG4,
        rate: FrameRate::FPS_25,
        frames: 50,
        gop: 10,
        b_frames: 1,
    };
    let path = encode(dir.path(), "ts", &clip);
    let mut dec = VideoDecoder::open(&path, soft()).unwrap();
    assert_eq!(index_of(&dec.frame_at(Time::ZERO).unwrap()), 0);
    for i in [25u32, 3, 49, 10, 9] {
        assert_eq!(index_of(&dec.frame_at_index(u64::from(i)).unwrap()), i);
    }
}

#[test]
fn hardware_auto_matches_software_or_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "hw", &MP4_BFRAMES);
    let mut auto = VideoDecoder::open(&path, DecodeOptions::default()).unwrap();
    eprintln!("hardware decoder: {:?}", auto.hardware());
    let mut sw = VideoDecoder::open(&path, soft()).unwrap();
    for i in [0u64, 40, 7, 89] {
        let a = auto.frame_at_index(i).unwrap();
        let b = sw.frame_at_index(i).unwrap();
        assert_eq!(index_of(&a), index_of(&b));
        assert_eq!(a.time, b.time);
    }
}

#[test]
fn bad_inputs_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    assert!(VideoDecoder::open(&dir.path().join("missing.mp4"), soft()).is_err());
    let junk = dir.path().join("junk.mp4");
    std::fs::write(&junk, b"definitely not a video").unwrap();
    assert!(VideoDecoder::open(&junk, soft()).is_err());
    assert!(probe(&junk).is_err());
}

#[test]
fn exhaustive_random_access_across_containers() {
    let dir = tempfile::tempdir().unwrap();
    let clips = [
        (
            "mp4-b",
            Clip {
                container: "mp4",
                codec: codec::Id::MPEG4,
                rate: FrameRate::FPS_30,
                frames: 75,
                gop: 12,
                b_frames: 2,
            },
        ),
        (
            "mkv-b",
            Clip {
                container: "mkv",
                codec: codec::Id::MPEG4,
                rate: FrameRate::FPS_29_97,
                frames: 75,
                gop: 15,
                b_frames: 2,
            },
        ),
        (
            "ts-b",
            Clip {
                container: "ts",
                codec: codec::Id::MPEG4,
                rate: FrameRate::FPS_25,
                frames: 75,
                gop: 10,
                b_frames: 1,
            },
        ),
        (
            "mov-intra",
            Clip {
                container: "mov",
                codec: codec::Id::FFV1,
                rate: FrameRate::FPS_24,
                frames: 75,
                gop: 1,
                b_frames: 0,
            },
        ),
    ];
    for (name, clip) in &clips {
        let path = encode(dir.path(), name, clip);
        let mut dec = VideoDecoder::open(&path, soft()).unwrap();
        // Deterministic pseudo-random order (LCG), each followed by its
        // neighbour (the common "next frame" case).
        let mut x: u64 = 0x2545_F491;
        for step in 0..400u64 {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let i = (x >> 33) % u64::from(clip.frames);
            let f = dec.frame_at_index(i).unwrap();
            assert_eq!(u64::from(index_of(&f)), i, "{name}: step {step}, frame {i}");
            if i + 1 < u64::from(clip.frames) {
                let g = dec.frame_at_index(i + 1).unwrap();
                assert_eq!(u64::from(index_of(&g)), i + 1, "{name}: neighbour of {i}");
            }
        }
    }
}

/// The middle of frame `i`'s interval.
fn mid(rate: FrameRate, i: i64) -> Time {
    let start = rate.frame_to_time(i);
    start + Time((rate.frame_to_time(i + 1) - start).0 / 2)
}

#[test]
fn gop_index_decides_between_decoding_forward_and_seeking() {
    let dir = tempfile::tempdir().unwrap();
    let rate = FrameRate::FPS_30;
    let clip = Clip {
        container: "mp4",
        codec: codec::Id::MPEG4,
        rate,
        frames: 120,
        gop: 12,
        b_frames: 0,
    };
    let path = encode(dir.path(), "gop", &clip);
    let mut dec = VideoDecoder::open(&path, soft()).unwrap();
    let gop = dec.gop_index();
    scan_keyframes(&path, &gop, &AtomicBool::new(false)).unwrap();
    let tb = dec.info().time_base;
    {
        let g = gop.lock().unwrap();
        assert!(g.is_complete());
        let frames: Vec<i64> = g
            .keyframes()
            .map(|k| rate.time_to_frame(tb.to_time(k) + Time::from_millis(1)))
            .collect();
        assert_eq!(frames, (0..120).step_by(12).collect::<Vec<_>>());
    }

    assert_eq!(index_of(&dec.frame_at(mid(rate, 2)).unwrap()), 2);
    assert_eq!(dec.plan(mid(rate, 2)), SeekPlan::Ready);
    assert_eq!(dec.plan(mid(rate, 11)), SeekPlan::Forward, "same GOP");
    assert_eq!(
        dec.plan(mid(rate, 40)),
        SeekPlan::Seek,
        "keyframe 36 is close"
    );
    assert_eq!(dec.plan(mid(rate, 1)), SeekPlan::Seek, "backward");

    // Seeking to frame 40 decodes from keyframe 36, not from frame 3.
    let before = (dec.frames_decoded(), dec.seeks());
    assert_eq!(index_of(&dec.frame_at(mid(rate, 40)).unwrap()), 40);
    let decoded = dec.frames_decoded() - before.0;
    assert_eq!(dec.seeks() - before.1, 1);
    assert!(decoded <= 8, "decoded {decoded} frames to reach frame 40");
    // ...and within its GOP it decodes forward.
    assert_eq!(dec.plan(mid(rate, 47)), SeekPlan::Forward);
    assert_eq!(index_of(&dec.frame_at(mid(rate, 47)).unwrap()), 47);
    assert_eq!(dec.seeks() - before.1, 1);

    // Without a scan, the decoder learns the GOPs it reads.
    let mut learner = VideoDecoder::open(&path, soft()).unwrap();
    assert_eq!(index_of(&learner.frame_at(mid(rate, 30)).unwrap()), 30);
    let g = learner.gop_index();
    let learned: Vec<i64> = g
        .lock()
        .unwrap()
        .keyframes()
        .map(|k| rate.time_to_frame(tb.to_time(k) + Time::from_millis(1)))
        .collect();
    assert_eq!(learned, vec![0, 12, 24]);
    assert_eq!(learner.plan(mid(rate, 33)), SeekPlan::Forward);
}

#[test]
fn stepping_reaches_the_same_frames_as_frame_at() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "steps", &MP4_BFRAMES);
    let rate = MP4_BFRAMES.rate;
    let mut dec = VideoDecoder::open(&path, soft()).unwrap();
    for target in [5, 70, 30, 31, 89, 0, 60, 59, 89] {
        let t = mid(rate, target);
        let mut steps = 0;
        while dec.step_toward(t).unwrap() == Step::Continue {
            steps += 1;
            assert!(steps < 1000, "no progress toward frame {target}");
        }
        let f = dec.current_frame().unwrap();
        assert_eq!(index_of(&f), target as u32, "target {target}");
        assert_eq!(dec.current_time(), Some(f.time));
        let end = dec.current_end().unwrap();
        if target == 89 {
            assert_eq!(end, Time::MAX, "the last frame answers every later time");
        } else {
            assert_eq!(end, f.time + f.duration);
        }
    }
}
