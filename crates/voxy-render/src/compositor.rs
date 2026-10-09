//! The wgpu compositor: executes a [`RenderPlan`].

use std::collections::HashMap;
use std::path::PathBuf;

use voxy_core::glam::{DAffine2, DMat2, DVec2};
use voxy_core::{AssetId, AssetTable, WorkingSpace};

use crate::plan::{DrawSource, RenderPlan};

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

struct WorkingTarget {
    size: (u32, u32),
    view: wgpu::TextureView,
    output_bind_group: wgpu::BindGroup,
    _texture: wgpu::Texture,
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
    uniform_stride: u64,
    layer_uniforms: Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    output_flags: wgpu::Buffer,
    working: Option<WorkingTarget>,
    readback: Option<Readback>,
    images: HashMap<AssetId, ImageEntry>,
    failed: HashMap<AssetId, PathBuf>,
    asset_root: Option<PathBuf>,
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
            uniform_stride,
            layer_uniforms: None,
            output_flags,
            working: None,
            readback: None,
            images: HashMap::new(),
            failed: HashMap::new(),
            asset_root: None,
        }
    }

    /// Sets the directory relative asset paths resolve against (the folder
    /// containing the `.voxy` file) and drops cached images.
    pub fn set_asset_root(&mut self, root: Option<PathBuf>) {
        if self.asset_root != root {
            self.asset_root = root;
            self.images.clear();
            self.failed.clear();
        }
    }

    /// Renders `plan` into `target`, which must be an [`OUTPUT_FORMAT`]
    /// texture of the plan's size with `RENDER_ATTACHMENT` usage.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &AssetTable,
        plan: &RenderPlan,
        target: &wgpu::TextureView,
        alpha: OutputAlpha,
    ) -> RenderReport {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("voxy-frame"),
        });
        let report = self.encode(device, queue, &mut encoder, assets, plan, target, alpha);
        queue.submit([encoder.finish()]);
        report
    }

    /// Renders `plan` and reads the result back to the CPU.
    pub fn render_to_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &AssetTable,
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
        let report = self.encode(device, queue, &mut encoder, assets, plan, &rb.view, alpha);
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
        plan: &RenderPlan,
        target: &wgpu::TextureView,
        alpha: OutputAlpha,
    ) -> RenderReport {
        let mut report = RenderReport::default();
        self.ensure_working(device, (plan.width, plan.height));

        // Load and prepare any images this frame needs.
        for draw in &plan.draws {
            if let DrawSource::Image { asset, .. } = draw.source
                && !self.ensure_image(device, queue, encoder, assets, asset, plan.working_space)
                && !report.missing_assets.contains(&asset)
            {
                report.missing_assets.push(asset);
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
        for draw in &plan.draws {
            let (color, mode, size, bind) = match draw.source {
                DrawSource::Solid(c) => (c, 0, draw.local_size, None),
                DrawSource::Image { asset, opacity } => {
                    let Some(entry) = self.images.get(&asset) else {
                        continue;
                    };
                    (
                        [opacity, 0.0, 0.0, 0.0],
                        1,
                        draw.local_size.or(Some(entry.size)),
                        Some(asset),
                    )
                }
            };
            let Some(size) = size else { continue };
            let m = clip_from_output * draw.output_from_local;
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
            let offset = uniform_bytes.len() as u64;
            uniform_bytes.extend_from_slice(bytemuck::bytes_of(&u));
            uniform_bytes.resize((offset + self.uniform_stride) as usize, 0);
            draws.push((offset as u32, bind));
        }
        if !uniform_bytes.is_empty() {
            self.ensure_layer_uniforms(device, uniform_bytes.len() as u64);
            let (buffer, _, _) = self.layer_uniforms.as_ref().expect("ensured");
            queue.write_buffer(buffer, 0, &uniform_bytes);
        }
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

        let working = self.working.as_ref().expect("ensured");
        {
            let [r, g, b, a] = plan.clear;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voxy-composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &working.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if let Some((_, uniforms, _)) = &self.layer_uniforms {
                pass.set_pipeline(&self.layer_pipeline);
                for (offset, image) in &draws {
                    let image_group = image
                        .and_then(|a| self.images.get(&a))
                        .map_or(&self.blank_image, |e| &e.bind_group);
                    pass.set_bind_group(0, uniforms, &[*offset]);
                    pass.set_bind_group(1, image_group, &[]);
                    pass.draw(0..6, 0..1);
                }
            }
        }
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
        report
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let output_bind_group = self.source_bind_group(device, &view, &self.output_flags);
        self.working = Some(WorkingTarget {
            size,
            view,
            output_bind_group,
            _texture: texture,
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
        let extent = wgpu::Extent3d {
            width: decoded.width,
            height: decoded.height,
            depth_or_array_layers: 1,
        };
        let source = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxy-image-source"),
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
        queue.write_texture(
            source.as_image_copy(),
            &decoded.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(decoded.width * 4),
                rows_per_image: Some(decoded.height),
            },
            extent,
        );
        let prepared = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxy-image-prepared"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let prepared_view = prepared.create_view(&wgpu::TextureViewDescriptor::default());
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
        let source_group = self.source_bind_group(
            device,
            &source.create_view(&wgpu::TextureViewDescriptor::default()),
            &flags,
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voxy-prepare-image"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &prepared_view,
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
            pass.set_bind_group(0, &source_group, &[]);
            pass.draw(0..3, 0..1);
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxy-image"),
            layout: &self.image_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&prepared_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
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
