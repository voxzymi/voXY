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
    /// Which texture holds the result.
    result: usize,
    /// Two textures, or three for chains with a kernel that keeps its
    /// input while its passes run (`Kernel::keeps_input`).
    views: Vec<wgpu::TextureView>,
    /// For sampling each texture in the composite pass (image layout).
    pub(super) draw_groups: Vec<wgpu::BindGroup>,
    textures: Vec<wgpu::Texture>,
}

impl ChainEntry {
    pub(super) fn draw_group(&self) -> &wgpu::BindGroup {
        &self.draw_groups[self.result]
    }
}

/// The shaders of `kernel`'s stages ([`crate::kernels::KernelParams::stage`]).
pub(super) fn kernel_shaders(kernel: Kernel) -> Vec<&'static str> {
    match kernel {
        Kernel::DropShadow | Kernel::InnerShadow => vec![
            include_str!("../shaders/effects/shadow_mask.wgsl"),
            include_str!("../shaders/effects/convolve.wgsl"),
            include_str!("../shaders/effects/shadow.wgsl"),
        ],
        Kernel::MatteChoker => vec![
            include_str!("../shaders/effects/morphology.wgsl"),
            include_str!("../shaders/effects/convolve.wgsl"),
        ],
        Kernel::Stroke => vec![
            include_str!("../shaders/effects/stroke_seed.wgsl"),
            include_str!("../shaders/effects/stroke_flood.wgsl"),
            include_str!("../shaders/effects/stroke.wgsl"),
        ],
        Kernel::Glow => vec![
            include_str!("../shaders/effects/glow_bright.wgsl"),
            include_str!("../shaders/effects/convolve.wgsl"),
            include_str!("../shaders/effects/glow.wgsl"),
        ],
        _ => vec![kernel_shader(kernel)],
    }
}

fn kernel_shader(kernel: Kernel) -> &'static str {
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
        Kernel::Curves => include_str!("../shaders/effects/curves.wgsl"),
        Kernel::ColorBalance => include_str!("../shaders/effects/color_balance.wgsl"),
        Kernel::GradientMap => include_str!("../shaders/effects/gradient_map.wgsl"),
        Kernel::Lut => include_str!("../shaders/effects/lut.wgsl"),
        Kernel::Vignette => include_str!("../shaders/effects/vignette.wgsl"),
        Kernel::Pixelate => include_str!("../shaders/effects/pixelate.wgsl"),
        Kernel::Grain => include_str!("../shaders/effects/grain.wgsl"),
        Kernel::ChromaticAberration => {
            include_str!("../shaders/effects/chromatic_aberration.wgsl")
        }
        Kernel::DropShadow | Kernel::InnerShadow => {
            include_str!("../shaders/effects/shadow.wgsl")
        }
        Kernel::Glow => include_str!("../shaders/effects/glow.wgsl"),
        Kernel::Stroke => include_str!("../shaders/effects/stroke.wgsl"),
        Kernel::ChromaKey => include_str!("../shaders/effects/chroma_key.wgsl"),
        Kernel::LumaKey => include_str!("../shaders/effects/luma_key.wgsl"),
        Kernel::MatteChoker => include_str!("../shaders/effects/morphology.wgsl"),
        Kernel::Transform => include_str!("../shaders/effects/transform.wgsl"),
        Kernel::Mirror => include_str!("../shaders/effects/mirror.wgsl"),
        Kernel::Wave => include_str!("../shaders/effects/wave.wgsl"),
        Kernel::Ripple => include_str!("../shaders/effects/ripple.wgsl"),
        Kernel::Bulge => include_str!("../shaders/effects/bulge.wgsl"),
        Kernel::Tile => include_str!("../shaders/effects/tile.wgsl"),
        Kernel::Crossfade => include_str!("../shaders/effects/crossfade.wgsl"),
        Kernel::Wipe => include_str!("../shaders/effects/wipe.wgsl"),
        Kernel::Slide => include_str!("../shaders/effects/slide.wgsl"),
        Kernel::Zoom => include_str!("../shaders/effects/zoom_transition.wgsl"),
        Kernel::DisplacementMap => include_str!("../shaders/effects/displacement_map.wgsl"),
    }
}

