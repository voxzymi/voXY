//! Integer time.
//!
//! All document times are stored in *flicks*: 1/705 600 000 of a second. Every
//! common video frame rate (23.976, 24, 25, 29.97, 30, 48, 50, 59.94, 60, 120)
//! and audio sample rate (8 kHz to 192 kHz) divides a second of flicks exactly,
//! so converting between frames and time never accumulates error.

use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// Flicks in one second.
pub const FLICKS_PER_SECOND: i64 = 705_600_000;

/// A point in time (or a duration) in flicks.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Time(pub i64);

impl Time {
    pub const ZERO: Time = Time(0);
    pub const MAX: Time = Time(i64::MAX);

    pub const fn from_flicks(flicks: i64) -> Self {
        Time(flicks)
    }

    pub const fn flicks(self) -> i64 {
        self.0
    }

    /// Whole seconds, exact.
    pub const fn from_secs(secs: i64) -> Self {
        Time(secs * FLICKS_PER_SECOND)
    }

    /// Milliseconds, exact (705 600 flicks per millisecond).
    pub const fn from_millis(ms: i64) -> Self {
        Time(ms * (FLICKS_PER_SECOND / 1000))
    }

    /// Seconds as `f64`, rounded to the nearest flick.
    pub fn from_secs_f64(secs: f64) -> Self {
        Time((secs * FLICKS_PER_SECOND as f64).round() as i64)
    }

    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / FLICKS_PER_SECOND as f64
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, rhs: Time) -> Time {
        Time(self.0 + rhs.0)
    }
}

impl AddAssign for Time {
    fn add_assign(&mut self, rhs: Time) {
        self.0 += rhs.0;
    }
}

impl Sub for Time {
    type Output = Time;
    fn sub(self, rhs: Time) -> Time {
        Time(self.0 - rhs.0)
    }
}

impl SubAssign for Time {
    fn sub_assign(&mut self, rhs: Time) {
        self.0 -= rhs.0;
    }
}

impl Neg for Time {
    type Output = Time;
    fn neg(self) -> Time {
        Time(-self.0)
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.6}s", self.as_secs_f64())
    }
}

/// A rational frame rate, `num / den` frames per second (e.g. 30000/1001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "FrameRateRepr", into = "FrameRateRepr")]
pub struct FrameRate {
    num: u32,
    den: u32,
}

impl FrameRate {
    pub const FPS_23_976: FrameRate = FrameRate {
        num: 24_000,
        den: 1001,
    };
    pub const FPS_24: FrameRate = FrameRate { num: 24, den: 1 };
    pub const FPS_25: FrameRate = FrameRate { num: 25, den: 1 };
    pub const FPS_29_97: FrameRate = FrameRate {
        num: 30_000,
        den: 1001,
    };
    pub const FPS_30: FrameRate = FrameRate { num: 30, den: 1 };
    pub const FPS_50: FrameRate = FrameRate { num: 50, den: 1 };
    pub const FPS_59_94: FrameRate = FrameRate {
        num: 60_000,
        den: 1001,
    };
    pub const FPS_60: FrameRate = FrameRate { num: 60, den: 1 };

    /// Returns `None` unless `num` and `den` are non-zero and the rate is below
    /// one frame per flick.
    pub const fn new(num: u32, den: u32) -> Option<Self> {
        if num == 0 || den == 0 || (num as i64) >= FLICKS_PER_SECOND * den as i64 {
            None
        } else {
            Some(FrameRate { num, den })
        }
    }

    pub const fn num(self) -> u32 {
        self.num
    }

    pub const fn den(self) -> u32 {
        self.den
    }

    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    /// The start time of `frame`: the first flick that belongs to it.
    ///
    /// Rounds up, so `time_to_frame(frame_to_time(f)) == f` holds for every
    /// rate, including those that do not divide a second of flicks exactly.
    pub fn frame_to_time(self, frame: i64) -> Time {
        let n = i128::from(frame) * i128::from(FLICKS_PER_SECOND) * i128::from(self.den);
        let d = i128::from(self.num);
        Time(div_ceil_i128(n, d) as i64)
    }

