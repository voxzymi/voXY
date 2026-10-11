// Color Balance (voxy.color.color_balance v1). Formula in docs/effects.md.
//
// p0, p1, p2 (rgb): the shadow, midtone and highlight shifts; i0.x: 1 =
// preserve luminosity. The ranges are weighted by Rec. 709 luma L of the
// straight colour (clamped to [0, 1]): (1 − L)², 2L(1 − L) and L², which
// sum to 1.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let rgb = unpremultiply(c);
    let l = clamp(luma709(rgb), 0.0, 1.0);
    let shift = fx.p0.rgb * ((1.0 - l) * (1.0 - l))
        + fx.p1.rgb * (2.0 * l * (1.0 - l))
        + fx.p2.rgb * (l * l);
    var out = rgb + shift;
    if fx.i0.x == 1u {
        out = out + vec3<f32>(luma709(rgb) - luma709(out));
    }
    return premultiply(max(out, vec3<f32>(0.0)), c.a);
}
