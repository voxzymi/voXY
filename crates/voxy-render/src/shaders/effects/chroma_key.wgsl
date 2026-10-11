// Chroma Key (voxy.keying.chroma_key v1). Formula in docs/effects.md.
//
// p0.rgb: key colour (working space); p1.x: tolerance; p1.y: softness;
// p1.z: spill suppression. Colours are compared in the Rec. 709 YCbCr
// chroma plane, so brightness alone never keys a pixel out.

fn chroma(c: vec3<f32>) -> vec2<f32> {
    let y = luma709(c);
    return vec2<f32>((c.b - y) / 1.8556, (c.r - y) / 1.5748);
}

fn from_ycc(y: f32, cc: vec2<f32>) -> vec3<f32> {
    let r = y + 1.5748 * cc.y;
    let b = y + 1.8556 * cc.x;
    let g = (y - 0.2126 * r - 0.0722 * b) / 0.7152;
    return vec3<f32>(r, g, b);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let rgb = unpremultiply(c);
    let key = chroma(fx.p0.rgb);
    var cc = chroma(rgb);
    let d = distance(cc, key);
    let keep = smoothstep(fx.p1.x, fx.p1.x + max(fx.p1.y, 1e-4), d);
    // Spill: remove the part of the chroma that points towards the key.
    let len = length(key);
    if len > 1e-5 {
        let dir = key / len;
        cc = cc - dir * max(dot(cc, dir), 0.0) * fx.p1.z;
    }
    let out = from_ycc(luma709(rgb), cc);
    return premultiply(max(out, vec3<f32>(0.0)), c.a * keep);
}
