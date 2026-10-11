// Zoom (voxy.transition.zoom v1). Formula in docs/effects.md.
//
// p0.xy: the frame's centre in texels; p0.z: the scale; p0.w: 1 − progress;
// p1: the layer's frame in texels, outside of which nothing shows.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if pos.x < fx.p1.x || pos.y < fx.p1.y || pos.x > fx.p1.z || pos.y > fx.p1.w {
        return vec4<f32>(0.0);
    }
    return sample_clear(fx.p0.xy + (pos.xy - fx.p0.xy) / fx.p0.z) * fx.p0.w;
}
