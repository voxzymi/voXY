//! Audio output, the master clock, and the threads between them.
//!
//! ```text
//!   AudioSource ──render()──▶ feeder thread ──rtrb ring──▶ device callback ──▶ speakers
//!   (decoding, mixing:          (may allocate,             (real time: never allocates,
//!    may block briefly)          decode, sleep)              locks or blocks)
//!                                                                 │ anchors
//!                                                                 ▼
//!                                       PlaybackClock::time() ◀── Shared (seqlock)
//! ```
//!
//! **The audio device is the master clock.** On every buffer the callback
//! publishes an anchor: "timeline frame *F* becomes audible at instant *T*"
//! (*T* = now + the device's reported output latency). [`PlaybackClock`]
//! interpolates from the latest anchor, so video scheduled against it stays
//! in sync with what is actually heard, and device-clock drift is corrected
//! every buffer. Without an output device, a timer thread drives the same
//! callback code (the *null* output), so playback and the clock behave the
//! same.
//!
//! **Underruns** (the feeder falling behind) output silence. The clock still
//! advances, because time does not stop, and the feeder skips ahead by the
//! same amount, so later audio stays aligned with the clock.
//!
//! **Play, pause and seek** hand the callback a fresh, pre-filled ring
//! through a lock-free channel; old rings are dropped on the feeder thread,
//! never on the real-time thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded, unbounded};
use voxy_core::{FLICKS_PER_SECOND, Time};

/// Interleaved stereo throughout the engine; the callback maps to the
/// device's channel count.
pub const CHANNELS: usize = 2;
/// Frames rendered per feeder step.
const CHUNK_FRAMES: usize = 512;
/// Audio buffered ahead of the device.
const RING_SECONDS: f64 = 0.25;
/// Longest the clock extrapolates past its last anchor (protects against a
/// stalled device callback).
const MAX_EXTRAPOLATION: Duration = Duration::from_millis(250);

/// Produces the audio of the timeline.
///
/// Called only on the feeder thread, never the real-time thread, so it may
/// allocate, decode and block briefly.
pub trait AudioSource: Send {
    /// Fills `out` (interleaved stereo) with audio starting at timeline frame
    /// `start` (frames at the engine's sample rate). Must fill all of `out`;
    /// silence where nothing plays.
    fn render(&mut self, start: i64, out: &mut [f32]);
}

/// A source that is always silent (playback without audio layers).
#[derive(Debug, Default, Clone, Copy)]
pub struct Silence;

impl AudioSource for Silence {
    fn render(&mut self, _start: i64, out: &mut [f32]) {
        out.fill(0.0);
    }
}

/// Timeline frame → time at `rate` (exact for every common rate).
pub fn frames_to_time(frames: i64, rate: u32) -> Time {
    Time((i128::from(frames) * i128::from(FLICKS_PER_SECOND) / i128::from(rate)) as i64)
}

/// Time → the timeline frame containing it.
pub fn time_to_frames(time: Time, rate: u32) -> i64 {
    (i128::from(time.0) * i128::from(rate)).div_euclid(i128::from(FLICKS_PER_SECOND)) as i64
}

/// State shared by the callback (sole writer of the anchor) and readers.
#[derive(Debug)]
struct Shared {
    rate: u32,
    epoch: Instant,
    /// Seqlock sequence: odd while the anchor is being written.
    seq: AtomicU64,
    /// Timeline frame audible at `anchor_ns`.
    anchor_frame: AtomicI64,
    /// Nanoseconds since `epoch`.
    anchor_ns: AtomicU64,
    playing: AtomicBool,
    /// Total frames of silence output because the ring ran dry.
    underrun_frames: AtomicU64,
    /// Highest time handed out since the last seek (keeps the clock
    /// monotonic across re-anchoring).
    high_water: AtomicI64,
    seek_count: AtomicU64,
}

impl Shared {
    fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    fn publish(&self, frame: i64, ns: u64, playing: bool) {
        self.seq.fetch_add(1, Ordering::AcqRel); // odd: writing
        self.anchor_frame.store(frame, Ordering::Relaxed);
        self.anchor_ns.store(ns, Ordering::Relaxed);
        self.playing.store(playing, Ordering::Relaxed);
        self.seq.fetch_add(1, Ordering::Release); // even: stable
    }

    fn read(&self) -> (i64, u64, bool) {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let f = self.anchor_frame.load(Ordering::Relaxed);
            let n = self.anchor_ns.load(Ordering::Relaxed);
            let p = self.playing.load(Ordering::Relaxed);
            if self.seq.load(Ordering::Acquire) == s1 {
                return (f, n, p);
            }
        }
    }
}

