//! Decode tests against clips encoded at test time with FFmpeg's built-in
//! (LGPL) encoders, so no binary fixtures are committed.
//!
//! Every frame encodes its own index as 10 black/white blocks, so a test can
//! tell exactly which frame it got back, even through lossy compression.

#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use ffmpeg::util::format::Pixel;
use ffmpeg::{Rational, codec, encoder, format, frame, software::scaling};
use ffmpeg_next as ffmpeg;
use voxy_core::{FrameRate, Time};
use voxy_media::video::{DecodeOptions, HwAccel, VideoDecoder, VideoFrame, probe};

const W: u32 = 160;
const H: u32 = 96;
const BITS: u32 = 10;
const BLOCK: u32 = W / BITS;

struct Clip {
    container: &'static str,
    codec: codec::Id,
    rate: FrameRate,
    frames: u32,
    gop: u32,
    b_frames: usize,
}

/// Encodes a clip whose frame `i` shows `i` in binary.
fn encode(dir: &Path, name: &str, clip: &Clip) -> PathBuf {
    ffmpeg::init().unwrap();
    let path = dir.join(format!("{name}.{}", clip.container));
    let mut octx = format::output(&path).unwrap();
    let codec = encoder::find(clip.codec).expect("built-in encoder");
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);
    let mut ost = octx.add_stream(codec).unwrap();
    let mut enc = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .unwrap();
    let (num, den) = (clip.rate.num() as i32, clip.rate.den() as i32);
    let enc_tb = Rational::new(den, num);
    enc.set_width(W);
    enc.set_height(H);
    enc.set_format(Pixel::YUV420P);
    enc.set_time_base(enc_tb);
    enc.set_frame_rate(Some(Rational::new(num, den)));
    enc.set_gop(clip.gop);
    enc.set_max_b_frames(clip.b_frames);
    enc.set_bit_rate(4_000_000);
    if global_header {
        enc.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let mut enc = enc.open_as(codec).unwrap();
    ost.set_parameters(&enc);
    ost.set_avg_frame_rate(Rational::new(num, den));
    octx.write_header().unwrap();
    let ost_tb = octx.stream(0).unwrap().time_base();

    let mut to_yuv = scaling::Context::get(
        Pixel::RGB24,
        W,
        H,
        Pixel::YUV420P,
        W,
        H,
        scaling::Flags::BILINEAR,
    )
    .unwrap();
    let drain = |enc: &mut encoder::Video, octx: &mut format::context::Output| {
        let mut packet = ffmpeg::Packet::empty();
        while enc.receive_packet(&mut packet).is_ok() {
            packet.set_stream(0);
            packet.rescale_ts(enc_tb, ost_tb);
            packet.write_interleaved(octx).unwrap();
        }
    };
    for i in 0..clip.frames {
        let mut rgb = frame::Video::new(Pixel::RGB24, W, H);
        let stride = rgb.stride(0);
        let data = rgb.data_mut(0);
        for y in 0..H {
            for x in 0..W {
                let o = y as usize * stride + x as usize * 3;
                let v = if y < H / 2 {
                    let bit = (x / BLOCK).min(BITS - 1);
                    if i >> bit & 1 == 1 { 235 } else { 16 }
                } else {
                    128
                };
                data[o..o + 3].copy_from_slice(&[v, v, v]);
            }
        }
        let mut yuv = frame::Video::empty();
        to_yuv.run(&rgb, &mut yuv).unwrap();
        yuv.set_pts(Some(i64::from(i)));
        enc.send_frame(&yuv).unwrap();
        drain(&mut enc, &mut octx);
    }
    enc.send_eof().unwrap();
    drain(&mut enc, &mut octx);
    octx.write_trailer().unwrap();
    path
}

/// Reads the frame index back from the pixels.
fn index_of(f: &VideoFrame) -> u32 {
    let mut n = 0;
    for bit in 0..BITS {
        let x = bit * BLOCK + BLOCK / 2;
        let y = H / 4;
        let r = f.pixels[(y * f.width + x) as usize * 4];
        if r > 128 {
            n |= 1 << bit;
        }
    }
    n
}

fn soft() -> DecodeOptions {
    DecodeOptions {
        hardware: HwAccel::Off,
        threads: None,
    }
}

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
