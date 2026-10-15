//! The control channel between the editor and a sandbox (design §6.7,
//! owner decision D3): a local socket from `interprocess`. Unix domain
//! sockets or named pipes keep the sandbox's stdout free for plugins that
//! print to it.
//!
//! A channel is named by a spec string the editor passes on the command
//! line: `ns:<name>` for a namespaced socket (Linux abstract sockets,
//! Windows named pipes), `fs:<path>` for a socket file in the session
//! folder (macOS).

use std::io;
use std::path::Path;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{
    ConnectOptions, GenericFilePath, GenericNamespaced, ListenerOptions, Name,
};

pub use interprocess::local_socket::{Listener, Stream};

/// A spec for a new channel called `id`, unique to this process.
pub fn spec(session: &Path, id: &str) -> String {
    if GenericNamespaced::is_supported() {
        format!("ns:voxy-ofx-{}-{id}", std::process::id())
    } else {
        format!("fs:{}", session.join(format!("{id}.sock")).display())
    }
}

fn name(spec: &str) -> io::Result<Name<'_>> {
    if let Some(n) = spec.strip_prefix("ns:") {
        n.to_ns_name::<GenericNamespaced>()
    } else if let Some(p) = spec.strip_prefix("fs:") {
        p.to_fs_name::<GenericFilePath>()
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "bad channel spec",
        ))
    }
}

/// Listens on `spec` (editor side).
pub fn listen(spec: &str) -> io::Result<Listener> {
    ListenerOptions::new().name(name(spec)?).create_sync()
}

/// Connects to `spec` (sandbox side).
pub fn connect(spec: &str) -> io::Result<Stream> {
    ConnectOptions::new().name(name(spec)?).connect_sync()
}

/// Waits for the sandbox to connect (editor side).
pub fn accept(listener: &Listener) -> io::Result<Stream> {
    listener.accept()
}
