// Levels (voxy.color.levels v1). Formula in docs/effects.md.
//
// p0.x: input black; p0.y: 1 / (input white − input black); p0.z: 1 / gamma;
// p1.x, p1.y: output black and white; i0.x: channel (0 RGB, 1 red,
// 2 green, 3 blue, 4 alpha). On straight colour; no clipping above white.

fn level(v: f32) -> f32 {
    let x = max((v - fx.p0.x) * fx.p0.y, 0.0);
    var bent = 0.0;
    if x > 0.0 {
        bent = pow(min(x, MAX_VALUE), fx.p0.z);
    }
    return fx.p1.x + (fx.p1.y - fx.p1.x) * bent;
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    var rgb = unpremultiply(c);
    var a = c.a;
    switch fx.i0.x {
        case 0u: { rgb = vec3<f32>(level(rgb.r), level(rgb.g), level(rgb.b)); }
        case 1u: { rgb.r = level(rgb.r); }
        case 2u: { rgb.g = level(rgb.g); }
        case 3u: { rgb.b = level(rgb.b); }
        default: { a = clamp(level(a), 0.0, 1.0); }
    }
    return premultiply(rgb, a);
}
