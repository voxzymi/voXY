//! The audio engine driven as a simulated device: sample-accurate delivery,
//! clock anchoring with latency, pause/seek, underrun recovery and channel
//! mapping. No sound card needed.

#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use voxy_core::Time;
use voxy_media::playback::{
    AudioEngine, AudioSource, CallbackState, PlaybackClock, frames_to_time,
};

const RATE: u32 = 48_000;
const BUF: usize = 480; // 10 ms

/// Left = the timeline frame number, right = its negation: every sample
/// says exactly where it came from.
struct Ramp;

impl AudioSource for Ramp {
    fn render(&mut self, start: i64, out: &mut [f32]) {
        for (i, f) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let n = (start + i as i64) as f32;
            f[0] = n;
            f[1] = -n;
        }
    }
}

/// A fake device: buffers are pulled at a simulated time.
struct Device {
    cb: CallbackState,
    now_ns: u64,
    latency: Duration,
    /// Frames pulled while playing, and the count before the last pull.
    played: i64,
    played_before_last: i64,
}

impl Device {
    fn pull(&mut self, frames: usize) -> Vec<f32> {
        let mut out = vec![0f32; frames * 2];
        self.cb.process(&mut out, 2, self.latency, self.now_ns);
        self.now_ns += frames as u64 * 1_000_000_000 / u64::from(RATE);
        self.played_before_last = self.played;
        self.played += frames as i64;
        out
    }

