// Crossfade (voxy.transition.crossfade v1). Formula in docs/effects.md.
//
// p0.x: 1 − progress. B is the "Transition To" layer (transparent if none).

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    return mix(load_input(p), load(p), fx.p0.x);
}
