// Matte Choker, stage 0: one axis of a square erosion or dilation.
// Formula in docs/effects.md.
//
// i0.x: axis (0 = x, 1 = y); i0.y: taps each side; i0.z: 0 = shrink
// (erode), 1 = grow (dilate). Shrinking scales each pixel by the smallest
// alpha around it, keeping its own colour; growing takes the largest of
// each premultiplied channel, so new edge pixels get their neighbours'
// colour.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    var dir = vec2<i32>(1, 0);
    if fx.i0.x == 1u {
        dir = vec2<i32>(0, 1);
    }
    let n = i32(fx.i0.y);
    let c = load(p);
    if fx.i0.z == 1u {
        var m = c;
        for (var k = 1; k <= n; k = k + 1) {
            m = max(m, max(load(p + dir * k), load(p - dir * k)));
        }
        return m;
    }
    var a = c.a;
    for (var k = 1; k <= n; k = k + 1) {
        a = min(a, min(load(p + dir * k).a, load(p - dir * k).a));
    }
    if c.a <= 0.0 {
        return vec4<f32>(0.0);
    }
    return c * (a / c.a);
}
