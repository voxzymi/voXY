//! A/V sync: which video frame to show for the master clock.
//!
//! The audio clock ([`crate::playback::PlaybackClock`]) is the master. Each
//! display refresh, [`FrameScheduler::present`] is given the clock time and
//! the newest frame the video pipeline has ready, and decides what to show:
//!
//! - the frame the clock is in, if it is ready;
//! - otherwise the newest ready frame (video is late: frames are *dropped*
//!   so video catches up instead of drifting behind);
//! - never a frame *after* the clock (video must not run ahead of audio).
//!
//! Showing late frames is only half of sync: a decoder that keeps decoding
//! every frame in order falls further behind forever. The pipeline should
//! ask [`FrameScheduler::request`] which frame to decode next. That is the
//! frame due when the decode will *finish*, so frames are skipped at the
//! source and lag stays bounded by one decode.
//!
//! An optional offset shifts video against audio (positive = video later),
//! for displays or devices with their own latency.

use voxy_core::{FrameRate, Time};

/// Counters for diagnostics and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Distinct frames shown.
    pub shown: u64,
    /// Frames skipped because video was late.
    pub dropped: u64,
    /// Refreshes that had to keep showing an older frame than the clock wants.
    pub late_refreshes: u64,
    /// Worst lateness seen, in frames.
    pub max_late_frames: i64,
}

/// What to put on screen this refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Present {
    /// Show (or keep showing) this frame.
    Frame(i64),
    /// Nothing is ready yet.
    Nothing,
}

#[derive(Debug, Clone)]
pub struct FrameScheduler {
    rate: FrameRate,
    offset: Time,
    last: Option<i64>,
    stats: SyncStats,
}

impl FrameScheduler {
    pub fn new(rate: FrameRate) -> Self {
        FrameScheduler {
            rate,
            offset: Time::ZERO,
            last: None,
            stats: SyncStats::default(),
        }
    }

    /// Delays video by `offset` relative to audio (negative = earlier).
    pub fn set_offset(&mut self, offset: Time) {
        self.offset = offset;
    }

    /// Forget the last frame (after a seek).
    pub fn reset(&mut self) {
        self.last = None;
    }

    pub fn stats(&self) -> SyncStats {
        self.stats
    }

    /// The frame the clock is in (frame 0 before the start).
    pub fn target(&self, clock: Time) -> i64 {
        self.rate.time_to_frame(clock - self.offset).max(0)
    }

    /// The frame to decode next, given how long a decode takes: the frame
    /// that will be due when it is ready, and never one already shown.
    pub fn request(&self, clock: Time, decode_time: Time) -> i64 {
        let due = self.target(clock + decode_time);
        self.last.map_or(due, |l| due.max(l + 1))
    }

    /// Decides what to show. `ready` is the newest frame the decoder has
    /// finished (frames up to it are assumed available).
    pub fn present(&mut self, clock: Time, ready: Option<i64>) -> Present {
        let target = self.target(clock);
        let Some(ready) = ready else {
            return self.last.map_or(Present::Nothing, Present::Frame);
        };
        // Never ahead of the clock; never backwards within a run.
        let mut show = ready.min(target);
        if let Some(last) = self.last {
            show = show.max(last);
        }
        if show < target {
            self.stats.late_refreshes += 1;
            self.stats.max_late_frames = self.stats.max_late_frames.max(target - show);
        }
        if self.last != Some(show) {
            if let Some(last) = self.last {
                self.stats.dropped += (show - last - 1).max(0) as u64;
            }
            self.stats.shown += 1;
            self.last = Some(show);
        }
        Present::Frame(show)
    }

    /// How far the shown frame's start is from the clock (positive: video
    /// behind audio).
    pub fn drift(&self, clock: Time) -> Option<Time> {
        self.last
            .map(|f| clock - self.offset - self.rate.frame_to_time(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: FrameRate = FrameRate::FPS_30;

    /// Simulates a 60 Hz display against a clock. The decoder takes
    /// `decode_secs` per frame (`None` = instant, always ahead) and decodes
    /// whatever the scheduler requests.
    fn simulate(decode_secs: Option<f64>, seconds: f64) -> (FrameScheduler, Vec<i64>) {
        let mut s = FrameScheduler::new(RATE);
        let mut shown = Vec::new();
        let mut ready: Option<i64> = None;
        let mut in_flight: Option<(i64, f64)> = None; // (frame, done at)
        let refreshes = (seconds * 60.0) as i64;
        for r in 0..refreshes {
            let now = r as f64 / 60.0;
            let clock = Time::from_secs_f64(now);
            match decode_secs {
                None => ready = Some(s.target(clock) + 3),
                Some(d) => {
                    if let Some((f, done)) = in_flight
                        && now >= done
                    {
                        ready = Some(f);
                        in_flight = None;
                    }
                    if in_flight.is_none() {
                        let f = s.request(clock, Time::from_secs_f64(d));
                        in_flight = Some((f, now + d));
                    }
                }
            }
            if let Present::Frame(f) = s.present(clock, ready) {
                shown.push(f);
                // Invariant: never ahead of the clock.
                assert!(RATE.frame_to_time(f) <= clock, "frame {f} ahead of {clock}");
            }
        }
        (s, shown)
    }

    #[test]
    fn fast_decoder_shows_every_frame_exactly_on_time() {
        let (s, shown) = simulate(None, 2.0);
        let mut distinct = shown.clone();
        distinct.dedup();
        assert_eq!(distinct, (0..60).collect::<Vec<_>>());
        assert_eq!(s.stats().dropped, 0);
        assert_eq!(s.stats().late_refreshes, 0);
        // 60 Hz display, 30 fps video: each frame on screen for 2 refreshes.
        assert_eq!(shown.len(), 120);
    }

    #[test]
    fn slow_decoder_drops_frames_and_never_runs_ahead() {
        // 20 frames per second of decoding for 30 fps video.
        let (s, shown) = simulate(Some(0.05), 10.0);
        assert!(shown.windows(2).all(|w| w[0] <= w[1]), "monotonic");
        let st = s.stats();
        assert!(
            st.dropped > 50,
            "about a third of the frames are skipped: {st:?}"
        );
        // Lag stays bounded by one decode (1.5 frames), not accumulating.
        assert!(st.max_late_frames <= 2, "{st:?}");
        // Still current at the end.
        assert!(*shown.last().unwrap() >= 297, "{:?}", shown.last());
    }

    #[test]
    fn nothing_before_the_first_frame_and_holding_last() {
        let mut s = FrameScheduler::new(RATE);
        assert_eq!(s.present(Time::ZERO, None), Present::Nothing);
        assert_eq!(s.present(Time::from_millis(40), Some(1)), Present::Frame(1));
        assert_eq!(s.present(Time::from_millis(80), None), Present::Frame(1));
    }

    #[test]
    fn offset_and_drift() {
        let mut s = FrameScheduler::new(RATE);
        s.set_offset(Time::from_millis(100)); // video 100 ms later
        // Clock 150 ms → video time 50 ms → frame 1 (33.3–66.7 ms).
        assert_eq!(
            s.present(Time::from_millis(150), Some(100)),
            Present::Frame(1)
        );
        assert_eq!(
            s.drift(Time::from_millis(150)).unwrap(),
            Time::from_millis(50) - RATE.frame_to_time(1)
        );
        // Before the delayed video starts, its first frame is held.
        s.reset();
        assert_eq!(s.present(Time::ZERO, Some(5)), Present::Frame(0));
    }
}
