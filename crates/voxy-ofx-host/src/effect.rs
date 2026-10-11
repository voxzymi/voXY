//! The host's record of one plugin effect handle (an `OfxImageEffectHandle`):
//! what the plugin reported through the message and progress suites, and
//! the time line it sees. The descriptor and instance parts (properties,
//! parameters, clips) join it with the image effect suite.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

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

/// One effect handle's host-side state. Shared between the suites (which
/// the plugin may call from several threads) and the host.
#[derive(Debug, Default)]
pub struct Effect {
    state: Mutex<State>,
    abort: AtomicBool,
}

impl Effect {
    pub fn new() -> Effect {
        Effect::default()
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

    pub fn set_time_line(&self, t: TimeLine) {
        self.lock().time = t;
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
