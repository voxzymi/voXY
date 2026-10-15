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

use voxy_core::external::{AUTO_PARAM, CLIP_PARAM_PREFIX, CpuFrame, ExternalEffects, Region};
use voxy_core::glam::DVec2;
use voxy_core::registry::EffectDef;
use voxy_core::{EffectId, EffectTypeId, EvaluatedEffect, Value};

use crate::cache::{BinaryKey, DescribeCache};
use crate::client::{Blocked, Blocklist, Sandbox, SandboxError, Timeouts};
use crate::discovery::{Arch, Os, bundle_binaries};
use crate::effect::Project;
use crate::image::{Components, Depth, Image, Premult, RectI};
use crate::mapping::{effect_def, extra_clips, mapped_context, to_param_value, type_id};
use crate::params::{ParamKind, ParamValue};
use crate::props::ofx_str;
use crate::protocol::{ImageSlot, ParamSnapshot, PluginDesc, Reply, Request};
use crate::shm::{HEADER, SessionDir, Slots};

/// Sandbox launches in this process.
static LAUNCHES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[derive(Debug, Clone)]
struct PluginEntry {
    bundle: PathBuf,
    identifier: String,
    /// The OpenFX context the effect runs in.
    context: String,
    /// The clip the effect's input goes to (`Source`, or a transition's
    /// `SourceFrom`); none for a generator.
    main: Option<String>,
    /// The other input clips, each fed by the layer-reference parameter
    /// `voxy.clip.<name>` (a transition's `SourceTo`).
    extra: Vec<String>,
    /// A transition: its `Transition` parameter follows Auto.
    transition: bool,
    /// Each parameter's OpenFX type, by name.
    kinds: HashMap<String, ParamKind>,
}

#[derive(Debug, Clone)]
struct Live {
    bundle_index: usize,
    instance: u32,
    /// The layer size the instance's project was created with.
    layer_size: DVec2,
    /// Each clip's preferred depth and components (from
    /// `GetClipPreferences`); float RGBA when it gave none.
    clips: HashMap<String, (Depth, Components)>,
}

