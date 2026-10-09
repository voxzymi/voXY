//! Audio decoding with FFmpeg: the best audio stream of a file, resampled to
//! the engine's rate as interleaved stereo `f32`, with sample-accurate
//! seeking.
//!
//! Positions are in *output* frames (samples per channel at the output
//! rate) from the stream's first sample. After a seek, which lands on a
//! packet boundary, the decoder trims (or, before the stream starts, pads
//! with silence) to exactly the requested frame. Resampling adds the
//! resampler's small, constant filter delay (well under a millisecond);
//! at matching rates output is sample-exact.

// FFI only for stream-precise seeking (as in `video`), with its invariant.
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};

use ffmpeg::util::format::Sample;
use ffmpeg::util::format::sample::Type as SampleType;
use ffmpeg::{ChannelLayout, codec, ffi, format, frame, media, software::resampling};
use ffmpeg_next as ffmpeg;
use voxy_core::Time;

use crate::MediaError;
use crate::playback::{AudioSource, CHANNELS, frames_to_time, time_to_frames};
use crate::timebase::TimeBase;

fn err(path: &Path, message: impl std::fmt::Display) -> MediaError {
    MediaError::Media {
        path: path.display().to_string(),
        message: message.to_string(),
    }
}

/// Facts about an audio stream.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioInfo {
    pub stream_index: usize,
    pub sample_rate: u32,
    pub channels: u16,
    pub codec: String,
    pub duration: Option<Time>,
    pub time_base: TimeBase,
}

/// Decodes one audio stream to interleaved stereo `f32` at a fixed rate.
pub struct AudioDecoder {
    path: PathBuf,
    info: AudioInfo,
    out_rate: u32,
    input: format::context::Input,
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<((Sample, i32, u32), resampling::Context)>,
    start_pts: i64,
    sent_eof: bool,
    drained: bool,
    /// Decoded, resampled samples not yet read (interleaved stereo).
    pending: Vec<f32>,
    /// Output frame index of `pending[0]`; `None` until the first frame
    /// after opening or seeking fixes it from that frame's timestamp.
    pending_start: Option<i64>,
}

impl std::fmt::Debug for AudioDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioDecoder")
            .field("path", &self.path)
            .field("info", &self.info)
            .field("out_rate", &self.out_rate)
            .finish_non_exhaustive()
    }
}

