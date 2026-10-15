//! Drives a plugin through its actions: Load, Describe, `DescribeInContext`,
//! `CreateInstance`, the render-time actions, and teardown.
//!
//! In O-A this runs only in the in-process test harness, on plugins linked
//! into the test binary. From O-B the same code runs inside
//! `voxy-ofx-sandbox` on a loaded bundle. The editor never calls it
//! (design §6.3).

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;

use voxy_ofx_sys as ofx;

use super::image_effect::{effect_handle, release_effect_handle};
use super::{HostStruct, guard, prop_handle, release_prop_handle};
use crate::clip::ClipInfo;
use crate::effect::{Effect, Project, TimeLine};
use crate::host::{self, host_props};
use crate::image::{Components, Depth, ImageSource, Premult};
use crate::params::{ParamKind, ParamValues};
use crate::props::{PropSet, Values, ofx_str};

/// An action that did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverError {
    /// The plugin is not an image effect plugin this host speaks.
    WrongApi { api: String, version: i32 },
    /// The plugin has no `setHost` or `mainEntry`.
    MissingEntry,
    /// An action returned an error status.
    Action {
        action: String,
        status: ofx::OfxStatus,
    },
    /// The plugin supports none of the host's contexts.
    NoContext,
    /// A context the plugin did not describe.
    UnknownContext(String),
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriverError::WrongApi { api, version } => {
                write!(f, "not an image effect plugin ({api} v{version})")
            }
            DriverError::MissingEntry => f.write_str("the plugin has no entry points"),
            DriverError::Action { action, status } => {
                write!(f, "{action} failed with status {status}")
            }
            DriverError::NoContext => f.write_str("the plugin supports no context voXY hosts"),
            DriverError::UnknownContext(c) => write!(f, "the plugin did not describe context {c}"),
        }
    }
}

impl std::error::Error for DriverError {}

/// What an action returned when it did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// `kOfxStatOK`: the plugin handled it.
    Handled,
    /// `kOfxStatReplyDefault`: the host does the default.
    Default,
}

/// One plugin, set up with the host and loaded.
#[derive(Debug)]
pub struct Plugin {
    raw: &'static ofx::OfxPlugin,
    host: HostStruct,
    pub identifier: String,
    pub version: (u32, u32),
    loaded: std::sync::atomic::AtomicBool,
}

/// Everything Describe and `DescribeInContext` reported.
#[derive(Debug)]
pub struct Description {
    pub general: Arc<Effect>,
    /// Context name (without NUL) → its descriptor.
    pub contexts: Vec<(String, Arc<Effect>)>,
}

impl Description {
    pub fn context(&self, name: &str) -> Option<&Arc<Effect>> {
        self.contexts
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, e)| e)
    }
}

/// A temporary argument set for one action.
#[derive(Debug)]
struct Args(Arc<PropSet>);

impl Args {
    fn new() -> Args {
        Args(Arc::new(PropSet::new("action arguments")))
    }

    fn handle(&self) -> ofx::OfxPropertySetHandle {
        prop_handle(&self.0)
    }
}

impl Drop for Args {
    fn drop(&mut self) {
        release_prop_handle(&self.0);
    }
}

fn s(name: &[u8]) -> Values {
    PropSet::consts(&[name])
}

