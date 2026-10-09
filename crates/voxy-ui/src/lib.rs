//! voXY application shell: an eframe (egui + wgpu) window with dockable
//! panels (`egui_dock`), menus, shortcuts and a GPU viewport.

mod app;
mod graph_editor;
mod media;
mod panels;
mod state;
mod theme;
mod timeline;
mod transport;
mod viewport;

use std::path::PathBuf;

pub use app::{PRODUCT_NAME, VoxyApp};

/// Compile-time proof that eframe's renderer and the voXY compositor share one
/// `wgpu` crate (see the README's "Pinned GPU stack" section).
#[allow(dead_code)]
fn same_wgpu_as_eframe(device: &egui_wgpu::wgpu::Device) -> &voxy_render::wgpu::Device {
    device
}

/// Opens the main window and runs until it is closed.
pub fn run(open: Option<PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(PRODUCT_NAME)
            .with_app_id("voxy")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([800.0, 500.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        PRODUCT_NAME,
        options,
        Box::new(|cc| Ok(Box::new(VoxyApp::new(cc, open)))),
    )
}
