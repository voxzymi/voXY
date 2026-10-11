//! LUT assets for the LUT effect: `.cube` files parsed by
//! `voxy_core::lut` and uploaded as 3D `Rgba32Float` textures. The shader
//! reads them with `textureLoad` and interpolates itself (trilinear, as
//! `CubeLut::apply`), so no filtering support is needed.

use std::path::PathBuf;

use voxy_core::lut::CubeLut;
use voxy_core::{AssetId, AssetTable};

use super::Compositor;

/// The largest `.cube` file read.
const MAX_LUT_BYTES: u64 = 96 * 1024 * 1024;

pub(super) struct LutEntry {
    path: PathBuf,
    pub(super) size: u32,
    pub(super) domain_min: [f32; 3],
    pub(super) domain_max: [f32; 3],
    pub(super) bind_group: wgpu::BindGroup,
    _texture: wgpu::Texture,
}

pub(super) fn lut_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("voxy-effect-lut"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        }],
    })
}

fn read(path: &std::path::Path) -> Result<CubeLut, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_LUT_BYTES {
        return Err("file too large".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    voxy_core::lut::parse_cube(&text).map_err(|e| e.to_string())
}

impl Compositor {
    /// Makes sure LUT `asset` is loaded. Returns `false` if it cannot be.
    pub(super) fn ensure_lut(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &AssetTable,
        asset: AssetId,
    ) -> bool {
        let Some(info) = assets.get(asset) else {
            return false;
        };
        let path = info.resolve(self.asset_root.as_deref());
        if self.luts.get(&asset).is_some_and(|e| e.path == path) {
            return true;
        }
        if self.failed.get(&asset) == Some(&path) {
            return false;
        }
        let lut = match read(&path) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("LUT asset {asset} unavailable: {e}");
                self.failed.insert(asset, path);
                return false;
            }
        };
        let n = lut.size as u32;
        if n > device.limits().max_texture_dimension_3d {
            tracing::warn!("LUT asset {asset} is larger than this GPU's 3D texture limit");
            self.failed.insert(asset, path);
            return false;
        }
        let extent = wgpu::Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: n,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxy-lut"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texels: Vec<f32> = lut
            .data
            .iter()
            .flat_map(|[r, g, b]| [*r, *g, *b, 1.0])
            .collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 16),
                rows_per_image: Some(n),
            },
            extent,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxy-lut"),
            layout: &self.lut_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        self.failed.remove(&asset);
        self.luts.insert(
            asset,
            LutEntry {
                path,
                size: n,
                domain_min: lut.domain_min,
                domain_max: lut.domain_max,
                bind_group,
                _texture: texture,
            },
        );
        true
    }
}
