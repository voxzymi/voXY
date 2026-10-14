//! The editor's OpenFX runtime (design §6.3, §6.7, §6.9): describes
//! bundles in sandboxes, turns their plugins into effect definitions, and
//! runs OpenFX effects for the renderer as an [`ExternalEffects`] provider.
//!
//! One sandbox runs per bundle, started when first needed and again after
//! it dies. Each voXY effect has its own plugin instance, created on its
//! first render. A crash, hang or protocol error kills the sandbox and puts
//! the plugin on the session [`Blocklist`]; the renderer then passes the
//! image through and shows the reason until Retry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use voxy_core::external::{CpuFrame, ExternalEffects};
use voxy_core::registry::EffectDef;
use voxy_core::{EffectId, EffectTypeId, EvaluatedEffect};

use crate::client::{Blocked, Blocklist, Sandbox, SandboxError, Timeouts};
use crate::effect::Project;
use crate::image::{Components, Depth, Image, Premult, RectI};
use crate::mapping::{effect_def, to_param_value, type_id};
use crate::params::ParamKind;
use crate::protocol::{ImageSlot, ParamSnapshot, PluginDesc, Reply, Request};
use crate::shm::{HEADER, SessionDir, Slots};

/// The filter context's name.
const FILTER: &str = "OfxImageEffectContextFilter";

#[derive(Debug, Clone)]
struct PluginEntry {
    bundle: PathBuf,
    identifier: String,
    /// Each parameter's OpenFX type, by name.
    kinds: HashMap<String, ParamKind>,
}

#[derive(Debug, Clone, Copy)]
struct Live {
    bundle_index: usize,
    instance: u32,
    source: (Depth, Components),
    output: (Depth, Components),
}

#[derive(Debug)]
struct Inner {
    exe: PathBuf,
    session: SessionDir,
    timeouts: Timeouts,
    plugins: HashMap<EffectTypeId, PluginEntry>,
    bundles: Vec<PathBuf>,
    sandboxes: HashMap<usize, Sandbox>,
    instances: HashMap<EffectId, Live>,
    next_instance: u32,
    /// Every launch gets fresh channel and mapping names.
    launches: u32,
    blocklist: Blocklist,
}

/// The OpenFX runtime. Thread-safe: renders are serialised.
#[derive(Debug)]
pub struct OfxRuntime {
    inner: Mutex<Inner>,
}

/// What scanning found.
#[derive(Debug, Default)]
pub struct ScanResult {
    pub definitions: Vec<EffectDef>,
    /// `(bundle or plugin, reason)` for everything that could not be used.
    pub problems: Vec<(String, String)>,
}

fn slot_for(offset: u64, w: u32, h: u32, (depth, components): (Depth, Components)) -> ImageSlot {
    let row = w as usize * components.count() * depth.bytes();
    ImageSlot {
        offset,
        bounds: [0, 0, w as i32, h as i32],
        row_bytes: row as u32,
        depth,
        components,
        premult: Premult::Premultiplied,
    }
}

/// Rows top-down ↔ bottom-up.
fn flip_rows(pixels: &[f32], w: u32, h: u32) -> Vec<f32> {
    let row = w as usize * 4;
    let mut out = Vec::with_capacity(pixels.len());
    for y in (0..h as usize).rev() {
        out.extend_from_slice(&pixels[y * row..(y + 1) * row]);
    }
    out
}

