//! Video decoding with FFmpeg.
//!
//! - [`probe`] reads a file's streams and timing without decoding.
//! - [`VideoDecoder`] demuxes and decodes the best video stream, optionally
//!   on hardware (VAAPI or CUDA on Linux, VideoToolbox on macOS, D3D11VA,
//!   DXVA2 or CUDA on Windows). If no device works it falls back to software
//!   and says so in [`VideoDecoder::hardware`].
//! - [`VideoDecoder::frame_at`] is **frame-accurate**: it returns the frame
//!   on screen at a source time, the last frame whose presentation
//!   timestamp is at or before it. Nearby requests decode forward; distant
//!   or backward ones seek to the preceding keyframe and decode up to the
//!   target, so B-frames and long GOPs are handled.
//! - All times are flicks ([`Time`]), measured from the stream's first
//!   frame: `frame_at(Time::ZERO)` is always the first frame, whatever the
//!   container's start timestamp. Conversions use [`TimeBase`], which is
//!   exact for every common time base and round-trips for all of them.
//!
//! Output frames are 8-bit RGBA (straight alpha, sRGB-encoded). YUV is
//! converted with the stream's own matrix (BT.601, BT.709 or BT.2020) and
//! range (limited or full). Higher-precision output for the linear-light
//! compositor follows with the GPU upload path.

// FFI is needed where ffmpeg-next has no safe wrapper: hardware device
// setup, the decoder's format negotiation callback, hardware frame
// download, stream-precise seeking and colour-matrix setup. Every unsafe
// block states the invariant it relies on.
#![allow(unsafe_code)]

use std::ffi::CStr;
use std::path::Path;
use std::sync::Once;

use ffmpeg::ffi;
use ffmpeg::util::format::Pixel;
use ffmpeg::{codec, format, frame, media, software::scaling};
use ffmpeg_next as ffmpeg;
use voxy_core::{FrameRate, Time};

use crate::MediaError;
use crate::timebase::TimeBase;

/// Decode forward instead of seeking when the target is at most this far
/// ahead of the current frame.
const FORWARD_WINDOW: Time = Time::from_secs(2);
/// How many times a seek that lands after its target is retried further back.
const SEEK_RETRIES: u32 = 4;

fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = ffmpeg::init();
        ffmpeg::log::set_level(ffmpeg::log::Level::Error);
    });
}

fn err(path: &Path, message: impl std::fmt::Display) -> MediaError {
    MediaError::Media {
        path: path.display().to_string(),
        message: message.to_string(),
    }
}

/// Facts about a video stream.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub stream_index: usize,
    pub width: u32,
    pub height: u32,
    /// Average frame rate, when the container knows it.
    pub frame_rate: Option<FrameRate>,
    pub time_base: TimeBase,
    /// Stream duration, when known.
    pub duration: Option<Time>,
    /// Number of frames, when the container records it.
    pub frame_count: Option<u64>,
    pub codec: String,
    pub pixel_format: String,
}

/// Facts about a media file.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub container: String,
    pub duration: Option<Time>,
    pub video: Option<VideoInfo>,
    pub audio_streams: usize,
}

/// Which decoding hardware to try.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HwAccel {
    /// Try the platform's devices in order, falling back to software.
    #[default]
    Auto,
    /// Software decoding only.
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DecodeOptions {
    pub hardware: HwAccel,
    /// Decoder threads; `None` lets FFmpeg choose.
    pub threads: Option<usize>,
}

/// A decoded frame, ready for upload.
#[derive(Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// Presentation time from the start of the stream.
    pub time: Time,
    /// How long it stays on screen (until the next frame, when known).
    pub duration: Time,
    pub width: u32,
    pub height: u32,
    /// RGBA8, straight alpha, rows top to bottom, no padding.
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("time", &self.time)
            .field("duration", &self.duration)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

fn time_base(r: ffmpeg::Rational) -> Option<TimeBase> {
    TimeBase::new(i64::from(r.numerator()), i64::from(r.denominator()))
}

fn frame_rate(r: ffmpeg::Rational) -> Option<FrameRate> {
    let (n, d) = (r.numerator(), r.denominator());
    (n > 0 && d > 0)
        .then(|| FrameRate::new(n as u32, d as u32))
        .flatten()
}

