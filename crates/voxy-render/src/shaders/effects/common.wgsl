// Shared by every effect kernel (prepended after color.wgsl).
//
// An effect pass reads the previous image of a layer's chain and writes the
// next one, texel for texel. Both are premultiplied RGBA16F in the project's
// working space, `fx.frame.xy` texels large. Pixel parameters arrive already
// multiplied by `fx.frame.z` (texels per layer pixel), so the look does not
// change with the preview resolution.

struct EffectUniform {
    // x, y: buffer size in texels; z: texels per layer pixel; w: 1 when the
    // working space is SrgbCompat (values are sRGB-encoded), 0 when linear.
    frame: vec4<f32>,
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
    i0: vec4<u32>,
    // Convolution weights, centre first: weight k is w[k / 4][k % 4].
    w: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var<uniform> fx: EffectUniform;
@group(1) @binding(0) var src: texture_2d<f32>;
@group(1) @binding(1) var src_sampler: sampler;
// The kernel's own input, for kernels that combine their result with it
// (the same texture as `src` for the others).
@group(1) @binding(2) var orig: texture_2d<f32>;
// The layer the kernel reads (Displacement Map's map, a transition's B),
// drawn in this buffer's texels; transparent when there is none.
@group(1) @binding(3) var layer_in: texture_2d<f32>;

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

// The input texel at `p`, clamped to the buffer (edge pixels repeat).
fn load_clamped(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(src));
    return textureLoad(src, clamp(p, vec2<i32>(0), size - vec2<i32>(1)), 0);
}

fn weight(k: u32) -> f32 {
    return fx.w[k / 4u][k % 4u];
}

// Straight colour of a premultiplied texel (zero where alpha is zero).
fn unpremultiply(c: vec4<f32>) -> vec3<f32> {
    if c.a <= 0.0 {
        return vec3<f32>(0.0);
    }
    return c.rgb / c.a;
}

// The largest value an effect writes: below the RGBA16F maximum (65504),
// so extreme settings never overflow to infinity (and then NaN).
const MAX_VALUE: f32 = 60000.0;

fn premultiply(rgb: vec3<f32>, a: f32) -> vec4<f32> {
    return vec4<f32>(clamp(rgb, vec3<f32>(-MAX_VALUE), vec3<f32>(MAX_VALUE)) * a, a);
}

// Working-space values to linear light, and back (for effects that compute
// in linear light, docs/effects.md).
fn to_linear(c: vec3<f32>) -> vec3<f32> {
    if fx.frame.w > 0.5 {
        return srgb_to_linear(max(c, vec3<f32>(0.0)));
    }
    return c;
}

fn from_linear(c: vec3<f32>) -> vec3<f32> {
    if fx.frame.w > 0.5 {
        return linear_to_srgb(max(c, vec3<f32>(0.0)));
    }
    return c;
}

// Rec. 709 luma of working-space values.
fn luma709(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Bilinear sample at `at` (texel units, texel centres at +0.5);
// transparent outside the buffer, so a sweep never repeats edge pixels.
fn sample_clear(at: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(src));
    if at.x < 0.0 || at.y < 0.0 || at.x > size.x || at.y > size.y {
        return vec4<f32>(0.0);
    }
    return textureSampleLevel(src, src_sampler, at / size, 0.0);
}

// How many samples a sweep of `len` texels takes: one per texel plus one,
// odd so one sits on the pixel itself, at most `fx.i0.y`.
fn sweep_count(len: f32) -> u32 {
    let n = min(u32(ceil(max(len, 0.0))) + 1u, fx.i0.y);
    return n | 1u;
}

// The kernel's own input at integer coordinates; transparent outside.
fn load_orig(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(orig));
    if p.x < 0 || p.y < 0 || p.x >= size.x || p.y >= size.y {
        return vec4<f32>(0.0);
    }
    return textureLoad(orig, p, 0);
}

// The layer input at integer coordinates; transparent outside.
fn load_input(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(layer_in));
    if p.x < 0 || p.y < 0 || p.x >= size.x || p.y >= size.y {
        return vec4<f32>(0.0);
    }
    return textureLoad(layer_in, p, 0);
}

// Bilinear sample of the layer input at `at` (texel units); transparent
// outside it.
fn sample_input(at: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(layer_in));
    if at.x < 0.0 || at.y < 0.0 || at.x > size.x || at.y > size.y {
        return vec4<f32>(0.0);
    }
    return textureSampleLevel(layer_in, src_sampler, at / size, 0.0);
}
