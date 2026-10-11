// Mirror (voxy.distort.mirror v1). Formula in docs/effects.md.
//
// p0.xy: a point on the reflection line, in texels; p0.zw: the line's
// normal. Pixels on the normal's side show the reflection of the other.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let n = fx.p0.zw;
    let side = dot(pos.xy - fx.p0.xy, n);
    if side <= 0.0 {
        return load(vec2<i32>(pos.xy));
    }
    return sample_clear(pos.xy - 2.0 * side * n);
}
