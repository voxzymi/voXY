//! The sandbox side of the protocol: one bundle, its plugins and their
//! instances, serving requests from the editor until `Unload` or until the
//! channel closes. Runs only in `voxy-ofx-sandbox`.
//!
//! Images come and go through shared memory: for a render, each input slot
//! is copied into an image the plugin reads, and the output image is copied
//! back into its slot afterwards. A heartbeat thread bumps the counter in
//! the mapping and passes the editor's abort flag on to every instance.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::driver::{Description, DriverError, Identity, Instance, Plugin};
use super::loader::load_bundle;
use crate::effect::{Effect, Message};
use crate::image::{Image, ImageSource, RectI};
use crate::params::{ParamValue, ParamValues};
use crate::protocol::{
    self, BundleDesc, ImageSlot, ParamEdit, ParamSnapshot, PluginDesc, Reply, Request, VERSION,
};
use crate::shm::SharedMemory;

/// How often the heartbeat thread beats.
pub const HEARTBEAT: Duration = Duration::from_millis(25);

/// A channel both ways.
trait Duplex: Read + Write + Send {}
impl<T: Read + Write + Send> Duplex for T {}

/// The channel to the editor, shared so that a plugin's parameter query
/// can be sent in the middle of an action (from any of its threads).
#[derive(Default)]
struct Editor {
    channel: Mutex<Option<Box<dyn Duplex>>>,
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Editor")
    }
}

impl Editor {
    /// Asks the editor for `name` at `time`; `None` when it has no answer
    /// (or the channel failed, which the serve loop then sees too).
    fn value_at(&self, name: &str, time: f64) -> Option<ParamValue> {
        let mut channel = self.channel.lock().unwrap_or_else(PoisonError::into_inner);
        let ch = channel.as_mut()?;
        let ask = Reply::ValueAt {
            name: name.to_owned(),
            time,
        };
        protocol::write_frame(&mut **ch, &ask).ok()?;
        match protocol::read_frame::<Request, _>(&mut **ch).ok()? {
            Request::Value { value } => value,
            _ => None,
        }
    }
}

/// Parameter values from the editor's snapshot, recording what the plugin
/// changes. A value at another time than the snapshot's, of a parameter
/// with keys, is asked of the editor (design §6.7).
#[derive(Debug)]
struct SnapshotValues {
    values: Mutex<HashMap<String, (ParamValue, Vec<f64>)>>,
    /// The snapshot's time (NaN before the first `SetParams`).
    time: Mutex<f64>,
    editor: Arc<Editor>,
    edits: Mutex<Vec<ParamEdit>>,
}

impl SnapshotValues {
    fn new(editor: Arc<Editor>) -> SnapshotValues {
        SnapshotValues {
            values: Mutex::default(),
            time: Mutex::new(f64::NAN),
            editor,
            edits: Mutex::default(),
        }
    }

    fn load(&self, time: f64, params: Vec<ParamSnapshot>) {
        *self.time.lock().unwrap_or_else(PoisonError::into_inner) = time;
        let mut v = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        for p in params {
            v.insert(p.name, (p.value, p.keys));
        }
    }