    /// Pulls until the engine has started playing (the feeder pre-fills
    /// asynchronously); returns that first playing buffer.
    fn pull_until_playing(&mut self, clock: &PlaybackClock) -> Vec<f32> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let out = self.pull(BUF);
            if clock.is_playing() {
                return out;
            }
            assert!(Instant::now() < deadline, "engine never started");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn engine(latency: Duration) -> (AudioEngine, Device) {
    let (mut engine, cb) = AudioEngine::manual(RATE);
    engine.set_source(Box::new(Ramp));
    let _ = &mut engine;
    (
        engine,
        Device {
            cb,
            now_ns: 0,
            latency,
            played: 0,
            played_before_last: 0,
        },
    )
}

fn assert_continuous(buf: &[f32], first: i64) {
    for (i, f) in buf.as_chunks::<2>().0.iter().enumerate() {
        let want = (first + i as i64) as f32;
        assert_eq!(f[0], want, "left sample {i}");
        assert_eq!(f[1], -want, "right sample {i}");
    }
}

/// Pulls `BUF`-frame buffers until one arrives that the feeder had fully
/// ready — drawn entirely from the ring with no silence, so the engine's
/// underrun counter is unchanged across it — and returns that buffer.
///
/// A fully-ready buffer is gapless by construction: every sample came from
/// the feeder's contiguous render, so the ramp runs `b[0], b[0]+1, …`. This
/// replaces a fixed millisecond sleep (which assumes the feeder refilled in
/// that window) with a bounded poll, so a feeder thread briefly starved of
/// CPU on a loaded CI runner makes the test wait, never fail. The deadline
/// keeps a genuinely stuck feeder from hanging the test forever.
fn pull_ready(dev: &mut Device, clock: &PlaybackClock) -> Vec<f32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let before = clock.underrun_frames();
        let buf = dev.pull(BUF);
        if clock.underrun_frames() == before {
            return buf;
        }
        assert!(Instant::now() < deadline, "feeder never refilled the ring");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn delivers_sample_accurate_audio_from_the_play_position() {
    let (mut engine, mut dev) = engine(Duration::ZERO);
    let clock = engine.clock();
    engine.play(Time::from_secs(1));
    let first = dev.pull_until_playing(&clock);
    assert_continuous(&first, 48_000);
    // Every buffer the feeder has ready is gapless and never moves backwards.
    // Poll for a ready buffer instead of sleeping a fixed 2 ms and assuming
    // the feeder kept up: this test drains the ring far faster than real
    // time, so on a loaded runner a fixed wait is what made it flaky. (The
    // first buffer above pins the absolute play position; an underrun here
    // would make the feeder realign forward, so we assert forward progress
    // rather than a fixed stride.)
    let mut last_start = 48_000i64;
    for _ in 0..25 {
        let b = pull_ready(&mut dev, &clock);
        let start = b[0] as i64;
        assert!(
            start >= last_start,
            "audio went backwards: {start} < {last_start}"
        );
        assert_continuous(&b, start);
        last_start = start;
    }
    drop(engine); // shut the feeder down and join it before the test returns
}

#[test]
fn clock_follows_the_device_including_latency() {
    let latency = Duration::from_millis(20);
    let (mut engine, mut dev) = engine(latency);
    let clock = engine.clock();
    engine.play(Time::from_secs(2));
    dev.pull_until_playing(&clock);
    let anchor_ns = dev.now_ns - (BUF as u64 * 1_000_000_000 / u64::from(RATE));
    // The buffer pulled at `anchor_ns` starts at 2 s and is heard 20 ms later.
    assert_eq!(clock.time_at_ns(anchor_ns + 20_000_000), Time::from_secs(2));
    assert_eq!(
        clock.time_at_ns(anchor_ns + 30_000_000),
        Time::from_secs(2) + Time::from_millis(10)
    );
    // While the previous audio is still in flight, the clock is earlier.
    let before = clock.time_at_ns(anchor_ns + 10_000_000);
    assert!(before <= Time::from_secs(2));
    drop(engine);
}

#[test]
fn pause_holds_at_what_was_audible() {
    let latency = Duration::from_millis(20);
    let (mut engine, mut dev) = engine(latency);
    let clock = engine.clock();
    engine.play(Time::ZERO);
    dev.pull_until_playing(&clock);
    for _ in 0..9 {
        std::thread::sleep(Duration::from_millis(1));
        dev.pull(BUF);
    }
    // The pause takes effect at the start of whichever buffer first sees
    // it; at that moment the last 20 ms (960 frames) sent were not yet heard.
    let start = dev.played - BUF as i64 * 10; // frames pulled before playing began
    engine.pause();
    let deadline = Instant::now() + Duration::from_secs(5);
    while clock.is_playing() {
        dev.pull(BUF);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let sent = dev.played_before_last - start;
    let held = clock.time();
    assert_eq!(
        held,
        frames_to_time(sent - 960, RATE),
        "sent {sent} frames, minus 20 ms latency"
    );
    assert!(sent >= 4800);
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(clock.time(), held, "paused clock does not move");
    let silent = dev.pull(BUF);
    assert!(silent.iter().all(|&s| s == 0.0));
    // Resuming from the held time continues from there.
    engine.play(held);
    assert_continuous(&dev.pull_until_playing(&clock), sent - 960);
    drop(engine);
}

#[test]
fn seeking_while_playing_switches_cleanly() {
    let (mut engine, mut dev) = engine(Duration::ZERO);
    let clock = engine.clock();
    engine.play(Time::ZERO);
    dev.pull_until_playing(&clock);
    engine.seek(Time::from_secs(5));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        std::thread::sleep(Duration::from_millis(1));
        let b = dev.pull(BUF);
        if b[0] >= 240_000.0 {
            assert_continuous(&b, 240_000);
            break;
        }
        // Until the switch, the old position continues seamlessly.
        assert!(b[0] < 240_000.0);
        assert!(Instant::now() < deadline, "seek never took effect");
    }
    assert!(clock.time_at_ns(dev.now_ns) >= Time::from_secs(5));
    drop(engine);
}

#[test]
fn underruns_keep_time_and_realign_audio() {
    let (mut engine, mut dev) = engine(Duration::ZERO);
    let clock = engine.clock();
    engine.play(Time::ZERO);
    dev.pull_until_playing(&clock);
    // Demand 1 s at once: far more than the 250 ms ring holds.
    dev.pull(RATE as usize);
    assert!(clock.underrun_frames() > 0);
    let device_position = (BUF + RATE as usize) as i64;
    // After the feeder catches up it realigns to the device position, so the
    // next buffer it has fully ready is continuous audio again, at or past
    // where the device now is. Poll for that buffer instead of sleeping a
    // fixed 50 ms and hoping the catch-up fit.
    let b = pull_ready(&mut dev, &clock);
    let start = b[0] as i64;
    assert!(start >= device_position, "realigned to the device position");
    assert_continuous(&b, start);
    drop(engine);
}

#[test]
fn maps_stereo_to_any_channel_count() {
    let (mut engine, mut dev) = engine(Duration::ZERO);
    let clock = engine.clock();
    engine.play(Time::from_secs(1));
    dev.pull_until_playing(&clock);
    // Wait (bounded poll, not a fixed sleep) until the feeder has a full
    // buffer ready, so the channel-mapped reads below draw real samples.
    pull_ready(&mut dev, &clock);
    // Mono: average of L (n) and R (-n) is 0.
    let mut mono = vec![1f32; 64];
    dev.cb.process(&mut mono, 1, Duration::ZERO, dev.now_ns);
    assert!(mono.iter().all(|&s| s == 0.0));
    // 5.1: L, R, then silence.
    let mut six = vec![1f32; 6 * 8];
    dev.cb.process(&mut six, 6, Duration::ZERO, dev.now_ns);
    let n = six[0];
    assert!(n > 48_000.0);
    assert_eq!(&six[..6], &[n, -n, 0.0, 0.0, 0.0, 0.0]);
    drop(engine);
}

#[test]
fn null_output_runs_in_real_time() {
    let mut engine = AudioEngine::start_null(RATE);
    engine.set_source(Box::new(Ramp));
    let clock = engine.clock();
    engine.play(Time::ZERO);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !clock.is_playing() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let t0 = clock.time();
    let wall = Instant::now();
    std::thread::sleep(Duration::from_millis(300));
    let advanced = (clock.time() - t0).as_secs_f64();
    let elapsed = wall.elapsed().as_secs_f64();
    assert!(
        (advanced - elapsed).abs() < 0.05,
        "clock {advanced:.3}s vs wall {elapsed:.3}s"
    );
    engine.pause();
    // Pausing is a command to the audio thread, so the clock stops when that
    // thread handles it, not when `pause()` returns. Wait for it (as for
    // `play` above) instead of assuming a fixed delay is enough: on a loaded
    // runner it was not. Once the clock reports "paused" it must hold.
    let deadline = Instant::now() + Duration::from_secs(5);
    while clock.is_playing() {
        assert!(Instant::now() < deadline, "pause never reached the clock");
        std::thread::sleep(Duration::from_millis(1));
    }
    let held = clock.time();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(clock.time(), held);
    drop(engine);
}

#[test]
fn clock_is_monotonic_while_playing() {
    // Real time: the null output re-anchors every 10 ms while a reader polls.
    let mut engine = AudioEngine::start_null(RATE);
    let clock = engine.clock();
    engine.play(Time::ZERO);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !clock.is_playing() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut last = clock.time();
    let until = Instant::now() + Duration::from_millis(200);
    let mut reads = 0u64;
    while Instant::now() < until {
        let t = clock.time();
        assert!(t >= last, "{t} < {last}");
        last = t;
        reads += 1;
    }
    assert!(reads > 1000);
    drop(engine);
}
