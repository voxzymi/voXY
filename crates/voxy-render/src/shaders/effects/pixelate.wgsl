// Pixelate (voxy.stylize.pixelate v1). Formula in docs/effects.md.
//
// p0.xy: the layer origin in texels (the block grid starts there); p0.z:
// block size in texels; i0.y: samples per block side (at most 16). Each
// pixel shows the average of its block, premultiplied.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let cell = fx.p0.z;
    let block = floor((pos.xy - fx.p0.xy) / cell);
    let start = fx.p0.xy + block * cell;
    let n = fx.i0.y;
    let step = cell / f32(n);
    var sum = vec4<f32>(0.0);
    for (var j = 0u; j < n; j = j + 1u) {
        for (var i = 0u; i < n; i = i + 1u) {
            let at = start + (vec2<f32>(f32(i), f32(j)) + 0.5) * step;
            sum = sum + load(vec2<i32>(floor(at)));
        }
    }
    return sum / f32(n * n);
}