fn video_info(stream: &format::stream::Stream<'_>, path: &Path) -> Result<VideoInfo, MediaError> {
    let params = stream.parameters();
    let ctx = codec::context::Context::from_parameters(params.clone()).map_err(|e| err(path, e))?;
    let video = ctx
        .decoder()
        .video()
        .map_err(|e| err(path, format!("cannot decode video: {e}")))?;
    let tb = time_base(stream.time_base()).ok_or_else(|| err(path, "stream has no time base"))?;
    let duration = (stream.duration() > 0).then(|| tb.to_time(stream.duration()));
    let frames = u64::try_from(stream.frames()).ok().filter(|&n| n > 0);
    Ok(VideoInfo {
        stream_index: stream.index(),
        width: video.width(),
        height: video.height(),
        frame_rate: frame_rate(stream.avg_frame_rate()).or_else(|| frame_rate(stream.rate())),
        time_base: tb,
        duration,
        frame_count: frames,
        codec: params.id().name().to_owned(),
        pixel_format: format!("{:?}", video.format()),
    })
}

/// Reads a file's streams and timing without decoding any frames.
pub fn probe(path: &Path) -> Result<MediaInfo, MediaError> {
    init();
    let input = format::input(path).map_err(|e| err(path, e))?;
    let video = match input.streams().best(media::Type::Video) {
        Some(s) => Some(video_info(&s, path)?),
        None => None,
    };
    let us = TimeBase::new(1, i64::from(ffi::AV_TIME_BASE)).expect("positive");
    Ok(MediaInfo {
        container: input.format().name().to_owned(),
        duration: (input.duration() > 0).then(|| us.to_time(input.duration())),
        video,
        audio_streams: input
            .streams()
            .filter(|s| s.parameters().medium() == media::Type::Audio)
            .count(),
    })
}

/// A hardware device attached to a decoder.
struct HwDevice {
    name: String,
    /// The hardware pixel format negotiated through `get_format`. Boxed so
    /// its address, stored in the codec context's `opaque`, never moves.
    pix_fmt: Box<ffi::AVPixelFormat>,
    device: *mut ffi::AVBufferRef,
}

// SAFETY: `device` is a reference-counted FFmpeg buffer that is only used
// by the thread owning the decoder; FFmpeg's refcounting is thread-safe.
unsafe impl Send for HwDevice {}

impl Drop for HwDevice {
    fn drop(&mut self) {
        // SAFETY: `device` was returned by av_hwdevice_ctx_create and this is
        // our only reference to it (the codec context holds its own).
        unsafe { ffi::av_buffer_unref(&raw mut self.device) };
    }
}

/// `get_format` callback: picks the negotiated hardware format if offered,
/// otherwise the first software format (graceful fallback).
///
/// SAFETY (caller: FFmpeg): `ctx` is a live codec context whose `opaque`
/// points at the `HwDevice::pix_fmt` box, and `fmts` is a list terminated
/// by `AV_PIX_FMT_NONE`.
unsafe extern "C" fn negotiate_format(
    ctx: *mut ffi::AVCodecContext,
    fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    // SAFETY: see the function contract above.
    unsafe {
        let wanted = *((*ctx).opaque as *const ffi::AVPixelFormat);
        let mut first_software = ffi::AVPixelFormat::AV_PIX_FMT_NONE;
        let mut p = fmts;
        while *p != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p == wanted {
                return *p;
            }
            let desc = ffi::av_pix_fmt_desc_get(*p);
            let is_hw =
                !desc.is_null() && ((*desc).flags & ffi::AV_PIX_FMT_FLAG_HWACCEL as u64) != 0;
            if !is_hw && first_software == ffi::AVPixelFormat::AV_PIX_FMT_NONE {
                first_software = *p;
            }
            p = p.add(1);
        }
        first_software
    }
}

fn hw_candidates() -> &'static [ffi::AVHWDeviceType] {
    use ffi::AVHWDeviceType as T;
    if cfg!(target_os = "macos") {
        &[T::AV_HWDEVICE_TYPE_VIDEOTOOLBOX]
    } else if cfg!(target_os = "windows") {
        &[
            T::AV_HWDEVICE_TYPE_D3D11VA,
            T::AV_HWDEVICE_TYPE_DXVA2,
            T::AV_HWDEVICE_TYPE_CUDA,
        ]
    } else {
        &[T::AV_HWDEVICE_TYPE_VAAPI, T::AV_HWDEVICE_TYPE_CUDA]
    }
}

