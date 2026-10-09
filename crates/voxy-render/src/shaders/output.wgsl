// Output transform: working buffer → 8-bit sRGB-encoded output, either
// premultiplied (for on-screen display) or straight alpha (for export).

struct OutputUniform {
    // x: 1 = working space is Linear (encode to sRGB), 0 = already encoded.
    // y: 1 = premultiplied output, 0 = straight alpha.
    flags: vec4<u32>,
};

@group(0) @binding(0) var working: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: OutputUniform;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_position(vi);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(working, vec2<i32>(pos.xy), 0);
    let a = clamp(c.a, 0.0, 1.0);
    var rgb = vec3<f32>(0.0);
    if c.a > 0.0 {
        rgb = clamp(c.rgb / c.a, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if params.flags.x == 1u {
        rgb = linear_to_srgb(rgb);
    }
    if params.flags.y == 1u {
        rgb = rgb * a;
    }
    return vec4<f32>(rgb, a);
}
