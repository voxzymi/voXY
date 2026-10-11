// Ripple (voxy.distort.ripple v1). Formula in docs/effects.md.
//
// p0.xy: centre in texels; p0.z: wavelength and p0.w: amplitude, in
// texels; p1.x: phase in radians. Each pixel moves along the line from the
// centre by a sine of its distance to it.

const TAU: f32 = 6.283185307179586;

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let d = pos.xy - fx.p0.xy;
    let r = length(d);
    if r <= 0.0 {
        return load(vec2<i32>(pos.xy));
    }
    let shift = fx.p0.w * sin(TAU * r / fx.p0.z - fx.p1.x);
    return sample_clear(pos.xy - shift * d / r);
}