impl Plugin {
    /// Checks the plugin's API, hands it the host and runs Load.
    ///
    /// # Safety
    /// `raw` is a valid `OfxPlugin` whose function pointers follow the
    /// OpenFX ABI.
    pub unsafe fn load(raw: &'static ofx::OfxPlugin) -> Result<Plugin, DriverError> {
        // SAFETY: the caller's contract: valid C strings.
        let (api, identifier) = unsafe {
            (
                CStr::from_ptr(raw.pluginApi).to_string_lossy().into_owned(),
                CStr::from_ptr(raw.pluginIdentifier)
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        if api != ofx_str(ofx::kOfxImageEffectPluginApi)
            || raw.apiVersion != ofx::kOfxImageEffectPluginApiVersion as i32
        {
            return Err(DriverError::WrongApi {
                api,
                version: raw.apiVersion,
            });
        }
        let (Some(set_host), Some(_)) = (raw.setHost, raw.mainEntry) else {
            return Err(DriverError::MissingEntry);
        };
        let mut host = HostStruct::new(host_props());
        // SAFETY: the plugin keeps the pointer while loaded; `host` lives
        // as long as this `Plugin` and its box never moves.
        unsafe { set_host(host.as_ptr()) };
        let plugin = Plugin {
            raw,
            host,
            identifier,
            version: (raw.pluginVersionMajor, raw.pluginVersionMinor),
            loaded: std::sync::atomic::AtomicBool::new(false),
        };
        // SAFETY: the plugin's own entry point.
        unsafe { plugin.call(ofx::kOfxActionLoad, std::ptr::null(), None, None) }?;
        plugin
            .loaded
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(plugin)
    }

    /// Runs one action.
    ///
    /// # Safety
    /// `handle` is null or a handle the plugin may receive for `action`.
    unsafe fn call(
        &self,
        action: &[u8],
        handle: *const c_void,
        in_args: Option<&Args>,
        out_args: Option<&Args>,
    ) -> Result<Reply, DriverError> {
        let main = self.raw.mainEntry.ok_or(DriverError::MissingEntry)?;
        let ins = in_args.map_or(std::ptr::null_mut(), Args::handle);
        let outs = out_args.map_or(std::ptr::null_mut(), Args::handle);
        // SAFETY: the caller's contract; the action name is a C string.
        let status = unsafe { main(action.as_ptr().cast::<c_char>(), handle, ins, outs) };
        match status {
            ofx::kOfxStatOK => Ok(Reply::Handled),
            ofx::kOfxStatReplyDefault => Ok(Reply::Default),
            status => Err(DriverError::Action {
                action: ofx_str(action).to_owned(),
                status,
            }),
        }
    }

    /// `fetchSuite`, for tests.
    pub fn host(&self) -> &HostStruct {
        &self.host
    }

    /// Describe, then `DescribeInContext` for each context both sides
    /// support.
    pub fn describe(&self) -> Result<Description, DriverError> {
        let general = Arc::new(Effect::descriptor());
        let h = effect_handle(&general);
        // SAFETY: a descriptor handle for Describe.
        let r = unsafe { self.call(ofx::kOfxActionDescribe, h.cast(), None, None) };
        release_effect_handle(&general);
        r?;
        let wanted = general
            .props
            .strings(ofx_str(ofx::kOfxImageEffectPropSupportedContexts));
        let mut contexts = Vec::new();
        for ctx in host::CONTEXTS {
            let name = ofx_str(ctx);
            if !wanted.iter().any(|w| w == name) {
                continue;
            }
            let fx = Arc::new(Effect::context_descriptor(&general, ctx));
            let args = Args::new();
            args.0.ro(ofx::kOfxImageEffectPropContext, s(ctx));
            let h = effect_handle(&fx);
            // SAFETY: a context descriptor handle and its arguments.
            let r = unsafe {
                self.call(
                    ofx::kOfxImageEffectActionDescribeInContext,
                    h.cast(),
                    Some(&args),
                    None,
                )
            };
            if r.is_ok() {
                // SAFETY: Describe-in-context has just returned, and the
                // spec keeps a Bytes default valid until then.
                unsafe { super::param::capture_bytes_defaults(&fx.params) };
            }
            release_effect_handle(&fx);
            r?;
            contexts.push((name.to_owned(), fx));
        }
        if contexts.is_empty() {
            return Err(DriverError::NoContext);
        }
        Ok(Description { general, contexts })
    }

    /// `CreateInstance` in `context`.
    pub fn create_instance(
        &self,
        description: &Description,
        context: &str,
        project: Project,
        values: Arc<dyn ParamValues>,
        images: Arc<dyn ImageSource>,
    ) -> Result<Instance<'_>, DriverError> {
        let desc = description
            .context(context)
            .ok_or_else(|| DriverError::UnknownContext(context.to_owned()))?;
        // Mask clips are never connected: voXY's effect matte masks the
        // result instead (design §6.4).
        let masks: Vec<String> = desc
            .clips()
            .iter()
            .filter(|c| c.props.int(ofx_str(ofx::kOfxImageClipPropIsMask), 0) == Some(1))
            .map(|c| c.name.clone())
            .collect();
        let fx = Arc::new(Effect::instance(desc, project, values, images, &|name| {
            ClipInfo {
                frame_rate: project.frame_rate,
                frame_range: [0.0, (project.duration - 1.0).max(0.0)],
                pixel_aspect: project.pixel_aspect,
                connected: !masks.iter().any(|m| m == name),
                ..ClipInfo::default()
            }
        }));
        let handle = effect_handle(&fx);
        // SAFETY: a fresh instance handle.
        let r = unsafe { self.call(ofx::kOfxActionCreateInstance, handle.cast(), None, None) };
        if let Err(e) = r {
            release_effect_handle(&fx);
            return Err(e);
        }
        Ok(Instance {
            plugin: self,
            fx,
            handle,
            context: context.to_owned(),
            project,
            output_premult: Premult::Premultiplied,
            live: true,
            sequence: std::sync::atomic::AtomicU8::new(0),
        })
    }
}

impl Plugin {
    /// Runs Unload, once. Instances must be gone (in the sandbox, plugins
    /// live as long as the process, so it unloads them explicitly).
    pub fn unload(&self) {
        if self.loaded.swap(false, std::sync::atomic::Ordering::SeqCst) {
            // SAFETY: the plugin's own entry point, once.
            let _ = unsafe { self.call(ofx::kOfxActionUnload, std::ptr::null(), None, None) };
        }
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        self.unload();
    }
}

/// The plugin's clip preferences, after `GetClipPreferences`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipPreferences {
    /// Clip name → depth and components.
    pub clips: HashMap<String, (Depth, Components)>,
    pub output_premult: Premult,
    pub frame_varying: bool,
}

