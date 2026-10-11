// Stroke, stage 0. Formula in docs/effects.md.
//
// Marks the pixels at least half opaque: (offset to the nearest marked
// pixel = 0, found = 1). Others are not found yet (z = 0).

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if load(vec2<i32>(pos.xy)).a >= 0.5 {
        return vec4<f32>(0.0, 0.0, 1.0, 0.0);
    }
    return vec4<f32>(0.0);
}
