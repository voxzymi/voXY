// Chromatic Aberration (voxy.stylize.chromatic_aberration v1). Formula in
// docs/effects.md.
//
// p0.xy: centre in texels; p0.z: k. Red is drawn scaled by 1 + k around
// the centre and blue by 1 − k; green stays. Premultiplied: each channel
// comes from its own sample, and alpha is the largest of the three.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = fx.p0.xy;
    let d = pos.xy - c;
    let red = sample_clear(c + d / (1.0 + fx.p0.z));
    let green = load(vec2<i32>(pos.xy));
    let blue = sample_clear(c + d / (1.0 - fx.p0.z));
    return vec4<f32>(red.r, green.g, blue.b, max(max(red.a, green.a), blue.a));
}