/// The master clock. Cheap to clone; readable from any thread.
#[derive(Debug, Clone)]
pub struct PlaybackClock {
    shared: Arc<Shared>,
}

impl PlaybackClock {
    /// The timeline time being heard now. Never goes backwards between
    /// seeks, even when a new anchor lands a hair earlier than the previous
    /// extrapolation.
    pub fn time(&self) -> Time {
        let raw = self.time_at_ns(self.shared.now_ns());
        if !self.is_playing() {
            return raw;
        }
        let prev = self.shared.high_water.fetch_max(raw.0, Ordering::AcqRel);
        Time(prev.max(raw.0))
    }

    /// The timeline time heard at `ns` nanoseconds after the engine started,
    /// from the latest anchor (pure; for scheduling ahead and for tests).
    pub fn time_at_ns(&self, ns: u64) -> Time {
        let (frame, anchor_ns, playing) = self.shared.read();
        let base = frames_to_time(frame, self.shared.rate);
        if !playing {
            return base;
        }
        // Before the anchor instant the previous buffer is still playing:
        // extrapolating backwards is correct.
        let dt = (i128::from(ns) - i128::from(anchor_ns)).min(MAX_EXTRAPOLATION.as_nanos() as i128);
        Time(base.0 + (dt * i128::from(FLICKS_PER_SECOND) / 1_000_000_000) as i64)
    }

    pub fn is_playing(&self) -> bool {
        self.shared.read().2
    }

    pub fn sample_rate(&self) -> u32 {
        self.shared.rate
    }

    /// Frames of silence output so far because audio was not ready in time.
    pub fn underrun_frames(&self) -> u64 {
        self.shared.underrun_frames.load(Ordering::Relaxed)
    }
}

enum Control {
    /// Start playing `consumer`'s audio, whose first frame is `frame`.
    Start {
        consumer: rtrb::Consumer<f32>,
        frame: i64,
    },
    /// Stop at whatever is audible now.
    Pause,
    /// Stopped, positioned at `frame`.
    Park { frame: i64 },
}

/// The real-time side. Owned by the device callback (or a test).
pub struct CallbackState {
    shared: Arc<Shared>,
    control: Receiver<Control>,
    graveyard: Sender<rtrb::Consumer<f32>>,
    consumer: Option<rtrb::Consumer<f32>>,
    /// Timeline frame of the next sample to output.
    position: i64,
    playing: bool,
    last_latency_frames: i64,
    scratch: Vec<f32>,
}

impl std::fmt::Debug for CallbackState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackState")
            .field("position", &self.position)
            .field("playing", &self.playing)
            .finish_non_exhaustive()
    }
}

