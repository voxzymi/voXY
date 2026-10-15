//! The editor's side of a sandbox (design §6.7): launch, handshake,
//! requests with timeouts, and what happens when the plugin crashes, hangs
//! or breaks the protocol.
//!
//! On any of those the sandbox process is killed and every later request
//! fails at once with the same error; the editor renders the effect as
//! pass-through with an error badge, and the plugin goes on the session
//! [`Blocklist`]. Nothing here ever loads plugin code.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::channel;
use crate::params::ParamValue;
use crate::protocol::{self, FrameError, Reply, Request, VERSION};
use crate::shm::{SessionDir, SharedMemory};

/// How long each kind of action may take (design §6.7 defaults; adjustable
/// in Preferences).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timeouts {
    pub launch: Duration,
    pub describe: Duration,
    pub create_instance: Duration,
    pub render: Duration,
    pub other: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            launch: Duration::from_secs(10),
            describe: Duration::from_secs(10),
            create_instance: Duration::from_secs(5),
            render: Duration::from_secs(30),
            other: Duration::from_secs(5),
        }
    }
}

impl Timeouts {
    pub fn for_request(&self, r: &Request) -> Duration {
        match r {
            Request::Describe => self.describe,
            Request::CreateInstance { .. } => self.create_instance,
            Request::Render { .. } => self.render,
            _ => self.other,
        }
    }
}

/// What went wrong with a sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxError {
    /// The sandbox could not be started or did not connect.
    Launch(String),
    /// The process ended without being asked to (a plugin crash).
    Crashed { action: String, status: String },
    /// An action took longer than its timeout; the process was killed.
    Timeout { action: String, after: Duration },
    /// The sandbox sent something that is not a valid reply.
    Protocol(String),
    /// The sandbox is gone after an earlier error.
    Dead(Box<SandboxError>),
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxError::Launch(e) => write!(f, "the plugin host did not start: {e}"),
            SandboxError::Crashed { action, status } => {
                write!(f, "the plugin crashed during {action} ({status})")
            }
            SandboxError::Timeout { action, after } => {
                write!(
                    f,
                    "the plugin did not finish {action} within {} s",
                    after.as_secs_f64()
                )
            }
            SandboxError::Protocol(e) => write!(f, "the plugin host misbehaved: {e}"),
            SandboxError::Dead(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SandboxError {}

/// The action name for messages.
fn action(r: &Request) -> &'static str {
    match r {
        Request::Hello { .. } => "start-up",
        Request::Map { .. } => "memory set-up",
        Request::Describe => "Describe",
        Request::CreateInstance { .. } => "CreateInstance",
        Request::DestroyInstance { .. } => "DestroyInstance",
        Request::SetParams { .. } => "parameter update",
        Request::ClipPreferences { .. } => "GetClipPreferences",
        Request::RegionOfDefinition { .. } => "GetRegionOfDefinition",
        Request::RegionsOfInterest { .. } => "GetRegionsOfInterest",
        Request::IsIdentity { .. } => "IsIdentity",
        Request::Render { .. } => "Render",
        Request::Sequence { .. } => "sequence render",
        Request::InstanceChanged { .. } => "InstanceChanged",
        Request::Unload => "Unload",
        Request::Value { .. } => "parameter query",
    }
}

fn describe_status(s: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = s.signal() {
            return format!("signal {sig}");
        }
    }
    match s.code() {
        Some(c) => format!("exit code {c:#x}"),
        None => "ended".to_owned(),
    }
}

/// Where the editor finds the sandbox: next to its own executable.
pub fn default_sandbox_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe
        .parent()?
        .join(format!("voxy-ofx-sandbox{}", std::env::consts::EXE_SUFFIX));
    p.is_file().then_some(p)
}

/// One running sandbox.
#[derive(Debug)]
pub struct Sandbox {
    child: Child,
    to_io: Option<Sender<Request>>,
    from_io: Receiver<Result<Reply, FrameError>>,
    pub shm: SharedMemory,
    pub pid: u32,
    id: String,
    generation: u32,
    timeouts: Timeouts,
    dead: Option<SandboxError>,
}

