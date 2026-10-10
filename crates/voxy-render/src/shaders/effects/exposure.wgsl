// Exposure (voxy.color.exposure v1), in linear light. Formula in
// docs/effects.md. p0.x: 2^exposure; p0.y: offset; p0.z: 1 / gamma.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let lin = to_linear(unpremultiply(c));
    let lifted = max(lin * fx.p0.x + vec3<f32>(fx.p0.y), vec3<f32>(0.0));
    let out = pow(min(lifted, vec3<f32>(MAX_VALUE)), vec3<f32>(fx.p0.z));
    return premultiply(from_linear(out), c.a);
}
