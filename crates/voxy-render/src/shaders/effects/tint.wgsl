// Tint (voxy.color.tint v1). Formula in docs/effects.md.
// p0.rgb: black maps to; p1.rgb: white maps to (working space); p2.x: amount.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let rgb = unpremultiply(c);
    let l = clamp(luma709(rgb), 0.0, 1.0);
    let tinted = mix(fx.p0.rgb, fx.p1.rgb, l);
    return premultiply(mix(rgb, tinted, fx.p2.x), c.a);
}
