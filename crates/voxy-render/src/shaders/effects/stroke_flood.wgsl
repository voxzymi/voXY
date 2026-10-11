// Stroke, stage 1: one jump-flooding step. Formula in docs/effects.md.
//
// p0.x: the step in texels. Each texel looks at itself and the eight texels
// `step` away, and keeps the nearest marked pixel any of them knows of, as
// an offset (exact small integers in RGBA16F).

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let step = i32(fx.p0.x);
    var best = vec2<f32>(0.0);
    var found = false;
    var best_d = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let jump = vec2<i32>(dx, dy) * step;
            let q = load(p + jump);
            if q.z < 0.5 {
                continue;
            }
            let o = vec2<f32>(jump) + q.xy;
            let d = dot(o, o);
            if !found || d < best_d {
                best = o;
                best_d = d;
                found = true;
            }
        }
    }
    if !found {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(best, 1.0, 0.0);
}
