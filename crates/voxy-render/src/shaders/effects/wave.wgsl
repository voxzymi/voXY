// Wave (voxy.distort.wave v1). Formula in docs/effects.md.
//
// p0.xy: the layer origin in texels; p0.z: wavelength and p0.w: amplitude,
// in texels; p1.xy: the travel direction d; p1.z: phase in radians. Each
// pixel moves sideways (perpendicular to d) by a sine of its distance
// along d from the layer origin.

const TAU: f32 = 6.283185307179586;

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let d = fx.p1.xy;
    let along = dot(pos.xy - fx.p0.xy, d);
    let shift = fx.p0.w * sin(TAU * along / fx.p0.z + fx.p1.z);
    return sample_clear(pos.xy - shift * vec2<f32>(-d.y, d.x));
}
