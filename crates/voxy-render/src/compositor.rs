//! The wgpu compositor: executes a [`RenderPlan`].

mod chain;
mod lut;

pub use chain::ChainStats;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use voxy_core::glam::{DAffine2, DMat2, DVec2};
use voxy_core::{AssetId, AssetTable, BlendMode, Time, WorkingSpace};
use voxy_media::VideoFrame;

use crate::kernels::Kernel;
use crate::plan::{DrawSource, RenderPlan};

/// Supplies decoded video frames to the compositor.
///
/// The UI answers from the frame cache without waiting (the nearest frame
/// while scrubbing); export waits for the exact frame. Closures
/// `Fn(AssetId, Time) -> Option<Arc<VideoFrame>>` implement it.
pub trait VideoFrames {
    /// The frame to draw for `asset` at source time `time`. `None` skips the
    /// layer for this frame and lists it in [`RenderReport::pending_video`].
    fn frame(&self, asset: AssetId, time: Time) -> Option<Arc<VideoFrame>>;
}

impl<F: Fn(AssetId, Time) -> Option<Arc<VideoFrame>>> VideoFrames for F {
    fn frame(&self, asset: AssetId, time: Time) -> Option<Arc<VideoFrame>> {
        self(asset, time)
    }
}

/// No video available: video layers are skipped.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoVideo;

impl VideoFrames for NoVideo {
    fn frame(&self, _: AssetId, _: Time) -> Option<Arc<VideoFrame>> {
        None
    }
}

/// Prepared video frames kept on the GPU. Enough for several video layers
/// plus a few recent frames, so pausing, toggling or looping over the same
/// frames does not upload them again.
const VIDEO_SLOTS: usize = 8;

/// Format of the working buffer: premultiplied, half-float.
pub const WORKING_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Format of compositor output (display and export readback).
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Alpha convention of compositor output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputAlpha {
    /// For on-screen display (what egui expects).
    Premultiplied,
    /// For export to image and video files.
    Straight,
}

/// 8-bit sRGB-encoded RGBA pixels, rows top to bottom, no padding.
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RgbaImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// Problems that did not stop a frame from rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderReport {
    /// Image assets that could not be loaded; their layers were skipped.
    pub missing_assets: Vec<AssetId>,
    /// Video assets with no frame available yet; their layers were skipped.
    pub pending_video: Vec<AssetId>,
    /// Precomp layers left out because the nesting was too deep or circular.
    pub skipped_precomps: usize,
    /// Effects drawn as pass-through because this build has no kernel for
    /// them yet.
    pub unsupported_effects: usize,
    /// Placeholder effects (not installed) drawn as pass-through.
    pub missing_effects: usize,
    /// External (OpenFX) effects that failed and passed the image through,
    /// with the reason: the effect header shows it.
    pub external_errors: Vec<(voxy_core::EffectId, String)>,
}

/// GPU upload counters for video frames.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VideoUploadStats {
    /// Frames uploaded and prepared.
    pub uploads: u64,
    /// Draws that found their frame already on the GPU.
    pub reused: u64,
    /// Texture pairs created (an upload into an existing slot of the same
    /// size creates none).
    pub allocations: u64,
}

/// Counters for nested compositions rendered offscreen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NestedStats {
    /// Offscreen renders done.
    pub renders: u64,
    /// Draws that found their picture already rendered (same content key).
    pub reused: u64,
    /// Offscreen textures created (a render into a recycled one creates none).
    pub allocations: u64,
}

/// The memory the cache of rendered nested compositions may hold.
const NESTED_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

/// How deep `composite` recurses (the plan builder never exceeds
/// `voxy_core::MAX_NESTING`; this guards plans built by other means).
const MAX_RENDER_DEPTH: usize = 16;

/// A nested composition's rendered picture.
struct NestedEntry {
    /// The plan it was rendered from. A hit must equal it (the key is only a
    /// 64-bit hash), so a collision can never show the wrong picture.
    plan: Option<Arc<RenderPlan>>,
    /// The file each image or video asset in the plan resolved to when the
    /// picture was rendered. If one has since changed (a relink, a replaced
    /// file) the picture is stale even though the plan is the same.
    deps: Vec<(AssetId, Option<PathBuf>)>,
    size: (u32, u32),
    /// The frame counter when it was last drawn.
    used: u64,
    /// `false` if something it needs (an image, a video frame) was missing,
    /// so it must be rendered again next time.
    complete: bool,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    texture: wgpu::Texture,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("GPU readback failed: {0}")]
    Readback(String),
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    col0: [f32; 4],
    col1: [f32; 4],
    col2: [f32; 4],
    size: [f32; 4],
    color: [f32; 4],
    mode: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FlagsUniform {
    flags: [u32; 4],
}

struct ImageEntry {
    path: PathBuf,
    space: WorkingSpace,
    size: DVec2,
    bind_group: wgpu::BindGroup,
    _texture: wgpu::Texture,
}

/// What a draw samples.
#[derive(Debug, Clone, Copy)]
enum Bind {
    Image(AssetId),
    Video(usize),
    Nested(u64),
    Chain(u64),
    /// The result of the adjustment layer being drawn.
    Adjust,
}

