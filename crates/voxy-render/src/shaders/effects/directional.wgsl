// Directional Blur (voxy.blur.directional v1). Formula in docs/effects.md.
//
// p0.xy: step between samples, in texels; i0.y: number of samples (odd),
// centred on the pixel, averaged with equal weight. Bilinear sampling; the
// buffer's transparent margin covers half the length.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(src));
    let n = fx.i0.y;
    let half = f32(n - 1u) * 0.5;
    var sum = vec4<f32>(0.0);
    for (var k = 0u; k < n; k = k + 1u) {
        let at = pos.xy + fx.p0.xy * (f32(k) - half);
        sum = sum + textureSampleLevel(src, src_sampler, at / size, 0.0);
    }
    return sum / f32(n);
}
