// Brightness & Contrast (voxy.color.brightness_contrast v1). Formula in
// docs/effects.md. p0.x: brightness; p0.y: contrast factor k (from the CPU).

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let rgb = (unpremultiply(c) - vec3<f32>(0.5)) * fx.p0.y + vec3<f32>(0.5 + fx.p0.x);
    return premultiply(max(rgb, vec3<f32>(0.0)), c.a);
}
