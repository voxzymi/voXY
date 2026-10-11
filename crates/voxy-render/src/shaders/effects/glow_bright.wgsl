// Glow, stage 0. Formula in docs/effects.md.
//
// p0.x: threshold t (linear light). Writes the light above the threshold,
// premultiplied linear: lin·max(L − t, 0)/L·a, with L its Rec. 709 luma,
// so the glow keeps each colour's hue. Alpha is 0: stage 2 derives it.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let lin = to_linear(unpremultiply(c));
    let l = luma709(lin);
    if l <= fx.p0.x || l <= 0.0 {
        return vec4<f32>(0.0);
    }
    let over = min(lin * ((l - fx.p0.x) / l) * c.a, vec3<f32>(MAX_VALUE));
    return vec4<f32>(over, 0.0);
}
