// Grain (voxy.stylize.grain v1). Formula in docs/effects.md.
//
// p0.xy: the layer origin in texels; p0.z: grain size in texels; p0.w:
// amount; i0.x: 1 = monochrome; i0.z: base seed (layer, seed, frame). Each
// grain cell, counted in layer pixels from the layer origin, gets
// triangular noise in [−1, 1] from a PCG integer hash, which is bit-exact
// on every GPU.

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// Uniform in [0, 1) from 24 bits of a hash.
fn unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

fn noise(cell: vec2<i32>, channel: u32) -> f32 {
    let h = pcg(fx.i0.z ^ pcg(bitcast<u32>(cell.x) ^ pcg(bitcast<u32>(cell.y) ^ pcg(channel))));
    return unit(h) + unit(pcg(h)) - 1.0;
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    if c.a <= 0.0 {
        return c;
    }
    let cell = vec2<i32>(floor((pos.xy - fx.p0.xy) / fx.p0.z));
    var n = vec3<f32>(noise(cell, 0u));
    if fx.i0.x == 0u {
        n = vec3<f32>(n.x, noise(cell, 1u), noise(cell, 2u));
    }
    return premultiply(max(unpremultiply(c) + n * fx.p0.w, vec3<f32>(0.0)), c.a);
}
