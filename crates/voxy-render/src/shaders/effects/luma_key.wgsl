// Luma Key (voxy.keying.luma_key v1). Formula in docs/effects.md.
//
// p0.x: threshold t; p0.y: softness s; i0.x: 0 = key out darker, 1 = key
// out brighter. By Rec. 709 luma of the working-space colour.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let l = luma709(unpremultiply(c));
    let t = fx.p0.x;
    let s = max(fx.p0.y, 1e-4);
    var keep = smoothstep(t, t + s, l);
    if fx.i0.x == 1u {
        keep = 1.0 - smoothstep(t - s, t, l);
    }
    return c * keep;
}
