//! The host's record of one plugin effect handle (an `OfxImageEffectHandle`):
//! a descriptor (from the Describe and Describe-in-context actions) or an
//! instance, with
//! its properties, parameters and clips, plus what the plugin reported
//! through the message and progress suites and the time line it sees.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use voxy_ofx_sys as ofx;

use crate::clip::{Clip, ClipInfo};
use crate::image::ImageSource;
use crate::params::{ParamSet, ParamValues, Role};
use crate::props::{PropSet, Values};
/// A message type from `ofxMessage.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Fatal,
    Error,
    Warning,
    Message,
    Log,
    Question,
}

impl MessageKind {
    pub fn from_ofx(name: &[u8]) -> Option<MessageKind> {
        use voxy_ofx_sys as ofx;
        let is = |c: &[u8]| c.strip_suffix(b"\0").unwrap_or(c) == name;
        [
            (ofx::kOfxMessageFatal.as_slice(), MessageKind::Fatal),
            (ofx::kOfxMessageError, MessageKind::Error),
            (ofx::kOfxMessageWarning, MessageKind::Warning),
            (ofx::kOfxMessageMessage, MessageKind::Message),
            (ofx::kOfxMessageLog, MessageKind::Log),
            (ofx::kOfxMessageQuestion, MessageKind::Question),
        ]
        .into_iter()
        .find_map(|(c, k)| is(c).then_some(k))
    }
}

/// One message from the plugin, already formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: MessageKind,
    /// The plugin's message id, if it gave one.
    pub id: Option<String>,
    pub text: String,
}

/// Progress the plugin reported (`OfxProgressSuite`).
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub label: String,
    pub id: Option<String>,
    /// 0 to 1.
    pub fraction: f64,
}

/// The time line the plugin sees (`OfxTimeLineSuiteV1`), in frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeLine {
    pub time: f64,
    pub first: f64,
    pub last: f64,
}

impl Default for TimeLine {
    fn default() -> Self {
        TimeLine {
            time: 0.0,
            first: 0.0,
            last: 0.0,
        }
    }
}

#[derive(Debug, Default)]
struct State {
    messages: Vec<Message>,
    persistent: Option<Message>,
    progress: Option<Progress>,
    time: TimeLine,
}

/// The project an instance belongs to, as OpenFX describes it, in
/// canonical coordinates (`kOfxImageEffectPropProject*`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Project {
    pub size: [f64; 2],
    pub offset: [f64; 2],
    pub extent: [f64; 2],
    pub pixel_aspect: f64,
    pub frame_rate: f64,
    /// The effect's duration in frames.
    pub duration: f64,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            size: [1920.0, 1080.0],
            offset: [0.0, 0.0],
            extent: [1920.0, 1080.0],
            pixel_aspect: 1.0,
            frame_rate: 30.0,
            duration: 1.0,
        }
    }
}

/// One effect handle's host-side state. Shared between the suites (which
/// the plugin may call from several threads) and the host.
#[derive(Debug)]
pub struct Effect {
    pub role: Role,
    pub props: Arc<PropSet>,
    pub params: Arc<ParamSet>,
    clips: Mutex<Vec<Arc<Clip>>>,
    /// An instance's images (none on descriptors).
    pub images: Option<Arc<dyn ImageSource>>,
    state: Mutex<State>,
    abort: AtomicBool,
}

impl Default for Effect {
    fn default() -> Self {
        Effect::descriptor()
    }
}

impl Effect {
    /// A bare descriptor; same as [`Effect::descriptor`].
    pub fn new() -> Effect {
        Effect::descriptor()
    }

    /// An effect descriptor with the spec's default properties.
    pub fn descriptor() -> Effect {
        let props = Arc::new(PropSet::new("effect descriptor"));
        define_descriptor_props(&props);
        Effect {
            role: Role::Descriptor,
            params: Arc::new(ParamSet::descriptor(Arc::clone(&props))),
            props,
            clips: Mutex::new(Vec::new()),
            images: None,
            state: Mutex::default(),
            abort: AtomicBool::new(false),
        }
    }

