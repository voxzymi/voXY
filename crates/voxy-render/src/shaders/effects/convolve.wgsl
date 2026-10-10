// Separable convolution: one axis of Gaussian Blur (voxy.blur.gaussian v1)
// or Box Blur (voxy.blur.box v1). Formulas in docs/effects.md.
//
// i0.x: axis (0 = x, 1 = y); i0.y: taps on each side; i0.z: 1 = repeat
// edge pixels, 0 = transparent past the buffer. Weights from the CPU, centre
// first, summing to 1 over both sides. Premultiplied, so no colour bleeds
// from transparent pixels.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    var dir = vec2<i32>(1, 0);
    if fx.i0.x == 1u {
        dir = vec2<i32>(0, 1);
    }
    let taps = fx.i0.y;
    let repeat = fx.i0.z == 1u;
    var sum = vec4<f32>(0.0);
    for (var k = 0u; k <= taps; k = k + 1u) {
        let w = weight(k);
        let off = dir * i32(k);
        if k == 0u {
            if repeat {
                sum = sum + load_clamped(p) * w;
            } else {
                sum = sum + load(p) * w;
            }
        } else if repeat {
            sum = sum + (load_clamped(p + off) + load_clamped(p - off)) * w;
        } else {
            sum = sum + (load(p + off) + load(p - off)) * w;
        }
    }
    return sum;
}
