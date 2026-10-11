// Glow, stage 2. Formula in docs/effects.md.
//
// src: the blurred bright light g (premultiplied linear, alpha unused);
// orig: the effect's input. p0.x: intensity. Adds the light in linear
// light; where it falls outside the layer it brings its own coverage.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let g = max(load(p).rgb, vec3<f32>(0.0)) * fx.p0.x;
    let o = load_orig(p);
    let o_lin = to_linear(unpremultiply(o)) * o.a;
    let cover = clamp(max(max(g.r, g.g), g.b), 0.0, 1.0);
    let a = o.a + cover * (1.0 - o.a);
    if a <= 0.0 {
        return vec4<f32>(0.0);
    }
    let lin = (o_lin + g) / a;
    return premultiply(from_linear(lin), a);
}
