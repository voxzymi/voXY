// Drop Shadow and Inner Shadow, stage 0. Formula in docs/effects.md.
//
// p0.xy: the shadow's offset in texels. Writes the input's alpha moved by
// the offset, in every channel (a valid premultiplied grey).

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(sample_clear(pos.xy - fx.p0.xy).a);
}