/// One prepared video frame on the GPU.
struct VideoSlot {
    /// The frame it holds; held, so the pointer identifies it.
    frame: Arc<VideoFrame>,
    space: WorkingSpace,
    size: (u32, u32),
    /// The frame counter when it was last drawn.
    used: u64,
    source: wgpu::Texture,
    source_group: wgpu::BindGroup,
    flags: wgpu::Buffer,
    prepared_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    _prepared: wgpu::Texture,
}

struct WorkingTarget {
    size: (u32, u32),
    view: wgpu::TextureView,
    output_bind_group: wgpu::BindGroup,
    texture: wgpu::Texture,
}

/// A copy of a composite target, read by the blend shader.
struct Backdrop {
    size: (u32, u32),
    bind_group: wgpu::BindGroup,
    texture: wgpu::Texture,
}

struct Readback {
    size: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    padded_row: u32,
}

/// Owns the pipelines and GPU caches. Create one per `wgpu::Device`.
pub struct Compositor {
    layer_pipeline: wgpu::RenderPipeline,
    prepare_pipeline: wgpu::RenderPipeline,
    output_pipeline: wgpu::RenderPipeline,
    layer_uniform_layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    source_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    blank_image: wgpu::BindGroup,
    /// A transparent 1×1 texture: the layer input of kernels without one.
    blank_view: wgpu::TextureView,
    uniform_stride: u64,
    layer_uniforms: Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    output_flags: wgpu::Buffer,
    working: Option<WorkingTarget>,
    readback: Option<Readback>,
    images: HashMap<AssetId, ImageEntry>,
    failed: HashMap<AssetId, PathBuf>,
    asset_root: Option<PathBuf>,
    video: Vec<VideoSlot>,
    frame_counter: u64,
    video_stats: VideoUploadStats,
    /// Rendered nested compositions by plan content key.
    nested: HashMap<u64, NestedEntry>,
    nested_stats: NestedStats,
    /// Rendered effect chains by chain content key.
    chains: HashMap<u64, chain::ChainEntry>,
    chain_stats: ChainStats,
    /// One pipeline per kernel stage (most kernels have one stage).
    effect_pipelines: HashMap<(Kernel, u8), wgpu::RenderPipeline>,
    effect_chain_layout: wgpu::BindGroupLayout,
    effect_uniform_layout: wgpu::BindGroupLayout,
    effect_input_layout: wgpu::BindGroupLayout,
    /// LUT assets for the LUT effect, and their bind group layout.
    luts: HashMap<AssetId, lut::LutEntry>,
    /// Runs OpenFX effects on the CPU (design §6.9), when the app has one.
    external: Option<std::sync::Arc<dyn voxy_core::external::ExternalEffects>>,
    lut_layout: wgpu::BindGroupLayout,
    effect_uniforms: Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    effect_stride: u64,
    blend_pipeline: wgpu::RenderPipeline,
    backdrop: Option<Backdrop>,
    /// The textures adjustment layers run their effects in.
    adjust: Option<chain::ChainEntry>,
    /// Uniforms for drawing layer inputs, apart from the composite's.
    input_uniforms: Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
}

impl std::fmt::Debug for Compositor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compositor")
            .field("cached_images", &self.images.len())
            .field("asset_root", &self.asset_root)
            .finish_non_exhaustive()
    }
}

const COLOR_WGSL: &str = include_str!("shaders/color.wgsl");

fn shader(device: &wgpu::Device, label: &str, body: &str, with_color: bool) -> wgpu::ShaderModule {
    let source = if with_color {
        format!("{COLOR_WGSL}\n{body}")
    } else {
        body.to_owned()
    };
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    dynamic: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: None,
        },
        count: None,
    }
}

fn pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    layouts: &[Option<&wgpu::BindGroupLayout>],
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: layouts,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn space_flag(space: WorkingSpace) -> u32 {
    u32::from(space == WorkingSpace::Linear)
}

impl Compositor {
    /// Sets the provider that runs external (OpenFX) effects. Without one,
    /// they pass the image through and are reported.
    pub fn set_external(
        &mut self,
        external: Option<std::sync::Arc<dyn voxy_core::external::ExternalEffects>>,
    ) {
        self.external = external;
    }

