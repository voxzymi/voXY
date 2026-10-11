// Curves (voxy.color.curves v1). Formula in docs/effects.md.
//
// The weight array holds four tables of 256 samples: red, green and blue
// (each with the RGB curve applied after it), then alpha. Each straight
// channel, clamped to [0, 1], reads its table with linear interpolation.

fn table(t: u32, v: f32) -> f32 {
    let f = clamp(v, 0.0, 1.0) * 255.0;
    let i = min(u32(floor(f)), 254u);
    return mix(weight(t * 256u + i), weight(t * 256u + i + 1u), f - f32(i));
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let rgb = unpremultiply(c);
    let out = vec3<f32>(table(0u, rgb.r), table(1u, rgb.g), table(2u, rgb.b));
    return premultiply(out, clamp(table(3u, c.a), 0.0, 1.0));
}
