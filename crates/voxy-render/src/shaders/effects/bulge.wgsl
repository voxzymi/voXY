// Bulge (voxy.distort.bulge v1). Formula in docs/effects.md.
//
// p0.xy: centre in texels; p0.z: radius R in texels; p0.w: e = 2^amount.
// Inside the circle a pixel at distance r reads the input at distance
// R·(r/R)^e: e > 1 swells the centre, e < 1 pinches it; the edge stays.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let d = pos.xy - fx.p0.xy;
    let t = length(d) / fx.p0.z;
    if t <= 0.0 || t >= 1.0 {
        return load(vec2<i32>(pos.xy));
    }
    return sample_clear(fx.p0.xy + d * pow(t, fx.p0.w - 1.0));
}
