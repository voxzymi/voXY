//! Converting between container timestamps and flicks.
//!
//! Streams count time in ticks of a rational *time base* (`num/den`
//! seconds). voXY counts in flicks (1/705 600 000 s). Every common time base
//! divides a second of flicks exactly (1/90 000, 1/12 800, 1/1000, 1/48 000,
//! 1/44 100, 1/30 000, 1/24 000, …), so conversion is usually exact. Some do
//! not (FFmpeg's 1/15 360, common for 60 fps MP4 and WebM). For those:
//!
//! - [`TimeBase::to_time`] rounds to the nearest flick, and
//! - [`TimeBase::to_pts_floor`] returns the **last tick whose time is at or
//!   before** a given time.
//!
//! Together these guarantee `to_pts_floor(to_time(pts)) == pts` for every
//! time base, which is what frame-accurate seeking relies on: the time a
//! frame reports always seeks back to that same frame.

use voxy_core::{FLICKS_PER_SECOND, Time};

/// A stream time base: one tick is `num / den` seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeBase {
    num: i64,
    den: i64,
}

impl TimeBase {
    /// `None` unless both parts are positive.
    pub fn new(num: i64, den: i64) -> Option<Self> {
        (num > 0 && den > 0).then_some(TimeBase { num, den })
    }

    pub fn num(self) -> i64 {
        self.num
    }

    pub fn den(self) -> i64 {
        self.den
    }

    /// Whether every tick is a whole number of flicks.
    pub fn is_exact(self) -> bool {
        (i128::from(FLICKS_PER_SECOND) * i128::from(self.num)) % i128::from(self.den) == 0
    }

    /// Ticks → flicks, rounded to the nearest flick (half away from zero).
    pub fn to_time(self, pts: i64) -> Time {
        let n = i128::from(pts) * i128::from(self.num) * i128::from(FLICKS_PER_SECOND);
        let d = i128::from(self.den);
        let q = n.div_euclid(d);
        let r = n.rem_euclid(d);
        let rounded = if 2 * r >= d { q + 1 } else { q };
        Time(clamp_i64(rounded))
    }

    /// The last tick whose [`TimeBase::to_time`] is at or before `time`.
    pub fn to_pts_floor(self, time: Time) -> i64 {
        let n = i128::from(time.0) * i128::from(self.den);
        let d = i128::from(self.num) * i128::from(FLICKS_PER_SECOND);
        let mut p = clamp_i64(n.div_euclid(d));
        // Rounding in `to_time` can put the time of p + 1 at or before
        // `time` (or that of p after it); step to the right tick.
        while self.to_time(p.saturating_add(1)) <= time && p < i64::MAX {
            p += 1;
        }
        while self.to_time(p) > time && p > i64::MIN {
            p -= 1;
        }
        p
    }

    /// The duration of `ticks` ticks.
    pub fn duration(self, ticks: i64) -> Time {
        self.to_time(ticks)
    }
}

fn clamp_i64(v: i128) -> i64 {
    v.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn common_time_bases_are_exact() {
        for den in [
            1000, 12_800, 24_000, 25, 30_000, 44_100, 48_000, 60_000, 90_000,
        ] {
            assert!(TimeBase::new(1, den).unwrap().is_exact(), "1/{den}");
        }
        assert!(TimeBase::new(1001, 30_000).unwrap().is_exact());
        // These do not divide a second of flicks; they rely on rounding.
        assert!(!TimeBase::new(1, 15_360).unwrap().is_exact()); // FFmpeg's usual 60 fps MP4/WebM base
        assert!(!TimeBase::new(1, 1_000_000).unwrap().is_exact()); // FFmpeg's AV_TIME_BASE (µs)
        assert!(!TimeBase::new(1, 7_000_000).unwrap().is_exact());
    }

    #[test]
    fn known_conversions() {
        let tb = TimeBase::new(1, 90_000).unwrap();
        assert_eq!(tb.to_time(90_000), Time::from_secs(1));
        assert_eq!(tb.to_time(3003), Time(23_543_520)); // one 29.97 fps frame
        assert_eq!(tb.to_pts_floor(Time::from_secs(1)), 90_000);
        assert_eq!(tb.to_pts_floor(Time::from_secs(1) - Time(1)), 89_999);
        assert_eq!(tb.to_time(-90_000), Time::from_secs(-1));
        assert!(TimeBase::new(0, 1).is_none());
        assert!(TimeBase::new(1, -5).is_none());
    }

    proptest! {
        #[test]
        fn pts_round_trip_for_any_time_base(num in 1i64..10_000, den in 1i64..10_000_000, pts in -10_000_000_000i64..10_000_000_000) {
            // Only times flicks can represent (±292 years).
            prop_assume!((pts.unsigned_abs() as f64) * (num as f64) / (den as f64) < 1.0e10);
            let tb = TimeBase::new(num, den).unwrap();
            prop_assert_eq!(tb.to_pts_floor(tb.to_time(pts)), pts);
        }

        #[test]
        fn floor_is_the_last_tick_at_or_before(num in 1i64..1000, den in 1i64..1_000_000, t in -1_000_000_000_000i64..1_000_000_000_000) {
            let tb = TimeBase::new(num, den).unwrap();
            let p = tb.to_pts_floor(Time(t));
            prop_assert!(tb.to_time(p) <= Time(t));
            prop_assert!(tb.to_time(p + 1) > Time(t));
        }
    }
}
