//! Timecode formatting and parsing, with drop-frame support.
//!
//! Timecode is a display of a frame count: `H:MM:SS:FF`, or `H:MM:SS;FF`
//! for drop-frame. Nothing here moves layers or keyframes; times stay in
//! flicks and these functions only convert to and from frame numbers.
//!
//! Drop-frame skips frame *numbers* (not frames): 2 at the start of every
//! minute except each tenth for 29.97 fps, 4 for 59.94 fps, so the display
//! stays within a frame of the wall clock.
//!
//! # Parsing rules ([`parse`])
//!
//! 1. Leading and trailing spaces are ignored; a leading `-` makes the
//!    result negative (a start timecode may be before zero).
//! 2. If the text has separators (`:`, `;` or `.`), the fields are read from
//!    the right: frames, seconds, minutes, hours. Missing left fields are 0.
//!    A field may exceed its range and carries: `0:90:00` at base 30 is 90
//!    seconds (and `0:90` is 90 frames). `1.5` is 1 second and 5 frames.
//! 3. If the text is only digits, they are split into pairs from the right
//!    as frames, seconds, minutes, hours: `30` is 30 frames, `130` is
//!    1 second 30 frames, `1000` is 10 seconds, `10000` is 1 minute.
//! 4. At most 8 digits in total. Anything else is an error.
//!
//! In drop-frame mode the parsed fields are taken as a drop-frame timecode,
//! so `1:00;02` is the first frame that exists in minute 1.

use std::fmt;

use crate::time::FrameRate;

/// Why a timecode could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    /// A field was not a whole number.
    NotANumber,
    /// More than 8 digits, or more than four fields.
    TooLong,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParseError::Empty => "enter a time",
            ParseError::NotANumber => "use numbers like 30, 1:15 or 0:00:10:00",
            ParseError::TooLong => "that time has too many digits",
        })
    }
}

impl std::error::Error for ParseError {}

/// Frames dropped per minute: 2 at 29.97, 4 at 59.94, 0 otherwise.
fn drop_per_minute(rate: FrameRate, drop: bool) -> i64 {
    if drop && rate.supports_drop_frame() {
        i64::from(rate.timecode_base() / 15)
    } else {
        0
    }
}

/// Frame number to `(hours, minutes, seconds, frames)` on the timecode
/// display. Negative frames count down from zero (the sign is separate).
fn split(frames: i64, rate: FrameRate, drop: bool) -> (i64, i64, i64, i64) {
    let base = i64::from(rate.timecode_base());
    let d = drop_per_minute(rate, drop);
    let mut n = frames;
    if d > 0 {
        let per_min = base * 60 - d;
        let per_10 = per_min * 10 + d;
        let (tens, rem) = (n.div_euclid(per_10), n.rem_euclid(per_10));
        n += 9 * d * tens;
        if rem >= d {
            n += d * ((rem - d) / per_min);
        }
    }
    (
        n / (base * 3600),
        (n / (base * 60)) % 60,
        (n / base) % 60,
        n % base,
    )
}

/// `H:MM:SS:FF` (`;` before the frames in drop-frame) for a frame number.
/// `drop` is ignored for rates that do not support it.
pub fn format_frames(frames: i64, rate: FrameRate, drop: bool) -> String {
    let (h, m, s, f) = split(frames.abs(), rate, drop);
    let sep = if drop_per_minute(rate, drop) > 0 {
        ';'
    } else {
        ':'
    };
    let sign = if frames < 0 { "-" } else { "" };
    format!("{sign}{h}:{m:02}:{s:02}{sep}{f:02}")
}