/// Tries to attach a hardware device to an unopened codec context.
fn attach_hardware(ctx: &mut codec::context::Context, codec: &ffmpeg::Codec) -> Option<HwDevice> {
    for &kind in hw_candidates() {
        // Does this decoder support this device type through a device context?
        let mut pix_fmt = None;
        for i in 0.. {
            // SAFETY: `codec` is a valid decoder; the function returns NULL
            // past the last configuration.
            let cfg = unsafe { ffi::avcodec_get_hw_config(codec.as_ptr(), i) };
            if cfg.is_null() {
                break;
            }
            // SAFETY: non-null configurations are valid static data.
            let cfg = unsafe { &*cfg };
            if cfg.device_type == kind
                && (cfg.methods & ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32) != 0
            {
                pix_fmt = Some(cfg.pix_fmt);
                break;
            }
        }
        let Some(pix_fmt) = pix_fmt else { continue };
        let mut device: *mut ffi::AVBufferRef = std::ptr::null_mut();
        // SAFETY: out-pointer to a null buffer ref; default device and options.
        let ret = unsafe {
            ffi::av_hwdevice_ctx_create(
                &raw mut device,
                kind,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            )
        };
        if ret < 0 || device.is_null() {
            continue;
        }
        // SAFETY: av_hwdevice_get_type_name returns a static C string for
        // valid types.
        let name = unsafe { CStr::from_ptr(ffi::av_hwdevice_get_type_name(kind)) }
            .to_string_lossy()
            .into_owned();
        let hw = HwDevice {
            name,
            pix_fmt: Box::new(pix_fmt),
            device,
        };
        // SAFETY: the context is not opened yet, so these fields may be set.
        // The context takes its own reference to the device; `opaque` points
        // at the boxed pixel format, which outlives the context because the
        // decoder field is dropped before the HwDevice field.
        unsafe {
            let raw = ctx.as_mut_ptr();
            (*raw).hw_device_ctx = ffi::av_buffer_ref(hw.device);
            (*raw).opaque = (&raw const *hw.pix_fmt).cast_mut().cast();
            (*raw).get_format = Some(negotiate_format);
        }
        return Some(hw);
    }
    None
}

/// A decoded frame kept in FFmpeg's format until it is needed.
struct Raw {
    pts: i64,
    frame: frame::Video,
}

/// Decodes one video stream of a file.
///
/// Field order matters: `decoder` must drop before `hardware`.
pub struct VideoDecoder {
    path: std::path::PathBuf,
    info: VideoInfo,
    input: format::context::Input,
    decoder: ffmpeg::decoder::Video,
    hardware: Option<HwDevice>,
    /// Timestamp of the stream's first frame, in stream ticks.
    start_pts: i64,
    sent_eof: bool,
    /// After a seek: packets are skipped until a keyframe arrives, and
    /// frames presented before that keyframe are dropped (they may reference
    /// data from before the seek point).
    awaiting_keyframe: bool,
    keyframe_pts: Option<i64>,
    /// The decoder has delivered its last frame.
    at_end: bool,
    current: Option<Raw>,
    /// The frame after `current`, once decoded.
    lookahead: Option<Raw>,
    scaler: Option<(Pixel, u32, u32, scaling::Context)>,
    converted: Option<VideoFrame>,
}

impl std::fmt::Debug for VideoDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoDecoder")
            .field("path", &self.path)
            .field("info", &self.info)
            .field("hardware", &self.hardware())
            .finish_non_exhaustive()
    }
}

