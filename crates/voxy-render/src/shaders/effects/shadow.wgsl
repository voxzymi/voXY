// Drop Shadow and Inner Shadow, stage 2. Formula in docs/effects.md.
//
// src: the moved, blurred alpha m; orig: the kernel's input. p0.rgb:
// shadow colour (working space); p1.x: opacity; i0.x: 0 = drop shadow
// behind the input, 1 = the drop shadow alone, 2 = inner shadow.

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let m = clamp(load(p).a, 0.0, 1.0);
    let o = load_orig(p);
    let opacity = fx.p1.x;
    if fx.i0.x == 2u {
        // Inside the layer, where the moved alpha is missing.
        let s = (1.0 - m) * opacity;
        return premultiply(mix(unpremultiply(o), fx.p0.rgb, s), o.a);
    }
    let shadow = vec4<f32>(fx.p0.rgb * m, m) * opacity;
    if fx.i0.x == 1u {
        return shadow;
    }
    return o + shadow * (1.0 - o.a);
}
