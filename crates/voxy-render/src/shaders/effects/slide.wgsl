// Slide (voxy.transition.slide v1). Formula in docs/effects.md.
//
// p0.xy: how far the layer has moved, in texels; p1: the layer's frame in
// texels (min xy, max zw), outside of which nothing shows.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if pos.x < fx.p1.x || pos.y < fx.p1.y || pos.x > fx.p1.z || pos.y > fx.p1.w {
        return vec4<f32>(0.0);
    }
    return sample_clear(pos.xy - fx.p0.xy);
}
