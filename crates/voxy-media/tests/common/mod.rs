//! Test clips encoded at test time with FFmpeg's built-in (LGPL)
//! encoders, so no binary fixtures are committed.
//!
//! Every frame encodes its own index as 10 black/white blocks, so a test can
//! tell exactly which frame it got back, even through lossy compression.

#![allow(dead_code, clippy::unwrap_used, clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use ffmpeg::util::format::Pixel;
use ffmpeg::{Rational, codec, encoder, format, frame, software::scaling};
use ffmpeg_next as ffmpeg;
use voxy_core::FrameRate;
use voxy_media::VideoFrame;
use voxy_media::video::{DecodeOptions, HwAccel};

pub const W: u32 = 160;
pub const H: u32 = 96;
pub const BITS: u32 = 10;
pub const BLOCK: u32 = W / BITS;

pub struct Clip {
    pub container: &'static str,
    pub codec: codec::Id,
    pub rate: FrameRate,
    pub frames: u32,
    pub gop: u32,
    pub b_frames: usize,
}

/// Encodes a clip whose frame `i` shows `i` in binary.
pub fn encode(dir: &Path, name: &str, clip: &Clip) -> PathBuf {
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
pub fn index_of(f: &VideoFrame) -> u32 {
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

pub fn soft() -> DecodeOptions {
    DecodeOptions {
        hardware: HwAccel::Off,
        threads: None,
    }
}
