// Shared by every effect kernel (prepended after color.wgsl).
//
// An effect pass reads the previous image of a layer's chain and writes the
// next one, texel for texel. Both are premultiplied RGBA16F in the project's
// working space, `fx.frame.xy` texels large. Pixel parameters arrive already
// multiplied by `fx.frame.z` (texels per layer pixel), so the look does not
// change with the preview resolution.

struct EffectUniform {
    // x, y: buffer size in texels; z: texels per layer pixel; w: unused.
    frame: vec4<f32>,
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
    i0: vec4<u32>,
};

@group(0) @binding(0) var<uniform> fx: EffectUniform;
@group(1) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_position(vi);
}

// The input texel at integer coordinates; transparent outside the buffer.
fn load(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(src));
    if p.x < 0 || p.y < 0 || p.x >= size.x || p.y >= size.y {
        return vec4<f32>(0.0);
    }
    return textureLoad(src, p, 0);
}

// Straight colour of a premultiplied texel (zero where alpha is zero).
fn unpremultiply(c: vec4<f32>) -> vec3<f32> {
    if c.a <= 0.0 {
        return vec3<f32>(0.0);
    }
    return c.rgb / c.a;
}
