// Apply LUT (voxy.color.lut v1). Formula in docs/effects.md.
//
// p0.x: amount; p1.rgb, p2.rgb: the table's domain; i0.x: what the table
// expects (0 = sRGB-encoded values, 1 = linear light); i0.y: entries per
// side. Trilinear interpolation by hand (as voxy_core::lut::CubeLut::apply),
// so the 32-bit float table needs no filtering support.

@group(2) @binding(0) var table: texture_3d<f32>;

fn entry(p: vec3<i32>) -> vec3<f32> {
    return textureLoad(table, p, 0).rgb;
}

fn lookup(c: vec3<f32>) -> vec3<f32> {
    let last = f32(fx.i0.y - 1u);
    let t = clamp((c - fx.p1.rgb) / (fx.p2.rgb - fx.p1.rgb), vec3<f32>(0.0), vec3<f32>(1.0)) * last;
    let i = vec3<i32>(min(floor(t), vec3<f32>(last - 1.0)));
    let f = t - vec3<f32>(i);
    let x00 = mix(entry(i), entry(i + vec3<i32>(1, 0, 0)), f.x);
    let x10 = mix(entry(i + vec3<i32>(0, 1, 0)), entry(i + vec3<i32>(1, 1, 0)), f.x);
    let x01 = mix(entry(i + vec3<i32>(0, 0, 1)), entry(i + vec3<i32>(1, 0, 1)), f.x);
    let x11 = mix(entry(i + vec3<i32>(0, 1, 1)), entry(i + vec3<i32>(1, 1, 1)), f.x);
    return mix(mix(x00, x10, f.y), mix(x01, x11, f.y), f.z);
}

// Working-space values to what the table expects, and back.
fn encode(v: vec3<f32>) -> vec3<f32> {
    if fx.i0.x == 1u {
        return to_linear(v);
    }
    if fx.frame.w > 0.5 {
        return v;
    }
    return linear_to_srgb(max(v, vec3<f32>(0.0)));
}

fn decode(v: vec3<f32>) -> vec3<f32> {
    if fx.i0.x == 1u {
        return from_linear(v);
    }
    if fx.frame.w > 0.5 {
        return v;
    }
    return srgb_to_linear(max(v, vec3<f32>(0.0)));
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let e = encode(unpremultiply(c));
    let out = mix(e, lookup(e), fx.p0.x);
    return premultiply(decode(out), c.a);
}
