// Converts a decoded sRGB8 straight-alpha image into a premultiplied RGBA16F
// texture encoded for the working space, once per image. Doing this before
// sampling keeps bilinear filtering free of dark/bright fringes.

struct PrepareUniform {
    // x: 1 = Linear working space (decode sRGB), 0 = SrgbCompat (keep encoded).
    flags: vec4<u32>,
};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: PrepareUniform;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_position(vi);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let s = textureLoad(source, vec2<i32>(pos.xy), 0);
    var rgb = s.rgb;
    if params.flags.x == 1u {
        rgb = srgb_to_linear(rgb);
    }
    return vec4<f32>(rgb * s.a, s.a);
}
