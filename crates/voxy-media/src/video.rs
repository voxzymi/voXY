//! Video decoding with FFmpeg.
//!
//! - [`probe`] reads a file's streams and timing without decoding.
//! - [`VideoDecoder`] demuxes and decodes the best video stream, optionally
//!   on hardware (VAAPI or CUDA on Linux, VideoToolbox on macOS, D3D11VA,
//!   DXVA2 or CUDA on Windows). If no device works it falls back to software
//!   and says so in [`VideoDecoder::hardware`].
//! - [`VideoDecoder::frame_at`] is **frame-accurate**: it returns the frame
//!   on screen at a source time, the last frame whose presentation
//!   timestamp is at or before it. Backward requests seek to the preceding
//!   keyframe and decode up to the target, so B-frames and long GOPs are
//!   handled.
//! - Forward requests are **GOP-aware** ([`VideoDecoder::plan`]): the
//!   decoder keeps a [`GopIndex`] of keyframes it has seen (or that
//!   [`scan_keyframes`] found), and decodes forward while the target is in
//!   the current GOP, seeking only when a keyframe close to the target makes
//!   that cheaper. Without index data it falls back to a time window.
//! - [`VideoDecoder::step_toward`] does the same work one frame at a time,
//!   so a decoder thread can abandon a stale request between frames.
//! - All times are flicks ([`Time`]), measured from the stream's first
//!   frame: `frame_at(Time::ZERO)` is always the first frame, whatever the
//!   container's start timestamp. Conversions use [`TimeBase`], which is
//!   exact for every common time base and round-trips for all of them.
//!
//! Output frames are 8-bit RGBA (straight alpha, sRGB-encoded). YUV is
//! converted with the stream's own matrix (BT.601, BT.709 or BT.2020) and
//! range (limited or full). The compositor uploads them through the same
//! prepare pass as still images.

// FFI is needed where ffmpeg-next has no safe wrapper: hardware device
// setup, the decoder's format negotiation callback, hardware frame
// download, stream-precise seeking and colour-matrix setup. Every unsafe
// block states the invariant it relies on.
#![allow(unsafe_code)]

use std::ffi::CStr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError};

use ffmpeg::ffi;
use ffmpeg::util::format::Pixel;
use ffmpeg::{codec, format, frame, media, software::scaling};
use ffmpeg_next as ffmpeg;
use voxy_core::{FrameRate, Time};

use crate::MediaError;
pub use crate::VideoFrame;
use crate::gop::{GopIndex, KeyframeSpan};
use crate::timebase::TimeBase;

/// Without keyframe information: decode forward instead of seeking when the
/// target is at most this far ahead of the current frame.
const FORWARD_WINDOW: Time = Time::from_secs(2);
/// What a seek costs on top of decoding from the keyframe, in frames
/// (decoder flush, pipeline refill with frame threading).
const SEEK_COST_FRAMES: i64 = 8;
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

/// How [`VideoDecoder`] will reach a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekPlan {
    /// The current frame is the answer.
    Ready,
    /// Decode forward from the current position.
    Forward,
    /// Seek to the keyframe at or before the target, then decode forward.
    Seek,
}

/// Progress of one [`VideoDecoder::step_toward`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// One more frame was decoded or moved into place; call again.
    Continue,
    /// The current frame is the one on screen at the target.
    Ready,
}

/// One decoding step's internal result.
enum StepState {
    Continue,
    Ready,
    /// The first frame after a seek is already past the target.
    Overshot,
}

fn lock(gop: &Mutex<GopIndex>) -> MutexGuard<'_, GopIndex> {
    gop.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reads every packet of the best video stream without decoding, recording
/// keyframes into `index` as it goes, until the end of the file or until
/// `stop` is set. Demuxing is much cheaper than decoding, so this finds the
/// whole GOP structure quickly. Marks the index complete at the end.
pub fn scan_keyframes(
    path: &Path,
    index: &Mutex<GopIndex>,
    stop: &AtomicBool,
) -> Result<(), MediaError> {
    init();
    let mut input = format::input(path).map_err(|e| err(path, e))?;
    let stream = input
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| err(path, "no video stream"))?;
    let stream_index = stream.index();
    let start = stream.start_time();
    let mut run_start = if start == ffi::AV_NOPTS_VALUE {
        None
    } else {
        Some(start)
    };
    let mut pending = Vec::new();
    let mut unpublished = 0_u32;
    let mut run_end = i64::MIN;
    let mut packet = ffmpeg::Packet::empty();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        match packet.read(&mut input) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(e) => return Err(err(path, format!("reading failed: {e}"))),
        }
        if packet.stream() != stream_index {
            continue;
        }
        let Some(pts) = packet.pts().or(packet.dts()) else {
            continue;
        };
        if packet.is_key() {
            pending.push(pts);
        }
        run_end = run_end.max(pts);
        let s = *run_start.get_or_insert(pts);
        unpublished += 1;
        // Publish in batches so the decoder thread rarely waits on the lock.
        if unpublished >= 32 || packet.is_key() {
            unpublished = 0;
            let mut g = lock(index);
            for k in pending.drain(..) {
                g.add_keyframe(k);
            }
            g.cover(s, run_end);
        }
    }
    let mut g = lock(index);
    for k in pending {
        g.add_keyframe(k);
    }
    g.mark_complete();
    Ok(())
}

