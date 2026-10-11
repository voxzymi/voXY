// Stroke, stage 2. Formula in docs/effects.md.
//
// src: offsets to the nearest marked pixel; orig: the effect's input.
// p0.rgb: colour (working space); p1.x: width w in texels; p1.y: opacity;
// i0.x: 1 = the outline alone (without the shape it surrounds). Coverage clamp(w − d + 1, 0, 1), with d the
// distance to the nearest marked pixel's centre, which is about half a
// pixel beyond the shape's edge.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let f = load(p);
    var cover = 0.0;
    if f.z >= 0.5 {
        cover = clamp(fx.p1.x - length(f.xy) + 1.0, 0.0, 1.0);
    }
    let a = cover * fx.p1.y;
    let line = vec4<f32>(fx.p0.rgb * a, a);
    let o = load_orig(p);
    // The part of the line the shape does not cover.
    let visible = line * (1.0 - o.a);
    if fx.i0.x == 1u {
        return visible;
    }
    return o + visible;
}