    pub fn new(device: &wgpu::Device) -> Self {
        let layer_uniform_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("voxy-layer-uniform"),
                entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT, true)],
            });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voxy-layer-image"),
            entries: &[
                texture_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let source_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voxy-source"),
            entries: &[
                texture_entry(0),
                uniform_entry(1, wgpu::ShaderStages::FRAGMENT, false),
            ],
        });

        let premultiplied_over = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;
        let layer_pipeline = pipeline(
            device,
            "voxy-layer",
            &shader(
                device,
                "voxy-layer",
                include_str!("shaders/layer.wgsl"),
                false,
            ),
            &[Some(&layer_uniform_layout), Some(&image_layout)],
            WORKING_FORMAT,
            Some(premultiplied_over),
        );
        let prepare_pipeline = pipeline(
            device,
            "voxy-prepare-image",
            &shader(
                device,
                "voxy-prepare-image",
                include_str!("shaders/prepare_image.wgsl"),
                true,
            ),
            &[Some(&source_layout)],
            WORKING_FORMAT,
            None,
        );
        let output_pipeline = pipeline(
            device,
            "voxy-output",
            &shader(
                device,
                "voxy-output",
                include_str!("shaders/output.wgsl"),
                true,
            ),
            &[Some(&source_layout)],
            OUTPUT_FORMAT,
            None,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voxy-image-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let blank = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxy-blank"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        // Textures start zeroed, so this one is transparent.
        let blank_view = blank.create_view(&wgpu::TextureViewDescriptor::default());
        let blank_image = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxy-blank"),
            layout: &image_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &blank.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let size = std::mem::size_of::<LayerUniform>() as u64;
        let uniform_stride = size.div_ceil(align) * align;
        let effect_size = std::mem::size_of::<chain::EffectUniform>() as u64;
        let effect_stride = effect_size.div_ceil(align) * align;

        let effect_uniform_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("voxy-effect-uniform"),
                entries: &[uniform_entry(0, wgpu::ShaderStages::FRAGMENT, true)],
            });
        let effect_input_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("voxy-effect-input"),
                entries: &[
                    texture_entry(0),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let blend_pipeline = pipeline(
            device,
            "voxy-blend",
            &shader(
                device,
                "voxy-blend",
                include_str!("shaders/blend.wgsl"),
                false,
            ),
            &[
                Some(&layer_uniform_layout),
                Some(&image_layout),
                Some(&effect_input_layout),
            ],
            WORKING_FORMAT,
            None,
        );
        // Effect passes read the previous image (binding 0, with a sampler)
        // and the kernel's own input (binding 2, for kernels that combine
        // their result with it).
        let effect_chain_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("voxy-effect-chain"),
                entries: &[
                    texture_entry(0),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    texture_entry(2),
                    texture_entry(3),
                ],
            });
        let lut_layout = lut::lut_layout(device);
        let layouts = [
            Some(&effect_uniform_layout),
            Some(&effect_chain_layout),
            Some(&lut_layout),
        ];
        let effect_pipelines = Kernel::ALL
            .iter()
            .flat_map(|&k| {
                chain::kernel_shaders(k)
                    .into_iter()
                    .enumerate()
                    .map(move |(stage, body)| (k, stage as u8, body))
            })
            .map(|(k, stage, body)| {
                let body = format!("{}\n{}", include_str!("shaders/effects/common.wgsl"), body);
                let label = format!("voxy-effect-{}-{stage}", k.type_id());
                let module = shader(device, &label, &body, true);
                let p = pipeline(
                    device,
                    &label,
                    &module,
                    if k.uses_lut() {
                        &layouts[..]
                    } else {
                        &layouts[..2]
                    },
                    WORKING_FORMAT,
                    None,
                );
                ((k, stage), p)
            })
            .collect();

        let output_flags = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxy-output-flags"),
            size: std::mem::size_of::<FlagsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Compositor {
            layer_pipeline,
            prepare_pipeline,
            output_pipeline,
            layer_uniform_layout,
            image_layout,
            source_layout,
            sampler,
            blank_image,
            blank_view,
            uniform_stride,
            layer_uniforms: None,
            output_flags,
            working: None,
            readback: None,
            images: HashMap::new(),
            failed: HashMap::new(),
            asset_root: None,
            video: Vec::new(),
            frame_counter: 0,
            video_stats: VideoUploadStats::default(),
            nested: HashMap::new(),
            nested_stats: NestedStats::default(),
            chains: HashMap::new(),
            chain_stats: ChainStats::default(),
            effect_pipelines,
            effect_uniform_layout,
            effect_input_layout,
            luts: HashMap::new(),
            external: None,
            lut_layout,
            effect_chain_layout,
            effect_uniforms: None,
            effect_stride,
            blend_pipeline,
            backdrop: None,
            adjust: None,
            input_uniforms: None,
        }
    }

    pub fn video_stats(&self) -> VideoUploadStats {
        self.video_stats
    }

    pub fn nested_stats(&self) -> NestedStats {
        self.nested_stats
    }

    /// Sets the directory relative asset paths resolve against (the folder
    /// containing the `.voxy` file) and drops cached images.
    pub fn set_asset_root(&mut self, root: Option<PathBuf>) {
        if self.asset_root != root {
            self.asset_root = root;
            self.images.clear();
            self.failed.clear();
            self.luts.clear();
            self.nested.clear();
            self.chains.clear();
        }
    }

    /// Renders `plan` into `target`, which must be an [`OUTPUT_FORMAT`]
    /// texture of the plan's size with `RENDER_ATTACHMENT` usage.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &AssetTable,
        video: &dyn VideoFrames,
        plan: &RenderPlan,
        target: &wgpu::TextureView,
        alpha: OutputAlpha,
    ) -> RenderReport {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("voxy-frame"),
        });
        let report = self.encode(
            device,
            queue,
            &mut encoder,
            assets,
            video,
            plan,
            target,
            alpha,
        );
        queue.submit([encoder.finish()]);
        report
    }

    /// Renders `plan` and reads the result back to the CPU.
    pub fn render_to_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &AssetTable,
        video: &dyn VideoFrames,
        plan: &RenderPlan,
        alpha: OutputAlpha,
    ) -> Result<(RgbaImage, RenderReport), RenderError> {
        let size = (plan.width, plan.height);
        if self.readback.as_ref().is_none_or(|r| r.size != size) {
            self.readback = Some(create_readback(device, size));
        }
        let rb = self.readback.take().expect("created above");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("voxy-frame-readback"),
        });
        let report = self.encode(
            device,
            queue,
            &mut encoder,
            assets,
            video,
            plan,
            &rb.view,
            alpha,
        );
        encoder.copy_texture_to_buffer(
            rb.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &rb.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(rb.padded_row),
                    rows_per_image: Some(size.1),
                },
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        let submission = queue.submit([encoder.finish()]);
        let result = read_buffer(device, &rb, submission);
        self.readback = Some(rb);
        Ok((result?, report))
    }

    #[allow(clippy::too_many_arguments)]
    fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        assets: &AssetTable,
        video: &dyn VideoFrames,
        plan: &RenderPlan,
        target: &wgpu::TextureView,
        alpha: OutputAlpha,
    ) -> RenderReport {
        let mut report = RenderReport::default();
        self.frame_counter += 1;
        self.ensure_working(device, (plan.width, plan.height));
        // Out of `self` while compositing, so the recursion can borrow it.
        let working = self.working.take().expect("ensured");
        self.composite(
            device,
            queue,
            encoder,
            assets,
            video,
            plan,
            &working.view,
            &working.texture,
            0,
            &mut report,
        );
        queue.write_buffer(
            &self.output_flags,
            0,
            bytemuck::bytes_of(&FlagsUniform {
                flags: [
                    space_flag(plan.working_space),
                    u32::from(alpha == OutputAlpha::Premultiplied),
                    0,
                    0,
                ],
            }),
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voxy-output"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.output_pipeline);
            pass.set_bind_group(0, &working.output_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.working = Some(working);
        report.skipped_precomps += plan.skipped_precomps;
        report.unsupported_effects += plan.unsupported_effects;
        report.missing_effects += plan.missing_effects;
        self.trim_nested();
        self.trim_chains();
        report
    }

    /// Draws `plan` into `target` (cleared to the plan's clear colour):
    /// first any nested compositions, each rendered offscreen, then the
    /// layers. Problems are added to `report`.
    #[allow(clippy::too_many_arguments)]
    fn composite(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        assets: &AssetTable,
        video: &dyn VideoFrames,
        plan: &RenderPlan,
        target: &wgpu::TextureView,
        target_texture: &wgpu::Texture,
        depth: usize,
        report: &mut RenderReport,
    ) {
        // Nested compositions first: each is rendered and submitted on its
        // own, so its per-draw uniforms are written (and used) before this
        // plan's are.
        let mut nested_keys: Vec<Option<u64>> = vec![None; plan.draws.len()];
        for (i, draw) in plan.draws.iter().enumerate() {
            match &draw.source {
                DrawSource::Comp { plan: child, .. } => {
                    nested_keys[i] =
                        self.render_nested(device, queue, assets, video, child, depth + 1, report);
                }
                // A chain is rendered (and its source prepared) on its own.
                DrawSource::Chain { chain, .. } => {
                    nested_keys[i] =
                        self.render_chain(device, queue, assets, video, chain, depth, report);
                }
                // Nested compositions an adjustment layer reads are
                // rendered now: rendering one mid-composite would overwrite
                // this composite's uniforms.
                DrawSource::Adjustment { chain, .. } => {
                    for (_, input) in chain.effects.iter().flat_map(|n| &n.inputs) {
                        if let DrawSource::Comp { plan: child, .. } = &input.source {
                            self.render_nested(
                                device,
                                queue,
                                assets,
                                video,
                                child,
                                depth + 1,
                                report,
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        // Load and prepare any images and video frames this frame needs.
        let mut video_slots = vec![None; plan.draws.len()];
        for (i, draw) in plan.draws.iter().enumerate() {
            match &draw.source {
                DrawSource::Image { asset, .. } => {
                    let asset = *asset;
                    if !self.ensure_image(device, queue, encoder, assets, asset, plan.working_space)
                        && !report.missing_assets.contains(&asset)
                    {
                        report.missing_assets.push(asset);
                    }
                }
                DrawSource::Video { asset, time, .. } => {
                    let (asset, time) = (*asset, *time);
                    video_slots[i] = video.frame(asset, time).and_then(|frame| {
                        self.ensure_video(device, queue, encoder, &frame, plan.working_space)
                    });
                    if video_slots[i].is_none() && !report.pending_video.contains(&asset) {
                        report.pending_video.push(asset);
                    }
                }
                DrawSource::Solid(_)
                | DrawSource::Comp { .. }
                | DrawSource::Chain { .. }
                | DrawSource::Adjustment { .. } => {}
            }
        }

        // Per-draw uniforms, one aligned slot each.
        let clip_from_output = DAffine2::from_mat2_translation(
            DMat2::from_diagonal(DVec2::new(
                2.0 / f64::from(plan.width),
                -2.0 / f64::from(plan.height),
            )),
            DVec2::new(-1.0, 1.0),
        );
        let mut draws = Vec::with_capacity(plan.draws.len());
        let mut uniform_bytes = Vec::new();
        for (i, draw) in plan.draws.iter().enumerate() {
            let (color, mode, size, bind) = match &draw.source {
                DrawSource::Solid(c) => (*c, 0, draw.local_size, None),
                DrawSource::Image { asset, opacity } => {
                    let Some(entry) = self.images.get(asset) else {
                        continue;
                    };
                    (
                        [*opacity, 0.0, 0.0, 0.0],
                        1,
                        draw.local_size.or(Some(entry.size)),
                        Some(Bind::Image(*asset)),
                    )
                }
                DrawSource::Video { opacity, .. } => {
                    let Some(slot) = video_slots[i] else {
                        continue;
                    };
                    let (w, h) = self.video[slot].size;
                    (
                        [*opacity, 0.0, 0.0, 0.0],
                        1,
                        draw.local_size
                            .or(Some(DVec2::new(f64::from(w), f64::from(h)))),
                        Some(Bind::Video(slot)),
                    )
                }
                DrawSource::Comp {
                    opacity,
                    plan: child,
                } => {
                    let Some(key) = nested_keys[i] else {
                        continue;
                    };
                    // The picture is the child's own size (times the preview
                    // scale); the quad is the child's size in layer pixels.
                    let _ = child;
                    (
                        [*opacity, 0.0, 0.0, 0.0],
                        1,
                        draw.local_size,
                        Some(Bind::Nested(key)),
                    )
                }
                DrawSource::Adjustment { opacity, .. } => (
                    [*opacity, 0.0, 0.0, 0.0],
                    1,
                    draw.local_size,
                    Some(Bind::Adjust),
                ),
                DrawSource::Chain { opacity, .. } => {
                    let Some(key) = nested_keys[i] else {
                        continue;
                    };
                    let Some(entry) = self.chains.get(&key) else {
                        continue;
                    };
                    // The quad covers the chain's region, in layer pixels.
                    let rod = entry.rod;
                    (
                        [*opacity, 0.0, 0.0, 0.0],
                        1,
                        Some(rod.size()),
                        Some(Bind::Chain(key)),
                    )
                }
            };
            let Some(size) = size else { continue };
            let origin = match (&draw.source, nested_keys[i]) {
                (DrawSource::Chain { .. }, Some(key)) => {
                    self.chains.get(&key).map_or(DVec2::ZERO, |e| e.rod.min)
                }
                _ => DVec2::ZERO,
            };
            let m = clip_from_output * draw.output_from_local * DAffine2::from_translation(origin);
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
                mode: [
                    mode,
                    crate::blend::mode_index(draw.blend),
                    u32::from(matches!(draw.source, DrawSource::Adjustment { .. })),
                    0,
                ],
            };
            let offset = uniform_bytes.len() as u64;
            uniform_bytes.extend_from_slice(bytemuck::bytes_of(&u));
            uniform_bytes.resize((offset + self.uniform_stride) as usize, 0);
            let adjust = match &draw.source {
                DrawSource::Adjustment { chain, .. } => Some(Arc::clone(chain)),
                _ => None,
            };
            // Blend modes and adjustment layers read what is below them.
            let special = draw.blend != BlendMode::Normal || adjust.is_some();
            draws.push((offset as u32, bind, special, adjust));
        }
        // With adjustment layers the composite is built on transparency and
        // the background goes under it at the end, so effects never touch
        // the background (as in After Effects, it is not a layer).
        let adjusted = draws.iter().any(|d| d.3.is_some());
        let background = (adjusted && plan.clear[3] > 0.0).then(|| {
            let m = clip_from_output
                * DAffine2::from_scale(DVec2::new(f64::from(plan.width), f64::from(plan.height)));
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
                size: [1.0, 1.0, 0.0, 0.0],
                color: plan.clear.map(|v| v as f32),
                mode: [0, 0, 2, 0],
            };
            let offset = uniform_bytes.len() as u64;
            uniform_bytes.extend_from_slice(bytemuck::bytes_of(&u));
            uniform_bytes.resize((offset + self.uniform_stride) as usize, 0);
            offset as u32
        });
        if !uniform_bytes.is_empty() {
            self.ensure_layer_uniforms(device, uniform_bytes.len() as u64);
            let (buffer, _, _) = self.layer_uniforms.as_ref().expect("ensured");
            queue.write_buffer(buffer, 0, &uniform_bytes);
        }
        if draws.iter().any(|d| d.2) {
            self.ensure_backdrop(device, (plan.width, plan.height));
        }
        // Normal draws share one pass; each draw with another blend mode
        // copies the target into the backdrop first and gets its own pass.
        let [r, g, b, a] = if adjusted { [0.0; 4] } else { plan.clear };
        let mut load = wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a });
        let mut start = 0;
        loop {
            let end = draws[start..]
                .iter()
                .position(|d| d.2)
                .map_or(draws.len(), |p| start + p);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("voxy-composite"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                if let Some((_, uniforms, _)) = &self.layer_uniforms {
                    pass.set_pipeline(&self.layer_pipeline);
                    for (offset, bind, ..) in &draws[start..end] {
                        pass.set_bind_group(0, uniforms, &[*offset]);
                        pass.set_bind_group(1, self.image_group(*bind), &[]);
                        pass.draw(0..6, 0..1);
                    }
                }
            }
            load = wgpu::LoadOp::Load;
            if end == draws.len() {
                break;
            }
            let (offset, bind, _, adjust) = draws[end].clone();
            if let Some(backdrop) = &self.backdrop {
                encoder.copy_texture_to_texture(
                    target_texture.as_image_copy(),
                    backdrop.texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: plan.width,
                        height: plan.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            if let Some(chain) = &adjust {
                // What is below must be drawn before the effects read it.
                let done = std::mem::replace(
                    encoder,
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("voxy-composite"),
                    }),
                );
                queue.submit([done.finish()]);
                self.render_adjustment(
                    device,
                    queue,
                    assets,
                    video,
                    depth,
                    chain,
                    target_texture,
                    (plan.width, plan.height),
                    report,
                );
            }
            if let (Some((_, uniforms, _)), Some(backdrop)) = (&self.layer_uniforms, &self.backdrop)
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("voxy-blend"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&self.blend_pipeline);
                pass.set_bind_group(0, uniforms, &[offset]);
                pass.set_bind_group(1, self.image_group(bind), &[]);
                pass.set_bind_group(2, &backdrop.bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
            if adjust.is_some() {
                // The next adjustment layer reuses the same textures.
                let done = std::mem::replace(
                    encoder,
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("voxy-composite"),
                    }),
                );
                queue.submit([done.finish()]);
            }
            start = end + 1;
        }
        if let (Some(offset), Some((_, uniforms, _)), Some(backdrop)) =
            (background, &self.layer_uniforms, &self.backdrop)
        {
            encoder.copy_texture_to_texture(
                target_texture.as_image_copy(),
                backdrop.texture.as_image_copy(),
                wgpu::Extent3d {
                    width: plan.width,
                    height: plan.height,
                    depth_or_array_layers: 1,
                },
            );
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voxy-background"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.blend_pipeline);
            pass.set_bind_group(0, uniforms, &[offset]);
            pass.set_bind_group(1, &self.blank_image, &[]);
            pass.set_bind_group(2, &backdrop.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    /// The image bind group a draw samples.
    fn image_group(&self, bind: Option<Bind>) -> &wgpu::BindGroup {
        match bind {
            Some(Bind::Image(a)) => self
                .images
                .get(&a)
                .map_or(&self.blank_image, |e| &e.bind_group),
            Some(Bind::Video(slot)) => &self.video[slot].bind_group,
            Some(Bind::Nested(key)) => self
                .nested
                .get(&key)
                .map_or(&self.blank_image, |e| &e.bind_group),
            Some(Bind::Chain(key)) => self
                .chains
                .get(&key)
                .map_or(&self.blank_image, chain::ChainEntry::draw_group),
            Some(Bind::Adjust) => self
                .adjust
                .as_ref()
                .map_or(&self.blank_image, chain::ChainEntry::draw_group),
            None => &self.blank_image,
        }
    }

    /// A backdrop texture of `size` (the composite target's size).
    fn ensure_backdrop(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.backdrop.as_ref().is_some_and(|b| b.size == size) {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxy-backdrop"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_FORMAT,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxy-backdrop"),
            layout: &self.effect_input_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.backdrop = Some(Backdrop {
            size,
            bind_group,
            texture,
        });
    }

    /// Renders a nested composition's plan offscreen (or finds it already
    /// rendered) and returns its cache key, or `None` if it cannot be drawn.
    /// Rendered pictures are shared by every draw with the same content key.
    #[allow(clippy::too_many_arguments)]
    fn render_nested(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &AssetTable,
        video: &dyn VideoFrames,
        plan: &Arc<RenderPlan>,
        depth: usize,
        report: &mut RenderReport,
    ) -> Option<u64> {
        if depth > MAX_RENDER_DEPTH {
            report.skipped_precomps += 1;
            return None;
        }
        let size = (plan.width, plan.height);
        let max = device.limits().max_texture_dimension_2d;
        if size.0 == 0 || size.1 == 0 || size.0 > max || size.1 > max {
            tracing::warn!(
                "a nested composition of {}x{} is outside this GPU's limits",
                size.0,
                size.1
            );
            return None;
        }
        let key = plan.content_key();
        let deps = self.asset_paths(assets, plan);
        if let Some(e) = self.nested.get_mut(&key)
            && e.complete
            && e.size == size
            && e.deps == deps
            && e.plan
                .as_ref()
                .is_some_and(|p| Arc::ptr_eq(p, plan) || **p == **plan)
        {
            e.used = self.frame_counter;
            self.nested_stats.reused += 1;
            return Some(key);
        }
        let mut entry = self.take_nested_target(device, key, size);
        let mut sub = RenderReport::default();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("voxy-nested"),
        });
        self.composite(
            device,
            queue,
            &mut encoder,
            assets,
            video,
            plan,
            &entry.view,
            &entry.texture,
            depth,
            &mut sub,
        );
        queue.submit([encoder.finish()]);
        self.nested_stats.renders += 1;
        entry.used = self.frame_counter;
        entry.plan = Some(Arc::clone(plan));
        entry.deps = deps;
        entry.complete = sub.missing_assets.is_empty()
            && sub.pending_video.is_empty()
            && sub.skipped_precomps == 0;
        for a in sub.missing_assets {
            if !report.missing_assets.contains(&a) {
                report.missing_assets.push(a);
            }
        }
        for a in sub.pending_video {
            if !report.pending_video.contains(&a) {
                report.pending_video.push(a);
            }
        }
        report.skipped_precomps += sub.skipped_precomps;
        self.nested.insert(key, entry);
        Some(key)
    }

    /// The file each image or video asset in `plan` (and its nested plans)
    /// resolves to now, in asset-id order.
    fn asset_paths(
        &self,
        assets: &AssetTable,
        plan: &RenderPlan,
    ) -> Vec<(AssetId, Option<PathBuf>)> {
        plan.assets()
            .into_iter()
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

    /// An offscreen target of `size` for `key`: the entry already there, or a
    /// recycled one of the same size that was not drawn this frame, or a new
    /// texture.
    fn take_nested_target(
        &mut self,
        device: &wgpu::Device,
        key: u64,
        size: (u32, u32),
    ) -> NestedEntry {
        if let Some(e) = self.nested.remove(&key)
            && e.size == size
        {
            return e;
        }
        let now = self.frame_counter;
        let stale = self
            .nested
            .iter()
            .filter(|(_, e)| e.size == size && e.used < now)
            .min_by_key(|(_, e)| e.used)
            .map(|(k, _)| *k);
        if let Some(k) = stale
            && let Some(e) = self.nested.remove(&k)
        {
            return e;
        }
        self.nested_stats.allocations += 1;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxy-nested"),
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
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.image_bind_group(device, &view);
        NestedEntry {
            plan: None,
            deps: Vec::new(),
            size,
            used: 0,
            complete: false,
            view,
            bind_group,
            texture,
        }
    }

    /// Drops the least recently drawn nested pictures (never one drawn this
    /// frame) until the cache fits its budget.
    fn trim_nested(&mut self) {
        let bytes = |e: &NestedEntry| u64::from(e.size.0) * u64::from(e.size.1) * 8;
        let mut total: u64 = self.nested.values().map(bytes).sum();
        while total > NESTED_BUDGET_BYTES {
            let oldest = self
                .nested
                .iter()
                .filter(|(_, e)| e.used < self.frame_counter)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k);
            let Some(k) = oldest else { break };
            if let Some(e) = self.nested.remove(&k) {
                total -= bytes(&e);
            }
        }
    }

    fn ensure_working(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.working.as_ref().is_some_and(|w| w.size == size) {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxy-working"),
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
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let output_bind_group = self.source_bind_group(device, &view, &self.output_flags);
        self.working = Some(WorkingTarget {
            size,
            view,
            output_bind_group,
            texture,
        });
    }

    fn ensure_layer_uniforms(&mut self, device: &wgpu::Device, len: u64) {
        if self
            .layer_uniforms
            .as_ref()
            .is_some_and(|(_, _, cap)| *cap >= len)
        {
            return;
        }
        let cap = len.next_power_of_two().max(self.uniform_stride * 16);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxy-layer-uniforms"),
            size: cap,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxy-layer-uniforms"),
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
        self.layer_uniforms = Some((buffer, bind_group, cap));
    }

    fn source_bind_group(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        flags: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxy-source"),
            layout: &self.source_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: flags.as_entire_binding(),
                },
            ],
        })
    }

    /// Makes sure `asset` is decoded and prepared for `space`. Returns
    /// `false` if it cannot be loaded.
    fn ensure_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        assets: &AssetTable,
        asset: AssetId,
        space: WorkingSpace,
    ) -> bool {
        let Some(info) = assets.get(asset) else {
            return false;
        };
        let path = info.resolve(self.asset_root.as_deref());
        if let Some(e) = self.images.get(&asset)
            && e.path == path
            && e.space == space
        {
            return true;
        }
        if self.failed.get(&asset) == Some(&path) {
            return false;
        }
        let decoded = match voxy_media::load_image(&path) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("image asset {asset} unavailable: {e}");
                self.failed.insert(asset, path);
                return false;
            }
        };
        let max = device.limits().max_texture_dimension_2d;
        if decoded.width == 0 || decoded.height == 0 || decoded.width > max || decoded.height > max
        {
            tracing::warn!(
                "image asset {asset} is {}x{}, outside this GPU's 1..={max} texture limit",
                decoded.width,
                decoded.height
            );
            self.failed.insert(asset, path);
            return false;
        }
        let size = (decoded.width, decoded.height);
        let (source, prepared, prepared_view) = texture_pair(device, size, "voxy-image");
        write_rgba(queue, &source, size, &decoded.pixels);
        let flags = flags_buffer(device, queue, space);
        let source_group = self.source_bind_group(
            device,
            &source.create_view(&wgpu::TextureViewDescriptor::default()),
            &flags,
        );
        self.prepare_pass(encoder, &source_group, &prepared_view);
        let bind_group = self.image_bind_group(device, &prepared_view);
        self.failed.remove(&asset);
        self.images.insert(
            asset,
            ImageEntry {
                path,
                space,
                size: DVec2::new(f64::from(decoded.width), f64::from(decoded.height)),
                bind_group,
                _texture: prepared,
            },
        );
        true
    }

    fn image_bind_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxy-image"),
            layout: &self.image_layout,
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
    }

    /// Premultiplies and encodes an sRGB8 source for the working space.
    fn prepare_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source_group: &wgpu::BindGroup,
        target: &wgpu::TextureView,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voxy-prepare"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.prepare_pipeline);
        pass.set_bind_group(0, source_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Makes sure `frame` is on the GPU, prepared for `space`, and returns
    /// its slot. Frames are uploaded once and identified by their `Arc`.
    /// A new frame goes into the least recently drawn slot not used by this
    /// render, reusing its textures when the size matches (the usual case:
    /// every frame of a video has the same size). Returns `None` for frames
    /// the GPU cannot hold.
    fn ensure_video(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Arc<VideoFrame>,
        space: WorkingSpace,
    ) -> Option<usize> {
        let now = self.frame_counter;
        if let Some(i) = self
            .video
            .iter()
            .position(|s| Arc::ptr_eq(&s.frame, frame) && s.space == space)
        {
            self.video[i].used = now;
            self.video_stats.reused += 1;
            return Some(i);
        }
        let size = (frame.width, frame.height);
        let max = device.limits().max_texture_dimension_2d;
        if size.0 == 0
            || size.1 == 0
            || size.0 > max
            || size.1 > max
            || frame.pixels.len() != size.0 as usize * size.1 as usize * 4
        {
            tracing::warn!("cannot upload a {}x{} video frame", size.0, size.1);
            return None;
        }
        // The least recently drawn slot not drawn in this render.
        let victim = (self.video.len() >= VIDEO_SLOTS)
            .then(|| {
                self.video
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.used < now)
                    .min_by_key(|(_, s)| s.used)
                    .map(|(i, _)| i)
            })
            .flatten();
        let index = match victim {
            Some(i) if self.video[i].size == size => i,
            _ => {
                let (source, prepared, prepared_view) = texture_pair(device, size, "voxy-video");
                let flags = flags_buffer(device, queue, space);
                let source_group = self.source_bind_group(
                    device,
                    &source.create_view(&wgpu::TextureViewDescriptor::default()),
                    &flags,
                );
                let bind_group = self.image_bind_group(device, &prepared_view);
                let slot = VideoSlot {
                    frame: frame.clone(),
                    space,
                    size,
                    used: now,
                    source,
                    source_group,
                    flags,
                    prepared_view,
                    bind_group,
                    _prepared: prepared,
                };
                self.video_stats.allocations += 1;
                if let Some(i) = victim {
                    self.video[i] = slot;
                    i
                } else {
                    self.video.push(slot);
                    self.video.len() - 1
                }
            }
        };
        let slot = &mut self.video[index];
        slot.frame = frame.clone();
        slot.space = space;
        slot.used = now;
        write_rgba(queue, &slot.source, size, &frame.pixels);
        queue.write_buffer(
            &slot.flags,
            0,
            bytemuck::bytes_of(&FlagsUniform {
                flags: [space_flag(space), 0, 0, 0],
            }),
        );
        let slot = &self.video[index];
        self.prepare_pass(encoder, &slot.source_group, &slot.prepared_view);
        self.video_stats.uploads += 1;
        Some(index)
    }
}