    /// The frame that contains `time` (floor).
    pub fn time_to_frame(self, time: Time) -> i64 {
        let n = i128::from(time.0) * i128::from(self.num);
        let d = i128::from(FLICKS_PER_SECOND) * i128::from(self.den);
        n.div_euclid(d) as i64
    }

    /// Duration of one frame if it is a whole number of flicks.
    pub fn exact_frame_duration(self) -> Option<Time> {
        let n = FLICKS_PER_SECOND * i64::from(self.den);
        let d = i64::from(self.num);
        (n % d == 0).then_some(Time(n / d))
    }
}

impl fmt::Display for FrameRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{} fps", self.num)
        } else {
            write!(f, "{:.3} fps", self.as_f64())
        }
    }
}

fn div_ceil_i128(n: i128, d: i128) -> i128 {
    let q = n.div_euclid(d);
    if n.rem_euclid(d) == 0 { q } else { q + 1 }
}

#[derive(Serialize, Deserialize)]
struct FrameRateRepr {
    num: u32,
    den: u32,
}

impl TryFrom<FrameRateRepr> for FrameRate {
    type Error = String;
    fn try_from(r: FrameRateRepr) -> Result<Self, String> {
        FrameRate::new(r.num, r.den)
            .ok_or_else(|| format!("invalid frame rate {}/{}", r.num, r.den))
    }
}

impl From<FrameRate> for FrameRateRepr {
    fn from(r: FrameRate) -> Self {
        FrameRateRepr {
            num: r.num,
            den: r.den,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn common_rates_are_exact() {
        for rate in [
            FrameRate::FPS_23_976,
            FrameRate::FPS_24,
            FrameRate::FPS_25,
            FrameRate::FPS_29_97,
            FrameRate::FPS_30,
            FrameRate::FPS_50,
            FrameRate::FPS_59_94,
            FrameRate::FPS_60,
            FrameRate::new(120, 1).unwrap(),
        ] {
            assert!(rate.exact_frame_duration().is_some(), "{rate}");
        }
        assert_eq!(
            FrameRate::FPS_29_97.exact_frame_duration(),
            Some(Time(23_543_520))
        );
        for hz in [8_000, 22_050, 44_100, 48_000, 96_000, 192_000] {
            assert_eq!(FLICKS_PER_SECOND % hz, 0, "{hz} Hz");
        }
    }

    #[test]
    fn frame_conversions() {
        let r = FrameRate::FPS_30;
        assert_eq!(r.frame_to_time(30), Time::from_secs(1));
        assert_eq!(r.time_to_frame(Time::from_secs(1)), 30);
        assert_eq!(r.time_to_frame(Time::from_secs(1) - Time(1)), 29);
        assert_eq!(r.time_to_frame(Time(-1)), -1);
        assert_eq!(Time::from_millis(1).flicks(), 705_600);
    }

    #[test]
    fn invalid_rates_rejected() {
        assert!(FrameRate::new(0, 1).is_none());
        assert!(FrameRate::new(1, 0).is_none());
        assert!(serde_json::from_str::<FrameRate>(r#"{"num":0,"den":1}"#).is_err());
    }

    proptest! {
        #[test]
        fn frame_round_trip(num in 1u32..100_000, den in 1u32..10_000, frame in -1_000_000i64..1_000_000) {
            let rate = FrameRate::new(num, den).unwrap();
            prop_assert_eq!(rate.time_to_frame(rate.frame_to_time(frame)), frame);
            // The flick before a frame's start belongs to the previous frame.
            prop_assert_eq!(rate.time_to_frame(rate.frame_to_time(frame) - Time(1)), frame - 1);
        }
    }
}
