//! Audio decode tests against files encoded at test time with FFmpeg's
//! built-in encoders.
//!
//! In the "counter" signal, sample frame `n` stores `n % 16384` on the left
//! and `n / 16384` on the right (as 16-bit integers), so every decoded
//! sample says exactly which frame it is.

#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use ffmpeg::util::format::Sample;
use ffmpeg::util::format::sample::Type as SampleType;
use ffmpeg::{ChannelLayout, Rational, codec, encoder, format, frame};
use ffmpeg_next as ffmpeg;
use voxy_core::Time;
use voxy_media::audio::{AudioClip, AudioDecoder};
use voxy_media::playback::{AudioEngine, frames_to_time};

/// Encodes `frames` stereo frames at `rate`, value of frame `n` from `f`.
fn encode(
    dir: &Path,
    name: &str,
    codec_id: codec::Id,
    rate: u32,
    frames: usize,
    f: impl Fn(usize) -> (i16, i16),
) -> PathBuf {
    ffmpeg::init().unwrap();
    let path = dir.join(name);
    let mut octx = format::output(&path).unwrap();
    let codec = encoder::find(codec_id).expect("built-in encoder");
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);
    let mut ost = octx.add_stream(codec).unwrap();
    let mut enc = codec::context::Context::new_with_codec(codec)
        .encoder()
        .audio()
        .unwrap();
    let layout = ChannelLayout::default(2);
    let fmt = Sample::I16(SampleType::Packed);
    enc.set_rate(rate as i32);
    enc.set_channel_layout(layout);
    enc.set_format(fmt);
    enc.set_time_base(Rational::new(1, rate as i32));
    if global_header {
        enc.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let mut enc = enc.open_as(codec).unwrap();
    ost.set_parameters(&enc);
    octx.write_header().unwrap();
    let ost_tb = octx.stream(0).unwrap().time_base();
    let chunk = if enc.frame_size() > 0 {
        enc.frame_size() as usize
    } else {
        1024
    };
    let drain = |enc: &mut encoder::Audio, octx: &mut format::context::Output| {
        let mut p = ffmpeg::Packet::empty();
        while enc.receive_packet(&mut p).is_ok() {
            p.set_stream(0);
            p.rescale_ts(Rational::new(1, rate as i32), ost_tb);
            p.write_interleaved(octx).unwrap();
        }
    };
    let mut n = 0;
    while n < frames {
        let len = chunk.min(frames - n);
        let mut af = frame::Audio::new(fmt, len, layout);
        af.set_rate(rate);
        {
            let data = af.data_mut(0);
            for i in 0..len {
                let (l, r) = f(n + i);
                data[i * 4..i * 4 + 2].copy_from_slice(&l.to_le_bytes());
                data[i * 4 + 2..i * 4 + 4].copy_from_slice(&r.to_le_bytes());
            }
        }
        af.set_pts(Some(n as i64));
        enc.send_frame(&af).unwrap();
        drain(&mut enc, &mut octx);
        n += len;
    }
    enc.send_eof().unwrap();
    drain(&mut enc, &mut octx);
    octx.write_trailer().unwrap();
    path
}

fn counter(n: usize) -> (i16, i16) {
    ((n % 16384) as i16, (n / 16384) as i16)
}

/// Which frame a decoded stereo sample pair encodes.
fn frame_of(l: f32, r: f32) -> i64 {
    let to_i = |v: f32| (v * 32768.0).round() as i64;
    to_i(r) * 16384 + to_i(l)
}

fn check_counter(buf: &[f32], first: i64) {
    for (i, s) in buf.chunks_exact(2).enumerate() {
        assert_eq!(
            frame_of(s[0], s[1]),
            first + i as i64,
            "sample {i} (expected frame {})",
            first + i as i64
        );
    }
}

const SECONDS: usize = 3;