/// A draw source ready to draw: what to bind, its size, colour and shader
/// mode.
type Prepared = (SourceBind, Option<DVec2>, [f32; 4], u32);

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
        let (bind, size, color, mode) = self.prepare_source(
            device,
            queue,
            &mut encoder,
            assets,
            video,
            &chain.source,
            chain.source_size,
            space,
            depth,
            report,
        )?;
        // The layers the effects read (Q-E1), prepared the same way; one
        // that cannot be drawn reads as transparent.
        let inputs: Vec<Option<Prepared>> = chain
            .effects
            .iter()
            .map(|n| {
                let input = n.input.as_ref()?;
                self.prepare_source(
                    device,
                    queue,
                    &mut encoder,
                    assets,
                    video,
                    &input.source,
                    input.size,
                    space,
                    depth,
                    report,
                )
            })
            .collect();
        let size = size.filter(|s| s.x > 0.0 && s.y > 0.0)?;

        let nodes: Vec<(Kernel, &voxy_core::EvaluatedEffect)> = chain
            .effects
            .iter()
            .map(|n| (n.kernel, &n.effect))
            .collect();
        let rod = chain_rod(&nodes, Rect::of_size(size), Rect::of_size(size)).round_out();
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
            let image_group = self.source_group(&bind);
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

        let input_textures = self.draw_inputs(
            device,
            queue,
            &mut encoder,
            &chain.effects,
            &inputs,
            texels,
            texel_from_layer,
        );
        let ctx = PassContext {
            texel_scale: s,
            space,
            origin: rod.min,
            layer_size: size,
            seed: 0,
        };
        let (cur, lut_missing) = self.run_effects(
            device,
            queue,
            &mut encoder,
            &mut entry,
            &chain.effects,
            &input_textures,
            ctx,
            texels,
            assets,
            report,
        );
        queue.submit([encoder.finish()]);
        self.chain_stats.renders += 1;
        entry.result = cur;
        entry.used = self.frame_counter;
        entry.chain = Some(Arc::clone(chain));
        entry.deps = deps;
        // Drawn without a LUT it could not load: try again next frame.
        entry.complete = !lut_missing;
        self.chains.insert(key, entry);
        Some(key)
    }

    /// Prepares a draw source for drawing into a chain texture: loads
    /// images and video frames, renders nested compositions. Returns what
    /// to bind, the size, colour and shader mode, or `None` if it cannot be
    /// drawn this frame (reported).
    #[allow(clippy::too_many_arguments)]
    fn prepare_source(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        assets: &AssetTable,
        video: &dyn VideoFrames,
        source: &DrawSource,
        size: Option<DVec2>,
        space: voxy_core::WorkingSpace,
        depth: usize,
        report: &mut RenderReport,
    ) -> Option<Prepared> {
        Some(match source {
            DrawSource::Solid(c) => (SourceBind::Blank, size, *c, 0),
            DrawSource::Image { asset, opacity } => {
                if !self.ensure_image(device, queue, encoder, assets, *asset, space) {
                    if !report.missing_assets.contains(asset) {
                        report.missing_assets.push(*asset);
                    }
                    return None;
                }
                let decoded = self.images.get(asset).map(|e| e.size);
                (
                    SourceBind::Image(*asset),
                    size.or(decoded),
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
                    .and_then(|f| self.ensure_video(device, queue, encoder, &f, space));
                let Some(slot) = slot else {
                    if !report.pending_video.contains(asset) {
                        report.pending_video.push(*asset);
                    }
                    return None;
                };
                let (w, h) = self.video[slot].size;
                (
                    SourceBind::Video(slot),
                    size.or(Some(DVec2::new(f64::from(w), f64::from(h)))),
                    [*opacity, 0.0, 0.0, 0.0],
                    1,
                )
            }
            DrawSource::Comp { plan, opacity } => {
                let key =
                    self.render_nested(device, queue, assets, video, plan, depth + 1, report)?;
                (SourceBind::Nested(key), size, [*opacity, 0.0, 0.0, 0.0], 1)
            }
            // The planner never nests chains, and adjustment layers are
            // rendered by `render_adjustment`.
            DrawSource::Chain { .. } | DrawSource::Adjustment { .. } => return None,
        })
    }

    /// The image bind group a prepared source samples.
    fn source_group(&self, bind: &SourceBind) -> &wgpu::BindGroup {
        match bind {
            SourceBind::Blank => &self.blank_image,
            SourceBind::Image(a) => self
                .images
                .get(a)
                .map_or(&self.blank_image, |e| &e.bind_group),
            SourceBind::Video(slot) => &self.video[*slot].bind_group,
            SourceBind::Nested(k) => self
                .nested
                .get(k)
                .map_or(&self.blank_image, |e| &e.bind_group),
        }
    }

    /// Draws each prepared layer input into a texture of `texels`, through
    /// `texel_from_layer` (the effect layer's pixels to texels) and its own
    /// placement. Input draws have their own uniform buffer, so they never
    /// disturb a composite in progress (adjustment layers run mid-way).
    #[allow(clippy::too_many_arguments)]
    fn draw_inputs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        effects: &[crate::plan::EffectNode],
        inputs: &[Option<Prepared>],
        texels: (u32, u32),
        texel_from_layer: DAffine2,
    ) -> Vec<Option<(wgpu::Texture, wgpu::TextureView)>> {
        let clip_from_texel = DAffine2::from_mat2_translation(
            DMat2::from_diagonal(DVec2::new(
                2.0 / f64::from(texels.0),
                -2.0 / f64::from(texels.1),
            )),
            DVec2::new(-1.0, 1.0),
        );
        let mut bytes = Vec::new();
        let mut draws = Vec::new();
        for (node, prepared) in effects.iter().zip(inputs) {
            let (Some(input), Some((_, Some(size), color, mode))) = (&node.input, prepared) else {
                draws.push(None);
                continue;
            };
            if size.x <= 0.0 || size.y <= 0.0 {
                draws.push(None);
                continue;
            }
            let m = clip_from_texel * texel_from_layer * input.from_input;
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
                color: *color,
                mode: [*mode, 0, 0, 0],
            };
            let offset = bytes.len() as u32;
            bytes.extend_from_slice(bytemuck::bytes_of(&u));
            bytes.resize(offset as usize + self.uniform_stride as usize, 0);
            draws.push(Some(offset));
        }
        if bytes.is_empty() {
            return draws.iter().map(|_| None).collect();
        }
        self.ensure_input_uniforms(device, bytes.len() as u64);
        let (buffer, uniforms, _) = self.input_uniforms.as_ref().expect("ensured");
        queue.write_buffer(buffer, 0, &bytes);
        let mut out = Vec::new();
        for (prepared, offset) in inputs.iter().zip(draws) {
            let (Some((bind, ..)), Some(offset)) = (prepared, offset) else {
                out.push(None);
                continue;
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("voxy-layer-input"),
                size: wgpu::Extent3d {
                    width: texels.0,
                    height: texels.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORKING_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("voxy-layer-input"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
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
                pass.set_bind_group(0, uniforms, &[offset]);
                pass.set_bind_group(1, self.source_group(bind), &[]);
                pass.draw(0..6, 0..1);
            }
            out.push(Some((texture, view)));
        }
        out
    }

    fn ensure_input_uniforms(&mut self, device: &wgpu::Device, len: u64) {
        if self
            .input_uniforms
            .as_ref()
            .is_some_and(|(_, _, cap)| *cap >= len)
        {
            return;
        }
        let cap = len.next_power_of_two().max(self.uniform_stride * 4);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxy-input-uniforms"),
            size: cap,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxy-input-uniforms"),
            layout: &self.layer_uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<LayerUniform>() as u64),
                }),
            }],
        });
        self.input_uniforms = Some((buffer, bind_group, cap));
    }

    /// Runs an adjustment layer's effects on `below`, the composite drawn so
    /// far (`size` texels, the whole composition), and leaves the result in
    /// `self.adjust` for the draw that mixes it back (design §1.5).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_adjustment(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &AssetTable,
        video: &dyn VideoFrames,
        depth: usize,
        chain: &ChainPlan,
        below: &wgpu::Texture,
        size: (u32, u32),
        report: &mut RenderReport,
    ) {
        let mut entry = match self.adjust.take() {
            Some(e) if e.size == size => e,
            _ => self.new_chain_target(device, size),
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("voxy-adjustment"),
        });
        encoder.copy_texture_to_texture(
            below.as_image_copy(),
            entry.textures[0].as_image_copy(),
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        let comp = chain.source_size.unwrap_or(DVec2::ONE);
        let texel_scale = f64::from(size.0) / comp.x.max(1.0);
        // Nested compositions among the inputs were rendered before the
        // composite started (it must not be disturbed now).
        let inputs: Vec<Option<Prepared>> = chain
            .effects
            .iter()
            .map(|n| {
                let input = n.input.as_ref()?;
                self.prepare_source(
                    device,
                    queue,
                    &mut encoder,
                    assets,
                    video,
                    &input.source,
                    input.size,
                    chain.working_space,
                    depth,
                    report,
                )
            })
            .collect();
        let input_textures = self.draw_inputs(
            device,
            queue,
            &mut encoder,
            &chain.effects,
            &inputs,
            size,
            DAffine2::from_scale(DVec2::splat(texel_scale)),
        );
        let ctx = PassContext {
            texel_scale,
            space: chain.working_space,
            origin: DVec2::ZERO,
            layer_size: comp,
            seed: 0,
        };
        let (cur, _) = self.run_effects(
            device,
            queue,
            &mut encoder,
            &mut entry,
            &chain.effects,
            &input_textures,
            ctx,
            size,
            assets,
            report,
        );
        queue.submit([encoder.finish()]);
        entry.result = cur;
        self.adjust = Some(entry);
    }

    /// Runs `effects` on `entry`, whose texture 0 holds their input, and
    /// returns the texture holding the result and whether a LUT was missing
    /// (design §3.1). Passes ping-pong between the textures; a kernel may
    /// take several passes and keep its input while they run.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_effects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        entry: &mut ChainEntry,
        effects: &[crate::plan::EffectNode],
        inputs: &[Option<(wgpu::Texture, wgpu::TextureView)>],
        ctx: PassContext,
        texels: (u32, u32),
        assets: &AssetTable,
        report: &mut RenderReport,
    ) -> (usize, bool) {
        // The effects, ping-ponging between the two textures; a kernel may
        // take several passes (a separable blur takes one per axis).
        let stride = self.effect_stride;
        let mut bytes = Vec::new();
        let mut passes = Vec::new();
        let mut lut_missing = false;
        let (s, space) = (ctx.texel_scale, ctx.space);
        for (index, node) in effects.iter().enumerate() {
            let (kernel, fx, seed) = (&node.kernel, &node.effect, &node.seed);
            // A LUT that cannot be loaded passes the image through.
            let lut = kernel.asset(fx);
            if let Some(a) = lut
                && !self.ensure_lut(device, queue, assets, a)
            {
                if !report.missing_assets.contains(&a) {
                    report.missing_assets.push(a);
                }
                lut_missing = true;
                continue;
            }
            let ctx = PassContext { seed: *seed, ..ctx };
            let mut first = true;
            for mut k in kernel.passes(fx, &ctx) {
                if let Some(e) = lut.and_then(|a| self.luts.get(&a)) {
                    k.p[1] = [e.domain_min[0], e.domain_min[1], e.domain_min[2], 0.0];
                    k.p[2] = [e.domain_max[0], e.domain_max[1], e.domain_max[2], 0.0];
                    k.i[1] = e.size;
                }
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
                passes.push((*kernel, lut, k.stage, first, index));
                first = false;
            }
        }
        if passes.iter().any(|(k, ..)| k.keeps_input()) {
            self.grow_chain_target(device, entry, 3);
        }
        let mut cur = 0;
        // The texture holding the current kernel's input, while it runs.
        let mut kept: Option<usize> = None;
        if !bytes.is_empty() {
            self.ensure_effect_uniforms(device, bytes.len() as u64);
            queue.write_buffer(
                &self.effect_uniforms.as_ref().expect("ensured").0,
                0,
                &bytes,
            );
            for (i, (kernel, lut, stage, first, index)) in passes.iter().enumerate() {
                if *first {
                    kept = kernel.keeps_input().then_some(cur);
                }
                let Some(pipeline) = self.effect_pipelines.get(&(*kernel, *stage)) else {
                    continue;
                };
                let lut_group = lut.and_then(|a| self.luts.get(&a)).map(|e| &e.bind_group);
                if kernel.uses_lut() && lut_group.is_none() {
                    continue;
                }
                let next = (0..entry.views.len())
                    .find(|&t| t != cur && Some(t) != kept)
                    .expect("a free texture");
                let layer_in = inputs
                    .get(*index)
                    .and_then(Option::as_ref)
                    .map_or(&self.blank_view, |(_, v)| v);
                let input = self.effect_input(
                    device,
                    &entry.views[cur],
                    &entry.views[kept.unwrap_or(cur)],
                    layer_in,
                );
                let (_, group, _) = self.effect_uniforms.as_ref().expect("ensured");
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
                pass.set_bind_group(1, &input, &[]);
                if let Some(g) = lut_group {
                    pass.set_bind_group(2, g, &[]);
                }
                pass.draw(0..3, 0..1);
                drop(pass);
                cur = next;
                self.chain_stats.passes += 1;
            }
        }
        (cur, lut_missing)
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
            DrawSource::Solid(_) | DrawSource::Chain { .. } | DrawSource::Adjustment { .. } => {}
        }
        ids.extend(
            chain
                .effects
                .iter()
                .filter_map(|n| n.kernel.asset(&n.effect)),
        );
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
        self.new_chain_target(device, size)
    }

    /// Two fresh textures of `size`.
    fn new_chain_target(&self, device: &wgpu::Device, size: (u32, u32)) -> ChainEntry {
        let mut entry = ChainEntry {
            chain: None,
            deps: Vec::new(),
            size,
            rod: Rect::of_size(DVec2::ONE),
            used: 0,
            complete: false,
            result: 0,
            views: Vec::new(),
            draw_groups: Vec::new(),
            textures: Vec::new(),
        };
        self.grow_chain_target(device, &mut entry, 2);
        entry
    }

    /// Gives `entry` at least `count` textures of its size.
    fn grow_chain_target(&self, device: &wgpu::Device, entry: &mut ChainEntry, count: usize) {
        while entry.textures.len() < count {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("voxy-chain"),
                size: wgpu::Extent3d {
                    width: entry.size.0,
                    height: entry.size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORKING_FORMAT,
                // COPY_DST: an adjustment layer's input is copied in.
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            entry.draw_groups.push(self.image_bind_group(device, &view));
            entry.views.push(view);
            entry.textures.push(texture);
        }
    }

    /// The effect-pass input: `src` (binding 0) and the kernel's own input
    /// `orig` (binding 2).
    fn effect_input(
        &self,
        device: &wgpu::Device,
        src: &wgpu::TextureView,
        orig: &wgpu::TextureView,
        layer_in: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxy-effect-input"),
            layout: &self.effect_chain_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(orig),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(layer_in),
                },
            ],
        })
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
        // Two or three RGBA16F textures per entry.
        let bytes = |e: &ChainEntry| {
            u64::from(e.size.0) * u64::from(e.size.1) * 8 * e.textures.len() as u64
        };
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
