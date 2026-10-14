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
use voxy_core::{AssetId, CompId, Document, Time};
use voxy_render::{
    Compositor, EffectsView, OUTPUT_FORMAT, OutputAlpha, RenderPlan, RenderReport, RenderSettings,
};

use crate::media::MediaPool;

/// What the viewport shows: a composition of a document state at a time.
#[derive(Debug, Clone, Copy)]
pub struct Shot<'a> {
    pub doc: &'a Document,
    pub comp: CompId,
    /// [`voxy_core::History::revision`] of `doc`.
    pub revision: u64,
    pub time: Time,
    /// Which effects to draw (solo, or none for the "before" side).
    pub effects: EffectsView,
}

/// The viewport's two render targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// The frame as shown (with effects, or the soloed one).
    Main = 0,
    /// The "before" side of the before/after split.
    Before = 1,
}

/// Everything that determines a viewport frame. Rendering is skipped while
/// this is unchanged.
#[derive(Debug, Clone, PartialEq)]
struct FrameKey {
    comp: CompId,
    revision: u64,
    time: Time,
    size: (u32, u32),
    effects: EffectsView,
    asset_root: Option<PathBuf>,
    /// The video frames drawn.
    video: Vec<(AssetId, Time, usize)>,
}

/// The scale the viewport renders at: fitting `max_px`, never above full
/// size, and never above the composition's preview resolution. Exports do not
/// use this; they render with [`RenderSettings::default`] (full size).
pub(crate) fn preview_scale(
    comp: (u32, u32),
    max_px: (f32, f32),
    res: voxy_core::PreviewResolution,
) -> f64 {
    let fit = f64::from((max_px.0 / comp.0 as f32).min(max_px.1 / comp.1 as f32));
    fit.clamp(0.01, 1.0).min(res.scale())
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
    targets: [Option<Target>; 2],
    last: [Option<FrameKey>; 2],
    /// The report of the last [`Slot::Main`] render.
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
            targets: [None, None],
            last: [None, None],
            last_report: RenderReport::default(),
            approximate: false,
        }
    }

    /// Lets the compositor run OpenFX effects through `runtime`.
    pub fn set_external(
        &mut self,
        runtime: std::sync::Arc<dyn voxy_core::external::ExternalEffects>,
    ) {
        self.compositor.set_external(Some(runtime));
    }

    /// The GPU's largest texture side in pixels.
    pub fn max_texture_dimension(&self) -> u32 {
        self.render_state.device.limits().max_texture_dimension_2d
    }

    /// Renders `shot.comp` at `shot.time`, sized to fit `max_px` physical
    /// pixels (never above full resolution), and returns the texture to show.
    /// `playing` makes video sources prefetch ahead instead of scrubbing.
    /// Each [`Slot`] has its own target and is skipped while unchanged.
    pub fn frame(
        &mut self,
        shot: &Shot<'_>,
        max_px: egui::Vec2,
        asset_root: Option<PathBuf>,
        playing: bool,
        slot: Slot,
    ) -> Option<(egui::TextureId, (u32, u32))> {
        let &Shot {
            doc,
            comp: eval_comp,
            revision,
            time,
            effects,
        } = shot;
        let comp = doc.comp(eval_comp)?;
        let settings = RenderSettings {
            scale: preview_scale(
                (comp.width, comp.height),
                (max_px.x, max_px.y),
                comp.preview_resolution,
            ),
            effects,
        };
        let size = settings.output_size(comp.width, comp.height);
        let eval = voxy_core::evaluate(doc, eval_comp, time)?;
        let video = self
            .media
            .resolve(doc, &eval, asset_root.as_deref(), playing);
        if slot == Slot::Main {
            self.approximate = video.approximate;
        }
        let key = FrameKey {
            comp: eval_comp,
            revision,
            time,
            size,
            effects,
            asset_root: asset_root.clone(),
            video: video.identity(),
        };
        let i = slot as usize;
        if self.last[i].as_ref() == Some(&key)
            && let Some(t) = &self.targets[i]
        {
            return Some((t.id, t.size));
        }

        self.compositor.set_asset_root(asset_root);
        self.ensure_target(i, size);
        let plan = RenderPlan::build(&eval, &settings);
        let target = self.targets[i].as_ref().expect("ensured");
        let report = self.compositor.render(
            &self.render_state.device,
            &self.render_state.queue,
            &doc.assets,
            &video,
            &plan,
            &target.view,
            OutputAlpha::Premultiplied,
        );
        if slot == Slot::Main {
            self.last_report = report;
        }
        self.last[i] = Some(key);
        Some((target.id, target.size))
    }

    fn ensure_target(&mut self, i: usize, size: (u32, u32)) {
        if self.targets[i].as_ref().is_some_and(|t| t.size == size) {
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
        let id = if let Some(old) = &self.targets[i] {
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
        self.targets[i] = Some(Target {
            size,
            view,
            id,
            _texture: texture,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxy_core::PreviewResolution as P;

    #[test]
    fn preview_resolution_lowers_the_viewport_scale() {
        let comp = (1920, 1080);
        let big = (4000.0, 4000.0);
        assert_eq!(preview_scale(comp, big, P::Full), 1.0);
        assert_eq!(preview_scale(comp, big, P::Half), 0.5);
        assert_eq!(preview_scale(comp, big, P::Third), 1.0 / 3.0);
        assert_eq!(preview_scale(comp, big, P::Custom(8)), 0.125);
        // A small panel already renders smaller than the setting asks for.
        assert_eq!(preview_scale(comp, (480.0, 270.0), P::Half), 0.25);
        // Never below the floor, never above full size.
        assert_eq!(preview_scale(comp, (1.0, 1.0), P::Full), 0.01);
    }

    #[test]
    fn preview_renders_get_a_different_key_but_export_does_not_depend_on_it() {
        let size = |res| {
            let s = RenderSettings::at_scale(preview_scale((1920, 1080), (2000.0, 2000.0), res));
            s.output_size(1920, 1080)
        };
        // The preview key holds the output size, so changing the preview
        // resolution changes the key and forces a re-render.
        assert_ne!(size(P::Full), size(P::Half));
        assert_eq!(size(P::Half), (960, 540));
        // Exports use the default settings, whatever the preview says.
        let export = RenderSettings::default();
        assert_eq!(export.output_size(1920, 1080), (1920, 1080));
        assert_eq!(export.effects, EffectsView::All);
    }
}