    fn take_edits(&self) -> Vec<ParamEdit> {
        std::mem::take(&mut *self.edits.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn edit(&self, e: ParamEdit) {
        self.edits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(e);
    }
}

impl ParamValues for SnapshotValues {
    fn value_at(&self, name: &str, time: f64) -> Option<ParamValue> {
        let (value, keyed) = {
            let v = self.values.lock().unwrap_or_else(PoisonError::into_inner);
            let (value, keys) = v.get(name)?;
            (value.clone(), !keys.is_empty())
        };
        let at = *self.time.lock().unwrap_or_else(PoisonError::into_inner);
        if !keyed || time == at {
            return Some(value);
        }
        Some(self.editor.value_at(name, time).unwrap_or(value))
    }

    fn key_times(&self, name: &str) -> Vec<f64> {
        let v = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        v.get(name).map(|(_, k)| k.clone()).unwrap_or_default()
    }

    fn set(&self, name: &str, time: Option<f64>, value: ParamValue) {
        {
            let mut v = self.values.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = v
                .entry(name.to_owned())
                .or_insert((value.clone(), Vec::new()));
            entry.0 = value.clone();
            if let Some(t) = time
                && !entry.1.contains(&t)
            {
                entry.1.push(t);
                entry.1.sort_by(f64::total_cmp);
            }
        }
        self.edit(ParamEdit::Set {
            name: name.to_owned(),
            time,
            value,
        });
    }

    fn delete_key(&self, name: &str, time: f64) -> bool {
        let found = {
            let mut v = self.values.lock().unwrap_or_else(PoisonError::into_inner);
            v.get_mut(name).is_some_and(|(_, k)| {
                let before = k.len();
                k.retain(|t| *t != time);
                k.len() != before
            })
        };
        if found {
            self.edit(ParamEdit::DeleteKey {
                name: name.to_owned(),
                time,
            });
        }
        found
    }

    fn delete_all_keys(&self, name: &str) {
        if let Some((_, k)) = self
            .values
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(name)
        {
            k.clear();
        }
        self.edit(ParamEdit::DeleteAllKeys {
            name: name.to_owned(),
        });
    }
}

/// The images of the current request.
#[derive(Debug, Default)]
struct SlotImages {
    images: Mutex<HashMap<String, Arc<Image>>>,
    rods: Mutex<HashMap<String, [f64; 4]>>,
}

impl ImageSource for SlotImages {
    fn image(&self, clip: &str, _time: f64) -> Option<Arc<Image>> {
        self.images
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(clip)
            .cloned()
    }

    fn region_of_definition(&self, clip: &str, _time: f64) -> Option<[f64; 4]> {
        self.rods
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(clip)
            .copied()
    }
}

struct Live {
    inst: Instance<'static>,
    values: Arc<SnapshotValues>,
    images: Arc<SlotImages>,
}

/// One bundle's server.
pub struct Server {
    bundle: Vec<&'static voxy_ofx_sys::OfxPlugin>,
    plugins: Vec<(&'static Plugin, Description)>,
    instances: HashMap<u32, Live>,
    shm: Arc<Mutex<Option<Arc<SharedMemory>>>>,
    effects: Arc<Mutex<Vec<Arc<Effect>>>>,
    stop: Arc<AtomicBool>,
    editor: Arc<Editor>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("plugins", &self.bundle.len())
            .field("instances", &self.instances.len())
            .finish_non_exhaustive()
    }
}

fn slot_rect(s: &ImageSlot) -> RectI {
    RectI::new(s.bounds[0], s.bounds[1], s.bounds[2], s.bounds[3])
}

fn slot_len(s: &ImageSlot) -> Option<usize> {
    (s.row_bytes as usize).checked_mul(slot_rect(s).height())
}

fn failed(e: DriverError, messages: Vec<Message>) -> Reply {
    match e {
        DriverError::Action { action, status } => Reply::Failed {
            action,
            status,
            messages,
        },
        other => Reply::Error(other.to_string()),
    }
}

impl Server {
    /// Loads the bundle at `path` and starts the heartbeat.
    ///
    /// # Safety
    /// Loads and runs the plugin binary: call it only in the sandbox.
    pub unsafe fn new(path: &Path) -> Result<Server, String> {
        // SAFETY: the caller's contract.
        let bundle = unsafe { load_bundle(path) }.map_err(|e| e.to_string())?;
        let server = Server {
            bundle: bundle.plugins,
            plugins: Vec::new(),
            instances: HashMap::new(),
            shm: Arc::new(Mutex::new(None)),
            effects: Arc::new(Mutex::new(Vec::new())),
            stop: Arc::new(AtomicBool::new(false)),
            editor: Arc::new(Editor::default()),
        };
        let (shm, effects, stop) = (
            Arc::clone(&server.shm),
            Arc::clone(&server.effects),
            Arc::clone(&server.stop),
        );
        std::thread::Builder::new()
            .name("ofx-heartbeat".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let mapped = shm.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    if let Some(m) = mapped {
                        m.beat();
                        let abort = m.aborted();
                        for fx in effects
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .iter()
                        {
                            if abort {
                                fx.set_abort(true);
                            }
                        }
                    }
                    std::thread::sleep(HEARTBEAT);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(server)
    }

    /// Serves frames from `channel` until `Unload` or the channel closes.
    /// The channel is shared with the instances' parameter queries, which
    /// use it while an action runs (the lock is free then).
    pub fn serve(
        &mut self,
        channel: impl Read + Write + Send + 'static,
    ) -> Result<(), protocol::FrameError> {
        *self.channel() = Some(Box::new(channel));
        loop {
            let read = match self.channel().as_mut() {
                Some(c) => protocol::read_frame::<Request, _>(&mut **c),
                None => return Ok(()),
            };
            let request = match read {
                Ok(r) => r,
                Err(protocol::FrameError::Io(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.shutdown();
                    return Ok(());
                }
                Err(e) => return Err(e),
            };
            let last = request == Request::Unload;
            let reply = self.handle(request);
            if let Some(c) = self.channel().as_mut() {
                protocol::write_frame(&mut **c, &reply)?;
            }
            if last {
                return Ok(());
            }
        }
    }

    fn channel(&self) -> std::sync::MutexGuard<'_, Option<Box<dyn Duplex>>> {
        self.editor
            .channel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn shutdown(&mut self) {
        self.instances.clear();
        for (p, _) in &self.plugins {
            p.unload();
        }
        self.stop.store(true, Ordering::Relaxed);
    }

    fn live(&mut self, id: u32) -> Result<&mut Live, Reply> {
        self.instances
            .get_mut(&id)
            .ok_or_else(|| Reply::Error(format!("no instance {id}")))
    }

    fn done(live: &Live) -> Reply {
        Reply::Done {
            messages: live.inst.fx.take_messages(),
            edits: live.values.take_edits(),
        }
    }

    /// Answers one request.
    pub fn handle(&mut self, request: Request) -> Reply {
        match self.answer(request) {
            Ok(r) | Err(r) => r,
        }
    }

    fn answer(&mut self, request: Request) -> Result<Reply, Reply> {
        Ok(match request {
            Request::Hello { version } => {
                if version != VERSION {
                    return Err(Reply::Error(format!(
                        "protocol version {version}, sandbox speaks {VERSION}"
                    )));
                }
                Reply::Hello {
                    version: VERSION,
                    pid: std::process::id(),
                }
            }
            Request::Map { path } => {
                let m = SharedMemory::open(Path::new(&path))
                    .map_err(|e| Reply::Error(e.to_string()))?;
                *self.shm.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(m));
                Reply::Done {
                    messages: Vec::new(),
                    edits: Vec::new(),
                }
            }
            Request::Describe => self.describe(),
            Request::CreateInstance {
                instance,
                plugin,
                context,
                project,
                params,
            } => {
                let (p, d) = self
                    .plugins
                    .iter()
                    .find(|(p, _)| p.identifier == plugin)
                    .ok_or_else(|| Reply::Error(format!("no plugin {plugin}")))?;
                let values = Arc::new(SnapshotValues::new(Arc::clone(&self.editor)));
                values.load(f64::NAN, params);
                let images = Arc::new(SlotImages::default());
                let inst = p
                    .create_instance(d, &context, project, values.clone(), images.clone())
                    .map_err(|e| failed(e, Vec::new()))?;
                values.take_edits();
                self.effects
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Arc::clone(&inst.fx));
                let live = Live {
                    inst,
                    values,
                    images,
                };
                let reply = Self::done(&live);
                self.instances.insert(instance, live);
                reply
            }
            Request::DestroyInstance { instance } => {
                let live = self
                    .instances
                    .remove(&instance)
                    .ok_or_else(|| Reply::Error(format!("no instance {instance}")))?;
                self.effects
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retain(|fx| !Arc::ptr_eq(fx, &live.inst.fx));
                let messages = live.inst.fx.take_messages();
                drop(live);
                Reply::Done {
                    messages,
                    edits: Vec::new(),
                }
            }
            Request::SetParams {
                instance,
                time,
                params,
            } => {
                let live = self.live(instance)?;
                live.values.load(time, params);
                live.inst.fx.params.set_time(time);
                Self::done(live)
            }
            Request::ClipPreferences { instance } => {
                let live = self.live(instance)?;
                let prefs = live
                    .inst
                    .clip_preferences()
                    .map_err(|e| failed(e, live.inst.fx.take_messages()))?;
                let mut clips: Vec<_> = prefs
                    .clips
                    .into_iter()
                    .map(|(n, (d, c))| (n, d, c))
                    .collect();
                clips.sort_by(|a, b| a.0.cmp(&b.0));
                Reply::ClipPreferences {
                    clips,
                    output_premult: prefs.output_premult,
                    frame_varying: prefs.frame_varying,
                }
            }
            Request::RegionOfDefinition {
                instance,
                time,
                scale,
                inputs,
            } => {
                let live = self.live(instance)?;
                *live
                    .images
                    .rods
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = inputs.into_iter().collect();
                let r = live
                    .inst
                    .region_of_definition(time, scale)
                    .map_err(|e| failed(e, live.inst.fx.take_messages()))?;
                Reply::Region(r)
            }
            Request::RegionsOfInterest {
                instance,
                time,
                scale,
                region,
            } => {
                let live = self.live(instance)?;
                let r = live
                    .inst
                    .regions_of_interest(time, scale, region)
                    .map_err(|e| failed(e, live.inst.fx.take_messages()))?;
                let mut r: Vec<_> = r.into_iter().collect();
                r.sort_by(|a, b| a.0.cmp(&b.0));
                Reply::Regions(r)
            }
            Request::IsIdentity {
                instance,
                time,
                window,
                scale,
            } => {
                let live = self.live(instance)?;
                let id = live
                    .inst
                    .is_identity(time, window, scale)
                    .map_err(|e| failed(e, live.inst.fx.take_messages()))?;
                Reply::Identity(match id {
                    Identity::No => None,
                    Identity::Clip { name, time } => Some((name, time)),
                })
            }
            Request::Render {
                instance,
                time,
                window,
                scale,
                inputs,
                output,
            } => self.render(instance, time, window, scale, &inputs, output)?,
            Request::Sequence {
                instance,
                begin,
                frames,
                interactive,
            } => {
                let live = self.live(instance)?;
                live.inst
                    .sequence(begin, frames, interactive)
                    .map_err(|e| failed(e, live.inst.fx.take_messages()))?;
                Self::done(live)
            }
            Request::InstanceChanged {
                instance,
                param,
                time,
            } => {
                let live = self.live(instance)?;
                live.inst
                    .param_changed(&param, time)
                    .map_err(|e| failed(e, live.inst.fx.take_messages()))?;
                Self::done(live)
            }
            // Only expected inside an action, where `Editor::value_at`
            // reads it.
            Request::Value { .. } => {
                return Err(Reply::Error("a value with no query pending".into()));
            }
            Request::Unload => {
                self.shutdown();
                Reply::Done {
                    messages: Vec::new(),
                    edits: Vec::new(),
                }
            }
        })
    }

    fn describe(&mut self) -> Reply {
        let mut out = BundleDesc::default();
        if self.plugins.is_empty() {
            for (i, raw) in self.bundle.iter().enumerate() {
                // SAFETY: a plugin struct the loaded binary returned.
                match unsafe { Plugin::load(raw) } {
                    Ok(p) => {
                        let p: &'static Plugin = Box::leak(Box::new(p));
                        match p.describe() {
                            Ok(d) => self.plugins.push((p, d)),
                            Err(e) => {
                                out.failed.push((p.identifier.clone(), e.to_string()));
                                p.unload();
                            }
                        }
                    }
                    Err(e) => out.failed.push((format!("#{i}"), e.to_string())),
                }
            }
        }
        out.plugins = self
            .plugins
            .iter()
            .map(|(p, d)| PluginDesc::from_description(&p.identifier, p.version, d))
            .collect();
        Reply::Described(out)
    }

    fn render(
        &mut self,
        instance: u32,
        time: f64,
        window: [i32; 4],
        scale: [f64; 2],
        inputs: &[(String, ImageSlot)],
        output: ImageSlot,
    ) -> Result<Reply, Reply> {
        let shm = self
            .shm
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| Reply::Error("no shared memory mapped".into()))?;
        let read = |s: &ImageSlot| -> Result<Arc<Image>, Reply> {
            let len = slot_len(s).ok_or_else(|| Reply::Error("bad slot".into()))?;
            let mut bytes = vec![0u8; len];
            if !shm.read_into(s.offset, &mut bytes) {
                return Err(Reply::Error("slot outside the mapping".into()));
            }
            Image::from_raw(slot_rect(s), s.depth, s.components, s.premult, &bytes)
                .map(Arc::new)
                .ok_or_else(|| Reply::Error("slot size does not match its image".into()))
        };
        let mut images = HashMap::new();
        let mut rods = HashMap::new();
        for (name, s) in inputs {
            images.insert(name.clone(), read(s)?);
            // The clip's region in canonical coordinates: its pixel bounds
            // over the render scale.
            let b = s.bounds;
            let (sx, sy) = (scale[0].max(1e-9), scale[1].max(1e-9));
            rods.insert(
                name.clone(),
                [
                    f64::from(b[0]) / sx,
                    f64::from(b[1]) / sy,
                    f64::from(b[2]) / sx,
                    f64::from(b[3]) / sy,
                ],
            );
        }
        let out_name =
            crate::props::ofx_str(voxy_ofx_sys::kOfxImageEffectOutputClipName).to_owned();
        let out = read(&output)?;
        images.insert(out_name, Arc::clone(&out));
        let live = self.live(instance)?;
        *live
            .images
            .images
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = images;
        *live
            .images
            .rods
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = rods;
        let result = live.inst.render(time, window, scale);
        live.images
            .images
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        result.map_err(|e| failed(e, live.inst.fx.take_messages()))?;
        if !shm.write_from(output.offset, &out.raw()) {
            return Err(Reply::Error("output slot outside the mapping".into()));
        }
        Ok(Self::done(live))
    }
}