impl AudioDecoder {
    /// Opens the best audio stream, to be delivered at `out_rate`.
    pub fn open(path: &Path, out_rate: u32) -> Result<Self, MediaError> {
        let _ = ffmpeg::init();
        let input = format::input(path).map_err(|e| err(path, e))?;
        let stream = input
            .streams()
            .best(media::Type::Audio)
            .ok_or_else(|| err(path, "no audio stream"))?;
        let tb = TimeBase::new(
            i64::from(stream.time_base().numerator()),
            i64::from(stream.time_base().denominator()),
        )
        .ok_or_else(|| err(path, "stream has no time base"))?;
        let ctx = codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| err(path, e))?;
        let mut decoder = ctx
            .decoder()
            .audio()
            .map_err(|e| err(path, format!("cannot decode audio: {e}")))?;
        decoder.set_packet_time_base(stream.time_base());
        let start = stream.start_time();
        let info = AudioInfo {
            stream_index: stream.index(),
            sample_rate: decoder.rate(),
            channels: decoder.channels(),
            codec: stream.parameters().id().name().to_owned(),
            duration: (stream.duration() > 0).then(|| tb.to_time(stream.duration())),
            time_base: tb,
        };
        Ok(AudioDecoder {
            path: path.to_path_buf(),
            info,
            out_rate,
            input,
            decoder,
            resampler: None,
            start_pts: if start == ffi::AV_NOPTS_VALUE {
                0
            } else {
                start
            },
            sent_eof: false,
            drained: false,
            pending: Vec::new(),
            pending_start: Some(0),
        })
    }

    pub fn info(&self) -> &AudioInfo {
        &self.info
    }

    pub fn output_rate(&self) -> u32 {
        self.out_rate
    }

    /// Output frame index of the next frame [`AudioDecoder::read`] returns.
    pub fn position(&self) -> i64 {
        self.pending_start.unwrap_or(0)
    }

    /// Fills `out` (interleaved stereo) from the current position; past the
    /// end, with silence. Returns the number of frames that were real audio.
    pub fn read(&mut self, out: &mut [f32]) -> Result<usize, MediaError> {
        let frames = out.len() / CHANNELS;
        while self.pending.len() < out.len() && !self.drained {
            self.decode_more()?;
        }
        let n = self.pending.len().min(frames * CHANNELS);
        out[..n].copy_from_slice(&self.pending[..n]);
        out[n..].fill(0.0);
        self.pending.drain(..n);
        // Position advances by the whole request, audio or silence.
        self.pending_start = Some(self.position() + frames as i64);
        if n < frames * CHANNELS {
            self.pending.clear();
        }
        Ok(n / CHANNELS)
    }

    /// Positions the decoder so the next read starts exactly at `time`.
    pub fn seek(&mut self, time: Time) -> Result<(), MediaError> {
        let target = time_to_frames(time, self.out_rate);
        let back = Time::from_secs(1);
        let mut seek_time = time;
        for _ in 0..3 {
            self.seek_raw(seek_time)?;
            self.fill_to(target)?;
            if self.pending_start.is_some_and(|s| s <= target) || seek_time <= Time::ZERO {
                break;
            }
            // Landed after the target: back off.
            seek_time = (seek_time - back).max(Time::ZERO);
        }
        self.trim_to(target);
        Ok(())
    }

    fn seek_raw(&mut self, time: Time) -> Result<(), MediaError> {
        let pts = self
            .start_pts
            .saturating_add(self.info.time_base.to_pts_floor(time.max(Time::ZERO)));
        let index = i32::try_from(self.info.stream_index).unwrap_or(0);
        // SAFETY: `input` is an open demuxer and `index` one of its streams;
        // max_ts = pts asks for the closest seek point at or before `pts`.
        let ret = unsafe {
            ffi::avformat_seek_file(self.input.as_mut_ptr(), index, i64::MIN, pts, pts, 0)
        };
        if ret < 0 || time <= Time::ZERO {
            self.input = format::input(&self.path).map_err(|e| err(&self.path, e))?;
        }
        self.decoder.flush();
        self.resampler = None;
        self.sent_eof = false;
        self.drained = false;
        self.pending.clear();
        self.pending_start = if time <= Time::ZERO { Some(0) } else { None };
        Ok(())
    }

    /// Decodes until the pending audio reaches `target` (or the end).
    fn fill_to(&mut self, target: i64) -> Result<(), MediaError> {
        loop {
            if let Some(s) = self.pending_start
                && s + (self.pending.len() / CHANNELS) as i64 > target
            {
                return Ok(());
            }
            if self.drained {
                return Ok(());
            }
            self.decode_more()?;
        }
    }

    /// Makes `pending_start == target` by dropping earlier samples or
    /// padding with silence.
    fn trim_to(&mut self, target: i64) {
        let start = self.pending_start.unwrap_or(target);
        if start < target {
            let drop = (((target - start) as usize) * CHANNELS).min(self.pending.len());
            self.pending.drain(..drop);
        } else if start > target {
            let pad = (start - target) as usize * CHANNELS;
            self.pending.splice(0..0, std::iter::repeat_n(0.0, pad));
        }
        self.pending_start = Some(target);
    }

    /// Decodes one more frame (or drains the resampler at the end).
    fn decode_more(&mut self) -> Result<(), MediaError> {
        loop {
            let mut f = frame::Audio::empty();
            match self.decoder.receive_frame(&mut f) {
                Ok(()) => return self.push(&mut f),
                Err(ffmpeg::Error::Eof) => {
                    self.flush_resampler();
                    self.drained = true;
                    return Ok(());
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {}
                Err(e) => return Err(err(&self.path, format!("decoding failed: {e}"))),
            }
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) => {
                    if packet.stream() == self.info.stream_index
                        && let Err(e) = self.decoder.send_packet(&packet)
                    {
                        tracing::debug!("{}: skipping bad audio packet: {e}", self.path.display());
                    }
                }
                Err(ffmpeg::Error::Eof) => {
                    if !self.sent_eof {
                        let _ = self.decoder.send_eof();
                        self.sent_eof = true;
                    }
                }
                Err(e) => return Err(err(&self.path, format!("reading failed: {e}"))),
            }
        }
    }

    fn push(&mut self, f: &mut frame::Audio) -> Result<(), MediaError> {
        // A stream without a channel mask (a plain WAV, for example) decodes
        // with an unspecified layout: no mask bits before FFmpeg 7, an
        // "unspecified order" layout from 7 on. The resampler would treat
        // that as a layout change on every frame ("Input changed"), so
        // give it the default layout for its channel count.
        if f.channel_layout().is_empty() {
            f.set_channel_layout(ChannelLayout::default(i32::from(f.channels())));
        }
        if self.pending_start.is_none() {
            let pts = f.timestamp().or_else(|| f.pts()).unwrap_or(self.start_pts);
            let t = self.info.time_base.to_time(pts - self.start_pts);
            self.pending_start =
                Some(time_to_frames(t, self.out_rate) + i64::from(rounding_bias(t, self.out_rate)));
        }
        let key = (f.format(), f.channel_layout().channels(), f.rate());
        if self.resampler.as_ref().is_none_or(|(k, _)| *k != key) {
            let ctx = resampling::Context::get(
                f.format(),
                f.channel_layout(),
                f.rate(),
                Sample::F32(SampleType::Packed),
                ChannelLayout::default(CHANNELS as i32),
                self.out_rate,
            )
            .map_err(|e| err(&self.path, format!("cannot resample: {e}")))?;
            self.resampler = Some((key, ctx));
        }
        let resampler = &mut self.resampler.as_mut().expect("created above").1;
        // ffmpeg-next sizes an empty output frame to the *input* sample
        // count, which truncates upsampled output; allocate for the rate
        // ratio plus whatever the resampler still holds.
        let held = resampler.delay().map_or(0, |d| d.output.max(0) as usize);
        let capacity = (f.samples() as u64 * u64::from(self.out_rate) / u64::from(f.rate().max(1)))
            as usize
            + held
            + 256;
        let mut out = output_frame(capacity);
        resampler.run(f, &mut out).map_err(|e| err(&self.path, e))?;
        append_packed(&mut self.pending, &out);
        Ok(())
    }

    fn flush_resampler(&mut self) {
        if let Some((_, r)) = &mut self.resampler {
            // Drain the filter tail.
            for _ in 0..64 {
                let mut out = output_frame(4096);
                if r.flush(&mut out).is_err() || out.samples() == 0 {
                    break;
                }
                append_packed(&mut self.pending, &out);
            }
        }
    }
}