impl Live {
    fn format(&self, clip: &str) -> (Depth, Components) {
        self.clips
            .get(clip)
            .copied()
            .unwrap_or((Depth::Float, Components::Rgba))
    }
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

fn slot_for(offset: u64, bounds: RectI, (depth, components): (Depth, Components)) -> ImageSlot {
    let row = (bounds.x2 - bounds.x1) as usize * components.count() * depth.bytes();
    ImageSlot {
        offset,
        bounds: [bounds.x1, bounds.y1, bounds.x2, bounds.y2],
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
        self.scan_with(bundles, &mut None)
    }

    /// [`OfxRuntime::scan`] with the discovery cache at `path`: bundles
    /// whose binary is unchanged are not loaded; the cache is updated.
    pub fn scan_cached(&self, bundles: &[PathBuf], path: &Path) -> ScanResult {
        let mut cache = Some(DescribeCache::load(path));
        let out = self.scan_with(bundles, &mut cache);
        if let Some(c) = cache
            && let Err(e) = c.save(path)
        {
            let mut out = out;
            out.problems.push((
                path.display().to_string(),
                format!("the cache was not saved: {e}"),
            ));
            return out;
        }
        out
    }

    fn scan_with(&self, bundles: &[PathBuf], cache: &mut Option<DescribeCache>) -> ScanResult {
        let mut inner = self.lock();
        let mut out = ScanResult::default();
        let mut binaries = Vec::new();
        for bundle in bundles {
            let index = inner.bundle_index(bundle);
            let key = bundle_binaries(bundle, Os::current(), Arch::current())
                .into_iter()
                .find(|b| b.is_file())
                .and_then(|b| BinaryKey::of(&b));
            if let Some(k) = &key {
                binaries.push(k.path.clone());
            }
            let cached = key
                .as_ref()
                .and_then(|k| cache.as_ref().and_then(|c| c.get(k)).cloned());
            let described = match cached {
                Some(d) => Ok(Reply::Described(d)),
                None => inner
                    .sandbox(index)
                    .and_then(|sb| sb.request(Request::Describe)),
            };
            if let (Ok(Reply::Described(d)), Some(k), Some(c)) = (&described, &key, cache.as_mut())
            {
                c.put(bundle, k.clone(), d.clone());
            }
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
        if let Some(c) = cache.as_mut() {
            c.retain_binaries(&binaries);
        }
        out
    }

    /// How many sandboxes are running (for tests and diagnostics).
    pub fn sandboxes_running(&self) -> usize {
        self.lock()
            .sandboxes
            .values()
            .filter(|s| s.dead().is_none())
            .count()
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

    /// Tells `fx`'s plugin that its parameter `param` changed (a user edit,
    /// or a push button pressed): `InstanceChanged` with the new values.
    /// Returns what the plugin changed in response and what it said. The
    /// instance is created if it has none yet, for a layer of
    /// `layer_size` (an existing instance keeps its own).
    pub fn instance_changed(
        &self,
        fx: &EvaluatedEffect,
        layer_size: DVec2,
        param: &str,
    ) -> Result<InstanceChange, String> {
        self.lock().instance_changed(fx, layer_size, param)
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

/// A plugin parameter change, in OpenFX times (frames).
#[derive(Debug, Clone, PartialEq)]
pub enum PluginChange {
    /// `frame: None`: the value now (`paramSetValue`).
    Set {
        name: String,
        frame: Option<f64>,
        value: Value,
    },
    DeleteKey {
        name: String,
        frame: f64,
    },
    DeleteAllKeys {
        name: String,
    },
}

/// What a plugin did in `InstanceChanged`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InstanceChange {
    pub changes: Vec<PluginChange>,
    pub messages: Vec<crate::effect::Message>,
}

fn entry(bundle: &Path, p: &PluginDesc) -> PluginEntry {
    let ctx = mapped_context(p);
    let clips: Vec<String> = ctx
        .map(|c| c.clips.iter().map(|k| k.name.clone()).collect())
        .unwrap_or_default();
    let main = [SOURCE, SOURCE_FROM]
        .into_iter()
        .find(|m| clips.iter().any(|c| c == m))
        .map(str::to_owned);
    PluginEntry {
        bundle: bundle.to_path_buf(),
        identifier: p.identifier.clone(),
        context: ctx.map(|c| c.context.clone()).unwrap_or_default(),
        extra: ctx
            .map(|c| extra_clips(c).map(|k| k.name.clone()).collect())
            .unwrap_or_default(),
        main,
        transition: ctx
            .is_some_and(|c| c.context == ofx_str(voxy_ofx_sys::kOfxImageEffectContextTransition)),
        kinds: ctx
            .map(|c| c.params.iter().map(|d| (d.name.clone(), d.kind)).collect())
            .unwrap_or_default(),
    }
}

/// The filter context's input clip.
const SOURCE: &str = "Source";
/// The transition context's input clip.
const SOURCE_FROM: &str = "SourceFrom";
const OUTPUT: &str = "Output";
/// The transition context's progress parameter.
const TRANSITION: &str = "Transition";

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
            // Channel and mapping names are unique in the process, across
            // runtimes and relaunches.
            let n = LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let sb = Sandbox::launch(
                &self.exe,
                &self.bundles[index],
                &self.session,
                &format!("b{index}-{n}"),
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
                // A transition on Auto runs over the layer's duration
                // (design §5); its keys do not apply then.
                if Self::auto(entry, fx) && &**name == TRANSITION {
                    return Some(ParamSnapshot {
                        name: name.to_string(),
                        value: ParamValue::Doubles(vec![fx.layer_progress]),
                        keys: Vec::new(),
                    });
                }
                Some(ParamSnapshot {
                    name: name.to_string(),
                    value: to_param_value(*kind, v)?,
                    keys: fx
                        .timeline
                        .as_ref()
                        .and_then(|t| t.key_frames(name))
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    fn auto(entry: &PluginEntry, fx: &EvaluatedEffect) -> bool {
        entry.transition && fx.param(AUTO_PARAM) == Some(&Value::Bool(true))
    }

    /// `req` to `fx`'s sandbox, answering the plugin's queries for
    /// parameter values at other times from `fx`'s animation.
    fn ask(
        &mut self,
        index: usize,
        entry: &PluginEntry,
        fx: &EvaluatedEffect,
        req: Request,
    ) -> Result<Reply, SandboxError> {
        let mut answer = |name: &str, frame: f64| {
            if Self::auto(entry, fx) && name == TRANSITION {
                return None;
            }
            let v = fx.timeline.as_ref()?.value_at(name, frame)?;
            to_param_value(*entry.kinds.get(name)?, &v)
        };
        self.sandbox(index)?.request_with(req, &mut answer)
    }

    /// Runs `f` against `fx`'s plugin: refused when the plugin is missing
    /// or blocked; a sandbox failure blocks it.
    fn with_plugin<T>(
        &mut self,
        fx: &EvaluatedEffect,
        f: impl FnOnce(&mut Self, usize, &PluginEntry) -> Result<Result<T, String>, SandboxError>,
    ) -> Result<T, String> {
        let entry = self
            .plugins
            .get(&fx.type_id)
            .cloned()
            .ok_or_else(|| "the plugin is not installed".to_owned())?;
        if let Some(b) = self.blocklist.blocked(&entry.identifier) {
            return Err(b.reason);
        }
        let index = self.bundle_index(&entry.bundle);
        match f(self, index, &entry) {
            Ok(r) => r,
            Err(e) => {
                let b = self.blocklist.record(&entry.identifier, &e);
                Err(b.reason)
            }
        }
    }

    /// `fx`'s plugin instance for a layer of `layer_size`, created (with
    /// its clip preferences) if it has none in a live sandbox, or if the
    /// layer's size changed (the project size is fixed per instance).
    fn instance(
        &mut self,
        index: usize,
        entry: &PluginEntry,
        fx: &EvaluatedEffect,
        layer_size: DVec2,
    ) -> Result<Result<Live, String>, SandboxError> {
        let alive = self
            .sandboxes
            .get(&index)
            .is_some_and(|s| s.dead().is_none());
        match self.instances.get(&fx.id).cloned() {
            Some(l) if l.bundle_index == index && alive && l.layer_size == layer_size => {
                return Ok(Ok(l));
            }
            Some(l) if l.bundle_index == index && alive => {
                self.instances.remove(&fx.id);
                self.sandbox(index)?.request(Request::DestroyInstance {
                    instance: l.instance,
                })?;
            }
            _ => {}
        }
        let instance = self.next_instance;
        self.next_instance += 1;
        match self.ask(
            index,
            entry,
            fx,
            Request::CreateInstance {
                instance,
                plugin: entry.identifier.clone(),
                context: entry.context.clone(),
                project: Project {
                    size: layer_size.to_array(),
                    extent: layer_size.to_array(),
                    ..Project::default()
                },
                params: Self::snapshot(entry, fx),
            },
        )? {
            Reply::Done { .. } => {}
            Reply::Failed { action, status, .. } => {
                return Ok(Err(format!(
                    "the plugin refused {action} (status {status})"
                )));
            }
            other => return Ok(Err(format!("unexpected reply {other:?}"))),
        }
        let prefs = self.ask(index, entry, fx, Request::ClipPreferences { instance })?;
        let clips = match prefs {
            Reply::ClipPreferences { clips, .. } => {
                clips.into_iter().map(|(n, d, c)| (n, (d, c))).collect()
            }
            _ => HashMap::new(),
        };
        let l = Live {
            bundle_index: index,
            instance,
            layer_size,
            clips,
        };
        self.instances.insert(fx.id, l.clone());
        Ok(Ok(l))
    }

    /// The live instance with `fx`'s parameters at its frame.
    fn prepared(
        &mut self,
        index: usize,
        entry: &PluginEntry,
        fx: &EvaluatedEffect,
        layer_size: DVec2,
    ) -> Result<Result<Live, String>, SandboxError> {
        let live = match self.instance(index, entry, fx, layer_size)? {
            Ok(l) => l,
            Err(e) => return Ok(Err(e)),
        };
        self.sandbox(index)?.request(Request::SetParams {
            instance: live.instance,
            time: fx.frame as f64,
            params: Self::snapshot(entry, fx),
        })?;
        Ok(Ok(live))
    }

    fn region_of_definition(
        &mut self,
        fx: &EvaluatedEffect,
        input: Region,
        layer_size: DVec2,
    ) -> Result<Region, String> {
        self.with_plugin(fx, |this, index, entry| {
            let live = match this.prepared(index, entry, fx, layer_size)? {
                Ok(l) => l,
                Err(e) => return Ok(Err(e)),
            };
            // Every input (the layer, and the layers its extra clips read)
            // covers the same region.
            let h = layer_size.y;
            let inputs = entry
                .main
                .iter()
                .chain(&entry.extra)
                .map(|c| (c.clone(), canonical(input, h)))
                .collect();
            Ok(
                match this.ask(
                    index,
                    entry,
                    fx,
                    Request::RegionOfDefinition {
                        instance: live.instance,
                        time: fx.frame as f64,
                        // Regions are canonical; the scale does not change them.
                        scale: [1.0, 1.0],
                        inputs,
                    },
                )? {
                    Reply::Region(r) => Ok(from_canonical(r, h)),
                    Reply::Failed { action, status, .. } => {
                        Err(format!("the plugin failed {action} (status {status})"))
                    }
                    other => Err(format!("unexpected reply {other:?}")),
                },
            )
        })
    }

    fn instance_changed(
        &mut self,
        fx: &EvaluatedEffect,
        layer_size: DVec2,
        param: &str,
    ) -> Result<InstanceChange, String> {
        let size = self
            .instances
            .get(&fx.id)
            .map_or(layer_size, |l| l.layer_size);
        self.with_plugin(fx, |this, index, entry| {
            let live = match this.prepared(index, entry, fx, size)? {
                Ok(l) => l,
                Err(e) => return Ok(Err(e)),
            };
            let reply = this.ask(
                index,
                entry,
                fx,
                Request::InstanceChanged {
                    instance: live.instance,
                    param: param.to_owned(),
                    time: fx.frame as f64,
                },
            )?;
            Ok(match reply {
                Reply::Done { messages, edits } => Ok(InstanceChange {
                    changes: edits
                        .into_iter()
                        .filter_map(|e| plugin_change(entry, e))
                        .collect(),
                    messages,
                }),
                Reply::Failed { action, status, .. } => {
                    Err(format!("the plugin failed {action} (status {status})"))
                }
                other => Err(format!("unexpected reply {other:?}")),
            })
        })
    }

    fn render(&mut self, fx: &EvaluatedEffect, frame: &mut CpuFrame) -> Result<(), String> {
        self.with_plugin(fx, |this, index, entry| {
            this.render_in(index, entry, fx, frame)
        })
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
        let s = frame.texel_scale;
        let time = fx.frame as f64;
        let layer_size = layer_size_of(frame);
        let live = match self.prepared(index, entry, fx, layer_size)? {
            Ok(l) => l,
            Err(e) => return Ok(Err(e)),
        };
        // The buffer in the plugin's pixel coordinates: canonical (y up,
        // the project being the layer) times the render scale.
        let x0 = (frame.origin.x * s).round() as i32;
        let y1 = ((layer_size.y - frame.origin.y) * s).round() as i32;
        let bounds = RectI::new(x0, y1 - h as i32, x0 + w as i32, y1);
        let window = [bounds.x1, bounds.y1, bounds.x2, bounds.y2];
        let scale = [s, s];
        let image_len =
            |d: (Depth, Components)| w as usize * h as usize * d.1.count() * d.0.bytes();
        // The input clips and their images (rows top-down): the effect's
        // input, then each extra clip's layer, or transparency when its
        // parameter names no active layer.
        let transparent;
        let mut sources: Vec<(&str, &[f32])> = Vec::new();
        if let Some(m) = &entry.main {
            sources.push((m, &frame.pixels));
        }
        if !entry.extra.is_empty() {
            transparent = vec![0.0f32; frame.pixels.len()];
            for clip in &entry.extra {
                let pixels = layer_pixels(frame, clip).unwrap_or(&transparent);
                sources.push((clip, pixels));
            }
        }
        let need = HEADER
            + sources
                .iter()
                .map(|(c, _)| image_len(live.format(c)) + crate::shm::ALIGN)
                .sum::<usize>()
            + image_len(live.format(OUTPUT))
            + crate::shm::ALIGN;
        let dir = self.session.path().to_path_buf();
        self.sandbox(index)?.ensure_mapping(&dir, need)?;
        // An identity skips Render: an input passes through. An identity at
        // another time needs images the editor does not send yet, so the
        // plugin renders instead.
        if !sources.is_empty()
            && let Reply::Identity(Some((clip, at))) = self.ask(
                index,
                entry,
                fx,
                Request::IsIdentity {
                    instance: live.instance,
                    time,
                    window,
                    scale,
                },
            )?
            && at == time
            && let Some((_, pixels)) = sources.iter().find(|(c, _)| *c == clip)
        {
            let pixels = pixels.to_vec();
            frame.pixels = pixels;
            return Ok(Ok(()));
        }
        let sb = self.sandbox(index)?;
        let mut slots = Slots::new(sb.shm.len());
        let mut inputs = Vec::new();
        for (clip, pixels) in &sources {
            let format = live.format(clip);
            let Some(at) = slots.take(image_len(format)) else {
                return Ok(Err("the image does not fit the shared memory".into()));
            };
            let image = Image::from_rgba(
                bounds,
                &flip_rows(pixels, w, h),
                format.0,
                format.1,
                Premult::Premultiplied,
            )
            .ok_or_else(|| SandboxError::Protocol("bad input image".into()))?;
            sb.shm.write_from(at, &image.raw());
            inputs.push(((*clip).to_owned(), slot_for(at, bounds, format)));
        }
        let output = live.format(OUTPUT);
        let Some(dst) = slots.take(image_len(output)) else {
            return Ok(Err("the image does not fit the shared memory".into()));
        };
        let reply = self.ask(
            index,
            entry,
            fx,
            Request::Render {
                instance: live.instance,
                time,
                window,
                scale,
                inputs,
                output: slot_for(dst, bounds, output),
            },
        )?;
        match reply {
            Reply::Done { .. } => {}
            Reply::Failed { action, status, .. } => {
                return Ok(Err(format!("the plugin failed {action} (status {status})")));
            }
            other => return Ok(Err(format!("unexpected reply {other:?}"))),
        }
        let mut bytes = vec![0u8; image_len(output)];
        self.sandbox(index)?.shm.read_into(dst, &mut bytes);
        let out = Image::from_raw(bounds, output.0, output.1, Premult::Premultiplied, &bytes)
            .ok_or_else(|| SandboxError::Protocol("bad output image".into()))?;
        frame.pixels = flip_rows(&out.to_rgba(), w, h);
        Ok(Ok(()))
    }
}

/// A plugin's edit as a voXY change; `None` for a parameter it does not
/// have or a value that does not fit.
fn plugin_change(entry: &PluginEntry, e: crate::protocol::ParamEdit) -> Option<PluginChange> {
    use crate::protocol::ParamEdit;
    Some(match e {
        ParamEdit::Set { name, time, value } => {
            let kind = *entry.kinds.get(&name)?;
            PluginChange::Set {
                value: crate::mapping::to_value(kind, &value)?,
                name,
                frame: time,
            }
        }
        ParamEdit::DeleteKey { name, time } => PluginChange::DeleteKey { name, frame: time },
        ParamEdit::DeleteAllKeys { name } => PluginChange::DeleteAllKeys { name },
    })
}

/// The pixels of the layer feeding `clip`, if its parameter names one.
fn layer_pixels<'f>(frame: &'f CpuFrame, clip: &str) -> Option<&'f [f32]> {
    frame
        .layers
        .iter()
        .find(|(n, _)| n.strip_prefix(CLIP_PARAM_PREFIX) == Some(clip))
        .map(|(_, p)| p.as_slice())
        .filter(|p| p.len() == frame.pixels.len())
}

/// The layer's size, or the buffer's in layer pixels when it is unknown.
fn layer_size_of(frame: &CpuFrame) -> DVec2 {
    if frame.layer_size.x > 0.0 && frame.layer_size.y > 0.0 {
        frame.layer_size
    } else {
        DVec2::new(f64::from(frame.width), f64::from(frame.height)) / frame.texel_scale.max(1e-9)
    }
}

/// A layer region in OpenFX canonical coordinates (`[x1, y1, x2, y2]`, y up
/// from the layer's bottom edge), for a layer `h` pixels tall.
fn canonical(r: Region, h: f64) -> [f64; 4] {
    [r.min.x, h - r.max.y, r.max.x, h - r.min.y]
}

/// The inverse of [`canonical`].
fn from_canonical(c: [f64; 4], h: f64) -> Region {
    Region {
        min: DVec2::new(c[0], h - c[3]),
        max: DVec2::new(c[2], h - c[1]),
    }
}

impl ExternalEffects for OfxRuntime {
    fn supports(&self, type_id: &EffectTypeId) -> bool {
        self.lock().plugins.contains_key(type_id)
    }

    fn render(&self, fx: &EvaluatedEffect, frame: &mut CpuFrame) -> Result<(), String> {
        self.lock().render(fx, frame)
    }

    /// The plugin's `GetRegionOfDefinition`; the input when it fails (the
    /// render then reports the failure).
    fn region_of_definition(
        &self,
        fx: &EvaluatedEffect,
        input: Region,
        layer_size: DVec2,
    ) -> Region {
        self.lock()
            .region_of_definition(fx, input, layer_size)
            .unwrap_or(input)
    }
}

/// The type ID an OpenFX plugin registers under.
pub fn plugin_type(identifier: &str) -> EffectTypeId {
    type_id(identifier)
}