    /// A descriptor for `context` (the Describe-in-context action), starting
    /// from what
    /// the plugin said in Describe.
    pub fn context_descriptor(general: &Effect, context: &[u8]) -> Effect {
        let props = Arc::new(general.props.duplicate("effect descriptor"));
        props.ro(ofx::kOfxImageEffectPropContext, PropSet::consts(&[context]));
        Effect {
            role: Role::Descriptor,
            params: Arc::new(ParamSet::descriptor(Arc::clone(&props))),
            props,
            clips: Mutex::new(Vec::new()),
            images: None,
            state: Mutex::default(),
            abort: AtomicBool::new(false),
        }
    }

    /// An instance of a context descriptor. Its descriptor properties are
    /// read-only copies; it adds the instance properties, instances of the
    /// parameters (values in `values`) and the clips (`info` for each).
    pub fn instance(
        descriptor: &Effect,
        project: Project,
        values: Arc<dyn ParamValues>,
        images: Arc<dyn ImageSource>,
        info: &dyn Fn(&str) -> ClipInfo,
    ) -> Effect {
        let p = descriptor.props.duplicate("effect instance");
        for name in p.names() {
            p.set_plugin_writable(&name, false);
        }
        p.ro(
            ofx::kOfxPropType,
            PropSet::consts(&[ofx::kOfxTypeImageEffectInstance]),
        );
        p.rw(ofx::kOfxPropInstanceData, Values::Pointer(vec![0]));
        p.ro(ofx::kOfxPropIsInteractive, Values::Int(vec![0]));
        p.ro(
            ofx::kOfxImageEffectPropProjectSize,
            Values::Double(project.size.to_vec()),
        );
        p.ro(
            ofx::kOfxImageEffectPropProjectOffset,
            Values::Double(project.offset.to_vec()),
        );
        p.ro(
            ofx::kOfxImageEffectPropProjectExtent,
            Values::Double(project.extent.to_vec()),
        );
        p.ro(
            ofx::kOfxImageEffectPropProjectPixelAspectRatio,
            Values::Double(vec![project.pixel_aspect]),
        );
        p.ro(
            ofx::kOfxImageEffectPropFrameRate,
            Values::Double(vec![project.frame_rate]),
        );
        p.ro(
            ofx::kOfxImageEffectInstancePropEffectDuration,
            Values::Double(vec![project.duration]),
        );
        p.rw(
            ofx::kOfxImageEffectInstancePropSequentialRender,
            Values::Int(vec![0]),
        );
        // The plugin may change these on an instance.
        for name in [
            ofx::kOfxImageEffectPropSupportsTiles.as_slice(),
            ofx::kOfxImageEffectPropSupportsMultiResolution,
        ] {
            p.set_plugin_writable(crate::props::ofx_str(name), true);
        }
        let props = Arc::new(p);
        let params = Arc::new(ParamSet::instance(
            &descriptor.params,
            Arc::clone(&props),
            values,
        ));
        let clips = descriptor
            .clips()
            .iter()
            .map(|c| Arc::new(Clip::instance(c, info(&c.name))))
            .collect();
        Effect {
            role: Role::Instance,
            props,
            params,
            clips: Mutex::new(clips),
            images: Some(images),
            state: Mutex::default(),
            abort: AtomicBool::new(false),
        }
    }

