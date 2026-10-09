//! Video sources for the viewport: one decoder thread per video asset,
//! all sharing one frame cache.
//!
//! Before each viewport render, [`MediaPool::resolve`] tells every stream
//! what the frame needs (scrub to a time, or play from it) and collects the
//! best frame each video layer can show right now. Nothing here waits for
//! decoding: a frame that is not ready yet shows the nearest cached one
//! (while scrubbing, the keyframe the decoder seeked to), and the decoder
//! thread requests a repaint when the exact frame arrives.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use voxy_core::{AssetId, AssetKind, Document, EvaluatedComp, EvaluatedContent, Time};
use voxy_media::VideoFrame;
use voxy_media::cache::{CacheStats, FrameCache};
use voxy_media::stream::{Demand, Notify, StreamOptions, VideoStream};
use voxy_render::VideoFrames;

/// RAM for decoded frames, shared by every video: about 120 frames of
/// 1080p.
const CACHE_BYTES: usize = 1 << 30;

enum Source {
    Open(Box<VideoStream>),
    Failed(String),
}

struct Entry {
    path: PathBuf,
    source: Source,
}

/// The frames chosen for one render.
#[derive(Debug, Default)]
pub struct ResolvedFrames {
    frames: Vec<(AssetId, Time, Arc<VideoFrame>)>,
    /// Some layer shows a stand-in until its exact frame decodes.
    pub approximate: bool,
}

impl ResolvedFrames {
    /// Identifies the chosen frames, so the viewport re-renders exactly when
    /// one changes.
    pub fn identity(&self) -> Vec<(AssetId, Time, usize)> {
        self.frames
            .iter()
            .map(|(a, t, f)| (*a, *t, Arc::as_ptr(f) as usize))
            .collect()
    }
}

impl VideoFrames for ResolvedFrames {
    fn frame(&self, asset: AssetId, time: Time) -> Option<Arc<VideoFrame>> {
        self.frames
            .iter()
            .find(|(a, t, _)| *a == asset && *t == time)
            .map(|(_, _, f)| f.clone())
    }
}

pub struct MediaPool {
    cache: Arc<FrameCache>,
    sources: HashMap<AssetId, Entry>,
    notify: Option<Notify>,
}

impl std::fmt::Debug for MediaPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaPool")
            .field("sources", &self.sources.len())
            .field("cache", &self.cache.stats())
            .finish_non_exhaustive()
    }
}

impl MediaPool {
    /// `notify` runs on decoder threads when a frame worth showing arrives.
    pub fn new(notify: Option<Notify>) -> Self {
        MediaPool {
            cache: Arc::new(FrameCache::new(CACHE_BYTES)),
            sources: HashMap::new(),
            notify,
        }
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Why a video asset cannot be shown, if it cannot.
    pub fn failure(&self, asset: AssetId) -> Option<&str> {
        match &self.sources.get(&asset)?.source {
            Source::Failed(e) => Some(e),
            Source::Open(_) => None,
        }
    }

    /// Opens, closes and steers streams for `eval` and picks each video
    /// layer's frame.
    pub fn resolve(
        &mut self,
        doc: &Document,
        eval: &EvaluatedComp,
        asset_root: Option<&Path>,
        playing: bool,
    ) -> ResolvedFrames {
        // Close sources whose asset is gone or now points elsewhere.
        self.sources.retain(|id, e| {
            doc.assets
                .get(*id)
                .is_some_and(|a| a.kind == AssetKind::Video && a.resolve(asset_root) == e.path)
        });

        let mut out = ResolvedFrames::default();
        let mut demanded = Vec::new();
        // Topmost layer first.
        for layer in eval.layers.iter().rev() {
            let EvaluatedContent::Video {
                asset, source_time, ..
            } = layer.content
            else {
                continue;
            };
            let Some(info) = doc.assets.get(asset) else {
                continue;
            };
            let path = info.resolve(asset_root);
            let entry = self.sources.entry(asset).or_insert_with(|| {
                let source = match VideoStream::open(
                    &path,
                    self.cache.clone(),
                    StreamOptions::default(),
                    self.notify.clone(),
                ) {
                    Ok(s) => {
                        if let Some(hw) = s.hardware() {
                            tracing::info!("{}: decoding with {hw}", path.display());
                        }
                        Source::Open(Box::new(s))
                    }
                    Err(e) => {
                        tracing::warn!("video asset {asset} unavailable: {e}");
                        Source::Failed(e.to_string())
                    }
                };
                Entry { path, source }
            });
            let Source::Open(stream) = &entry.source else {
                continue;
            };
            // One demand per stream: the topmost layer using it wins.
            if !demanded.contains(&asset) {
                demanded.push(asset);
                stream.demand(if playing {
                    Demand::Play(source_time)
                } else {
                    Demand::Scrub(source_time)
                });
            }
            if let Some((frame, exact)) = stream.preview(source_time) {
                out.approximate |= !exact;
                out.frames.push((asset, source_time, frame));
            } else {
                out.approximate = true;
            }
        }
        for (id, e) in &self.sources {
            if let Source::Open(s) = &e.source
                && !demanded.contains(id)
            {
                s.demand(Demand::Idle);
            }
        }
        out
    }
}
