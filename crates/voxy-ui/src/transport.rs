//! Playback: the audio engine's master clock drives the playhead.
//!
//! The engine starts on first use (so the editor opens instantly and tests
//! never touch audio hardware). While playing, each UI frame asks the clock
//! what is audible and the frame scheduler which video frame belongs to
//! it; the playhead jumps to that frame. Scrubbing while playing seeks the
//! audio. Playback loops at the end of the composition.

use voxy_core::{FrameRate, Time};
use voxy_media::playback::{AudioEngine, OutputChoice};
use voxy_media::sync::{FrameScheduler, Present};

#[derive(Default)]
pub struct Transport {
    engine: Option<AudioEngine>,
    scheduler: Option<FrameScheduler>,
    /// The playhead time this transport last set; anything else means the
    /// user moved the playhead.
    last_set: Option<Time>,
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport")
            .field("playing", &self.is_playing())
            .field("output", &self.engine.as_ref().map(AudioEngine::output))
            .finish_non_exhaustive()
    }
}

impl Transport {
    pub fn is_playing(&self) -> bool {
        self.engine.as_ref().is_some_and(AudioEngine::is_playing)
    }

    fn engine(&mut self) -> &mut AudioEngine {
        self.engine
            .get_or_insert_with(|| AudioEngine::start(OutputChoice::Default))
    }

    pub fn play(&mut self, from: Time, rate: FrameRate) {
        let mut scheduler = FrameScheduler::new(rate);
        scheduler.reset();
        self.scheduler = Some(scheduler);
        self.last_set = Some(from);
        self.engine().play(from);
    }

    /// Stops; returns the frame-aligned time to leave the playhead at.
    pub fn pause(&mut self, rate: FrameRate) -> Option<Time> {
        let engine = self.engine.as_mut()?;
        if !engine.is_playing() {
            return None;
        }
        let heard = engine.clock().time();
        engine.pause();
        self.last_set = None;
        Some(rate.frame_to_time(rate.time_to_frame(heard)))
    }

    pub fn toggle(&mut self, time: Time, rate: FrameRate) -> Option<Time> {
        if self.is_playing() {
            self.pause(rate)
        } else {
            self.play(time, rate);
            None
        }
    }

    /// Advances the playhead from the clock. Call once per UI frame with the
    /// current playhead; returns the new one while playing.
    pub fn tick(&mut self, playhead: Time, duration: Time, rate: FrameRate) -> Option<Time> {
        if !self.is_playing() {
            return None;
        }
        // The user moved the playhead (scrubbing, keyframe navigation):
        // follow them.
        if self.last_set.is_some_and(|t| t != playhead) {
            self.play(playhead, rate);
            return Some(playhead);
        }
        let engine = self.engine.as_mut()?;
        let clock = engine.clock().time();
        if clock >= duration && duration > Time::ZERO {
            // Loop.
            self.play(Time::ZERO, rate);
            return Some(Time::ZERO);
        }
        let scheduler = self
            .scheduler
            .get_or_insert_with(|| FrameScheduler::new(rate));
        // Frames are rendered on demand, so the target is always ready.
        let target = scheduler.target(clock);
        let t = match scheduler.present(clock, Some(target)) {
            Present::Frame(f) => rate.frame_to_time(f),
            Present::Nothing => return None,
        };
        self.last_set = Some(t);
        Some(t)
    }

    /// For the status bar.
    pub fn output_label(&self) -> Option<String> {
        let out = self.engine.as_ref()?.output();
        Some(match &out.device {
            Some(name) => format!("Audio: {name}, {} kHz", f64::from(out.sample_rate) / 1000.0),
            None => "Audio: no device (internal clock)".to_owned(),
        })
    }
}
