//! Out-of-process OpenFX plugin runner (one process per plugin bundle).
//!
//! Placeholder until Milestone 7. The real helper loads a single `.ofx` bundle,
//! exchanges images with voXY through shared memory, and receives actions over
//! a local pipe/socket, so a crashing plugin can never take voXY down.

fn main() {
    eprintln!("voxy-ofx-sandbox: OpenFX hosting arrives in Milestone 7");
    std::process::exit(2);
}