impl OfxRuntime {
    /// A runtime that starts sandboxes from `exe`, with its session folder
    /// under `base` (left-over folders from crashed sessions are removed).
    pub fn new(exe: &Path, base: &Path, timeouts: Timeouts) -> std::io::Result<OfxRuntime> {
        crate::shm::clean_stale(base);
        Ok(OfxRuntime {
            inner: Mutex::new(Inner {
                exe: exe.to_path_buf(),
                session: SessionDir::create(base)?,
                timeouts,
                plugins: HashMap::new(),
                bundles: Vec::new(),
                sandboxes: HashMap::new(),
                instances: HashMap::new(),
                next_instance: 1,
                launches: 0,
                blocklist: Blocklist::new(),
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Describes every bundle, each in its own sandbox, and returns the
    /// effect definitions of the plugins voXY can run.
    pub fn scan(&self, bundles: &[PathBuf]) -> ScanResult {
        let mut inner = self.lock();
        let mut out = ScanResult::default();
        for bundle in bundles {
            let index = inner.bundle_index(bundle);
            let described = inner
                .sandbox(index)
                .and_then(|sb| sb.request(Request::Describe));
            let desc = match described {
                Ok(Reply::Described(d)) => d,
                Ok(other) => {
                    out.problems
                        .push((bundle.display().to_string(), format!("{other:?}")));
                    continue;
                }
                Err(e) => {
                    out.problems
                        .push((bundle.display().to_string(), e.to_string()));
                    inner.sandboxes.remove(&index);
                    continue;
                }
            };
            out.problems.extend(desc.failed);
            for (i, p) in desc.plugins.iter().enumerate() {
                match effect_def(p, bundle, u32::try_from(i).unwrap_or(u32::MAX)) {
                    Ok(def) => {
                        inner.plugins.insert(def.type_id.clone(), entry(bundle, p));
                        out.definitions.push(def);
                    }
                    Err(e) => out.problems.push((p.identifier.clone(), e.to_string())),
                }
            }
        }
        out
    }

    /// Why `type_id` is blocked, if it is.
    pub fn blocked(&self, type_id: &EffectTypeId) -> Option<Blocked> {
        let inner = self.lock();
        let p = inner.plugins.get(type_id)?;
        inner.blocklist.blocked(&p.identifier)
    }

    /// The Retry action: unblocks the plugin; its sandbox restarts cleanly
    /// on the next render.
    pub fn retry(&self, type_id: &EffectTypeId) {
        let mut inner = self.lock();
        if let Some(id) = inner.plugins.get(type_id).map(|p| p.identifier.clone()) {
            inner.blocklist.retry(&id);
        }
    }

    /// Persistent blocks, for Preferences.
    pub fn persistent_blocks(&self) -> Vec<(String, String)> {
        self.lock().blocklist.persistent()
    }

    /// Restores persistent blocks from Preferences.
    pub fn restore_blocks(&self, blocks: Vec<(String, String)>) {
        self.lock().blocklist = Blocklist::with_persistent(blocks);
    }

    /// Ends the plugin instance of an effect that no longer exists.
    pub fn forget(&self, effect: EffectId) {
        let mut inner = self.lock();
        if let Some(live) = inner.instances.remove(&effect)
            && let Some(sb) = inner.sandboxes.get_mut(&live.bundle_index)
        {
            let _ = sb.request(Request::DestroyInstance {
                instance: live.instance,
            });
        }
    }
}

fn entry(bundle: &Path, p: &PluginDesc) -> PluginEntry {
    let kinds = p
        .contexts
        .iter()
        .find(|c| c.context == FILTER)
        .map(|c| c.params.iter().map(|d| (d.name.clone(), d.kind)).collect())
        .unwrap_or_default();
    PluginEntry {
        bundle: bundle.to_path_buf(),
        identifier: p.identifier.clone(),
        kinds,
    }
}

impl Inner {
    fn bundle_index(&mut self, bundle: &Path) -> usize {
        if let Some(i) = self.bundles.iter().position(|b| b == bundle) {
            return i;
        }
        self.bundles.push(bundle.to_path_buf());
        self.bundles.len() - 1
    }

    /// The live sandbox for a bundle, starting one if needed. A dead one is
    /// replaced, and its instances are forgotten.
    fn sandbox(&mut self, index: usize) -> Result<&mut Sandbox, SandboxError> {
        if self
            .sandboxes
            .get(&index)
            .is_some_and(|s| s.dead().is_some())
        {
            self.sandboxes.remove(&index);
            self.instances.retain(|_, l| l.bundle_index != index);
        }
        if !self.sandboxes.contains_key(&index) {
            self.launches += 1;
            let sb = Sandbox::launch(
                &self.exe,
                &self.bundles[index],
                &self.session,
                &format!("b{index}-{}", self.launches),
                HEADER + (1 << 20),
                self.timeouts,
            )?;
            // A new sandbox has loaded nothing yet.
            let sb = self.sandboxes.entry(index).or_insert(sb);
            sb.request(Request::Describe)?;
        }
        Ok(self.sandboxes.get_mut(&index).expect("present"))
    }

    fn snapshot(entry: &PluginEntry, fx: &EvaluatedEffect) -> Vec<ParamSnapshot> {
        fx.params
            .iter()
            .filter_map(|(name, v)| {
                let kind = entry.kinds.get(&**name)?;
                Some(ParamSnapshot {
                    name: name.to_string(),
                    value: to_param_value(*kind, v)?,
                    keys: Vec::new(),
                })
            })
            .collect()
    }

    fn render(&mut self, fx: &EvaluatedEffect, frame: &mut CpuFrame) -> Result<(), String> {
        let entry = self
            .plugins
            .get(&fx.type_id)
            .cloned()
            .ok_or_else(|| "the plugin is not installed".to_owned())?;
        if let Some(b) = self.blocklist.blocked(&entry.identifier) {
            return Err(b.reason);
        }
        let index = self.bundle_index(&entry.bundle);
        match self.render_in(index, &entry, fx, frame) {
            Ok(r) => r,
            Err(e) => {
                let b = self.blocklist.record(&entry.identifier, &e);
                Err(b.reason)
            }
        }
    }

    /// The render, with sandbox failures as the outer error and plugin
    /// status failures as the inner one.
    fn render_in(
        &mut self,
        index: usize,
        entry: &PluginEntry,
        fx: &EvaluatedEffect,
        frame: &mut CpuFrame,
    ) -> Result<Result<(), String>, SandboxError> {
        let (w, h) = (frame.width, frame.height);
        let time = fx.frame as f64;
        let params = Self::snapshot(entry, fx);
        let project = Project {
            size: [f64::from(w), f64::from(h)],
            extent: [f64::from(w), f64::from(h)],
            ..Project::default()
        };
        let live = match self.instances.get(&fx.id).copied() {
            Some(l)
                if l.bundle_index == index
                    && self
                        .sandboxes
                        .get(&index)
                        .is_some_and(|s| s.dead().is_none()) =>
            {
                l
            }
            _ => {
                let instance = self.next_instance;
                self.next_instance += 1;
                let sb = self.sandbox(index)?;
                match sb.request(Request::CreateInstance {
                    instance,
                    plugin: entry.identifier.clone(),
                    context: FILTER.into(),
                    project,
                    params: params.clone(),
                })? {
                    Reply::Done { .. } => {}
                    Reply::Failed { action, status, .. } => {
                        return Ok(Err(format!(
                            "the plugin refused {action} (status {status})"
                        )));
                    }
                    other => return Ok(Err(format!("unexpected reply {other:?}"))),
                }
                let prefs = sb.request(Request::ClipPreferences { instance })?;
                let (mut source, mut output) = (
                    (Depth::Float, Components::Rgba),
                    (Depth::Float, Components::Rgba),
                );
                if let Reply::ClipPreferences { clips, .. } = prefs {
                    for (name, d, c) in clips {
                        match name.as_str() {
                            "Source" => source = (d, c),
                            "Output" => output = (d, c),
                            _ => {}
                        }
                    }
                }
                let l = Live {
                    bundle_index: index,
                    instance,
                    source,
                    output,
                };
                self.instances.insert(fx.id, l);
                l
            }
        };
        let session_len =
            |d: (Depth, Components)| w as usize * h as usize * d.1.count() * d.0.bytes();
        let need =
            HEADER + session_len(live.source) + session_len(live.output) + 2 * crate::shm::ALIGN;
        let dir = self.session.path().to_path_buf();
        let sb = self.sandbox(index)?;
        sb.ensure_mapping(&dir, need)?;
        sb.request(Request::SetParams {
            instance: live.instance,
            time,
            params,
        })?;
        let mut slots = Slots::new(sb.shm.len());
        let (Some(src), Some(dst)) = (
            slots.take(session_len(live.source)),
            slots.take(session_len(live.output)),
        ) else {
            return Ok(Err("the image does not fit the shared memory".into()));
        };
        let bounds = RectI::new(0, 0, w as i32, h as i32);
        let bottom_up = flip_rows(&frame.pixels, w, h);
        let input = Image::from_rgba(
            bounds,
            &bottom_up,
            live.source.0,
            live.source.1,
            Premult::Premultiplied,
        )
        .ok_or_else(|| SandboxError::Protocol("bad input image".into()))?;
        sb.shm.write_from(src, &input.raw());
        let reply = sb.request(Request::Render {
            instance: live.instance,
            time,
            window: [0, 0, w as i32, h as i32],
            scale: [frame.texel_scale, frame.texel_scale],
            inputs: vec![("Source".into(), slot_for(src, w, h, live.source))],
            output: slot_for(dst, w, h, live.output),
        })?;
        match reply {
            Reply::Done { .. } => {}
            Reply::Failed { action, status, .. } => {
                return Ok(Err(format!("the plugin failed {action} (status {status})")));
            }
            other => return Ok(Err(format!("unexpected reply {other:?}"))),
        }
        let mut bytes = vec![0u8; session_len(live.output)];
        sb.shm.read_into(dst, &mut bytes);
        let out = Image::from_raw(
            bounds,
            live.output.0,
            live.output.1,
            Premult::Premultiplied,
            &bytes,
        )
        .ok_or_else(|| SandboxError::Protocol("bad output image".into()))?;
        frame.pixels = flip_rows(&out.to_rgba(), w, h);
        Ok(Ok(()))
    }
}

impl ExternalEffects for OfxRuntime {
    fn supports(&self, type_id: &EffectTypeId) -> bool {
        self.lock().plugins.contains_key(type_id)
    }

    fn render(&self, fx: &EvaluatedEffect, frame: &mut CpuFrame) -> Result<(), String> {
        self.lock().render(fx, frame)
    }
}

/// The type ID an OpenFX plugin registers under.
pub fn plugin_type(identifier: &str) -> EffectTypeId {
    type_id(identifier)
}
