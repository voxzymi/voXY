// Posterize (voxy.color.posterize v1). Formula in docs/effects.md.
//
// i0.x: levels n (2..256). Each straight channel, clamped to [0, 1], is
// rounded to the nearest of n evenly spaced steps; alpha is kept.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let steps = f32(fx.i0.x - 1u);
    let v = clamp(unpremultiply(c), vec3<f32>(0.0), vec3<f32>(1.0));
    return premultiply(floor(v * steps + vec3<f32>(0.5)) / steps, c.a);
}
