// Radial Blur (voxy.blur.radial v1). Formula in docs/effects.md.
//
// p0.xy: centre, in texels; p0.z: half the spin, in radians; i0.y: most
// samples per pixel. Each pixel averages the input along its arc around the
// centre, from −half to +half, with equal weights.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = fx.p0.xy;
    let half = fx.p0.z;
    let d = pos.xy - c;
    let n = sweep_count(2.0 * half * length(d));
    if n <= 1u {
        return load(vec2<i32>(pos.xy));
    }
    var sum = vec4<f32>(0.0);
    for (var k = 0u; k < n; k = k + 1u) {
        let a = half * (2.0 * f32(k) / f32(n - 1u) - 1.0);
        let cs = cos(a);
        let sn = sin(a);
        sum = sum + sample_clear(c + vec2<f32>(d.x * cs - d.y * sn, d.x * sn + d.y * cs));
    }
    return sum / f32(n);
}
