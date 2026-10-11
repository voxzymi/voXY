// Crossfade (voxy.transition.crossfade v1). Formula in docs/effects.md.
//
// p0.x: 1 − progress. Until a B layer can be chosen (progress.md, Q-E1),
// B is transparent, so the layer fades out.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return load(vec2<i32>(pos.xy)) * fx.p0.x;
}