/// An sRGB8 upload texture and the RGBA16F texture it is prepared into.
fn texture_pair(
    device: &wgpu::Device,
    size: (u32, u32),
    label: &str,
) -> (wgpu::Texture, wgpu::Texture, wgpu::TextureView) {
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    };
    let source = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&format!("{label}-source")),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Raw sRGB-encoded bytes; the prepare shader decodes them
        // explicitly so both working spaces share one path.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let prepared = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&format!("{label}-prepared")),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: WORKING_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = prepared.create_view(&wgpu::TextureViewDescriptor::default());
    (source, prepared, view)
}

fn write_rgba(queue: &wgpu::Queue, texture: &wgpu::Texture, size: (u32, u32), pixels: &[u8]) {
    queue.write_texture(
        texture.as_image_copy(),
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.0 * 4),
            rows_per_image: Some(size.1),
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
}

/// A prepare-pass flags uniform for `space`.
fn flags_buffer(device: &wgpu::Device, queue: &wgpu::Queue, space: WorkingSpace) -> wgpu::Buffer {
    let flags = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("voxy-prepare-flags"),
        size: std::mem::size_of::<FlagsUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &flags,
        0,
        bytemuck::bytes_of(&FlagsUniform {
            flags: [space_flag(space), 0, 0, 0],
        }),
    );
    flags
}

fn create_readback(device: &wgpu::Device, size: (u32, u32)) -> Readback {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("voxy-readback"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let padded_row = (size.0 * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("voxy-readback"),
        size: u64::from(padded_row) * u64::from(size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    Readback {
        size,
        texture,
        view,
        buffer,
        padded_row,
    }
}

fn read_buffer(
    device: &wgpu::Device,
    rb: &Readback,
    submission: wgpu::SubmissionIndex,
) -> Result<RgbaImage, RenderError> {
    let slice = rb.buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .map_err(|e| RenderError::Readback(e.to_string()))?;
    rx.recv()
        .map_err(|e| RenderError::Readback(e.to_string()))?
        .map_err(|e| RenderError::Readback(e.to_string()))?;
    let (w, h) = rb.size;
    let row = (w * 4) as usize;
    let mut pixels = Vec::with_capacity(row * h as usize);
    {
        let data = slice
            .get_mapped_range()
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        for y in 0..h as usize {
            let start = y * rb.padded_row as usize;
            pixels.extend_from_slice(&data[start..start + row]);
        }
    }
    rb.buffer.unmap();
    Ok(RgbaImage {
        width: w,
        height: h,
        pixels,
    })
}
