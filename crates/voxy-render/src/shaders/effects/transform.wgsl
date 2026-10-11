// Transform (voxy.distort.transform v1). Formula in docs/effects.md.
//
// p0: the output-to-input texel map's matrix (columns xy, zw); p1.xy: its
// translation; p1.z: opacity. Bilinear, transparent outside the buffer.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let m = mat2x2<f32>(fx.p0.xy, fx.p0.zw);
    return sample_clear(m * pos.xy + fx.p1.xy) * fx.p1.z;
}
