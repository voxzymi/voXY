// Composites one layer quad onto the working buffer.
//
// The working buffer is premultiplied RGBA16F. In the Linear working space it
// holds linear light; in SrgbCompat it holds sRGB-encoded values. Colors and
// prepared image textures arrive already encoded for the working space, so
// this shader never converts color.

struct LayerUniform {
    // Columns of the affine map from layer-local pixels to clip space.
    col0: vec4<f32>,
    col1: vec4<f32>,
    col2: vec4<f32>,
    // xy: quad size in layer-local pixels.
    size: vec4<f32>,
    // Solid: premultiplied color with opacity applied. Image: x = opacity.
    color: vec4<f32>,
    // x: 0 = solid, 1 = image.
    mode: vec4<u32>,
};

@group(0) @binding(0) var<uniform> layer: LayerUniform;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let uv = corners[vi];
    let local = uv * layer.size.xy;
    let clip = layer.col0.xy * local.x + layer.col1.xy * local.y + layer.col2.xy;
    var out: VsOut;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Sample unconditionally so the call stays in uniform control flow.
    let texel = textureSample(image, image_sampler, in.uv);
    if layer.mode.x == 0u {
        return layer.color;
    }
    return texel * layer.color.x;
}