impl CallbackState {
    /// Fills one device buffer.
    ///
    /// - `out`: interleaved samples for `device_channels` channels.
    /// - `latency`: time until the first sample of `out` is audible.
    /// - `now_ns`: nanoseconds since the engine started.
    ///
    /// Never allocates (after the first buffer of a given size), locks or
    /// blocks.
    pub fn process(
        &mut self,
        out: &mut [f32],
        device_channels: usize,
        latency: Duration,
        now_ns: u64,
    ) {
        let rate = i64::from(self.shared.rate);
        let latency_frames = (latency.as_nanos() as i64).saturating_mul(rate) / 1_000_000_000;
        loop {
            match self.control.try_recv() {
                Ok(Control::Start { consumer, frame }) => {
                    if let Some(old) = self.consumer.replace(consumer) {
                        let _ = self.graveyard.try_send(old);
                    }
                    self.position = frame;
                    self.playing = true;
                }
                Ok(Control::Pause) => {
                    if self.playing {
                        // What is audible now, not what was queued last.
                        self.position -= self.last_latency_frames;
                    }
                    self.playing = false;
                    if let Some(old) = self.consumer.take() {
                        let _ = self.graveyard.try_send(old);
                    }
                }
                Ok(Control::Park { frame }) => {
                    self.position = frame;
                    self.playing = false;
                    if let Some(old) = self.consumer.take() {
                        let _ = self.graveyard.try_send(old);
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        self.last_latency_frames = latency_frames;
        let dc = device_channels.max(1);
        let frames = out.len() / dc;
        if !self.playing {
            out.fill(0.0);
            self.shared.publish(self.position, now_ns, false);
            return;
        }
        let want = frames * CHANNELS;
        if self.scratch.len() < want {
            // Only on the first buffer of a new, larger size.
            self.scratch.resize(want, 0.0);
        }
        let stereo = &mut self.scratch[..want];
        let got = match &mut self.consumer {
            Some(c) => {
                let n = c.slots().min(want) / CHANNELS * CHANNELS;
                if let Ok(chunk) = c.read_chunk(n) {
                    let (a, b) = chunk.as_slices();
                    stereo[..a.len()].copy_from_slice(a);
                    stereo[a.len()..a.len() + b.len()].copy_from_slice(b);
                    chunk.commit_all();
                }
                n
            }
            None => 0,
        };
        stereo[got..].fill(0.0);
        let missing = (want - got) / CHANNELS;
        if missing > 0 {
            self.shared
                .underrun_frames
                .fetch_add(missing as u64, Ordering::Relaxed);
        }
        for (i, frame) in out.chunks_exact_mut(dc).enumerate() {
            let (l, r) = (stereo[i * 2], stereo[i * 2 + 1]);
            if dc == 1 {
                frame[0] = 0.5 * (l + r);
            } else {
                frame[0] = l;
                frame[1] = r;
                frame[2..].fill(0.0);
            }
        }
        // The first frame of this buffer becomes audible after `latency`.
        self.shared
            .publish(self.position, now_ns + latency.as_nanos() as u64, true);
        self.position += frames as i64;
    }
}

enum FeederCmd {
    Play { from: i64 },
    Pause,
    Park { frame: i64 },
    SetSource(Box<dyn AudioSource>),
    Shutdown,
}

// The feeder thread owns its channels for its whole life.
#[allow(clippy::needless_pass_by_value)]
fn feeder(
    shared: Arc<Shared>,
    cmds: Receiver<FeederCmd>,
    control: Sender<Control>,
    graveyard: Receiver<rtrb::Consumer<f32>>,
) {
    let rate = shared.rate;
    let capacity = ((f64::from(rate) * RING_SECONDS) as usize).max(CHUNK_FRAMES * 4) * CHANNELS;
    let mut source: Box<dyn AudioSource> = Box::new(Silence);
    let mut producer: Option<rtrb::Producer<f32>> = None;
    let mut write_pos = 0i64;
    let mut seen_underruns = shared.underrun_frames.load(Ordering::Relaxed);
    let mut buf = vec![0f32; CHUNK_FRAMES * CHANNELS];

    let mut fill = |producer: &mut rtrb::Producer<f32>,
                    source: &mut Box<dyn AudioSource>,
                    write_pos: &mut i64| {
        while producer.slots() >= buf.len() {
            source.render(*write_pos, &mut buf);
            if let Ok(mut chunk) = producer.write_chunk(buf.len()) {
                let (a, b) = chunk.as_mut_slices();
                let split = a.len();
                a.copy_from_slice(&buf[..split]);
                b.copy_from_slice(&buf[split..]);
                chunk.commit_all();
            }
            *write_pos += CHUNK_FRAMES as i64;
        }
    };

    loop {
        // Free rings the callback has finished with (dropping allocates/frees).
        while graveyard.try_recv().is_ok() {}
        let wait = if producer.is_some() {
            Duration::from_millis(3)
        } else {
            Duration::from_millis(50)
        };
        match cmds.recv_timeout(wait) {
            Ok(FeederCmd::Play { from }) => {
                let (mut p, c) = rtrb::RingBuffer::new(capacity);
                write_pos = from;
                fill(&mut p, &mut source, &mut write_pos);
                seen_underruns = shared.underrun_frames.load(Ordering::Relaxed);
                let _ = control.send(Control::Start {
                    consumer: c,
                    frame: from,
                });
                producer = Some(p);
            }
            Ok(FeederCmd::Pause) => {
                producer = None;
                let _ = control.send(Control::Pause);
            }
            Ok(FeederCmd::Park { frame }) => {
                producer = None;
                let _ = control.send(Control::Park { frame });
            }
            Ok(FeederCmd::SetSource(s)) => source = s,
            Ok(FeederCmd::Shutdown) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
        if let Some(p) = &mut producer {
            // The callback output silence for frames we did not deliver:
            // skip them so the next audio lands at the right time.
            let under = shared.underrun_frames.load(Ordering::Relaxed);
            if under > seen_underruns {
                write_pos += (under - seen_underruns) as i64;
                seen_underruns = under;
            }
            fill(p, &mut source, &mut write_pos);
        }
    }
}

/// Which output to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputChoice {
    /// The system's default output device, or the null output if there is none.
    #[default]
    Default,
    /// No device: a timer drives playback (headless, tests, no sound card).
    Null,
}

/// What the engine ended up using.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputInfo {
    /// Device name, or `None` for the null output.
    pub device: Option<String>,
    pub sample_rate: u32,
    pub channels: usize,
}

enum Sink {
    Null {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    },
    #[cfg(feature = "audio-device")]
    Device(#[allow(dead_code)] cpal::Stream), // kept alive; dropping it stops output
    Manual,
}

/// Plays timeline audio and keeps the master clock.
pub struct AudioEngine {
    shared: Arc<Shared>,
    cmds: Sender<FeederCmd>,
    feeder: Option<JoinHandle<()>>,
    sink: Sink,
    output: OutputInfo,
    playing: bool,
}

impl std::fmt::Debug for AudioEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioEngine")
            .field("output", &self.output)
            .field("playing", &self.playing)
            .finish_non_exhaustive()
    }
}

fn parts(
    rate: u32,
) -> (
    Arc<Shared>,
    Sender<FeederCmd>,
    JoinHandle<()>,
    CallbackState,
) {
    let shared = Arc::new(Shared {
        rate,
        epoch: Instant::now(),
        seq: AtomicU64::new(0),
        anchor_frame: AtomicI64::new(0),
        anchor_ns: AtomicU64::new(0),
        playing: AtomicBool::new(false),
        underrun_frames: AtomicU64::new(0),
        high_water: AtomicI64::new(i64::MIN),
        seek_count: AtomicU64::new(0),
    });
    let (cmd_tx, cmd_rx) = unbounded();
    // Bounded (array-backed) so receiving on the real-time thread never
    // frees memory.
    let (ctl_tx, ctl_rx) = bounded(32);
    let (grave_tx, grave_rx) = bounded(16);
    let feeder_shared = shared.clone();
    let feeder = std::thread::Builder::new()
        .name("voxy-audio-feeder".into())
        .spawn(move || feeder(feeder_shared, cmd_rx, ctl_tx, grave_rx))
        .expect("spawn audio feeder");
    let cb = CallbackState {
        shared: shared.clone(),
        control: ctl_rx,
        graveyard: grave_tx,
        consumer: None,
        position: 0,
        playing: false,
        last_latency_frames: 0,
        scratch: Vec::with_capacity(8192 * CHANNELS),
    };
    (shared, cmd_tx, feeder, cb)
}

impl AudioEngine {
    /// Starts the engine on the chosen output, falling back to the null
    /// output if the device cannot be opened.
    pub fn start(choice: OutputChoice) -> Self {
        #[cfg(feature = "audio-device")]
        if choice == OutputChoice::Default {
            match Self::start_device() {
                Ok(engine) => return engine,
                Err(e) => tracing::warn!("no audio output ({e}); using the internal clock"),
            }
        }
        let _ = choice;
        Self::start_null(48_000)
    }

    /// The null output: a timer thread calls the real-time code every 10 ms.
    pub fn start_null(rate: u32) -> Self {
        let (shared, cmds, feeder, mut cb) = parts(rate);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let shared2 = shared.clone();
        let thread = std::thread::Builder::new()
            .name("voxy-audio-null".into())
            .spawn(move || {
                let period_frames = (rate / 100) as usize;
                let period =
                    Duration::from_nanos(period_frames as u64 * 1_000_000_000 / u64::from(rate));
                let mut buf = vec![0f32; period_frames * CHANNELS];
                let mut next = shared2.now_ns();
                while !stop2.load(Ordering::Relaxed) {
                    cb.process(&mut buf, CHANNELS, Duration::ZERO, next);
                    next += period.as_nanos() as u64;
                    let now = shared2.now_ns();
                    if next > now {
                        std::thread::sleep(Duration::from_nanos(next - now));
                    } else if now - next > 1_000_000_000 {
                        next = now; // fell far behind (suspended): don't spin
                    }
                }
            })
            .expect("spawn null audio output");
        AudioEngine {
            shared,
            cmds,
            feeder: Some(feeder),
            sink: Sink::Null {
                stop,
                thread: Some(thread),
            },
            output: OutputInfo {
                device: None,
                sample_rate: rate,
                channels: CHANNELS,
            },
            playing: false,
        }
    }

    /// An engine whose "device" is driven by the caller through the
    /// returned [`CallbackState`] (tests, offline rendering).
    pub fn manual(rate: u32) -> (Self, CallbackState) {
        let (shared, cmds, feeder, cb) = parts(rate);
        let engine = AudioEngine {
            shared,
            cmds,
            feeder: Some(feeder),
            sink: Sink::Manual,
            output: OutputInfo {
                device: None,
                sample_rate: rate,
                channels: CHANNELS,
            },
            playing: false,
        };
        (engine, cb)
    }

    #[cfg(feature = "audio-device")]
    fn start_device() -> Result<Self, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no default output device")?;
        let name = device
            .description()
            .map_or_else(|_| "default output".to_owned(), |d| d.name().to_owned());
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let rate = supported.sample_rate();
        let channels = usize::from(supported.channels());
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let (shared, cmds, feeder, mut cb) = parts(rate);
        let shared_cb = shared.clone();
        let err_fn = |e: cpal::Error| tracing::warn!("audio stream: {e}");
        let latency_of = |info: &cpal::OutputCallbackInfo| {
            let ts = info.timestamp();
            ts.playback.saturating_duration_since(ts.callback)
        };
        let stream = match format {
            cpal::SampleFormat::F32 => device.build_output_stream(
                config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    cb.process(data, channels, latency_of(info), shared_cb.now_ns());
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => {
                let mut f = Vec::<f32>::with_capacity(8192 * channels);
                device.build_output_stream(
                    config,
                    move |data: &mut [i16], info: &cpal::OutputCallbackInfo| {
                        if f.len() < data.len() {
                            f.resize(data.len(), 0.0);
                        }
                        let buf = &mut f[..data.len()];
                        cb.process(buf, channels, latency_of(info), shared_cb.now_ns());
                        for (d, s) in data.iter_mut().zip(buf.iter()) {
                            *d = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                        }
                    },
                    err_fn,
                    None,
                )
            }
            other => return Err(format!("unsupported device sample format {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        tracing::info!("audio output: {name}, {rate} Hz, {channels} ch");
        Ok(AudioEngine {
            shared,
            cmds,
            feeder: Some(feeder),
            sink: Sink::Device(stream),
            output: OutputInfo {
                device: Some(name),
                sample_rate: rate,
                channels,
            },
            playing: false,
        })
    }

    pub fn clock(&self) -> PlaybackClock {
        PlaybackClock {
            shared: self.shared.clone(),
        }
    }

    pub fn output(&self) -> &OutputInfo {
        &self.output
    }

    pub fn sample_rate(&self) -> u32 {
        self.shared.rate
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Replaces what is played (takes effect from the next rendered chunk).
    pub fn set_source(&self, source: Box<dyn AudioSource>) {
        let _ = self.cmds.send(FeederCmd::SetSource(source));
    }

    fn reset_monotonic(&self) {
        self.shared.seek_count.fetch_add(1, Ordering::Relaxed);
        self.shared.high_water.store(i64::MIN, Ordering::Release);
    }

    /// Starts playing from `from`.
    pub fn play(&mut self, from: Time) {
        self.reset_monotonic();
        self.playing = true;
        let _ = self.cmds.send(FeederCmd::Play {
            from: time_to_frames(from, self.shared.rate),
        });
    }

    /// Stops; the clock holds at what was audible.
    pub fn pause(&mut self) {
        self.playing = false;
        let _ = self.cmds.send(FeederCmd::Pause);
    }

    /// Jumps to `to`, keeping the play/pause state.
    pub fn seek(&mut self, to: Time) {
        if self.playing {
            self.play(to);
        } else {
            self.reset_monotonic();
            let _ = self.cmds.send(FeederCmd::Park {
                frame: time_to_frames(to, self.shared.rate),
            });
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let _ = self.cmds.send(FeederCmd::Shutdown);
        if let Sink::Null { stop, thread } = &mut self.sink {
            stop.store(true, Ordering::Relaxed);
            if let Some(t) = thread.take() {
                let _ = t.join();
            }
        }
        if let Some(f) = self.feeder.take() {
            let _ = f.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_time_conversions() {
        assert_eq!(frames_to_time(48_000, 48_000), Time::from_secs(1));
        assert_eq!(frames_to_time(1, 44_100), Time(16_000));
        assert_eq!(time_to_frames(Time::from_secs(2), 48_000), 96_000);
        assert_eq!(time_to_frames(Time(-1), 48_000), -1);
        for f in [0i64, 1, 47_999, 123_457] {
            assert_eq!(time_to_frames(frames_to_time(f, 48_000), 48_000), f);
        }
    }
}
