// Displacement Map (voxy.distort.displacement_map v1). Formula in
// docs/effects.md.
//
// p0.x, p0.y: the largest horizontal and vertical shifts, in texels;
// i0.x, i0.y: the map channel for each (0 red, 1 green, 2 blue, 3 alpha,
// 4 luminance). The map is the map layer's own pixels in this buffer; where
// it is transparent the value is the neutral 0.5 (no shift).

fn channel(m: vec4<f32>, c: u32) -> f32 {
    let rgb = unpremultiply(m);
    var v = 0.0;
    switch c {
        case 0u: { v = rgb.r; }
        case 1u: { v = rgb.g; }
        case 2u: { v = rgb.b; }
        case 3u: { return m.a; }
        default: { v = luma709(rgb); }
    }
    return mix(0.5, clamp(v, 0.0, 1.0), m.a);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let m = load_input(vec2<i32>(pos.xy));
    let shift = vec2<f32>(
        (channel(m, fx.i0.x) - 0.5) * 2.0 * fx.p0.x,
        (channel(m, fx.i0.y) - 0.5) * 2.0 * fx.p0.y,
    );
    return sample_clear(pos.xy + shift);
}
