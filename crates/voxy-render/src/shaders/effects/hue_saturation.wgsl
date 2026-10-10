// Hue / Saturation / Lightness (voxy.color.hue_saturation v1). Formula in
// docs/effects.md.
//
// p0.x: hue shift (turns); p0.y: saturation (-1..1); p0.z: lightness
// (-1..1); i0.x: 1 = colorize; p1.x: colorize hue (turns); p1.y: colorize
// saturation (0..1).

fn rgb_to_hsl(c: vec3<f32>) -> vec3<f32> {
    let mx = max(max(c.r, c.g), c.b);
    let mn = min(min(c.r, c.g), c.b);
    let l = (mx + mn) * 0.5;
    let d = mx - mn;
    if d <= 0.0 {
        return vec3<f32>(0.0, 0.0, l);
    }
    var s = d / (1.0 - abs(2.0 * l - 1.0));
    var h = 0.0;
    if mx == c.r {
        h = (c.g - c.b) / d;
        if h < 0.0 {
            h = h + 6.0;
        }
    } else if mx == c.g {
        h = (c.b - c.r) / d + 2.0;
    } else {
        h = (c.r - c.g) / d + 4.0;
    }
    return vec3<f32>(h / 6.0, clamp(s, 0.0, 1.0), l);
}

fn hsl_to_rgb(hsl: vec3<f32>) -> vec3<f32> {
    let h = fract(hsl.x) * 6.0;
    let c = (1.0 - abs(2.0 * hsl.z - 1.0)) * hsl.y;
    let x = c * (1.0 - abs(h % 2.0 - 1.0));
    var rgb = vec3<f32>(0.0);
    if h < 1.0 {
        rgb = vec3<f32>(c, x, 0.0);
    } else if h < 2.0 {
        rgb = vec3<f32>(x, c, 0.0);
    } else if h < 3.0 {
        rgb = vec3<f32>(0.0, c, x);
    } else if h < 4.0 {
        rgb = vec3<f32>(0.0, x, c);
    } else if h < 5.0 {
        rgb = vec3<f32>(x, 0.0, c);
    } else {
        rgb = vec3<f32>(c, 0.0, x);
    }
    return rgb + vec3<f32>(hsl.z - c * 0.5);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let rgb = clamp(unpremultiply(c), vec3<f32>(0.0), vec3<f32>(1.0));
    var hsl = rgb_to_hsl(rgb);
    if fx.i0.x == 1u {
        hsl = vec3<f32>(fx.p1.x, fx.p1.y, hsl.z);
    } else {
        hsl.x = hsl.x + fx.p0.x;
        hsl.y = clamp(hsl.y * (1.0 + fx.p0.y), 0.0, 1.0);
    }
    let l = fx.p0.z;
    if l > 0.0 {
        hsl.z = hsl.z + (1.0 - hsl.z) * l;
    } else {
        hsl.z = hsl.z * (1.0 + l);
    }
    return premultiply(hsl_to_rgb(hsl), c.a);
}
