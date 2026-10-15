//! `voxy-ofx-sandbox --bundle <path> --channel <spec>`: runs the plugins of
//! one OpenFX bundle for voXY, out of the editor's process (design §6.7),
//! so a crashing or hanging plugin can never take the editor down.
//!
//! It connects to the editor's channel, then serves requests until the
//! editor sends `Unload` or the channel closes. Exit codes: 0 after a
//! clean end, 2 for bad arguments, 3 if the bundle did not load, 4 if the
//! channel failed.

use std::path::PathBuf;
use std::process::ExitCode;

use voxy_ofx_host::channel;
use voxy_ofx_host::suites::server::Server;

fn main() -> ExitCode {
    let mut bundle = None;
    let mut spec = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bundle" => bundle = args.next().map(PathBuf::from),
            "--channel" => spec = args.next(),
            _ => {
                eprintln!("voxy-ofx-sandbox: unknown argument {a}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(bundle), Some(spec)) = (bundle, spec) else {
        eprintln!("usage: voxy-ofx-sandbox --bundle <path> --channel <spec>");
        return ExitCode::from(2);
    };
    let mut stream = match channel::connect(&spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("voxy-ofx-sandbox: cannot reach the editor: {e}");
            return ExitCode::from(4);
        }
    };
    let mut server = match load(&bundle) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("voxy-ofx-sandbox: {e}");
            // Answer the editor's greeting with the reason, then go.
            let _ = voxy_ofx_host::protocol::read_frame::<voxy_ofx_host::protocol::Request, _>(
                &mut stream,
            );
            let _ = voxy_ofx_host::protocol::write_frame(
                &mut stream,
                &voxy_ofx_host::protocol::Reply::Error(e),
            );
            return ExitCode::from(3);
        }
    };
    match server.serve(stream) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("voxy-ofx-sandbox: {e}");
            ExitCode::from(4)
        }
    }
}

/// Loads the bundle. This process exists to load and run plugin code.
#[allow(unsafe_code)]
fn load(bundle: &std::path::Path) -> Result<Server, String> {
    // SAFETY: running the plugin's code is the sandbox's whole purpose;
    // the editor stays safe in its own process.
    unsafe { Server::new(bundle) }
}