    pub fn clips(&self) -> Vec<Arc<Clip>> {
        self.clips
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn clip(&self, name: &str) -> Option<Arc<Clip>> {
        self.clips().into_iter().find(|c| c.name == name)
    }

    /// `clipDefine` on a descriptor: the clip, new or existing.
    pub fn define_clip(&self, name: &str) -> Option<Arc<Clip>> {
        if self.role != Role::Descriptor {
            return None;
        }
        let mut clips = self.clips.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = clips.iter().find(|c| c.name == name) {
            return Some(Arc::clone(c));
        }
        let c = Arc::new(Clip::descriptor(name));
        clips.push(Arc::clone(&c));
        Some(c)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn post(&self, m: Message) {
        self.lock().messages.push(m);
    }

    /// The transient messages so far, oldest first; clears them.
    pub fn take_messages(&self) -> Vec<Message> {
        std::mem::take(&mut self.lock().messages)
    }

    pub fn set_persistent(&self, m: Option<Message>) {
        self.lock().persistent = m;
    }

    pub fn persistent(&self) -> Option<Message> {
        self.lock().persistent.clone()
    }

    pub fn set_progress(&self, p: Option<Progress>) {
        self.lock().progress = p;
    }

    pub fn update_progress(&self, fraction: f64) -> bool {
        match &mut self.lock().progress {
            Some(p) => {
                p.fraction = fraction.clamp(0.0, 1.0);
                true
            }
            None => false,
        }
    }

    pub fn progress(&self) -> Option<Progress> {
        self.lock().progress.clone()
    }

    /// Sets the time line, and the time "current value" calls use.
    pub fn set_time_line(&self, t: TimeLine) {
        self.lock().time = t;
        self.params.set_time(t.time);
    }

    pub fn time_line(&self) -> TimeLine {
        self.lock().time
    }

    /// Asks the plugin to stop: `abort()` and progress updates report it.
    pub fn set_abort(&self, abort: bool) {
        self.abort.store(abort, Ordering::Relaxed);
    }

    pub fn aborted(&self) -> bool {
        self.abort.load(Ordering::Relaxed)
    }
}

/// The effect descriptor properties, with the spec's defaults.
fn define_descriptor_props(p: &PropSet) {
    let int = |n: &[u8], v: i32| p.rw(n, Values::Int(vec![v]));
    p.ro(
        ofx::kOfxPropType,
        PropSet::consts(&[ofx::kOfxTypeImageEffect]),
    );
    p.rw(ofx::kOfxPropLabel, Values::strings(&[""]));
    p.rw(ofx::kOfxPropShortLabel, Values::strings(&[""]));
    p.rw(ofx::kOfxPropLongLabel, Values::strings(&[""]));
    p.var(ofx::kOfxPropVersion, Values::Int(Vec::new()));
    p.rw(ofx::kOfxPropVersionLabel, Values::strings(&[""]));
    p.rw(ofx::kOfxPropPluginDescription, Values::strings(&[""]));
    p.ro(ofx::kOfxPluginPropFilePath, Values::strings(&[""]));
    p.rw(
        ofx::kOfxImageEffectPluginPropGrouping,
        Values::strings(&[""]),
    );
    p.var(
        ofx::kOfxImageEffectPropSupportedContexts,
        Values::String(Vec::new()),
    );
    p.var(
        ofx::kOfxImageEffectPropSupportedPixelDepths,
        Values::String(Vec::new()),
    );
    int(ofx::kOfxImageEffectPluginPropSingleInstance, 0);
    p.rw(
        ofx::kOfxImageEffectPluginRenderThreadSafety,
        PropSet::consts(&[ofx::kOfxImageEffectRenderInstanceSafe]),
    );
    int(ofx::kOfxImageEffectPluginPropHostFrameThreading, 0);
    int(ofx::kOfxImageEffectPropSupportsMultiResolution, 1);
    int(ofx::kOfxImageEffectPropSupportsTiles, 1);
    int(ofx::kOfxImageEffectPropTemporalClipAccess, 0);
    int(ofx::kOfxImageEffectPluginPropFieldRenderTwiceAlways, 1);
    int(ofx::kOfxImageEffectPropSupportsMultipleClipDepths, 0);
    int(ofx::kOfxImageEffectPropSupportsMultipleClipPARs, 0);
    p.var(
        ofx::kOfxImageEffectPropClipPreferencesSlaveParam,
        Values::String(Vec::new()),
    );
    p.rw(
        ofx::kOfxImageEffectPluginPropOverlayInteractV1,
        Values::Pointer(vec![0]),
    );
    p.rw(
        ofx::kOfxImageEffectPluginPropOverlayInteractV2,
        Values::Pointer(vec![0]),
    );
    p.rw(
        crate::host::OPENGL_RENDER_SUPPORTED,
        Values::strings(&["false"]),
    );
    p.rw(
        ofx::kOfxImageEffectPropColourManagementStyle,
        PropSet::consts(&[ofx::kOfxImageEffectColourManagementNone]),
    );
    p.rw(
        ofx::kOfxImageEffectPropNoSpatialAwareness,
        Values::strings(&["false"]),
    );
    p.rw(ofx::kOfxPropIcon, Values::strings(&["", ""]));
}
