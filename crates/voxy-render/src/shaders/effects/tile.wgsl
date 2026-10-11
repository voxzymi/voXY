// Tile (voxy.distort.tile v1). Formula in docs/effects.md.
//
// p0: the layer's rectangle in texels (min xy, max zw); p1.xy: a tile's
// centre; p1.zw: tile size in texels; p2: the tiled area in texels; i0.x:
// 1 = mirror every other tile. Each tile shows the whole layer.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if pos.x < fx.p2.x || pos.y < fx.p2.y || pos.x > fx.p2.z || pos.y > fx.p2.w {
        return vec4<f32>(0.0);
    }
    let u = (pos.xy - fx.p1.xy) / fx.p1.zw + vec2<f32>(0.5);
    let cell = floor(u);
    var f = u - cell;
    if fx.i0.x == 1u {
        let odd = abs(cell - 2.0 * floor(cell * 0.5));
        f = mix(f, vec2<f32>(1.0) - f, odd);
    }
    // Kept half a texel inside the layer, so tiles meet without seams.
    let at = fx.p0.xy + f * (fx.p0.zw - fx.p0.xy);
    let inner = vec2<f32>(0.5);
    return sample_clear(clamp(at, fx.p0.xy + inner, max(fx.p0.zw - inner, fx.p0.xy + inner)));
}
