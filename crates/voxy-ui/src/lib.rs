//! voXY application shell: an eframe (egui + wgpu) window with dockable
//! panels (`egui_dock`), menus, shortcuts and a GPU viewport.

mod actions;
mod app;
#[cfg(test)]
mod audit;
mod comp_dialog;
mod comp_draft;
mod compat;
mod dialogs;
mod drops;
mod ease_params;
mod ease_ui;
mod effect_presets;
mod effects_panel;
mod empty_state;
mod footage;
mod gizmo;
mod graph_editor;
mod layout;
mod media;
mod menu;
mod nav_time;
mod panels;
mod prefs;
mod project_panel;
mod project_view;
mod recovery;
mod reveal;
mod shortcut_editor;
mod state;
mod tabs_ui;
mod theme;
mod timeline;
mod tips;
mod transport;
mod viewport;
mod workspace;

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

/// A context that has run one frame. On macOS `Context::format_shortcut`
/// looks at the font's glyphs (for the ⌘ symbol), which panics before the
/// first frame has set the fonts up; Linux and Windows never touch fonts
/// there, so tests that format shortcuts outside a frame need this.
#[cfg(test)]
pub(crate) fn warmed_context() -> egui::Context {
    let ctx = egui::Context::default();
    let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    ctx
}
