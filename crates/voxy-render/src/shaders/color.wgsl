// Shared color helpers (prepended to shaders that need them).

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / vec3<f32>(12.92);
    let hi = pow((c + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * vec3<f32>(12.92);
    let hi = vec3<f32>(1.055) * pow(c, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// Full-screen triangle; covers the whole target with three vertices.
fn fullscreen_position(vi: u32) -> vec4<f32> {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}
