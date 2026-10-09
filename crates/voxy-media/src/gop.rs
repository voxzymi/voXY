//! Where a stream's keyframes are.
//!
//! A decoder can only start at a keyframe, so the cost of reaching a frame
//! depends on the group of pictures (GOP) it is in. [`GopIndex`] records
//! keyframe timestamps and which stretches of the stream have been read,
//! so the decoder can tell "the next keyframe is beyond the target, decode
//! forward" from "there is a keyframe close to the target, seek to it"
//! (see [`crate::video::SeekPlan`]).
//!
//! The index is filled in two ways: by a demux-only scan of the whole file
//! on a background thread ([`crate::stream`]), and by the decoder itself
//! as it reads packets. It only ever affects speed, never which frame is
//! returned: the decoder is frame-accurate whichever way it gets there.
//!
//! Timestamps are in stream ticks (the stream's own time base).

use std::collections::BTreeSet;

/// What is known about the keyframes between two timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyframeSpan {
    /// The last keyframe in the range.
    Keyframe(i64),
    /// The range has been read and contains no keyframe.
    None,
    /// Not (completely) read yet.
    Unknown,
}

/// Keyframe timestamps and the read ("covered") stretches of one stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GopIndex {
    keyframes: BTreeSet<i64>,
    /// Sorted, disjoint, non-adjacent closed ranges `[start, end]` of
    /// timestamps whose packets have all been seen.
    covered: Vec<(i64, i64)>,
    complete: bool,
}

impl GopIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_keyframe(&mut self, pts: i64) {
        self.keyframes.insert(pts);
    }

    /// Records that every packet with a timestamp in `[start, end]` has been
    /// seen.
    pub fn cover(&mut self, start: i64, end: i64) {
        if start > end || self.complete {
            return;
        }
        let (mut s, mut e) = (start, end);
        // Merge every range that overlaps or touches [s, e].
        let first = self
            .covered
            .partition_point(|&(_, b)| b < s.saturating_sub(1));
        let mut last = first;
        while last < self.covered.len() && self.covered[last].0 <= e.saturating_add(1) {
            s = s.min(self.covered[last].0);
            e = e.max(self.covered[last].1);
            last += 1;
        }
        self.covered.splice(first..last, [(s, e)]);
    }

    /// Records that the whole stream has been read: every keyframe is known.
    pub fn mark_complete(&mut self) {
        self.complete = true;
        self.covered = vec![(i64::MIN, i64::MAX)];
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Whether every timestamp in `[start, end]` has been read.
    pub fn is_covered(&self, start: i64, end: i64) -> bool {
        if start > end {
            return true;
        }
        let i = self.covered.partition_point(|&(_, b)| b < start);
        self.covered
            .get(i)
            .is_some_and(|&(a, b)| a <= start && end <= b)
    }

    /// The keyframes in `(after, upto]`: the last one if any is known,
    /// otherwise whether the range is known to have none.
    pub fn keyframe_in(&self, after: i64, upto: i64) -> KeyframeSpan {
        if after >= upto {
            return KeyframeSpan::None;
        }
        if let Some(&k) = self
            .keyframes
            .range(after.saturating_add(1)..=upto)
            .next_back()
        {
            KeyframeSpan::Keyframe(k)
        } else if self.is_covered(after.saturating_add(1), upto) {
            KeyframeSpan::None
        } else {
            KeyframeSpan::Unknown
        }
    }

    /// The last known keyframe at or before `pts`.
    pub fn keyframe_at_or_before(&self, pts: i64) -> Option<i64> {
        self.keyframes.range(..=pts).next_back().copied()
    }

    /// Known keyframes in order.
    pub fn keyframes(&self) -> impl Iterator<Item = i64> + '_ {
        self.keyframes.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_merges_overlapping_and_touching_ranges() {
        let mut g = GopIndex::new();
        g.cover(10, 20);
        g.cover(30, 40);
        assert_eq!(g.covered, vec![(10, 20), (30, 40)]);
        g.cover(21, 29);
        assert_eq!(g.covered, vec![(10, 40)]);
        g.cover(0, 5);
        g.cover(50, 60);
        g.cover(4, 55);
        assert_eq!(g.covered, vec![(0, 60)]);
        g.cover(5, 3); // empty: ignored
        assert_eq!(g.covered, vec![(0, 60)]);
        assert!(g.is_covered(0, 60));
        assert!(!g.is_covered(0, 61));
    }

    #[test]
    fn keyframe_in_distinguishes_none_from_unknown() {
        let mut g = GopIndex::new();
        g.cover(0, 99);
        for k in [0, 30, 60, 90] {
            g.add_keyframe(k);
        }
        assert_eq!(g.keyframe_in(0, 29), KeyframeSpan::None);
        assert_eq!(g.keyframe_in(0, 30), KeyframeSpan::Keyframe(30));
        assert_eq!(g.keyframe_in(5, 75), KeyframeSpan::Keyframe(60));
        assert_eq!(g.keyframe_in(91, 150), KeyframeSpan::Unknown);
        assert_eq!(g.keyframe_in(50, 50), KeyframeSpan::None);
        assert_eq!(g.keyframe_at_or_before(59), Some(30));
        assert_eq!(g.keyframe_at_or_before(-1), None);

        g.mark_complete();
        assert_eq!(g.keyframe_in(91, 150), KeyframeSpan::None);
        g.cover(1000, 2000); // no effect once complete
        assert!(g.is_covered(i64::MIN, i64::MAX));
    }
}
