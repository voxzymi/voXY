// Wipe (voxy.transition.wipe v1). Formula in docs/effects.md.
//
// p0.xy: the edge's direction d; p0.z: the edge's position along d, in
// texels; p0.w: feather in texels. Pixels behind the edge show B, the
// "Transition To" layer (transparent if none).

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let along = dot(pos.xy, fx.p0.xy);
    let f = fx.p0.w;
    let keep = smoothstep(fx.p0.z - 0.5 * f, fx.p0.z + 0.5 * f, along);
    return mix(load_input(p), load(p), keep);
}