/// Parses timecode text into a frame number; see the module docs for the
/// rules.
#[allow(clippy::many_single_char_names)] // h, m, s, f are the timecode fields.
pub fn parse(text: &str, rate: FrameRate, drop: bool) -> Result<i64, ParseError> {
    let t = text.trim();
    let (neg, t) = match t.strip_prefix('-') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, t),
    };
    if t.is_empty() {
        return Err(ParseError::Empty);
    }
    let fields: Vec<i64> = if t.contains([':', ';', '.']) {
        let parts: Vec<&str> = t.split([':', ';', '.']).collect();
        if parts.len() > 4 {
            return Err(ParseError::TooLong);
        }
        let mut out = Vec::new();
        for p in parts.iter().rev() {
            let p = p.trim();
            if p.is_empty() {
                out.push(0);
                continue;
            }
            if p.len() > 8 {
                return Err(ParseError::TooLong);
            }
            out.push(p.parse::<i64>().map_err(|_| ParseError::NotANumber)?);
        }
        out
    } else {
        if !t.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseError::NotANumber);
        }
        if t.len() > 8 {
            return Err(ParseError::TooLong);
        }
        let mut digits = t.as_bytes();
        let mut out = Vec::new();
        while !digits.is_empty() {
            let at = digits.len().saturating_sub(2);
            let (rest, pair) = digits.split_at(at);
            out.push(
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|p| p.parse::<i64>().ok())
                    .ok_or(ParseError::NotANumber)?,
            );
            digits = rest;
        }
        out
    };
    let get = |i: usize| fields.get(i).copied().unwrap_or(0);
    let (f, s, m, h) = (get(0), get(1), get(2), get(3));
    let base = i64::from(rate.timecode_base());
    let mut frames = ((h * 60 + m) * 60 + s) * base + f;
    let d = drop_per_minute(rate, drop);
    if d > 0 {
        let minutes = h * 60 + m;
        frames -= d * (minutes - minutes / 10);
    }
    Ok(if neg { -frames } else { frames })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NTSC: FrameRate = FrameRate::FPS_29_97;

    #[test]
    fn formats_non_drop() {
        let r = FrameRate::FPS_30;
        assert_eq!(format_frames(0, r, false), "0:00:00:00");
        assert_eq!(format_frames(30 * 30 + 27, r, false), "0:00:30:27");
        assert_eq!(format_frames(30 * 3600, r, false), "1:00:00:00");
        assert_eq!(format_frames(-45, r, false), "-0:00:01:15");
        // 23.976 counts on base 24, 59.94 on base 60 without drop.
        assert_eq!(
            format_frames(24, FrameRate::FPS_23_976, false),
            "0:00:01:00"
        );
        assert_eq!(format_frames(60, FrameRate::FPS_59_94, false), "0:00:01:00");
    }

    #[test]
    fn drop_frame_skips_numbers_at_minute_starts() {
        assert_eq!(format_frames(1799, NTSC, true), "0:00:59;29");
        assert_eq!(format_frames(1800, NTSC, true), "0:01:00;02");
        // Every tenth minute keeps its frames 0 and 1.
        assert_eq!(format_frames(17_982, NTSC, true), "0:10:00;00");
        assert_eq!(format_frames(17_981, NTSC, true), "0:09:59;29");
        // One hour of 29.97 is 107 892 frames.
        assert_eq!(format_frames(107_892, NTSC, true), "1:00:00;00");
        // 59.94 drops four.
        assert_eq!(
            format_frames(3599, FrameRate::FPS_59_94, true),
            "0:00:59;59"
        );
        assert_eq!(
            format_frames(3600, FrameRate::FPS_59_94, true),
            "0:01:00;04"
        );
        assert_eq!(
            format_frames(35_964, FrameRate::FPS_59_94, true),
            "0:10:00;00"
        );
    }

    #[test]
    fn drop_is_ignored_where_unsupported() {
        assert_eq!(
            format_frames(1800, FrameRate::FPS_30, true),
            format_frames(1800, FrameRate::FPS_30, false)
        );
        assert!(!format_frames(5, FrameRate::FPS_24, true).contains(';'));
    }

    #[test]
    fn shorthand() {
        let r = FrameRate::FPS_30;
        let p = |s: &str| parse(s, r, false).unwrap();
        assert_eq!(p("30"), 30, "30 is 30 frames");
        assert_eq!(p("5"), 5);
        assert_eq!(p("130"), 60, "1 second 30 frames");
        assert_eq!(p("1000"), 10 * 30, "1000 is 10:00");
        assert_eq!(p("10000"), 60 * 30, "1 minute");
        assert_eq!(p("1.5"), 35, "1 second 5 frames");
        assert_eq!(p("0:00:10:00"), 300);
        assert_eq!(p("1:15"), 45);
        assert_eq!(p("0:90:00"), 90 * 30, "fields carry");
        assert_eq!(p("  12  "), 12);
        assert_eq!(p("-1:00"), -30);
        assert_eq!(p(":5"), 5, "an empty field is zero");
        assert_eq!(p("1:00:00:00"), 30 * 3600);
    }

    #[test]
    fn parse_errors() {
        let r = FrameRate::FPS_30;
        assert_eq!(parse("", r, false), Err(ParseError::Empty));
        assert_eq!(parse("  ", r, false), Err(ParseError::Empty));
        assert_eq!(parse("abc", r, false), Err(ParseError::NotANumber));
        assert_eq!(parse("1:xx", r, false), Err(ParseError::NotANumber));
        assert_eq!(parse("123456789", r, false), Err(ParseError::TooLong));
        assert_eq!(parse("1:2:3:4:5", r, false), Err(ParseError::TooLong));
        assert_eq!(parse("1.5e3", r, false), Err(ParseError::NotANumber));
    }

    #[test]
    fn round_trips() {
        for (rate, drop) in [
            (FrameRate::FPS_24, false),
            (FrameRate::FPS_25, false),
            (FrameRate::FPS_30, false),
            (FrameRate::FPS_60, false),
            (FrameRate::FPS_23_976, false),
            (NTSC, false),
            (NTSC, true),
            (FrameRate::FPS_59_94, true),
        ] {
            for frames in [
                0, 1, 29, 30, 1799, 1800, 1801, 17_981, 17_982, 17_983, 107_891, 107_892, 500_000,
            ] {
                let text = format_frames(frames, rate, drop);
                assert_eq!(
                    parse(&text, rate, drop),
                    Ok(frames),
                    "{text} at {rate} drop={drop}"
                );
            }
        }
        assert_eq!(parse("-0:00:01:15", FrameRate::FPS_30, false), Ok(-45));
    }

    #[test]
    fn frames_in_dropped_gap_parse_forward() {
        // 0:01:00;00 does not exist; it reads as the frame two before ;02.
        assert_eq!(parse("0:01:00;00", NTSC, true), Ok(1800 - 2));
    }
}
