//! The `voxy` executable (display name "voXY").
//!
//! Usage: `voxy [PROJECT.voxy]`

use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("VOXY_LOG")
                .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn")),
        )
        .init();

    let mut args = std::env::args_os().skip(1);
    let open = args.next().map(PathBuf::from);
    if open.as_ref().is_some_and(|p| p == "--version") {
        println!("{} {}", voxy_ui::PRODUCT_NAME, env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    voxy_ui::run(open).map_err(|e| anyhow::anyhow!("{e}"))
}