#[test]
fn sequential_and_seek_are_sample_exact() {
    let dir = tempfile::tempdir().unwrap();
    for (name, id) in [
        ("pcm.wav", codec::Id::PCM_S16LE),
        ("flac.mkv", codec::Id::FLAC),
    ] {
        let path = encode(dir.path(), name, id, 48_000, 48_000 * SECONDS, counter);
        let mut dec = AudioDecoder::open(&path, 48_000).unwrap();
        assert_eq!(dec.info().sample_rate, 48_000);
        assert_eq!(dec.info().channels, 2);
        // Sequential reads in odd sizes.
        let mut pos = 0i64;
        for size in [1usize, 777, 4096, 10_000, 3] {
            let mut buf = vec![0f32; size * 2];
            assert_eq!(dec.read(&mut buf).unwrap(), size);
            check_counter(&buf, pos);
            pos += size as i64;
            assert_eq!(dec.position(), pos);
        }
        // Random seeks, forward and backward, to exact frames.
        for frame in [100_000i64, 5, 143_999, 70_001, 48_000, 0, 96_123, 1] {
            dec.seek(frames_to_time(frame, 48_000)).unwrap();
            assert_eq!(dec.position(), frame, "{name}");
            let mut buf = vec![0f32; 2 * 500];
            let got = dec.read(&mut buf).unwrap();
            check_counter(&buf[..got * 2], frame);
        }
    }
}

#[test]
fn end_of_stream_is_silence() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(
        dir.path(),
        "short.wav",
        codec::Id::PCM_S16LE,
        48_000,
        1000,
        counter,
    );
    let mut dec = AudioDecoder::open(&path, 48_000).unwrap();
    let mut buf = vec![1f32; 2 * 1500];
    assert_eq!(dec.read(&mut buf).unwrap(), 1000);
    check_counter(&buf[..2000], 0);
    assert!(buf[2000..].iter().all(|&s| s == 0.0));
    assert_eq!(dec.read(&mut buf).unwrap(), 0);
}

#[test]
fn resampling_keeps_timing() {
    // 44.1 kHz with a single click at exactly 1.5 s; decoded at 48 kHz.
    let dir = tempfile::tempdir().unwrap();
    let click = 66_150; // 1.5 s at 44.1 kHz
    let path = encode(
        dir.path(),
        "click.wav",
        codec::Id::PCM_S16LE,
        44_100,
        44_100 * SECONDS,
        |n| {
            if n == click { (20_000, 20_000) } else { (0, 0) }
        },
    );
    let mut dec = AudioDecoder::open(&path, 48_000).unwrap();
    let mut all = vec![0f32; 2 * 48_000 * SECONDS];
    let got = dec.read(&mut all).unwrap();
    assert!(
        (got as i64 - 144_000).abs() <= 1,
        "duration preserved: {got}"
    );
    let peak = all
        .chunks_exact(2)
        .enumerate()
        .max_by(|a, b| a.1[0].abs().total_cmp(&b.1[0].abs()))
        .unwrap()
        .0 as i64;
    // 1.5 s at 48 kHz = 72 000 (measured exact; ±1 for resampler variation
    // between FFmpeg versions).
    assert!((peak - 72_000).abs() <= 1, "click at {peak}");
    // After a seek the click is in the same place.
    dec.seek(Time::from_secs(1)).unwrap();
    let mut part = vec![0f32; 2 * 48_000];
    dec.read(&mut part).unwrap();
    let peak2 = part
        .chunks_exact(2)
        .enumerate()
        .max_by(|a, b| a.1[0].abs().total_cmp(&b.1[0].abs()))
        .unwrap()
        .0 as i64;
    assert!(
        (48_000 + peak2 - peak).abs() <= 2,
        "{peak2} after seek vs {peak}"
    );
}

#[test]
fn clip_on_the_timeline_plays_through_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(
        dir.path(),
        "clip.flac",
        codec::Id::FLAC,
        48_000,
        48_000 * SECONDS,
        counter,
    );
    let dec = AudioDecoder::open(&path, 48_000).unwrap();
    // The clip starts 0.5 s into the timeline.
    let (mut engine, mut cb) = AudioEngine::manual(48_000);
    engine.set_source(Box::new(AudioClip::new(dec, Time::from_millis(500))));
    let clock = engine.clock();
    engine.play(Time::from_millis(400)); // 100 ms before the clip
    let mut now = 0u64;
    let mut first = loop {
        let mut out = vec![0f32; 2 * 480];
        cb.process(&mut out, 2, Duration::ZERO, now);
        now += 10_000_000;
        if clock.is_playing() {
            break out;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    // 10 buffers of 480 = 100 ms of silence, then the clip from its start.
    let mut collected = Vec::new();
    collected.append(&mut first);
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(2));
        let mut out = vec![0f32; 2 * 480];
        cb.process(&mut out, 2, Duration::ZERO, now);
        now += 10_000_000;
        collected.extend_from_slice(&out);
    }
    let silence = 4800 * 2;
    assert!(collected[..silence].iter().all(|&s| s == 0.0));
    check_counter(&collected[silence..], 0);
}
