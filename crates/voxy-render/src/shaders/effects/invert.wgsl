// Invert (voxy.color.invert v1). Formula in docs/effects.md.
//
// p0.x: blend with original (0..1); i0.x: channel (0 RGB, 1 red, 2 green,
// 3 blue, 4 alpha).

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    var rgb = clamp(unpremultiply(c), vec3<f32>(0.0), vec3<f32>(1.0));
    var a = c.a;
    switch fx.i0.x {
        case 0u: { rgb = vec3<f32>(1.0) - rgb; }
        case 1u: { rgb.r = 1.0 - rgb.r; }
        case 2u: { rgb.g = 1.0 - rgb.g; }
        case 3u: { rgb.b = 1.0 - rgb.b; }
        default: { a = 1.0 - a; }
    }
    let inverted = vec4<f32>(rgb * a, a);
    return mix(inverted, c, clamp(fx.p0.x, 0.0, 1.0));
}
