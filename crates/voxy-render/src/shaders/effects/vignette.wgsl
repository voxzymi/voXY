// Vignette (voxy.stylize.vignette v1). Formula in docs/effects.md.
//
// p0: the layer's rectangle in texels (min.xy, max.zw); p1.x: amount;
// p1.y: size; p1.z: feather; p2.rgb: colour (working space). Distances are
// measured in an ellipse fitted to the layer: 1 at the middle of each edge.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let centre = (fx.p0.xy + fx.p0.zw) * 0.5;
    let half = max((fx.p0.zw - fx.p0.xy) * 0.5, vec2<f32>(1e-3));
    let d = length((pos.xy - centre) / half);
    let t = smoothstep(fx.p1.y, fx.p1.y + max(fx.p1.z, 1e-3), d) * fx.p1.x;
    return premultiply(mix(unpremultiply(c), fx.p2.rgb, t), c.a);
}