impl Sandbox {
    /// Starts `exe` on `bundle`, waits for it to connect, checks the
    /// protocol version and hands it a mapping of `shm_len` bytes.
    pub fn launch(
        exe: &Path,
        bundle: &Path,
        session: &SessionDir,
        id: &str,
        shm_len: usize,
        timeouts: Timeouts,
    ) -> Result<Sandbox, SandboxError> {
        let launch = |e: &dyn std::fmt::Display| SandboxError::Launch(e.to_string());
        let spec = channel::spec(session.path(), id);
        let listener = channel::listen(&spec).map_err(|e| launch(&e))?;
        let mut child = Command::new(exe)
            .arg("--bundle")
            .arg(bundle)
            .arg("--channel")
            .arg(&spec)
            .stdin(Stdio::null())
            .spawn()
            .map_err(|e| launch(&e))?;
        // Accept on a thread so a sandbox that never connects cannot hang
        // the editor.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(channel::accept(&listener));
        });
        let deadline = Instant::now() + timeouts.launch;
        let stream = loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(s)) => break s,
                Ok(Err(e)) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(launch(&e));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Ok(Some(status)) = child.try_wait() {
                        return Err(SandboxError::Launch(format!(
                            "it exited first ({})",
                            describe_status(status)
                        )));
                    }
                    if Instant::now() > deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(SandboxError::Launch("it did not connect in time".into()));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SandboxError::Launch("the channel failed".into()));
                }
            }
        };
        let (to_io, io_rx) = mpsc::channel::<Request>();
        let (io_tx, from_io) = mpsc::channel();
        std::thread::Builder::new()
            .name("ofx-sandbox-io".into())
            .spawn(move || {
                let mut stream = stream;
                for req in io_rx {
                    let r = protocol::write_frame(&mut stream, &req)
                        .map_err(FrameError::Io)
                        .and_then(|()| protocol::read_frame(&mut stream));
                    let failed = r.is_err();
                    if io_tx.send(r).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|e| launch(&e))?;
        let shm_path = session.path().join(format!("{id}.shm"));
        let shm = SharedMemory::create(&shm_path, shm_len).map_err(|e| launch(&e))?;
        let pid = child.id();
        let mut sb = Sandbox {
            child,
            to_io: Some(to_io),
            from_io,
            shm,
            pid,
            id: id.to_owned(),
            generation: 0,
            timeouts,
            dead: None,
        };
        match sb.request(Request::Hello { version: VERSION })? {
            Reply::Hello {
                version: VERSION, ..
            } => {}
            Reply::Error(e) => {
                sb.kill();
                return Err(SandboxError::Launch(e));
            }
            other => {
                sb.kill();
                return Err(SandboxError::Launch(format!(
                    "unexpected greeting {other:?}"
                )));
            }
        }
        let path = sb.shm.path().display().to_string();
        match sb.request(Request::Map { path })? {
            Reply::Done { .. } => {}
            Reply::Error(e) => {
                sb.kill();
                return Err(SandboxError::Launch(e));
            }
            other => {
                sb.kill();
                return Err(SandboxError::Protocol(format!("{other:?}")));
            }
        }
        Ok(sb)
    }

    /// The error that ended this sandbox, if it is gone.
    pub fn dead(&self) -> Option<&SandboxError> {
        self.dead.as_ref()
    }

    fn kill(&mut self) {
        self.to_io = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn die(&mut self, e: SandboxError) -> SandboxError {
        self.kill();
        self.dead = Some(e.clone());
        e
    }

    /// Sends `req` and waits for the reply, within the request's timeout.
    pub fn request(&mut self, req: Request) -> Result<Reply, SandboxError> {
        self.request_with(req, &mut |_, _| None)
    }

    /// [`Sandbox::request`], answering the plugin's parameter queries at
    /// other times (`Reply::ValueAt`) with `values(name, time)` meanwhile.
    pub fn request_with(
        &mut self,
        req: Request,
        values: &mut dyn FnMut(&str, f64) -> Option<ParamValue>,
    ) -> Result<Reply, SandboxError> {
        if let Some(e) = &self.dead {
            return Err(SandboxError::Dead(Box::new(e.clone())));
        }
        let name = action(&req).to_owned();
        let limit = self.timeouts.for_request(&req);
        let Some(to_io) = &self.to_io else {
            return Err(SandboxError::Dead(Box::new(SandboxError::Protocol(
                "closed".into(),
            ))));
        };
        if to_io.send(req).is_err() {
            let e = self.crashed(&name);
            return Err(self.die(e));
        }
        let deadline = Instant::now() + limit;
        loop {
            match self.from_io.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(Reply::ValueAt { name: param, time })) => {
                    let value = values(&param, time);
                    let sent = self
                        .to_io
                        .as_ref()
                        .is_some_and(|t| t.send(Request::Value { value }).is_ok());
                    if !sent {
                        let e = self.crashed(&name);
                        return Err(self.die(e));
                    }
                }
                Ok(Ok(reply)) => return Ok(reply),
                Ok(Err(FrameError::Io(_))) | Err(RecvTimeoutError::Disconnected) => {
                    let e = self.crashed(&name);
                    return Err(self.die(e));
                }
                Ok(Err(e)) => {
                    let e = SandboxError::Protocol(e.to_string());
                    return Err(self.die(e));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() > deadline {
                        let e = SandboxError::Timeout {
                            action: name,
                            after: limit,
                        };
                        return Err(self.die(e));
                    }
                }
            }
        }
    }

    /// The crash error for `action`, with the exit status once the process
    /// has ended (waiting briefly for it).
    fn crashed(&mut self, action: &str) -> SandboxError {
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(s)) => break describe_status(s),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => break "the channel closed".to_owned(),
            }
        };
        SandboxError::Crashed {
            action: action.to_owned(),
            status,
        }
    }

    /// Makes sure the mapping holds at least `len` bytes, replacing it with
    /// a bigger one (and telling the sandbox) if not.
    pub fn ensure_mapping(&mut self, dir: &Path, len: usize) -> Result<(), SandboxError> {
        if self.shm.len() >= len {
            return Ok(());
        }
        self.generation += 1;
        let path = dir.join(format!("{}-{}.shm", self.id, self.generation));
        let bigger = SharedMemory::create(&path, len.next_power_of_two())
            .map_err(|e| SandboxError::Launch(e.to_string()))?;
        let p = bigger.path().display().to_string();
        match self.request(Request::Map { path: p })? {
            Reply::Done { .. } => {
                let old = std::mem::replace(&mut self.shm, bigger);
                let _ = std::fs::remove_file(old.path());
                Ok(())
            }
            other => Err(SandboxError::Protocol(format!("{other:?}"))),
        }
    }

    /// Asks the running action to stop (the plugin's `abort()` reads it).
    pub fn set_abort(&self, abort: bool) {
        self.shm.set_abort(abort);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.dead.is_none() {
            // A clean end: let the plugins unload, briefly.
            self.timeouts.other = Duration::from_millis(500);
            let _ = self.request(Request::Unload);
        }
        self.kill();
        let _ = std::fs::remove_file(self.shm.path());
    }
}

