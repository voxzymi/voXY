//! Decoder-thread tests: scrubbing, keyframe-first previews, backward
//! stepping, playback prefetch and the shared frame cache. Clips are
//! encoded at test time (see `common`).

#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{Clip, encode, index_of, soft};
use ffmpeg::codec;
use ffmpeg_next as ffmpeg;
use voxy_core::{FrameRate, Time};
use voxy_media::cache::FrameCache;
use voxy_media::stream::{Demand, StreamOptions, VideoStream};

const WAIT: Duration = Duration::from_secs(30);
const RATE: FrameRate = FrameRate::FPS_30;

/// 4 s at 30 fps, a keyframe every 12 frames, B-frames.
const CLIP: Clip = Clip {
    container: "mp4",
    codec: codec::Id::MPEG4,
    rate: RATE,
    frames: 120,
    gop: 12,
    b_frames: 2,
};

fn mid(i: i64) -> Time {
    let start = RATE.frame_to_time(i);
    start + Time((RATE.frame_to_time(i + 1) - start).0 / 2)
}

fn options() -> StreamOptions {
    StreamOptions {
        decode: soft(),
        ..StreamOptions::default()
    }
}

fn big_cache() -> Arc<FrameCache> {
    Arc::new(FrameCache::new(256 << 20))
}

#[test]
fn scrubbing_anywhere_is_frame_accurate() {
    let dir = tempfile::tempdir().unwrap();
    for (name, container) in [("s_mp4", "mp4"), ("s_mkv", "mkv"), ("s_ts", "ts")] {
        let clip = Clip { container, ..CLIP };
        let path = encode(dir.path(), name, &clip);
        let stream = VideoStream::open(&path, big_cache(), options(), None).unwrap();
        for i in [0, 57, 13, 119, 118, 60, 3, 95, 95, 40] {
            let f = stream.frame_blocking(mid(i), WAIT).unwrap();
            assert_eq!(index_of(&f), i as u32, "{container}: frame {i}");
        }
        // Past the end shows the last frame; before the start, the first.
        let f = stream.frame_blocking(Time::from_secs(60), WAIT).unwrap();
        assert_eq!(index_of(&f), 119);
        let f = stream.frame_blocking(Time(-5), WAIT).unwrap();
        assert_eq!(index_of(&f), 0);
    }
}

#[test]
fn a_distant_scrub_shows_the_keyframe_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "preview", &CLIP);
    let cache = big_cache();
    let stream = VideoStream::open(&path, cache.clone(), options(), None).unwrap();
    let f = stream.frame_blocking(mid(80), WAIT).unwrap();
    assert_eq!(index_of(&f), 80);
    let stats = stream.stats();
    assert!(stats.seeks >= 1 && stats.previews >= 1, "{stats:?}");
    // Keyframe 72 was cached on the way, as the preview.
    let preview = stream.frame(mid(72)).expect("keyframe preview cached");
    assert_eq!(index_of(&preview), 72);
    // The preview for an undecoded time is the nearest cached frame.
    let (near, exact) = stream.preview(mid(76)).unwrap();
    assert!(!exact);
    assert!([72, 80].contains(&index_of(&near)));
}

#[test]
fn stepping_backward_reuses_the_decoded_gop() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "back", &CLIP);
    let stream = VideoStream::open(&path, big_cache(), options(), None).unwrap();
    assert_eq!(index_of(&stream.frame_blocking(mid(83), WAIT).unwrap()), 83);
    let seeks = stream.stats().seeks;
    for i in (60..83).rev() {
        let f = stream.frame_blocking(mid(i), WAIT).unwrap();
        assert_eq!(index_of(&f), i as u32);
    }
    // 23 steps back across two GOPs: one seek per GOP, not one per step.
    let extra = stream.stats().seeks - seeks;
    assert!(extra <= 3, "{extra} seeks for 23 backward steps");
}

#[test]
fn playback_prefetches_ahead_of_the_playhead() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "play", &CLIP);
    let notified = Arc::new(AtomicUsize::new(0));
    let n = notified.clone();
    let stream = VideoStream::open(
        &path,
        big_cache(),
        options(),
        Some(Arc::new(move || {
            n.fetch_add(1, Ordering::Relaxed);
        })),
    )
    .unwrap();
    stream.demand(Demand::Play(RATE.frame_to_time(10)));
    assert!(stream.wait_idle(WAIT));
    // One second ahead (frames 10..40) is decoded without being asked for
    // frame by frame.
    for i in 10..40 {
        let f = stream
            .frame(mid(i))
            .unwrap_or_else(|| panic!("frame {i} not prefetched"));
        assert_eq!(index_of(&f), i as u32);
    }
    assert!(stream.frame(mid(45)).is_none(), "prefetch is bounded");
    // Moving the playhead tops the window up, decoding sequentially.
    let seeks = stream.stats().seeks;
    for i in 11..=50 {
        stream.demand(Demand::Play(RATE.frame_to_time(i)));
    }
    assert!(stream.wait_idle(WAIT));
    for i in 50..80 {
        assert_eq!(index_of(&stream.frame(mid(i)).unwrap()), i as u32);
    }
    assert_eq!(stream.stats().seeks, seeks, "playback never seeks");
    assert!(notified.load(Ordering::Relaxed) > 0);
}

#[test]
fn stale_scrub_targets_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "stale", &CLIP);
    let stream = VideoStream::open(&path, big_cache(), options(), None).unwrap();
    // A fast drag: every demand replaces the last; only the final one must
    // be reached.
    for i in (0..120).step_by(7) {
        stream.demand(Demand::Scrub(mid(i)));
    }
    let f = stream.frame_blocking(mid(101), WAIT).unwrap();
    assert_eq!(index_of(&f), 101);
    assert!(stream.wait_idle(WAIT));
}

#[test]
fn frames_bigger_than_the_cache_are_still_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "tiny", &CLIP);
    let cache = Arc::new(FrameCache::new(1024));
    let stream = VideoStream::open(&path, cache.clone(), options(), None).unwrap();
    let f = stream.frame_blocking(mid(30), WAIT).unwrap();
    assert_eq!(index_of(&f), 30);
    assert!(stream.wait_idle(WAIT), "an uncacheable frame must not spin");
    assert_eq!(cache.stats().frames, 0);
}

#[test]
fn closing_a_stream_frees_its_frames() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(dir.path(), "close", &CLIP);
    let cache = big_cache();
    let a = VideoStream::open(&path, cache.clone(), options(), None).unwrap();
    let b = VideoStream::open(&path, cache.clone(), options(), None).unwrap();
    assert_ne!(a.id(), b.id());
    a.frame_blocking(mid(5), WAIT).unwrap();
    b.frame_blocking(mid(5), WAIT).unwrap();
    assert!(a.wait_idle(WAIT) && b.wait_idle(WAIT));
    let both = cache.stats().frames;
    drop(a);
    let left = cache.stats().frames;
    assert!(left > 0 && left < both, "{left} of {both}");
    assert_eq!(index_of(&b.frame(mid(5)).unwrap()), 5);
}

#[test]
fn opening_a_missing_file_fails_up_front() {
    let err = VideoStream::open(
        std::path::Path::new("/nonexistent/clip.mp4"),
        big_cache(),
        options(),
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("clip.mp4"), "{err}");
}
