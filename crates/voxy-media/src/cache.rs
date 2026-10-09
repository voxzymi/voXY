//! A byte-budgeted LRU cache of decoded video frames, shared by every
//! source.
//!
//! Each entry holds one frame and the source-time span it answers,
//! `[start, end)`. Usually this is the frame's own presentation interval.
//! The last frame of a stream answers every later time, so its `end` is
//! [`Time::MAX`]. Lookups are by time, so variable frame rates need no
//! special handling.
//!
//! The decoder threads ([`crate::stream`]) fill the cache, and the
//! renderer reads it. Everything is behind one mutex; each operation is a
//! couple of B-tree lookups, far cheaper than decoding a frame.
//!
//! When an insert takes the cache over budget, it evicts the least recently
//! used frames. A lookup that finds a frame counts as a use.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use voxy_core::Time;

use crate::VideoFrame;

/// Identifies one video source (one open file) in a [`FrameCache`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(u64);

impl SourceId {
    /// A process-wide unique ID.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        SourceId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Bookkeeping cost of one entry on top of its pixels.
const ENTRY_OVERHEAD: usize = 128;

fn frame_bytes(frame: &VideoFrame) -> usize {
    frame.pixels.len() + ENTRY_OVERHEAD
}

/// Counters, for the status bar and for tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub frames: usize,
    pub bytes: usize,
    pub budget: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

struct Entry {
    frame: Arc<VideoFrame>,
    end: Time,
    bytes: usize,
    used: u64,
}

struct Inner {
    budget: usize,
    bytes: usize,
    clock: u64,
    entries: BTreeMap<(SourceId, Time), Entry>,
    /// Use stamp → entry key, oldest first.
    lru: BTreeMap<u64, (SourceId, Time)>,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl Inner {
    /// The entry whose span contains `t`.
    fn covering(&self, source: SourceId, t: Time) -> Option<(Time, &Entry)> {
        let (&(_, start), e) = self
            .entries
            .range((source, Time(i64::MIN))..=(source, t))
            .next_back()?;
        (t < e.end).then_some((start, e))
    }

    fn touch(&mut self, key: (SourceId, Time)) {
        self.clock += 1;
        let stamp = self.clock;
        if let Some(e) = self.entries.get_mut(&key) {
            self.lru.remove(&e.used);
            e.used = stamp;
            self.lru.insert(stamp, key);
        }
    }

    fn remove(&mut self, key: (SourceId, Time)) {
        if let Some(e) = self.entries.remove(&key) {
            self.lru.remove(&e.used);
            self.bytes -= e.bytes;
        }
    }

    fn evict_to(&mut self, budget: usize) {
        while self.bytes > budget {
            let Some((_, key)) = self.lru.pop_first() else {
                break;
            };
            if let Some(e) = self.entries.remove(&key) {
                self.bytes -= e.bytes;
                self.evictions += 1;
            }
        }
    }
}

/// See the [module documentation](self).
pub struct FrameCache {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for FrameCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameCache")
            .field("stats", &self.stats())
            .finish()
    }
}

impl FrameCache {
    /// A cache holding at most `budget` bytes of frames.
    pub fn new(budget: usize) -> Self {
        FrameCache {
            inner: Mutex::new(Inner {
                budget,
                bytes: 0,
                clock: 0,
                entries: BTreeMap::new(),
                lru: BTreeMap::new(),
                hits: 0,
                misses: 0,
                evictions: 0,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // The cache holds no invariants a panicking thread could break
        // halfway in a way that matters more than losing the cache.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores `frame` as the answer for source times `[start, end)`.
    /// Replaces any entry with the same start. Returns `false` (and stores
    /// nothing) if the frame alone exceeds the budget or the span is empty.
    pub fn insert(&self, source: SourceId, start: Time, end: Time, frame: Arc<VideoFrame>) -> bool {
        let bytes = frame_bytes(&frame);
        let mut c = self.lock();
        if bytes > c.budget || start >= end {
            return false;
        }
        let key = (source, start);
        c.remove(key);
        c.clock += 1;
        let used = c.clock;
        c.entries.insert(
            key,
            Entry {
                frame,
                end,
                bytes,
                used,
            },
        );
        c.lru.insert(used, key);
        c.bytes += bytes;
        let budget = c.budget;
        c.evict_to(budget);
        true
    }

    /// The frame on screen at `t`, if cached. Counts as a use.
    pub fn get(&self, source: SourceId, t: Time) -> Option<Arc<VideoFrame>> {
        let mut c = self.lock();
        let Some((start, frame)) = c.covering(source, t).map(|(s, e)| (s, e.frame.clone())) else {
            c.misses += 1;
            return None;
        };
        c.hits += 1;
        c.touch((source, start));
        Some(frame)
    }

    /// Whether the frame for `t` is cached. Not a use; not counted.
    pub fn contains(&self, source: SourceId, t: Time) -> bool {
        self.lock().covering(source, t).is_some()
    }

    /// The cached frame closest to `t`: the last one starting at or before
    /// it, otherwise the first one after it. For showing *something* while
    /// the exact frame decodes. Counts as a use.
    pub fn nearest(&self, source: SourceId, t: Time) -> Option<Arc<VideoFrame>> {
        let mut c = self.lock();
        let before = c
            .entries
            .range((source, Time(i64::MIN))..=(source, t))
            .next_back();
        let found = before
            .or_else(|| c.entries.range((source, t)..=(source, Time::MAX)).next())
            .map(|(&key, e)| (key, e.frame.clone()));
        let (key, frame) = found?;
        c.touch(key);
        Some(frame)
    }

    /// The first time in `[from, to)` whose frame is not cached.
    pub fn first_missing(&self, source: SourceId, from: Time, to: Time) -> Option<Time> {
        let c = self.lock();
        let mut t = from;
        while t < to {
            match c.covering(source, t) {
                Some((_, e)) => t = e.end,
                None => return Some(t),
            }
        }
        None
    }

    /// Drops every frame of `source`.
    pub fn remove_source(&self, source: SourceId) {
        let mut c = self.lock();
        let keys: Vec<_> = c
            .entries
            .range((source, Time(i64::MIN))..=(source, Time::MAX))
            .map(|(&k, _)| k)
            .collect();
        for k in keys {
            c.remove(k);
        }
    }

    pub fn clear(&self) {
        let mut c = self.lock();
        c.entries.clear();
        c.lru.clear();
        c.bytes = 0;
    }

    pub fn budget(&self) -> usize {
        self.lock().budget
    }

    /// Changes the budget, evicting at once if the cache is now over it.
    pub fn set_budget(&self, budget: usize) {
        let mut c = self.lock();
        c.budget = budget;
        c.evict_to(budget);
    }

    pub fn stats(&self) -> CacheStats {
        let c = self.lock();
        CacheStats {
            frames: c.entries.len(),
            bytes: c.bytes,
            budget: c.budget,
            hits: c.hits,
            misses: c.misses,
            evictions: c.evictions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Time = Time::from_millis(40);

    fn frame(i: i64, px: usize) -> Arc<VideoFrame> {
        Arc::new(VideoFrame {
            time: Time(FRAME.0 * i),
            duration: FRAME,
            width: 1,
            height: 1,
            pixels: vec![0; px],
        })
    }

    fn put(c: &FrameCache, s: SourceId, i: i64, px: usize) -> bool {
        let f = frame(i, px);
        c.insert(s, f.time, f.time + f.duration, f)
    }

    #[test]
    fn lookups_are_by_time_span() {
        let c = FrameCache::new(1 << 20);
        let s = SourceId::next();
        let other = SourceId::next();
        put(&c, s, 0, 4);
        put(&c, s, 1, 4);
        put(&c, s, 3, 4);
        assert_eq!(c.get(s, Time::ZERO).unwrap().time, Time::ZERO);
        assert_eq!(c.get(s, Time(FRAME.0 * 2 - 1)).unwrap().time, FRAME);
        assert!(c.get(s, Time(FRAME.0 * 2)).is_none()); // frame 2 missing
        assert!(c.get(other, Time::ZERO).is_none());
        assert_eq!(
            c.first_missing(s, Time::ZERO, Time(FRAME.0 * 4)),
            Some(Time(FRAME.0 * 2))
        );
        assert_eq!(c.first_missing(s, Time::ZERO, Time(FRAME.0 * 2)), None);
        // nearest prefers the frame before, then after.
        assert_eq!(c.nearest(s, Time(FRAME.0 * 2 + 1)).unwrap().time, FRAME);
        assert_eq!(c.nearest(s, Time(-5)).unwrap().time, Time::ZERO);
        assert!(c.nearest(other, Time::ZERO).is_none());
        let st = c.stats();
        assert_eq!((st.frames, st.hits, st.misses), (3, 2, 2));
    }

    #[test]
    fn last_frame_can_answer_every_later_time() {
        let c = FrameCache::new(1 << 20);
        let s = SourceId::next();
        let f = frame(9, 4);
        c.insert(s, f.time, Time::MAX, f);
        assert!(c.contains(s, Time::from_secs(3600)));
        assert_eq!(c.first_missing(s, f_time(9), Time::MAX), None);
    }

    fn f_time(i: i64) -> Time {
        Time(FRAME.0 * i)
    }

    #[test]
    fn eviction_is_least_recently_used_within_budget() {
        let per = 1000 + ENTRY_OVERHEAD;
        let c = FrameCache::new(per * 3);
        let s = SourceId::next();
        for i in 0..3 {
            assert!(put(&c, s, i, 1000));
        }
        // Use frame 0, so frame 1 is now the oldest.
        assert!(c.get(s, f_time(0)).is_some());
        put(&c, s, 3, 1000);
        assert!(c.contains(s, f_time(0)));
        assert!(!c.contains(s, f_time(1)));
        assert!(c.contains(s, f_time(2)));
        assert!(c.contains(s, f_time(3)));
        let st = c.stats();
        assert_eq!((st.frames, st.bytes, st.evictions), (3, per * 3, 1));

        // Replacing an entry does not double-count it.
        put(&c, s, 3, 1000);
        assert_eq!(c.stats().bytes, per * 3);

        // Too big for the budget: not stored, nothing evicted.
        assert!(!put(&c, s, 4, per * 3));
        assert_eq!(c.stats().frames, 3);

        c.set_budget(per);
        assert_eq!(c.stats().frames, 1);
        assert!(c.contains(s, f_time(3)), "the most recent frame survives");

        c.remove_source(s);
        assert_eq!(c.stats().bytes, 0);
    }

    #[test]
    fn many_sources_share_one_budget() {
        let per = 100 + ENTRY_OVERHEAD;
        let c = FrameCache::new(per * 4);
        let a = SourceId::next();
        let b = SourceId::next();
        for i in 0..4 {
            put(&c, a, i, 100);
        }
        for i in 0..2 {
            put(&c, b, i, 100);
        }
        let st = c.stats();
        assert_eq!((st.frames, st.evictions), (4, 2));
        assert!(!c.contains(a, f_time(0)) && !c.contains(a, f_time(1)));
        assert!(c.contains(b, f_time(1)));
    }
}