/// Why a plugin is blocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub reason: String,
    /// True once it has failed often enough to stay blocked across
    /// sessions (stored in Preferences).
    pub persistent: bool,
}

/// Plugins that crashed, hung or misbehaved. A failure blocks the plugin
/// for the session; [`Blocklist::PERSISTENT_AFTER`] failures make the
/// block persistent. Retry clears both.
#[derive(Debug, Clone, Default)]
pub struct Blocklist {
    failures: HashMap<String, (u32, String)>,
}

impl Blocklist {
    pub const PERSISTENT_AFTER: u32 = 3;

    pub fn new() -> Blocklist {
        Blocklist::default()
    }

    /// Restores persistent blocks from Preferences.
    pub fn with_persistent(ids: impl IntoIterator<Item = (String, String)>) -> Blocklist {
        let mut b = Blocklist::new();
        for (id, reason) in ids {
            b.failures.insert(id, (Self::PERSISTENT_AFTER, reason));
        }
        b
    }

    /// Records a failure of plugin `id`.
    pub fn record(&mut self, id: &str, error: &SandboxError) -> Blocked {
        let e = self
            .failures
            .entry(id.to_owned())
            .or_insert((0, String::new()));
        e.0 += 1;
        e.1 = error.to_string();
        Blocked {
            reason: e.1.clone(),
            persistent: e.0 >= Self::PERSISTENT_AFTER,
        }
    }

    pub fn blocked(&self, id: &str) -> Option<Blocked> {
        self.failures.get(id).map(|(n, r)| Blocked {
            reason: r.clone(),
            persistent: *n >= Self::PERSISTENT_AFTER,
        })
    }

    /// The Retry action: unblocks `id` entirely.
    pub fn retry(&mut self, id: &str) {
        self.failures.remove(id);
    }

    /// The persistent blocks, for Preferences, sorted.
    pub fn persistent(&self) -> Vec<(String, String)> {
        let mut v: Vec<_> = self
            .failures
            .iter()
            .filter(|(_, (n, _))| *n >= Self::PERSISTENT_AFTER)
            .map(|(id, (_, r))| (id.clone(), r.clone()))
            .collect();
        v.sort();
        v
    }
}
