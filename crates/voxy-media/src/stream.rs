//! One decoder thread per video source.
//!
//! A [`VideoStream`] owns a [`VideoDecoder`] on its own thread and fills a
//! shared [`FrameCache`]. The UI never waits for decoding: each frame it
//! states what it needs as a [`Demand`] and shows whatever the cache has.
//!
//! - **Latest demand wins.** A demand replaces the previous one. The thread
//!   works one frame at a time ([`VideoDecoder::step_toward`]) and re-reads
//!   the demand between frames, so dragging the playhead abandons stale
//!   targets immediately. When the new target is a little ahead in the same
//!   GOP, decoding simply carries on toward it.
//! - **Scrubbing is keyframe-first.** When reaching a target needs a seek,
//!   the keyframe it lands on is cached and shown at once, then the exact
//!   frame replaces it once decoded ([`VideoStream::preview`] says which one
//!   you got). Stepping backward a frame at a time also caches the frames
//!   decoded on the way, so the next steps back are cache hits instead of
//!   another seek each.
//! - **Playback prefetches.** While playing, the thread keeps the next
//!   [`StreamOptions::play_ahead`] of frames decoded ahead of the playhead.
//! - **GOP-aware.** A second thread scans the file's packets without decoding
//!   to fill the decoder's [`GopIndex`](crate::gop::GopIndex) up front, so
//!   decode-forward-or-seek decisions are exact from the first request.
//!
//! [`VideoStream::frame_blocking`] waits for the exact frame, for export
//! and tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use voxy_core::Time;

use crate::MediaError;
use crate::VideoFrame;
use crate::cache::{FrameCache, SourceId};
use crate::video::{DecodeOptions, Step, VideoDecoder, VideoInfo, scan_keyframes};

/// Called from the decoder thread whenever a frame worth showing arrives
/// (the UI passes "request a repaint").
pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamOptions {
    pub decode: DecodeOptions,
    /// During playback, keep this much decoded ahead of the playhead.
    pub play_ahead: Time,
    /// After a scrub target is ready, decode this far past it too, so
    /// stepping forward is instant.
    pub scrub_ahead: Time,
    /// When a scrub moves back by at most this much, cache the frames
    /// decoded on the way to it, so stepping backward again is instant.
    pub scrub_behind: Time,
    /// Scan the file for keyframes on a background thread.
    pub scan_keyframes: bool,
    /// While seeking during a scrub, show the keyframe first.
    pub keyframe_preview: bool,
}

impl Default for StreamOptions {
    fn default() -> Self {
        StreamOptions {
            decode: DecodeOptions::default(),
            play_ahead: Time::from_secs(1),
            scrub_ahead: Time::from_millis(100),
            scrub_behind: Time::from_secs(1),
            scan_keyframes: true,
            keyframe_preview: true,
        }
    }
}

/// What the UI needs from a stream right now. Source times, from the start
/// of the stream; negative times mean the first frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Demand {
    /// Nothing; the thread sleeps.
    Idle,
    /// Show the frame at this time as soon as possible.
    Scrub(Time),
    /// Playing forward from this time.
    Play(Time),
}

/// Counters, for the status bar and for tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamStats {
    /// Frames decoded, including ones decoded only to reach a later frame.
    pub decoded: u64,
    /// Frames converted to RGBA and cached.
    pub cached: u64,
    pub seeks: u64,
    /// Keyframes shown while seeking toward a scrub target.
    pub previews: u64,
    /// Scrub targets replaced before they were reached.
    pub abandoned: u64,
}

struct Mailbox {
    demand: Demand,
    generation: u64,
    shutdown: bool,
    /// The thread has nothing to do for the current generation.
    idle: bool,
    stats: StreamStats,
    error: Option<String>,
    /// The last scrub target reached, and its frame (kept even if the cache
    /// cannot hold it).
    delivered: Option<(Time, Arc<VideoFrame>)>,
}

