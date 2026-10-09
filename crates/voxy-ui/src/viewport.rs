//! Renders the composition into a texture egui can display, on eframe's own
//! `wgpu::Device`, so no pixels are copied between devices.
//!
//! Video frames come from [`MediaPool`] without waiting: each render shows
//! the best frame decoded so far, and the decoder threads request a repaint
//! when a better one arrives. Uploading a frame to the GPU happens once per
//! frame (see `Compositor::ensure_video`).

use std::path::PathBuf;
use std::sync::Arc;

use egui_wgpu::RenderState;
use voxy_core::{AssetId, Document, Time};
use voxy_render::{
    Compositor, OUTPUT_FORMAT, OutputAlpha, RenderPlan, RenderReport, RenderSettings,
};

use crate::media::MediaPool;

/// Everything that determines a viewport frame. Rendering is skipped while
/// this is unchanged.
#[derive(Debug, Clone, PartialEq)]
struct FrameKey {
    revision: u64,
    time: Time,
    size: (u32, u32),
    asset_root: Option<PathBuf>,
    /// The video frames drawn.
    video: Vec<(AssetId, Time, usize)>,
}

struct Target {
    size: (u32, u32),
    view: wgpu::TextureView,
    id: egui::TextureId,
    _texture: wgpu::Texture,
}

pub struct ViewportRenderer {
    render_state: RenderState,
    compositor: Compositor,
    pub media: MediaPool,
    target: Option<Target>,
    last: Option<FrameKey>,
    pub last_report: RenderReport,
    /// The last frame showed a stand-in for some video frame.
    pub approximate: bool,
}

impl std::fmt::Debug for ViewportRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewportRenderer").finish_non_exhaustive()
    }
}

impl ViewportRenderer {
    /// `ctx` is repainted from decoder threads when video frames arrive.
    pub fn new(render_state: RenderState, ctx: egui::Context) -> Self {
        let compositor = Compositor::new(&render_state.device);
        ViewportRenderer {
            render_state,
            compositor,
            media: MediaPool::new(Some(Arc::new(move || ctx.request_repaint()))),
            target: None,
            last: None,
            last_report: RenderReport::default(),
            approximate: false,
        }
    }

    /// Renders the root composition at `time`, sized to fit `max_px` physical
    /// pixels (never above full resolution), and returns the texture to show.
    /// `playing` makes video sources prefetch ahead instead of scrubbing.
    pub fn frame(
        &mut self,
        doc: &Document,
        revision: u64,
        time: Time,
        max_px: egui::Vec2,
        asset_root: Option<PathBuf>,
        playing: bool,
    ) -> Option<(egui::TextureId, (u32, u32))> {
        let comp = doc.root();
        let fit = f64::from((max_px.x / comp.width as f32).min(max_px.y / comp.height as f32));
        let settings = RenderSettings {
            scale: fit.clamp(0.01, 1.0),
        };
        let size = settings.output_size(comp.width, comp.height);
        let eval = voxy_core::evaluate(doc, doc.root_comp, time)?;
        let video = self
            .media
            .resolve(doc, &eval, asset_root.as_deref(), playing);
        self.approximate = video.approximate;
        let key = FrameKey {
            revision,
            time,
            size,
            asset_root: asset_root.clone(),
            video: video.identity(),
        };
        if self.last.as_ref() == Some(&key)
            && let Some(t) = &self.target
        {
            return Some((t.id, t.size));
        }

        self.compositor.set_asset_root(asset_root);
        self.ensure_target(size);
        let plan = RenderPlan::build(&eval, &settings);
        let target = self.target.as_ref().expect("ensured");
        self.last_report = self.compositor.render(
            &self.render_state.device,
            &self.render_state.queue,
            &doc.assets,
            &video,
            &plan,
            &target.view,
            OutputAlpha::Premultiplied,
        );
        self.last = Some(key);
        Some((target.id, target.size))
    }

    fn ensure_target(&mut self, size: (u32, u32)) {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let device = &self.render_state.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxy-viewport"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut renderer = self.render_state.renderer.write();
        let id = if let Some(old) = &self.target {
            renderer.update_egui_texture_from_wgpu_texture(
                device,
                &view,
                wgpu::FilterMode::Linear,
                old.id,
            );
            old.id
        } else {
            renderer.register_native_texture(device, &view, wgpu::FilterMode::Linear)
        };
        drop(renderer);
        self.target = Some(Target {
            size,
            view,
            id,
            _texture: texture,
        });
    }
}
