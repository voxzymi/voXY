// Composites one layer quad with a blend mode other than Normal.
//
// Same vertex stage and uniforms as layer.wgsl. The backdrop is a copy of
// the target taken just before this draw; the result replaces the target
// inside the quad (no fixed-function blending). The formulas mirror
// crates/voxy-render/src/blend.rs, which the tests compare against.

struct LayerUniform {
    col0: vec4<f32>,
    col1: vec4<f32>,
    col2: vec4<f32>,
    size: vec4<f32>,
    color: vec4<f32>,
    // x: 0 = solid, 1 = image; y: blend mode (BlendMode::ALL order).
    mode: vec4<u32>,
};

@group(0) @binding(0) var<uniform> layer: LayerUniform;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;
@group(2) @binding(0) var backdrop: texture_2d<f32>;

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

const LUMA = vec3<f32>(0.30, 0.59, 0.11);

fn lum(c: vec3<f32>) -> f32 {
    return dot(c, LUMA);
}

fn clip_color(c: vec3<f32>) -> vec3<f32> {
    let l = lum(c);
    let n = min(min(c.r, c.g), c.b);
    let x = max(max(c.r, c.g), c.b);
    var o = c;
    if n < 0.0 {
        o = vec3<f32>(l) + (o - vec3<f32>(l)) * l / (l - n);
    }
    if x > 1.0 {
        o = vec3<f32>(l) + (o - vec3<f32>(l)) * (1.0 - l) / (x - l);
    }
    return o;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    return clip_color(c + vec3<f32>(l - lum(c)));
}

fn sat(c: vec3<f32>) -> f32 {
    return max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b);
}

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let n = min(min(c.r, c.g), c.b);
    let x = max(max(c.r, c.g), c.b);
    if x > n {
        return (c - vec3<f32>(n)) * s / (x - n);
    }
    return vec3<f32>(0.0);
}

fn screen1(b: f32, s: f32) -> f32 {
    return b + s - b * s;
}

fn hard_light1(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        return b * (2.0 * s);
    }
    return screen1(b, 2.0 * s - 1.0);
}

fn soft_light1(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        return b - (1.0 - 2.0 * s) * b * (1.0 - b);
    }
    var d = sqrt(b);
    if b <= 0.25 {
        d = ((16.0 * b - 12.0) * b + 4.0) * b;
    }
    return b + (2.0 * s - 1.0) * (d - b);
}

fn dodge1(b: f32, s: f32) -> f32 {
    if b <= 0.0 {
        return 0.0;
    }
    if s >= 1.0 {
        return 1.0;
    }
    return min(1.0, b / (1.0 - s));
}

fn burn1(b: f32, s: f32) -> f32 {
    if b >= 1.0 {
        return 1.0;
    }
    if s <= 0.0 {
        return 0.0;
    }
    return 1.0 - min(1.0, (1.0 - b) / s);
}

fn blend_rgb(mode: u32, b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 1u: { return b * s; }
        case 2u: { return b + s - b * s; }
        case 3u: { return vec3<f32>(hard_light1(s.r, b.r), hard_light1(s.g, b.g), hard_light1(s.b, b.b)); }
        case 4u: { return vec3<f32>(soft_light1(b.r, s.r), soft_light1(b.g, s.g), soft_light1(b.b, s.b)); }
        case 5u: { return vec3<f32>(hard_light1(b.r, s.r), hard_light1(b.g, s.g), hard_light1(b.b, s.b)); }
        case 6u: { return b + s; }
        case 7u: { return max(b - s, vec3<f32>(0.0)); }
        case 8u: { return abs(b - s); }
        case 9u: { return vec3<f32>(dodge1(b.r, s.r), dodge1(b.g, s.g), dodge1(b.b, s.b)); }
        case 10u: { return vec3<f32>(burn1(b.r, s.r), burn1(b.g, s.g), burn1(b.b, s.b)); }
        case 11u: { return max(b, s); }
        case 12u: { return min(b, s); }
        case 13u: { return set_lum(set_sat(s, sat(b)), lum(b)); }
        case 14u: { return set_lum(set_sat(b, sat(s)), lum(b)); }
        case 15u: { return set_lum(s, lum(b)); }
        case 16u: { return set_lum(b, lum(s)); }
        default: { return s; }
    }
}

fn clamps_inputs(mode: u32) -> bool {
    // screen, overlay, soft light, hard light, dodge, burn, hue..luminosity
    return mode == 2u || mode == 3u || mode == 4u || mode == 5u || mode == 9u
        || mode == 10u || mode >= 13u;
}

fn straight(c: vec4<f32>) -> vec3<f32> {
    if c.a <= 0.0 {
        return vec3<f32>(0.0);
    }
    return c.rgb / c.a;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(image, image_sampler, in.uv);
    var src = layer.color;
    if layer.mode.x != 0u {
        src = texel * layer.color.x;
    }
    let dst = textureLoad(backdrop, vec2<i32>(in.position.xy), 0);
    let mode = layer.mode.y;
    var cb = straight(dst);
    var cs = straight(src);
    if clamps_inputs(mode) {
        cb = clamp(cb, vec3<f32>(0.0), vec3<f32>(1.0));
        cs = clamp(cs, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let b = blend_rgb(mode, cb, cs);
    let rgb = src.rgb * (1.0 - dst.a) + dst.rgb * (1.0 - src.a) + src.a * dst.a * b;
    return vec4<f32>(rgb, src.a + dst.a * (1.0 - src.a));
}