/// What `IsIdentity` said.
#[derive(Debug, Clone, PartialEq)]
pub enum Identity {
    /// The plugin renders.
    No,
    /// Use this clip at this time instead.
    Clip { name: String, time: f64 },
}

/// One live instance. Destroyed (`DestroyInstance`) on drop.
#[derive(Debug)]
pub struct Instance<'p> {
    plugin: &'p Plugin,
    pub fx: Arc<Effect>,
    handle: ofx::OfxImageEffectHandle,
    pub context: String,
    project: Project,
    output_premult: Premult,
    live: bool,
    /// Inside `BeginSequenceRender`/`EndSequenceRender`: 0 no, 1 yes, 2 yes
    /// and interactive. Render actions report it.
    sequence: std::sync::atomic::AtomicU8,
}

impl Instance<'_> {
    fn call(
        &self,
        action: &[u8],
        ins: Option<&Args>,
        outs: Option<&Args>,
    ) -> Result<Reply, DriverError> {
        // SAFETY: this instance's own handle, live until drop.
        unsafe { self.plugin.call(action, self.handle.cast(), ins, outs) }
    }

    fn set_time(&self, time: f64) {
        let tl = self.fx.time_line();
        self.fx.set_time_line(TimeLine { time, ..tl });
    }

    /// `GetClipPreferences`. The host offers float (or the best depth the
    /// plugin supports), RGBA (or what each clip supports) and
    /// premultiplied output; the answers update the instance's clips.
    pub fn clip_preferences(&mut self) -> Result<ClipPreferences, DriverError> {
        let supported = self
            .fx
            .props
            .strings(ofx_str(ofx::kOfxImageEffectPropSupportedPixelDepths));
        let depth = host::DEPTHS
            .iter()
            .find(|d| supported.is_empty() || supported.iter().any(|s| s == ofx_str(d)))
            .and_then(|d| Depth::from_ofx(ofx_str(d)))
            .unwrap_or(Depth::Float);
        let outs = Args::new();
        let clips = self.fx.clips();
        for c in &clips {
            let comps = c
                .props
                .strings(ofx_str(ofx::kOfxImageEffectPropSupportedComponents));
            let pick = [Components::Rgba, Components::Rgb, Components::Alpha]
                .into_iter()
                .find(|k| comps.is_empty() || comps.iter().any(|s| s == ofx_str(k.ofx())))
                .unwrap_or(Components::Rgba);
            let clip_name = &c.name;
            outs.0.define_rw(
                &format!("OfxImageClipPropComponents_{clip_name}"),
                s(pick.ofx()),
            );
            outs.0.define_rw(
                &format!("OfxImageClipPropDepth_{clip_name}"),
                s(depth.ofx()),
            );
            outs.0.define_rw(
                &format!("OfxImageClipPropPAR_{clip_name}"),
                Values::Double(vec![self.project.pixel_aspect]),
            );
        }
        outs.0.rw(
            ofx::kOfxImageEffectPropFrameRate,
            Values::Double(vec![self.project.frame_rate]),
        );
        outs.0
            .rw(ofx::kOfxImageClipPropFieldOrder, s(ofx::kOfxImageFieldNone));
        outs.0.rw(
            ofx::kOfxImageEffectPropPreMultiplication,
            s(ofx::kOfxImagePreMultiplied),
        );
        outs.0.rw(
            ofx::kOfxImageClipPropContinuousSamples,
            Values::Int(vec![0]),
        );
        outs.0
            .rw(ofx::kOfxImageEffectFrameVarying, Values::Int(vec![0]));
        self.call(
            ofx::kOfxImageEffectActionGetClipPreferences,
            None,
            Some(&outs),
        )?;

        let mut prefs = HashMap::new();
        for c in &clips {
            let name = &c.name;
            let d = outs
                .0
                .string(&format!("OfxImageClipPropDepth_{name}"), 0)
                .and_then(|d| Depth::from_ofx(&d))
                .unwrap_or(depth);
            let k = outs
                .0
                .string(&format!("OfxImageClipPropComponents_{name}"), 0)
                .and_then(|k| Components::from_ofx(&k))
                .unwrap_or(Components::Rgba);
            let _ = c
                .props
                .set(ofx_str(ofx::kOfxImageEffectPropPixelDepth), s(d.ofx()));
            let _ = c
                .props
                .set(ofx_str(ofx::kOfxImageEffectPropComponents), s(k.ofx()));
            prefs.insert(name.clone(), (d, k));
        }
        let premult = outs
            .0
            .string(ofx_str(ofx::kOfxImageEffectPropPreMultiplication), 0)
            .and_then(|p| Premult::from_ofx(&p))
            .unwrap_or(Premult::Premultiplied);
        if let Some(out) = self.fx.clip(ofx_str(ofx::kOfxImageEffectOutputClipName)) {
            let _ = out.props.set(
                ofx_str(ofx::kOfxImageEffectPropPreMultiplication),
                s(premult.ofx()),
            );
        }
        self.output_premult = premult;
        Ok(ClipPreferences {
            clips: prefs,
            output_premult: premult,
            frame_varying: outs.0.int(ofx_str(ofx::kOfxImageEffectFrameVarying), 0) == Some(1),
        })
    }

    /// `GetRegionOfDefinition`, with the spec's default when the plugin
    /// does not trap it: the source's region for filters, the project for
    /// generators.
    pub fn region_of_definition(
        &self,
        time: f64,
        scale: [f64; 2],
    ) -> Result<[f64; 4], DriverError> {
        self.set_time(time);
        let ins = Args::new();
        ins.0.ro(ofx::kOfxPropTime, Values::Double(vec![time]));
        ins.0.ro(
            ofx::kOfxImageEffectPropRenderScale,
            Values::Double(scale.to_vec()),
        );
        let outs = Args::new();
        outs.0.rw(
            ofx::kOfxImageEffectPropRegionOfDefinition,
            Values::Double(vec![0.0; 4]),
        );
        let reply = self.call(
            ofx::kOfxImageEffectActionGetRegionOfDefinition,
            Some(&ins),
            Some(&outs),
        )?;
        if reply == Reply::Handled
            && let Some(Values::Double(v)) = outs
                .0
                .get(ofx_str(ofx::kOfxImageEffectPropRegionOfDefinition))
        {
            return Ok([v[0], v[1], v[2], v[3]]);
        }
        let source = ofx_str(ofx::kOfxImageEffectSimpleSourceClipName);
        let from_source = self
            .fx
            .images
            .as_ref()
            .and_then(|i| i.region_of_definition(source, time))
            .filter(|_| self.context != ofx_str(ofx::kOfxImageEffectContextGenerator));
        let [ox, oy] = self.project.offset;
        let [ex, ey] = self.project.extent;
        Ok(from_source.unwrap_or([ox, oy, ox + ex, oy + ey]))
    }

    /// `GetRegionsOfInterest`: input clip name → region, defaulting to the
    /// requested region.
    pub fn regions_of_interest(
        &self,
        time: f64,
        scale: [f64; 2],
        region: [f64; 4],
    ) -> Result<HashMap<String, [f64; 4]>, DriverError> {
        self.set_time(time);
        let ins = Args::new();
        ins.0.ro(ofx::kOfxPropTime, Values::Double(vec![time]));
        ins.0.ro(
            ofx::kOfxImageEffectPropRenderScale,
            Values::Double(scale.to_vec()),
        );
        ins.0.ro(
            ofx::kOfxImageEffectPropRegionOfInterest,
            Values::Double(region.to_vec()),
        );
        let outs = Args::new();
        let inputs: Vec<String> = self
            .fx
            .clips()
            .into_iter()
            .map(|c| c.name.clone())
            .filter(|n| n != ofx_str(ofx::kOfxImageEffectOutputClipName))
            .collect();
        for n in &inputs {
            outs.0.define_rw(
                &format!("OfxImageClipPropRoI_{n}"),
                Values::Double(region.to_vec()),
            );
        }
        self.call(
            ofx::kOfxImageEffectActionGetRegionsOfInterest,
            Some(&ins),
            Some(&outs),
        )?;
        Ok(inputs
            .into_iter()
            .map(|n| {
                let r = match outs.0.get(&format!("OfxImageClipPropRoI_{n}")) {
                    Some(Values::Double(v)) => [v[0], v[1], v[2], v[3]],
                    _ => region,
                };
                (n, r)
            })
            .collect())
    }

    fn render_args(&self, time: f64, window: [i32; 4], scale: [f64; 2]) -> Args {
        let sequence = self.sequence.load(std::sync::atomic::Ordering::Relaxed);
        let ins = Args::new();
        ins.0.ro(ofx::kOfxPropTime, Values::Double(vec![time]));
        ins.0.ro(
            ofx::kOfxImageEffectPropFieldToRender,
            s(ofx::kOfxImageFieldNone),
        );
        ins.0.ro(
            ofx::kOfxImageEffectPropRenderWindow,
            Values::Int(window.to_vec()),
        );
        ins.0.ro(
            ofx::kOfxImageEffectPropRenderScale,
            Values::Double(scale.to_vec()),
        );
        ins.0.ro(
            ofx::kOfxImageEffectPropSequentialRenderStatus,
            Values::Int(vec![i32::from(sequence > 0)]),
        );
        ins.0.ro(
            ofx::kOfxImageEffectPropInteractiveRenderStatus,
            Values::Int(vec![i32::from(sequence == 2)]),
        );
        ins.0.ro(
            ofx::kOfxImageEffectPropRenderQualityDraft,
            Values::Int(vec![0]),
        );
        ins
    }

    /// `IsIdentity`.
    pub fn is_identity(
        &self,
        time: f64,
        window: [i32; 4],
        scale: [f64; 2],
    ) -> Result<Identity, DriverError> {
        self.set_time(time);
        let ins = self.render_args(time, window, scale);
        let outs = Args::new();
        outs.0.rw(ofx::kOfxPropName, Values::strings(&[""]));
        outs.0.rw(ofx::kOfxPropTime, Values::Double(vec![time]));
        let reply = self.call(
            ofx::kOfxImageEffectActionIsIdentity,
            Some(&ins),
            Some(&outs),
        )?;
        let name = outs
            .0
            .string(ofx_str(ofx::kOfxPropName), 0)
            .unwrap_or_default();
        if reply == Reply::Default || name.is_empty() {
            return Ok(Identity::No);
        }
        Ok(Identity::Clip {
            name,
            time: outs.0.double(ofx_str(ofx::kOfxPropTime), 0).unwrap_or(time),
        })
    }

    /// Render. Afterwards the output is made premultiplied, as voXY's
    /// buffers are (owner decision D2): an unpremultiplied result is
    /// premultiplied, an opaque one gets alpha 1.
    pub fn render(&self, time: f64, window: [i32; 4], scale: [f64; 2]) -> Result<(), DriverError> {
        self.set_time(time);
        self.fx.set_abort(false);
        let ins = self.render_args(time, window, scale);
        self.call(ofx::kOfxImageEffectActionRender, Some(&ins), None)?;
        let out = ofx_str(ofx::kOfxImageEffectOutputClipName);
        if let Some(img) = self.fx.images.as_ref().and_then(|i| i.image(out, time)) {
            match self.output_premult {
                Premult::Premultiplied => {}
                Premult::Unpremultiplied => img.premultiply(),
                Premult::Opaque => img.make_opaque(),
            }
        }
        Ok(())
    }

    /// `BeginSequenceRender` or `EndSequenceRender` over `frames`.
    pub fn sequence(
        &self,
        begin: bool,
        frames: [f64; 2],
        interactive: bool,
    ) -> Result<(), DriverError> {
        let ins = Args::new();
        ins.0.ro(
            ofx::kOfxImageEffectPropFrameRange,
            Values::Double(frames.to_vec()),
        );
        ins.0
            .ro(ofx::kOfxImageEffectPropFrameStep, Values::Double(vec![1.0]));
        ins.0.ro(
            ofx::kOfxPropIsInteractive,
            Values::Int(vec![i32::from(interactive)]),
        );
        ins.0.ro(
            ofx::kOfxImageEffectPropRenderScale,
            Values::Double(vec![1.0, 1.0]),
        );
        ins.0.ro(
            ofx::kOfxImageEffectPropSequentialRenderStatus,
            Values::Int(vec![1]),
        );
        ins.0.ro(
            ofx::kOfxImageEffectPropInteractiveRenderStatus,
            Values::Int(vec![i32::from(interactive)]),
        );
        let action = if begin {
            ofx::kOfxImageEffectActionBeginSequenceRender.as_slice()
        } else {
            ofx::kOfxImageEffectActionEndSequenceRender
        };
        let state = match (begin, interactive) {
            (false, _) => 0,
            (true, false) => 1,
            (true, true) => 2,
        };
        self.sequence
            .store(state, std::sync::atomic::Ordering::Relaxed);
        self.call(action, Some(&ins), None).map(|_| ())
    }

    /// `InstanceChanged` for a parameter the user edited, wrapped in
    /// `Begin/EndInstanceChanged`.
    pub fn param_changed(&self, name: &str, time: f64) -> Result<(), DriverError> {
        self.set_time(time);
        let reason = || {
            let a = Args::new();
            a.0.ro(ofx::kOfxPropChangeReason, s(ofx::kOfxChangeUserEdited));
            a
        };
        self.call(ofx::kOfxActionBeginInstanceChanged, Some(&reason()), None)?;
        let ins = Args::new();
        ins.0.ro(ofx::kOfxPropType, s(ofx::kOfxTypeParameter));
        ins.0.ro(ofx::kOfxPropName, Values::strings(&[name]));
        ins.0
            .ro(ofx::kOfxPropChangeReason, s(ofx::kOfxChangeUserEdited));
        ins.0.ro(ofx::kOfxPropTime, Values::Double(vec![time]));
        ins.0.ro(
            ofx::kOfxImageEffectPropRenderScale,
            Values::Double(vec![1.0, 1.0]),
        );
        let r = self.call(ofx::kOfxActionInstanceChanged, Some(&ins), None);
        self.call(ofx::kOfxActionEndInstanceChanged, Some(&reason()), None)?;
        r.map(|_| ())
    }

    /// The parameter's type, for callers mapping parameters.
    pub fn param_kind(&self, name: &str) -> Option<ParamKind> {
        self.fx.params.get(name).map(|p| p.kind)
    }
}

impl Drop for Instance<'_> {
    fn drop(&mut self) {
        if self.live {
            let _ = guard(|| {
                let _ = self.call(ofx::kOfxActionDestroyInstance, None, None);
                ofx::kOfxStatOK
            });
            release_effect_handle(&self.fx);
            self.live = false;
        }
    }
}
