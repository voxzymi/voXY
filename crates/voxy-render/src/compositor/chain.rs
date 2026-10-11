//! Executes effect chains (`docs/design-effects-ofx.md` §3.1 to §3.3).
//!
//! A chain renders a layer's source into a layer-local buffer covering the
//! chain's region of definition, at the preview scale, then runs each
//! effect as a full-buffer pass, ping-ponging between two textures. The
//! result is cached by the chain's content key, like nested compositions,
//! and drawn through the layer's transform by the composite pass.

use std::path::PathBuf;
use std::sync::Arc;

use voxy_core::glam::{DAffine2, DMat2, DVec2};
use voxy_core::{AssetId, AssetTable};

use super::{
    Compositor, LayerUniform, MAX_RENDER_DEPTH, RenderReport, VideoFrames, WORKING_FORMAT,
};
use crate::kernels::{Kernel, PassContext, Rect, chain_rod};
use crate::plan::{ChainPlan, DrawSource};

/// The memory the cache of rendered chains may hold.
const CHAIN_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

/// `EffectUniform` in `shaders/effects/common.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct EffectUniform {
    frame: [f32; 4],
    p: [[f32; 4]; 4],
    i: [u32; 4],
    w: [[f32; 4]; 256],
}

/// Counters for rendered effect chains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChainStats {
    /// Chains rendered (source plus every effect pass).
    pub renders: u64,
    /// Draws that found their chain already rendered (same content key).
    pub reused: u64,
    /// Effect passes run.
    pub passes: u64,
    /// Chains rendered at less than the preview scale because their region
    /// did not fit the GPU's texture limit.
    pub reduced: u64,
}

/// A rendered chain.
pub(super) struct ChainEntry {
    /// The chain it was rendered from; a hit must equal it.
    chain: Option<Arc<ChainPlan>>,
    deps: Vec<(AssetId, Option<PathBuf>)>,
    /// Texture size in texels.
    size: (u32, u32),
    /// The region the texture covers, in layer pixels.
    pub(super) rod: Rect,
    used: u64,
    complete: bool,
    /// Which of the two textures holds the result.
    result: usize,
    views: [wgpu::TextureView; 2],
    /// For sampling the result in the composite pass (image layout).
    pub(super) draw_groups: [wgpu::BindGroup; 2],
    /// For reading a texture in an effect pass (effect input layout).
    input_groups: [wgpu::BindGroup; 2],
    _textures: [wgpu::Texture; 2],
}

impl ChainEntry {
    pub(super) fn draw_group(&self) -> &wgpu::BindGroup {
        &self.draw_groups[self.result]
    }
}

pub(super) fn kernel_shader(kernel: Kernel) -> &'static str {
    match kernel {
        Kernel::Invert => include_str!("../shaders/effects/invert.wgsl"),
        Kernel::GaussianBlur | Kernel::BoxBlur => {
            include_str!("../shaders/effects/convolve.wgsl")
        }
        Kernel::DirectionalBlur => include_str!("../shaders/effects/directional.wgsl"),
        Kernel::RadialBlur => include_str!("../shaders/effects/radial.wgsl"),
        Kernel::ZoomBlur => include_str!("../shaders/effects/zoom.wgsl"),
        Kernel::BrightnessContrast => {
            include_str!("../shaders/effects/brightness_contrast.wgsl")
        }
        Kernel::Exposure => include_str!("../shaders/effects/exposure.wgsl"),
        Kernel::Tint => include_str!("../shaders/effects/tint.wgsl"),
        Kernel::HueSaturation => include_str!("../shaders/effects/hue_saturation.wgsl"),
        Kernel::Levels => include_str!("../shaders/effects/levels.wgsl"),
        Kernel::Posterize => include_str!("../shaders/effects/posterize.wgsl"),
    }
}

/// What a chain's source samples, once prepared.
enum SourceBind {
    Blank,
    Image(AssetId),
    Video(usize),
    Nested(u64),
}