impl VideoDecoder {
    pub fn open(path: &Path, options: DecodeOptions) -> Result<Self, MediaError> {
        init();
        let input = format::input(path).map_err(|e| err(path, e))?;
        let stream = input
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| err(path, "no video stream"))?;
        let info = video_info(&stream, path)?;
        let params = stream.parameters();
        let start = stream.start_time();
        let codec = ffmpeg::decoder::find(params.id())
            .ok_or_else(|| err(path, "no decoder for this codec"))?;
        let mut ctx = codec::context::Context::from_parameters(params).map_err(|e| err(path, e))?;
        if let Some(n) = options.threads {
            ctx.set_threading(ffmpeg::threading::Config {
                count: n,
                ..Default::default()
            });
        }
        let hardware = match options.hardware {
            HwAccel::Auto => attach_hardware(&mut ctx, &codec),
            HwAccel::Off => None,
        };
        let mut decoder = ctx.decoder().video().map_err(|e| err(path, e))?;
        decoder.set_packet_time_base(stream.time_base());
        let start_pts = if start == ffi::AV_NOPTS_VALUE {
            0
        } else {
            start
        };
        if let Some(hw) = &hardware {
            tracing::info!("decoding {} with {}", path.display(), hw.name);
        }
        Ok(VideoDecoder {
            path: path.to_path_buf(),
            info,
            input,
            decoder,
            hardware,
            start_pts,
            sent_eof: false,
            awaiting_keyframe: false,
            keyframe_pts: None,
            at_end: false,
            current: None,
            lookahead: None,
            scaler: None,
            converted: None,
        })
    }

    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// The hardware device in use (`"vaapi"`, `"videotoolbox"`, …), or
    /// `None` when decoding in software.
    pub fn hardware(&self) -> Option<&str> {
        self.hardware.as_ref().map(|h| h.name.as_str())
    }

    fn tb(&self) -> TimeBase {
        self.info.time_base
    }

    /// Stream ticks → time since the first frame.
    fn pts_to_time(&self, pts: i64) -> Time {
        self.tb().to_time(pts.saturating_sub(self.start_pts))
    }

    /// The frame on screen at `time` (from the start of the stream). Before
    /// the first frame this is the first frame; after the last, the last.
    pub fn frame_at(&mut self, time: Time) -> Result<VideoFrame, MediaError> {
        let target = self
            .start_pts
            .saturating_add(self.tb().to_pts_floor(time.max(Time::ZERO)));

        // Already showing the right frame?
        if let Some(c) = &self.current
            && c.pts <= target
            && (self.lookahead.as_ref().is_some_and(|l| target < l.pts)
                || (self.at_end && self.lookahead.is_none()))
        {
            return self.convert_current();
        }

        let forward = self.current.as_ref().is_some_and(|c| {
            c.pts <= target && self.pts_to_time(target) - self.pts_to_time(c.pts) <= FORWARD_WINDOW
        }) || (self.current.is_none()
            && self.pts_to_time(target) <= FORWARD_WINDOW
            && !self.sent_eof);

        if forward {
            self.advance_to(target)?;
        } else {
            self.seek_to(target)?;
        }
        self.convert_current()
    }

    /// Frame number `index` of a constant-frame-rate stream (0 = first).
    ///
    /// Samples the *middle* of the frame's nominal interval rather than its
    /// start: containers round timestamps (Matroska to the millisecond, for
    /// example, so 29.97 fps frame 2 at 66.73 ms is stored as 67 ms), and a
    /// request exactly on the nominal boundary could otherwise land on the
    /// previous frame. Rounding never moves a timestamp by half a frame.
    pub fn frame_at_index(&mut self, index: u64) -> Result<VideoFrame, MediaError> {
        let rate = self
            .info
            .frame_rate
            .ok_or_else(|| err(&self.path, "the stream has no frame rate"))?;
        let i = i64::try_from(index).unwrap_or(i64::MAX - 1);
        let start = rate.frame_to_time(i);
        let next = rate.frame_to_time(i + 1);
        self.frame_at(start + Time((next - start).0 / 2))
    }

    /// Decodes the next frame in presentation order, or `None` at the end.
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        let next = match self.lookahead.take() {
            Some(l) => Some(l),
            None => self.decode_next()?,
        };
        let Some(next) = next else {
            self.at_end = true;
            return Ok(None);
        };
        self.current = Some(next);
        self.converted = None;
        self.convert_current().map(Some)
    }

    /// Decodes forward until `current` is the frame on screen at `target`.
    /// Returns whether a frame at or before `target` was found.
    fn advance_to(&mut self, target: i64) -> Result<bool, MediaError> {
        if let Some(l) = self.lookahead.take() {
            if l.pts > target {
                self.lookahead = Some(l);
                return Ok(self.current.is_some());
            }
            self.current = Some(l);
            self.converted = None;
        }
        loop {
            let Some(raw) = self.decode_next()? else {
                self.at_end = true;
                return Ok(self.current.is_some());
            };
            if raw.pts > target {
                if self.current.is_none() {
                    // The first frame is already past the target.
                    self.current = Some(raw);
                    self.converted = None;
                    return Ok(false);
                }
                self.lookahead = Some(raw);
                return Ok(true);
            }
            self.current = Some(raw);
            self.converted = None;
        }
    }

    /// Seeks to the keyframe at or before `target` and decodes up to it.
    ///
    /// Some demuxers (MPEG-TS seeks by byte position, for example) can land
    /// after the target; the seek is then retried further back, and as a
    /// last resort the file is reopened and decoded from the start, which is
    /// always correct.
    fn seek_to(&mut self, target: i64) -> Result<(), MediaError> {
        let second = self.tb().to_pts_floor(Time::from_secs(1)).max(1);
        let mut seek_point = target;
        for attempt in 0..=SEEK_RETRIES {
            if seek_point <= self.start_pts {
                break;
            }
            self.seek_raw(seek_point)?;
            if self.advance_to(target)? {
                return Ok(());
            }
            seek_point = target.saturating_sub(second << attempt);
        }
        self.rewind()?;
        self.advance_to(target)?;
        Ok(())
    }

    fn seek_raw(&mut self, pts: i64) -> Result<(), MediaError> {
        let index = i32::try_from(self.info.stream_index).unwrap_or(0);
        // SAFETY: `input` is an open demuxer and `index` one of its streams.
        // avformat_seek_file with max_ts = pts asks for the closest keyframe
        // at or before `pts`.
        let ret = unsafe {
            ffi::avformat_seek_file(self.input.as_mut_ptr(), index, i64::MIN, pts, pts, 0)
        };
        if ret < 0 {
            // This demuxer cannot seek this way: start over from the top.
            return self.rewind();
        }
        self.reset_decoding();
        Ok(())
    }

    /// Reopens the file so decoding starts from its first byte.
    fn rewind(&mut self) -> Result<(), MediaError> {
        self.input = format::input(&self.path).map_err(|e| err(&self.path, e))?;
        self.reset_decoding();
        Ok(())
    }

    fn reset_decoding(&mut self) {
        self.decoder.flush();
        self.sent_eof = false;
        self.awaiting_keyframe = true;
        self.keyframe_pts = None;
        self.at_end = false;
        self.current = None;
        self.lookahead = None;
        self.converted = None;
    }

    /// The next decoded frame in presentation order (downloaded from the
    /// GPU if hardware-decoded).
    fn decode_next(&mut self) -> Result<Option<Raw>, MediaError> {
        let stream = self.info.stream_index;
        loop {
            let mut f = frame::Video::empty();
            match self.decoder.receive_frame(&mut f) {
                Ok(()) => {
                    let raw = self.to_raw(f)?;
                    if self.keyframe_pts.is_some_and(|k| raw.pts < k) {
                        continue; // leading frame of an open GOP after a seek
                    }
                    return Ok(Some(raw));
                }
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {}
                Err(e) => return Err(err(&self.path, format!("decoding failed: {e}"))),
            }
            if self.sent_eof {
                return Ok(None);
            }
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) => {
                    if packet.stream() != stream {
                        continue;
                    }
                    if self.awaiting_keyframe {
                        if !packet.is_key() {
                            continue;
                        }
                        self.awaiting_keyframe = false;
                        self.keyframe_pts = packet.pts().or(packet.dts());
                    }
                    if let Err(e) = self.decoder.send_packet(&packet) {
                        // Corrupt packets are skipped; the decoder recovers at
                        // the next keyframe.
                        tracing::debug!("{}: skipping bad packet: {e}", self.path.display());
                    }
                }
                Err(ffmpeg::Error::Eof) => {
                    let _ = self.decoder.send_eof();
                    self.sent_eof = true;
                }
                Err(e) => return Err(err(&self.path, format!("reading failed: {e}"))),
            }
        }
    }

    fn to_raw(&self, f: frame::Video) -> Result<Raw, MediaError> {
        let f = match &self.hardware {
            // SAFETY: reading the format field of a valid frame.
            Some(hw) if unsafe { (*f.as_ptr()).format } == *hw.pix_fmt as i32 => {
                let mut sw = frame::Video::empty();
                // SAFETY: `f` is a hardware frame from our decoder and `sw` an
                // empty frame; FFmpeg allocates `sw` in a software format.
                let ret = unsafe { ffi::av_hwframe_transfer_data(sw.as_mut_ptr(), f.as_ptr(), 0) };
                if ret < 0 {
                    return Err(err(&self.path, "could not download a hardware frame"));
                }
                // SAFETY: both frames are valid; copies timestamps and colour
                // metadata.
                unsafe { ffi::av_frame_copy_props(sw.as_mut_ptr(), f.as_ptr()) };
                sw
            }
            _ => f,
        };
        let pts = f.timestamp().or_else(|| f.pts()).unwrap_or_else(|| {
            // No timestamp at all: continue after the previous frame.
            self.current.as_ref().map_or(self.start_pts, |c| c.pts + 1)
        });
        Ok(Raw { pts, frame: f })
    }

    fn convert_current(&mut self) -> Result<VideoFrame, MediaError> {
        if let Some(c) = &self.converted {
            return Ok(c.clone());
        }
        let Some(cur) = &self.current else {
            return Err(err(&self.path, "the video has no frames"));
        };
        let (pts, fmt, w, h) = (
            cur.pts,
            cur.frame.format(),
            cur.frame.width(),
            cur.frame.height(),
        );
        let duration = match &self.lookahead {
            Some(l) if l.pts > pts => self.pts_to_time(l.pts) - self.pts_to_time(pts),
            _ => self
                .info
                .frame_rate
                .and_then(FrameRate::exact_frame_duration)
                .unwrap_or_else(|| Time::from_secs_f64(1.0 / 30.0)),
        };
        if self
            .scaler
            .as_ref()
            .is_none_or(|s| (s.0, s.1, s.2) != (fmt, w, h))
        {
            let mut ctx = scaling::Context::get(
                fmt,
                w,
                h,
                Pixel::RGBA,
                w,
                h,
                scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND,
            )
            .map_err(|e| err(&self.path, format!("no converter from {fmt:?}: {e}")))?;
            set_colorspace(&mut ctx, &cur.frame);
            self.scaler = Some((fmt, w, h, ctx));
        }
        let mut rgba = frame::Video::empty();
        let scaler = &mut self.scaler.as_mut().expect("created above").3;
        scaler
            .run(&cur.frame, &mut rgba)
            .map_err(|e| err(&self.path, e))?;
        let row = w as usize * 4;
        let stride = rgba.stride(0);
        let data = rgba.data(0);
        let mut pixels = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            pixels.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        let out = VideoFrame {
            time: self.pts_to_time(pts),
            duration,
            width: w,
            height: h,
            pixels,
        };
        self.converted = Some(out.clone());
        Ok(out)
    }
}

/// Uses the frame's own YUV matrix and range for the conversion to RGB.
/// Unspecified matrices follow the usual convention: BT.709 for HD and
/// larger, BT.601 below.
fn set_colorspace(ctx: &mut scaling::Context, f: &frame::Video) {
    use ffmpeg::util::color::{Range, Space};
    let matrix = match f.color_space() {
        Space::BT709 => ffi::SWS_CS_ITU709,
        Space::BT2020NCL | Space::BT2020CL => ffi::SWS_CS_BT2020,
        Space::BT470BG | Space::SMPTE170M | Space::FCC => ffi::SWS_CS_ITU601,
        _ if f.height() >= 720 => ffi::SWS_CS_ITU709,
        _ => ffi::SWS_CS_ITU601,
    };
    let full = i32::from(f.color_range() == Range::JPEG);
    // SAFETY: `ctx` is a valid scaler; the coefficient tables are static.
    // Returns an error for non-YUV sources, which is fine to ignore.
    unsafe {
        let table = ffi::sws_getCoefficients(matrix);
        ffi::sws_setColorspaceDetails(ctx.as_mut_ptr(), table, full, table, 1, 0, 1 << 16, 1 << 16);
    }
}
