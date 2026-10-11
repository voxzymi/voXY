// Zoom Blur (voxy.blur.zoom v1). Formula in docs/effects.md.
//
// p0.xy: centre, in texels; p0.z: amount a in [0, 0.9]; i0.y: most samples
// per pixel. Each pixel at p averages the input at c + (p − c)·k for k
// evenly from 1 down to 1 − a, with equal weights.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = fx.p0.xy;
    let a = fx.p0.z;
    let d = pos.xy - c;
    let n = sweep_count(a * length(d));
    if n <= 1u {
        return load(vec2<i32>(pos.xy));
    }
    var sum = vec4<f32>(0.0);
    for (var k = 0u; k < n; k = k + 1u) {
        let t = 1.0 - a * f32(k) / f32(n - 1u);
        sum = sum + sample_clear(c + d * t);
    }
    return sum / f32(n);
}