impl Compositor {
    pub fn chain_stats(&self) -> ChainStats {
        self.chain_stats
    }

    /// Renders `chain` (or finds it already rendered) and returns its cache
    /// key, or `None` if its source cannot be drawn this frame.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_chain(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &AssetTable,
        video: &dyn VideoFrames,
        chain: &Arc<ChainPlan>,
        depth: usize,
        report: &mut RenderReport,
    ) -> Option<u64> {
        if depth > MAX_RENDER_DEPTH {
            report.skipped_precomps += 1;
            return None;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("voxy-chain"),
        });
        // Prepare the source with this chain's own encoder, so it is ready
        // before the chain's passes in the same submission.
        let space = chain.working_space;
        let (bind, size, color, mode) = match &chain.source {
            DrawSource::Solid(c) => (SourceBind::Blank, chain.source_size, *c, 0),
            DrawSource::Image { asset, opacity } => {
                if !self.ensure_image(device, queue, &mut encoder, assets, *asset, space) {
                    if !report.missing_assets.contains(asset) {
                        report.missing_assets.push(*asset);
                    }
                    return None;
                }
                let decoded = self.images.get(asset).map(|e| e.size);
                (
                    SourceBind::Image(*asset),
                    chain.source_size.or(decoded),
                    [*opacity, 0.0, 0.0, 0.0],
                    1,
                )
            }
            DrawSource::Video {
                asset,
                time,
                opacity,
            } => {
                let slot = video
                    .frame(*asset, *time)
                    .and_then(|f| self.ensure_video(device, queue, &mut encoder, &f, space));
                let Some(slot) = slot else {
                    if !report.pending_video.contains(asset) {
                        report.pending_video.push(*asset);
                    }
                    return None;
                };
                let (w, h) = self.video[slot].size;
                (
                    SourceBind::Video(slot),
                    chain
                        .source_size
                        .or(Some(DVec2::new(f64::from(w), f64::from(h)))),
                    [*opacity, 0.0, 0.0, 0.0],
                    1,
                )
            }
            DrawSource::Comp { plan, opacity } => {
                let key =
                    self.render_nested(device, queue, assets, video, plan, depth + 1, report)?;
                (
                    SourceBind::Nested(key),
                    chain.source_size,
                    [*opacity, 0.0, 0.0, 0.0],
                    1,
                )
            }
            // The planner never nests chains.
            DrawSource::Chain { .. } => return None,
        };
        let size = size.filter(|s| s.x > 0.0 && s.y > 0.0)?;

        let nodes: Vec<(Kernel, &voxy_core::EvaluatedEffect)> = chain
            .effects
            .iter()
            .map(|n| (n.kernel, &n.effect))
            .collect();
        let rod = chain_rod(&nodes, Rect::of_size(size), chain.scale).round_out();
        if rod.is_empty() {
            return None;
        }
        // Texels per layer pixel: the preview scale, reduced if the region
        // would not fit the GPU (design §3.2, size cap).
        let max = f64::from(device.limits().max_texture_dimension_2d);
        let extent = rod.size();
        let mut s = chain.scale;
        if extent.x * s > max || extent.y * s > max {
            s = (max / extent.x).min(max / extent.y);
            self.chain_stats.reduced += 1;
            tracing::warn!(
                "an effect region of {}x{} layer pixels was rendered at reduced resolution",
                extent.x,
                extent.y
            );
        }
        let texels = (
            ((extent.x * s).ceil() as u32).clamp(1, max as u32),
            ((extent.y * s).ceil() as u32).clamp(1, max as u32),
        );

        let key = chain.content_key();
        let deps = self.chain_asset_paths(assets, chain);
        if let Some(e) = self.chains.get_mut(&key)
            && e.complete
            && e.size == texels
            && e.deps == deps
            && e.chain
                .as_ref()
                .is_some_and(|c| Arc::ptr_eq(c, chain) || **c == **chain)
        {
            e.used = self.frame_counter;
            self.chain_stats.reused += 1;
            return Some(key);
        }
        let mut entry = self.take_chain_target(device, key, texels);
        entry.rod = rod;

        // The source, into texture 0: layer pixels → texels → clip space.
        let texel_from_layer =
            DAffine2::from_scale(DVec2::splat(s)) * DAffine2::from_translation(-rod.min);
        let clip_from_texel = DAffine2::from_mat2_translation(
            DMat2::from_diagonal(DVec2::new(
                2.0 / f64::from(texels.0),
                -2.0 / f64::from(texels.1),
            )),
            DVec2::new(-1.0, 1.0),
        );
        let m = clip_from_texel * texel_from_layer;
        let u = LayerUniform {
            col0: [
                m.matrix2.x_axis.x as f32,
                m.matrix2.x_axis.y as f32,
                0.0,
                0.0,
            ],
            col1: [
                m.matrix2.y_axis.x as f32,
                m.matrix2.y_axis.y as f32,
                0.0,
                0.0,
            ],
            col2: [m.translation.x as f32, m.translation.y as f32, 0.0, 0.0],
            size: [size.x as f32, size.y as f32, 0.0, 0.0],
            color,
            mode: [mode, 0, 0, 0],
        };
        self.ensure_layer_uniforms(device, self.uniform_stride);
        let (buffer, uniforms, _) = self.layer_uniforms.as_ref().expect("ensured");
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(&u));
        {
            let image_group = match bind {
                SourceBind::Blank => &self.blank_image,
                SourceBind::Image(a) => self
                    .images
                    .get(&a)
                    .map_or(&self.blank_image, |e| &e.bind_group),
                SourceBind::Video(slot) => &self.video[slot].bind_group,
                SourceBind::Nested(k) => self
                    .nested
                    .get(&k)
                    .map_or(&self.blank_image, |e| &e.bind_group),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voxy-chain-source"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &entry.views[0],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.layer_pipeline);
            pass.set_bind_group(0, uniforms, &[0]);
            pass.set_bind_group(1, image_group, &[]);
            pass.draw(0..6, 0..1);
        }

        // The effects, ping-ponging between the two textures; a kernel may
        // take several passes (a separable blur takes one per axis).
        let stride = self.effect_stride;
        let mut bytes = Vec::new();
        let mut passes = Vec::new();
        let ctx = PassContext {
            texel_scale: s,
            space,
            origin: rod.min,
        };
        for (kernel, fx) in &nodes {
            for k in kernel.passes(fx, &ctx) {
                let mut w = [[0.0f32; 4]; 256];
                for (i, v) in k.w.iter().take(1024).enumerate() {
                    w[i / 4][i % 4] = *v;
                }
                let u = EffectUniform {
                    frame: [
                        texels.0 as f32,
                        texels.1 as f32,
                        s as f32,
                        if space == voxy_core::WorkingSpace::SrgbCompat {
                            1.0
                        } else {
                            0.0
                        },
                    ],
                    p: k.p,
                    i: k.i,
                    w,
                };
                let at = bytes.len();
                bytes.extend_from_slice(bytemuck::bytes_of(&u));
                bytes.resize(at + stride as usize, 0);
                passes.push(*kernel);
            }
        }
        let mut cur = 0;
        if !bytes.is_empty() {
            self.ensure_effect_uniforms(device, bytes.len() as u64);
            let (buffer, group, _) = self.effect_uniforms.as_ref().expect("ensured");
            queue.write_buffer(buffer, 0, &bytes);
            for (i, kernel) in passes.iter().enumerate() {
                let Some(pipeline) = self.effect_pipelines.get(kernel) else {
                    continue;
                };
                let next = 1 - cur;
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("voxy-effect"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &entry.views[next],
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[(i as u64 * stride) as u32]);
                pass.set_bind_group(1, &entry.input_groups[cur], &[]);
                pass.draw(0..3, 0..1);
                cur = next;
                self.chain_stats.passes += 1;
            }
        }
        queue.submit([encoder.finish()]);
        self.chain_stats.renders += 1;
        entry.result = cur;
        entry.used = self.frame_counter;
        entry.chain = Some(Arc::clone(chain));
        entry.deps = deps;
        entry.complete = true;
        self.chains.insert(key, entry);
        Some(key)
    }

    fn chain_asset_paths(
        &self,
        assets: &AssetTable,
        chain: &ChainPlan,
    ) -> Vec<(AssetId, Option<PathBuf>)> {
        let mut ids = Vec::new();
        match &chain.source {
            DrawSource::Image { asset, .. } | DrawSource::Video { asset, .. } => ids.push(*asset),
            DrawSource::Comp { plan, .. } => ids = plan.assets(),
            DrawSource::Solid(_) | DrawSource::Chain { .. } => {}
        }
        ids.into_iter()
            .map(|a| {
                (
                    a,
                    assets
                        .get(a)
                        .map(|info| info.resolve(self.asset_root.as_deref())),
                )
            })
            .collect()
    }

    /// Two textures of `size` for `key`: the entry already there, a
    /// recycled one of the same size not drawn this frame, or new ones.
    fn take_chain_target(
        &mut self,
        device: &wgpu::Device,
        key: u64,
        size: (u32, u32),
    ) -> ChainEntry {
        if let Some(e) = self.chains.remove(&key)
            && e.size == size
        {
            return e;
        }
        let now = self.frame_counter;
        let stale = self
            .chains
            .iter()
            .filter(|(_, e)| e.size == size && e.used < now)
            .min_by_key(|(_, e)| e.used)
            .map(|(k, _)| *k);
        if let Some(k) = stale
            && let Some(e) = self.chains.remove(&k)
        {
            return e;
        }
        let make = || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("voxy-chain"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORKING_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let textures = [make(), make()];
        let views = [
            textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];
        let draw_groups = [
            self.image_bind_group(device, &views[0]),
            self.image_bind_group(device, &views[1]),
        ];
        let input = |view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("voxy-effect-input"),
                layout: &self.effect_input_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            })
        };
        let input_groups = [input(&views[0]), input(&views[1])];
        ChainEntry {
            chain: None,
            deps: Vec::new(),
            size,
            rod: Rect::of_size(DVec2::ONE),
            used: 0,
            complete: false,
            result: 0,
            views,
            draw_groups,
            input_groups,
            _textures: textures,
        }
    }

    fn ensure_effect_uniforms(&mut self, device: &wgpu::Device, len: u64) {
        if self
            .effect_uniforms
            .as_ref()
            .is_some_and(|(_, _, cap)| *cap >= len)
        {
            return;
        }
        let cap = len.next_power_of_two().max(self.effect_stride * 8);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxy-effect-uniforms"),
            size: cap,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxy-effect-uniforms"),
            layout: &self.effect_uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<EffectUniform>() as u64),
                }),
            }],
        });
        self.effect_uniforms = Some((buffer, group, cap));
    }

    /// Drops the least recently drawn chains (never one drawn this frame)
    /// until the cache fits its budget.
    pub(super) fn trim_chains(&mut self) {
        // Two RGBA16F textures per entry.
        let bytes = |e: &ChainEntry| u64::from(e.size.0) * u64::from(e.size.1) * 16;
        let mut total: u64 = self.chains.values().map(bytes).sum();
        while total > CHAIN_BUDGET_BYTES {
            let oldest = self
                .chains
                .iter()
                .filter(|(_, e)| e.used < self.frame_counter)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k);
            let Some(k) = oldest else { break };
            if let Some(e) = self.chains.remove(&k) {
                total -= bytes(&e);
            }
        }
    }
}
