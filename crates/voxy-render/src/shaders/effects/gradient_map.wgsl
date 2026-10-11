// Gradient Map (voxy.color.gradient_map v1). Formula in docs/effects.md.
//
// p0, p1, p2 (rgb): shadow, midtone and highlight colours (working space);
// p3.x: the luma that maps to the midtone; p3.y: amount.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let rgb = unpremultiply(c);
    let l = clamp(luma709(rgb), 0.0, 1.0);
    let m = fx.p3.x;
    var mapped = mix(fx.p1.rgb, fx.p2.rgb, (l - m) / (1.0 - m));
    if l <= m {
        mapped = mix(fx.p0.rgb, fx.p1.rgb, l / m);
    }
    return premultiply(mix(rgb, mapped, fx.p3.y), c.a);
}