struct Shared {
    mailbox: Mutex<Mailbox>,
    /// Wakes the decoder thread.
    wake: Condvar,
    /// Wakes threads waiting for frames or idleness.
    changed: Condvar,
    stop_scan: AtomicBool,
    notify: Option<Notify>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Mailbox> {
        self.mailbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn notify(&self) {
        self.changed.notify_all();
        if let Some(n) = &self.notify {
            n();
        }
    }
}

/// See the [module documentation](self).
pub struct VideoStream {
    id: SourceId,
    path: PathBuf,
    info: VideoInfo,
    hardware: Option<String>,
    cache: Arc<FrameCache>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    scanner: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for VideoStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoStream")
            .field("id", &self.id)
            .field("path", &self.path)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl VideoStream {
    /// Opens `path` (errors are reported here, synchronously) and starts its
    /// decoder thread.
    pub fn open(
        path: &Path,
        cache: Arc<FrameCache>,
        options: StreamOptions,
        notify: Option<Notify>,
    ) -> Result<Self, MediaError> {
        let spawn_err = |e: std::io::Error| MediaError::Media {
            path: path.display().to_string(),
            message: format!("cannot start a decoder thread: {e}"),
        };
        let id = SourceId::next();
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox {
                demand: Demand::Idle,
                generation: 0,
                shutdown: false,
                idle: false,
                stats: StreamStats::default(),
                error: None,
                delivered: None,
            }),
            wake: Condvar::new(),
            changed: Condvar::new(),
            stop_scan: AtomicBool::new(false),
            notify,
        });
        // The decoder is opened on its own thread (FFmpeg's scaler may not
        // move between threads) and reports back here.
        let (opened_tx, opened_rx) = std::sync::mpsc::sync_channel(1);
        let worker = {
            let path = path.to_path_buf();
            let cache = cache.clone();
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("voxy-decode".into())
                .spawn(move || {
                    let dec = match VideoDecoder::open(&path, options.decode) {
                        Ok(dec) => dec,
                        Err(e) => {
                            let _ = opened_tx.send(Err(e));
                            return;
                        }
                    };
                    let facts = (
                        dec.info().clone(),
                        dec.hardware().map(str::to_owned),
                        dec.gop_index(),
                    );
                    if opened_tx.send(Ok(facts)).is_err() {
                        return;
                    }
                    Worker {
                        dec,
                        cache,
                        id,
                        options,
                        shared,
                        done: None,
                        failed: None,
                        last_scrub: None,
                    }
                    .run();
                })
                .map_err(spawn_err)?
        };
        let opened = opened_rx.recv().unwrap_or_else(|_| {
            Err(MediaError::Media {
                path: path.display().to_string(),
                message: "the decoder thread stopped".into(),
            })
        });
        let (info, hardware, gop) = match opened {
            Ok(facts) => facts,
            Err(e) => {
                let _ = worker.join();
                return Err(e);
            }
        };
        let mut stream = VideoStream {
            id,
            path: path.to_path_buf(),
            info,
            hardware,
            cache,
            shared,
            worker: Some(worker),
            scanner: None,
        };
        if options.scan_keyframes {
            let shared = stream.shared.clone();
            let scan_path = path.to_path_buf();
            stream.scanner = Some(
                std::thread::Builder::new()
                    .name("voxy-gop-scan".into())
                    .spawn(move || {
                        if let Err(e) = scan_keyframes(&scan_path, &gop, &shared.stop_scan) {
                            tracing::debug!("keyframe scan stopped: {e}");
                        }
                    })
                    .map_err(spawn_err)?,
            );
        }
        Ok(stream)
    }

    pub fn id(&self) -> SourceId {
        self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// The hardware decoder in use, if any.
    pub fn hardware(&self) -> Option<&str> {
        self.hardware.as_deref()
    }

    /// States what is needed now, replacing the previous demand. Cheap; call
    /// it every UI frame.
    pub fn demand(&self, demand: Demand) {
        let demand = match demand {
            Demand::Scrub(t) => Demand::Scrub(t.max(Time::ZERO)),
            Demand::Play(t) => Demand::Play(t.max(Time::ZERO)),
            Demand::Idle => Demand::Idle,
        };
        let mut m = self.shared.lock();
        if m.demand != demand {
            m.demand = demand;
            m.generation += 1;
            m.idle = false;
            m.error = None;
            drop(m);
            self.shared.wake.notify_all();
        }
    }

    /// The exact frame for source time `t`, if decoded.
    pub fn frame(&self, t: Time) -> Option<Arc<VideoFrame>> {
        self.cache.get(self.id, t.max(Time::ZERO))
    }

    /// The best frame to show for `t` right now, and whether it is exact.
    /// While the exact frame decodes this is the nearest cached frame, such
    /// as the keyframe a scrub seeked to.
    pub fn preview(&self, t: Time) -> Option<(Arc<VideoFrame>, bool)> {
        let t = t.max(Time::ZERO);
        if let Some(f) = self.cache.get(self.id, t) {
            return Some((f, true));
        }
        self.cache.nearest(self.id, t).map(|f| (f, false))
    }

    /// Demands `t` and waits for its exact frame.
    pub fn frame_blocking(
        &self,
        t: Time,
        timeout: Duration,
    ) -> Result<Arc<VideoFrame>, MediaError> {
        let t = t.max(Time::ZERO);
        self.demand(Demand::Scrub(t));
        let deadline = Instant::now() + timeout;
        let mut m = self.shared.lock();
        loop {
            if let Some(f) = self.cache.get(self.id, t) {
                return Ok(f);
            }
            if let Some((target, f)) = &m.delivered
                && *target == t
            {
                return Ok(f.clone());
            }
            if let Some(e) = &m.error
                && m.idle
            {
                return Err(self.media_err(e.clone()));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(self.media_err(format!("no frame for {t:?} within {timeout:?}")));
            }
            m = self
                .shared
                .changed
                .wait_timeout(m, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Waits until the thread has done everything the current demand asks
    /// for (prefetch included). Returns `false` on timeout.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut m = self.shared.lock();
        while !m.idle {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            m = self
                .shared
                .changed
                .wait_timeout(m, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    pub fn stats(&self) -> StreamStats {
        self.shared.lock().stats
    }

    /// The last decoding error, if any.
    pub fn error(&self) -> Option<String> {
        self.shared.lock().error.clone()
    }

    fn media_err(&self, message: String) -> MediaError {
        MediaError::Media {
            path: self.path.display().to_string(),
            message,
        }
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        self.shared.stop_scan.store(true, Ordering::Relaxed);
        self.shared.lock().shutdown = true;
        self.shared.wake.notify_all();
        for handle in [self.worker.take(), self.scanner.take()]
            .into_iter()
            .flatten()
        {
            let _ = handle.join();
        }
        self.cache.remove_source(self.id);
    }
}

/// One unit of work: reach `target`, caching frames from `keep_from` on.
#[derive(Debug, Clone, Copy)]
struct Task {
    target: Time,
    keep_from: Time,
    /// Show the keyframe if this needs a seek.
    preview: bool,
    /// A scrub target (counts toward `delivered` and `abandoned`).
    scrub: bool,
}

struct Worker {
    dec: VideoDecoder,
    cache: Arc<FrameCache>,
    id: SourceId,
    options: StreamOptions,
    shared: Arc<Shared>,
    /// The last target reached (its frame may not fit in the cache).
    done: Option<Time>,
    /// The last target that failed to decode; not retried until the demand
    /// changes to something else.
    failed: Option<Time>,
    last_scrub: Option<Time>,
}

impl Worker {
    fn run(&mut self) {
        let mut seen_generation = u64::MAX;
        let mut open_scrub: Option<Time> = None;
        loop {
            let (demand, generation) = {
                let m = self.shared.lock();
                if m.shutdown {
                    return;
                }
                (m.demand, m.generation)
            };
            if generation != seen_generation {
                seen_generation = generation;
                if let Some(t) = open_scrub.take()
                    && !self.satisfied(t)
                    && demand != Demand::Scrub(t)
                {
                    self.shared.lock().stats.abandoned += 1;
                }
            }
            let Some(task) = self.next_task(demand) else {
                if self.sleep(generation) {
                    return;
                }
                continue;
            };
            if task.scrub {
                open_scrub = Some(task.target);
            }
            if let Err(e) = self.work(task) {
                tracing::warn!("video decoding failed: {e}");
                self.failed = Some(task.target);
                self.shared.lock().error = Some(e.to_string());
                self.shared.notify();
            }
            self.publish_stats();
        }
    }

    /// Nothing to do for `generation`: marks the thread idle and waits for
    /// a new demand. Returns `true` when the stream is shutting down.
    fn sleep(&self, generation: u64) -> bool {
        let mut m = self.shared.lock();
        if m.generation == generation && !m.shutdown {
            m.idle = true;
            self.shared.changed.notify_all();
            m = self
                .shared
                .wake
                .wait_while(m, |m| m.generation == generation && !m.shutdown)
                .unwrap_or_else(PoisonError::into_inner);
            m.idle = false;
        }
        m.shutdown
    }

    fn satisfied(&self, t: Time) -> bool {
        self.done == Some(t) || self.failed == Some(t) || self.cache.contains(self.id, t)
    }

    /// The first missing frame in `[from, to)`, unless it is one already
    /// handled (reached but uncacheable, or failed).
    fn gap(&self, from: Time, to: Time) -> Option<Time> {
        let gap = self.cache.first_missing(self.id, from, to)?;
        (!self.satisfied(gap)).then_some(gap)
    }

    fn next_task(&mut self, demand: Demand) -> Option<Task> {
        match demand {
            Demand::Idle => None,
            Demand::Scrub(t) => {
                if !self.satisfied(t) {
                    // Stepping back a little: keep what is decoded on the way.
                    let back = self
                        .last_scrub
                        .is_some_and(|last| t < last && last - t <= self.options.scrub_behind);
                    let keep_from = if back {
                        t - self.options.scrub_behind
                    } else {
                        t
                    };
                    return Some(Task {
                        target: t,
                        keep_from,
                        preview: self.options.keyframe_preview,
                        scrub: true,
                    });
                }
                self.last_scrub = Some(t);
                let gap = self.gap(t, t + self.options.scrub_ahead)?;
                Some(Task {
                    target: gap,
                    keep_from: gap,
                    preview: false,
                    scrub: false,
                })
            }
            Demand::Play(t) => {
                self.last_scrub = None;
                let gap = self.gap(t, t + self.options.play_ahead)?;
                Some(Task {
                    target: gap,
                    keep_from: gap,
                    preview: false,
                    scrub: false,
                })
            }
        }
    }

    /// One frame's worth of work toward `task.target`.
    fn work(&mut self, task: Task) -> Result<(), MediaError> {
        let seeks = self.dec.seeks();
        let step = self.dec.step_toward(task.target)?;
        if step == Step::Ready {
            let frame = self.dec.current_frame()?;
            let start = frame.time.min(task.target);
            let end = self
                .dec
                .current_end()
                .unwrap_or(frame.time + frame.duration);
            self.cache.insert(self.id, start, end, frame.clone());
            self.done = Some(task.target);
            if task.scrub {
                self.last_scrub = Some(task.target);
            }
            let mut m = self.shared.lock();
            m.stats.cached += 1;
            if task.scrub {
                m.delivered = Some((task.target, frame));
            }
            drop(m);
            self.shared.notify();
            return Ok(());
        }
        let Some(time) = self.dec.current_time() else {
            return Ok(());
        };
        if self.dec.seeks() != seeks {
            if task.preview && !self.cache.contains(self.id, time) {
                let frame = self.dec.current_frame()?;
                let end = self
                    .dec
                    .current_end()
                    .unwrap_or(frame.time + frame.duration);
                self.cache.insert(self.id, time, end, frame);
                let mut m = self.shared.lock();
                m.stats.previews += 1;
                m.stats.cached += 1;
                drop(m);
                self.shared.notify();
            }
            return Ok(());
        }
        // A frame on the way: keep it if wanted, once its end is known.
        if time >= task.keep_from
            && let Some(end) = self.dec.current_end()
            && !self.cache.contains(self.id, time)
        {
            let frame = self.dec.current_frame()?;
            self.cache.insert(self.id, time, end, frame);
            self.shared.lock().stats.cached += 1;
        }
        Ok(())
    }

    fn publish_stats(&self) {
        let mut m = self.shared.lock();
        m.stats.decoded = self.dec.frames_decoded();
        m.stats.seeks = self.dec.seeks();
    }
}