/// Decodes one video stream of a file.
///
/// Field order matters: `decoder` must drop before `hardware`.
// The flags are independent facts about the demuxer and decoder pipeline,
// not states of one machine.
#[allow(clippy::struct_excessive_bools)]
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
    converted: Option<Arc<VideoFrame>>,
    /// Keyframes and read ranges, shared with a background scan.
    gop: Arc<Mutex<GopIndex>>,
    /// The range of packet timestamps read since the last seek:
    /// `(first keyframe, highest timestamp)`.
    run: Option<(i64, i64)>,
    /// Reading started at the top of the file (opened or rewound), so
    /// reaching the end means every packet has been seen.
    from_start: bool,
    /// `current` is the stream's first frame, which is also the answer for
    /// any time before it.
    at_first: bool,
    /// The last seek: `(target, timestamp of the frame it landed on)`.
    /// Seeking again for a target between the two would land in the same
    /// place.
    last_seek: Option<(i64, i64)>,
    /// How many frames were decoded and how many seeks done, in total.
    decoded: u64,
    seeks: u64,
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
            gop: Arc::new(Mutex::new(GopIndex::new())),
            run: Some((start_pts, start_pts)),
            from_start: true,
            at_first: false,
            last_seek: None,
            decoded: 0,
            seeks: 0,
        })
    }

    /// The keyframe index this decoder consults and adds to. Share it with
    /// [`scan_keyframes`] on another thread to fill it in ahead of time.
    pub fn gop_index(&self) -> Arc<Mutex<GopIndex>> {
        self.gop.clone()
    }

    /// Frames decoded so far (including ones decoded only to reach a
    /// later frame).
    pub fn frames_decoded(&self) -> u64 {
        self.decoded
    }

    /// Seeks performed so far.
    pub fn seeks(&self) -> u64 {
        self.seeks
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

    /// `time` (from the start of the stream) → the stream timestamp of
    /// the frame on screen then, at the latest.
    fn target_pts(&self, time: Time) -> i64 {
        self.start_pts
            .saturating_add(self.tb().to_pts_floor(time.max(Time::ZERO)))
    }

    fn is_ready(&self, target: i64) -> bool {
        self.current.as_ref().is_some_and(|c| {
            (c.pts <= target || self.at_first)
                && (self.lookahead.as_ref().is_some_and(|l| target < l.pts)
                    || (self.at_end && self.lookahead.is_none()))
        })
    }

    /// Nominal frame duration, for estimating decode costs.
    fn frame_ticks(&self) -> i64 {
        let d = self
            .info
            .frame_rate
            .map_or(Time::from_secs_f64(1.0 / 30.0), |r| {
                r.frame_to_time(1) - r.frame_to_time(0)
            });
        self.tb().to_pts_floor(d).max(1)
    }

    /// How the decoder would reach the frame on screen at `time`; see
    /// [`SeekPlan`]. GOP-aware: with keyframe information, decoding forward
    /// is chosen while no keyframe lies between the current position and the
    /// target, and a seek when one does and seeking skips enough frames to
    /// pay for itself. Without, a fixed time window decides.
    pub fn plan(&self, time: Time) -> SeekPlan {
        self.plan_pts(self.target_pts(time))
    }

    fn plan_pts(&self, target: i64) -> SeekPlan {
        if self.is_ready(target) {
            return SeekPlan::Ready;
        }
        // Where decoding forward would continue from.
        let position = match (&self.lookahead, &self.current) {
            (Some(l), _) => l.pts,
            (None, Some(c)) => c.pts,
            // Nothing decoded since opening or rewinding: at the start.
            (None, None) if !self.sent_eof && self.run.is_some() => self.start_pts - 1,
            (None, None) => return SeekPlan::Seek,
        };
        // Backward needs a seek, except before the first frame, which
        // answers for those times too.
        if !self.at_first && target < self.current.as_ref().map_or(position, |c| c.pts) {
            return SeekPlan::Seek;
        }
        if self.at_end && self.lookahead.is_none() {
            // Past the last frame, which `is_ready` already accepts; only an
            // earlier target gets here.
            return SeekPlan::Seek;
        }
        if let (Some((sought, landed)), Some(c)) = (self.last_seek, &self.current)
            && landed <= c.pts
            && target <= sought
        {
            // A seek for this target would land where the last one did,
            // which is behind us.
            return SeekPlan::Forward;
        }
        let frames = |ticks: i64| ticks / self.frame_ticks();
        let forward = frames(target - position);
        match lock(&self.gop).keyframe_in(position, target) {
            KeyframeSpan::None => SeekPlan::Forward,
            KeyframeSpan::Keyframe(k) => {
                if frames(target - k) + SEEK_COST_FRAMES < forward {
                    SeekPlan::Seek
                } else {
                    SeekPlan::Forward
                }
            }
            KeyframeSpan::Unknown => {
                if self.pts_to_time(target) - self.pts_to_time(position.max(self.start_pts))
                    <= FORWARD_WINDOW
                {
                    SeekPlan::Forward
                } else {
                    SeekPlan::Seek
                }
            }
        }
    }

    /// The frame on screen at `time` (from the start of the stream). Before
    /// the first frame this is the first frame; after the last, the last.
    pub fn frame_at(&mut self, time: Time) -> Result<VideoFrame, MediaError> {
        let target = self.target_pts(time);
        match self.plan_pts(target) {
            SeekPlan::Ready => {}
            SeekPlan::Forward => {
                self.advance_to(target)?;
            }
            SeekPlan::Seek => {
                self.seek_to(target)?;
                self.advance_to(target)?;
            }
        }
        self.convert_current().map(|f| (*f).clone())
    }

    /// Does one frame's worth of work toward the frame on screen at `time`:
    /// a seek to the right keyframe (decoding the first frame after it), or
    /// decoding one more frame. Returns [`Step::Ready`] once
    /// [`current_frame`](Self::current_frame) is the answer. Calling this
    /// until it is ready gives the same frame as [`frame_at`](Self::frame_at).
    pub fn step_toward(&mut self, time: Time) -> Result<Step, MediaError> {
        let target = self.target_pts(time);
        match self.plan_pts(target) {
            SeekPlan::Ready => return Ok(Step::Ready),
            SeekPlan::Seek => {
                self.seek_to(target)?;
                if self.current.is_none() {
                    return Err(err(&self.path, "the video has no frames"));
                }
            }
            SeekPlan::Forward => match self.step(target)? {
                StepState::Continue => {}
                StepState::Ready | StepState::Overshot => return Ok(Step::Ready),
            },
        }
        Ok(if self.is_ready(target) {
            Step::Ready
        } else {
            Step::Continue
        })
    }

    /// The decoded frame the decoder is on, converted to RGBA (once; later
    /// calls share it).
    pub fn current_frame(&mut self) -> Result<Arc<VideoFrame>, MediaError> {
        self.convert_current()
    }

    /// Presentation time of the frame the decoder is on, if any.
    pub fn current_time(&self) -> Option<Time> {
        self.current.as_ref().map(|c| self.pts_to_time(c.pts))
    }

    /// When the current frame stops being on screen: the next frame's time
    /// once that is decoded, [`Time::MAX`] for the last frame, `None` while
    /// unknown.
    pub fn current_end(&self) -> Option<Time> {
        self.current.as_ref()?;
        match &self.lookahead {
            Some(l) => Some(self.pts_to_time(l.pts)),
            None if self.at_end => Some(Time::MAX),
            None => None,
        }
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
        self.at_first = self.current.is_none() && self.from_start;
        self.current = Some(next);
        self.converted = None;
        self.convert_current().map(|f| Some((*f).clone()))
    }

    /// One step of decoding forward toward `target`: moves the lookahead
    /// frame into place, or decodes one frame. A decoded frame becomes the
    /// lookahead first, so the current frame's end is known before it is
    /// replaced.
    fn step(&mut self, target: i64) -> Result<StepState, MediaError> {
        if let Some(l) = self.lookahead.take() {
            if l.pts > target {
                self.lookahead = Some(l);
                return Ok(StepState::Ready);
            }
            self.current = Some(l);
            self.converted = None;
            self.at_first = false;
            return Ok(StepState::Continue);
        }
        if self.at_end {
            return Ok(StepState::Ready);
        }
        let Some(raw) = self.decode_next()? else {
            self.at_end = true;
            return Ok(StepState::Ready);
        };
        if self.current.is_none() {
            let past = raw.pts > target;
            self.current = Some(raw);
            self.converted = None;
            self.at_first = self.from_start;
            return Ok(if past && !self.at_first {
                StepState::Overshot
            } else {
                StepState::Continue
            });
        }
        self.lookahead = Some(raw);
        Ok(StepState::Continue)
    }

    /// Decodes forward until `current` is the frame on screen at `target`.
    /// Returns whether a frame at or before `target` was found.
    fn advance_to(&mut self, target: i64) -> Result<bool, MediaError> {
        loop {
            match self.step(target)? {
                StepState::Continue => {}
                StepState::Ready => return Ok(self.current.is_some()),
                StepState::Overshot => return Ok(false),
            }
        }
    }

    /// Decodes the first frame at the current read position. Returns whether
    /// it is at or before `target`.
    fn land(&mut self, target: i64) -> Result<bool, MediaError> {
        while self.current.is_none() {
            match self.step(target)? {
                StepState::Continue => {}
                StepState::Ready | StepState::Overshot => return Ok(false),
            }
        }
        Ok(self.current.as_ref().is_some_and(|c| c.pts <= target))
    }

    /// Seeks to the keyframe at or before `target` and decodes the first
    /// frame there.
    ///
    /// Some demuxers (MPEG-TS seeks by byte position, for example) can land
    /// after the target; the seek is then retried further back, and as a
    /// last resort the file is reopened and decoded from the start, which is
    /// always correct.
    fn seek_to(&mut self, target: i64) -> Result<(), MediaError> {
        self.seeks += 1;
        let second = self.tb().to_pts_floor(Time::from_secs(1)).max(1);
        // Seek straight to the keyframe when the index proves it is the
        // last one before the target: demuxers find exact entries reliably.
        let mut seek_point = {
            let g = lock(&self.gop);
            g.keyframe_at_or_before(target)
                .filter(|&k| g.keyframe_in(k, target) == KeyframeSpan::None)
                .unwrap_or(target)
        };
        for attempt in 0..=SEEK_RETRIES {
            if seek_point < self.start_pts || (attempt > 0 && seek_point <= self.start_pts) {
                break;
            }
            self.seek_raw(seek_point)?;
            if self.land(target)? {
                self.last_seek = self.current.as_ref().map(|c| (target, c.pts));
                return Ok(());
            }
            seek_point = seek_point.min(target).saturating_sub(second << attempt);
        }
        self.rewind()?;
        self.land(target)?;
        self.last_seek = self.current.as_ref().map(|c| (target, c.pts));
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
        self.from_start = true;
        self.run = Some((self.start_pts, self.start_pts));
        Ok(())
    }

    fn reset_decoding(&mut self) {
        self.decoder.flush();
        self.sent_eof = false;
        self.awaiting_keyframe = true;
        self.keyframe_pts = None;
        self.at_end = false;
        self.at_first = false;
        self.from_start = false;
        self.run = None;
        self.current = None;
        self.lookahead = None;
        self.converted = None;
    }

    /// Records a packet of our stream in the GOP index.
    fn record_packet(&mut self, pts: Option<i64>, key: bool) {
        let Some(pts) = pts else { return };
        let run = self.run.get_or_insert((pts, pts));
        run.1 = run.1.max(pts);
        let (start, end) = *run;
        let mut g = lock(&self.gop);
        if key {
            g.add_keyframe(pts);
        }
        g.cover(start, end);
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
                    self.decoded += 1;
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
                    self.record_packet(packet.pts().or(packet.dts()), packet.is_key());
                    if let Err(e) = self.decoder.send_packet(&packet) {
                        // Corrupt packets are skipped; the decoder recovers at
                        // the next keyframe.
                        tracing::debug!("{}: skipping bad packet: {e}", self.path.display());
                    }
                }
                Err(ffmpeg::Error::Eof) => {
                    let _ = self.decoder.send_eof();
                    self.sent_eof = true;
                    if self.from_start {
                        lock(&self.gop).mark_complete();
                    }
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

    fn convert_current(&mut self) -> Result<Arc<VideoFrame>, MediaError> {
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
        let out = Arc::new(VideoFrame {
            time: self.pts_to_time(pts),
            duration,
            width: w,
            height: h,
            pixels,
        });
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