/// Rounds a frame's start to the nearest output frame instead of down.
fn rounding_bias(t: Time, rate: u32) -> i32 {
    let floor = time_to_frames(t, rate);
    let rem = t.0 - frames_to_time(floor, rate).0;
    let frame = frames_to_time(1, rate).0;
    i32::from(2 * rem >= frame)
}

/// An allocated interleaved-stereo `f32` frame with room for `samples`.
fn output_frame(samples: usize) -> frame::Audio {
    frame::Audio::new(
        Sample::F32(SampleType::Packed),
        samples,
        ChannelLayout::default(CHANNELS as i32),
    )
}

fn append_packed(pending: &mut Vec<f32>, out: &frame::Audio) {
    let n = out.samples() * CHANNELS;
    if n == 0 {
        return;
    }
    let bytes = &out.data(0)[..n * 4];
    pending.extend(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_ne_bytes(*b)),
    );
}

/// A decoded file placed on the timeline: its first sample plays at
/// timeline frame `offset`.
pub struct AudioClip {
    decoder: AudioDecoder,
    offset: i64,
    gain: f32,
}

impl std::fmt::Debug for AudioClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioClip")
            .field("offset", &self.offset)
            .field("gain", &self.gain)
            .finish_non_exhaustive()
    }
}

impl AudioClip {
    pub fn new(decoder: AudioDecoder, offset: Time) -> Self {
        let offset = time_to_frames(offset, decoder.output_rate());
        AudioClip {
            decoder,
            offset,
            gain: 1.0,
        }
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
    }
}

impl AudioSource for AudioClip {
    fn render(&mut self, start: i64, out: &mut [f32]) {
        let frames = out.len() / CHANNELS;
        let local = start - self.offset;
        // Silence before the clip starts.
        let lead = (-local).clamp(0, frames as i64) as usize;
        out[..lead * CHANNELS].fill(0.0);
        if lead == frames {
            return;
        }
        let from = local.max(0);
        if self.decoder.position() != from
            && let Err(e) = self
                .decoder
                .seek(frames_to_time(from, self.decoder.output_rate()))
        {
            tracing::warn!("audio seek failed: {e}");
        }
        let body = &mut out[lead * CHANNELS..];
        if let Err(e) = self.decoder.read(body) {
            tracing::warn!("audio decode failed: {e}");
            body.fill(0.0);
        }
        if self.gain != 1.0 {
            for s in body.iter_mut() {
                *s *= self.gain;
            }
        }
    }
}
