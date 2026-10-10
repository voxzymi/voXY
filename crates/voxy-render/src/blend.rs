//! Blend modes: the CPU reference of `shaders/blend.wgsl`
//! (`docs/design-effects-ofx.md` §3.7, formulas in `docs/effects.md`).
//!
//! The formulas follow W3C Compositing and Blending Level 1, plus voXY's own
//! add and subtract. `B(Cb, Cs)` is applied to straight (unpremultiplied)
//! colours, then composited source-over:
//!
//! `co = cs·(1 − αb) + cb·(1 − αs) + αs·αb·B(Cb, Cs)`, `αo = αs + αb·(1 − αs)`
//!
//! with `cs`, `cb` premultiplied. Modes defined for `[0, 1]` (screen,
//! overlay, hard and soft light, dodge, burn and the four HSL modes) clamp
//! their straight inputs to `[0, 1]`; the others also work on HDR values.
//! The tests compare the GPU against this module.

use voxy_core::BlendMode;

/// W3C luma weights (owner decision D8).
pub const LUMA: [f64; 3] = [0.30, 0.59, 0.11];

/// The shader's index for `mode` (`BlendMode::ALL` order).
pub fn mode_index(mode: BlendMode) -> u32 {
    BlendMode::ALL.iter().position(|m| *m == mode).unwrap_or(0) as u32
}

/// Whether `mode` clamps its straight inputs to `[0, 1]`.
pub fn clamps_inputs(mode: BlendMode) -> bool {
    use BlendMode::{
        Color, ColorBurn, ColorDodge, HardLight, Hue, Luminosity, Overlay, Saturation, Screen,
        SoftLight,
    };
    matches!(
        mode,
        Screen
            | Overlay
            | SoftLight
            | HardLight
            | ColorDodge
            | ColorBurn
            | Hue
            | Saturation
            | Color
            | Luminosity
    )
}

fn multiply(b: f64, s: f64) -> f64 {
    b * s
}

fn screen(b: f64, s: f64) -> f64 {
    b + s - b * s
}

fn hard_light(b: f64, s: f64) -> f64 {
    if s <= 0.5 {
        multiply(b, 2.0 * s)
    } else {
        screen(b, 2.0 * s - 1.0)
    }
}

fn soft_light(b: f64, s: f64) -> f64 {
    if s <= 0.5 {
        b - (1.0 - 2.0 * s) * b * (1.0 - b)
    } else {
        let d = if b <= 0.25 {
            ((16.0 * b - 12.0) * b + 4.0) * b
        } else {
            b.sqrt()
        };
        b + (2.0 * s - 1.0) * (d - b)
    }
}

fn color_dodge(b: f64, s: f64) -> f64 {
    if b <= 0.0 {
        0.0
    } else if s >= 1.0 {
        1.0
    } else {
        (b / (1.0 - s)).min(1.0)
    }
}

fn color_burn(b: f64, s: f64) -> f64 {
    if b >= 1.0 {
        1.0
    } else if s <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - b) / s).min(1.0)
    }
}

fn lum(c: [f64; 3]) -> f64 {
    LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2]
}

fn clip_color(c: [f64; 3]) -> [f64; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        out = out.map(|v| l + (v - l) * l / (l - n));
    }
    if x > 1.0 {
        out = out.map(|v| l + (v - l) * (1.0 - l) / (x - l));
    }
    out
}

fn set_lum(c: [f64; 3], l: f64) -> [f64; 3] {
    let d = l - lum(c);
    clip_color(c.map(|v| v + d))
}

fn sat(c: [f64; 3]) -> f64 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

/// W3C `SetSat`: the minimum goes to 0, the maximum to `s`, the middle
/// proportionally; all 0 for a grey.
fn set_sat(c: [f64; 3], s: f64) -> [f64; 3] {
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    if x > n {
        c.map(|v| (v - n) * s / (x - n))
    } else {
        [0.0; 3]
    }
}

/// `B(Cb, Cs)` on straight colours (inputs already clamped where the mode
/// requires it).
pub fn blend_rgb(mode: BlendMode, cb: [f64; 3], cs: [f64; 3]) -> [f64; 3] {
    use BlendMode::{
        Add, Color, ColorBurn, ColorDodge, Darken, Difference, HardLight, Hue, Lighten, Luminosity,
        Multiply, Normal, Overlay, Saturation, Screen, SoftLight, Subtract,
    };
    let sep = |f: fn(f64, f64) -> f64| [f(cb[0], cs[0]), f(cb[1], cs[1]), f(cb[2], cs[2])];
    match mode {
        Normal => cs,
        Multiply => sep(multiply),
        Screen => sep(screen),
        Overlay => sep(|b, s| hard_light(s, b)),
        SoftLight => sep(soft_light),
        HardLight => sep(hard_light),
        Add => sep(|b, s| b + s),
        Subtract => sep(|b, s| (b - s).max(0.0)),
        Difference => sep(|b, s| (b - s).abs()),
        ColorDodge => sep(color_dodge),
        ColorBurn => sep(color_burn),
        Lighten => sep(f64::max),
        Darken => sep(f64::min),
        Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        Color => set_lum(cs, lum(cb)),
        Luminosity => set_lum(cb, lum(cs)),
    }
}

fn straight(c: [f64; 4]) -> [f64; 3] {
    if c[3] > 0.0 {
        [c[0] / c[3], c[1] / c[3], c[2] / c[3]]
    } else {
        [0.0; 3]
    }
}

/// Composites premultiplied `src` over premultiplied `dst` with `mode`.
pub fn composite(mode: BlendMode, dst: [f64; 4], src: [f64; 4]) -> [f64; 4] {
    let (ab, as_) = (dst[3], src[3]);
    let mut cb = straight(dst);
    let mut cs = straight(src);
    if clamps_inputs(mode) {
        cb = cb.map(|v| v.clamp(0.0, 1.0));
        cs = cs.map(|v| v.clamp(0.0, 1.0));
    }
    let b = blend_rgb(mode, cb, cs);
    let mut out = [0.0; 4];
    for i in 0..3 {
        out[i] = src[i] * (1.0 - ab) + dst[i] * (1.0 - as_) + as_ * ab * b[i];
    }
    out[3] = as_ + ab * (1.0 - as_);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use BlendMode::{
        Add, Color, ColorBurn, ColorDodge, Darken, Difference, HardLight, Hue, Lighten, Luminosity,
        Multiply, Normal, Overlay, Saturation, Screen, SoftLight, Subtract,
    };

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn known_values() {
        let b = [0.2, 0.5, 0.8];
        let s = [0.6, 0.5, 0.1];
        let cases: [(BlendMode, [f64; 3]); 13] = [
            (Normal, s),
            (Multiply, [0.12, 0.25, 0.08]),
            (Screen, [0.68, 0.75, 0.82]),
            (Overlay, [0.24, 0.5, 0.64]),
            (HardLight, [0.36, 0.5, 0.16]),
            (Add, [0.8, 1.0, 0.9]),
            (Subtract, [0.0, 0.0, 0.7]),
            (Difference, [0.4, 0.0, 0.7]),
            (ColorDodge, [0.5, 1.0, 0.8 / 0.9]),
            (ColorBurn, [0.0, 0.0, 0.0]),
            (Lighten, [0.6, 0.5, 0.8]),
            (Darken, [0.2, 0.5, 0.1]),
            // r: Cs > 0.5 with the polynomial D(Cb) (Cb ≤ 0.25);
            // g, b: the Cs ≤ 0.5 branch.
            (
                SoftLight,
                [
                    0.2 + 0.2 * (((16.0 * 0.2 - 12.0) * 0.2 + 4.0) * 0.2 - 0.2),
                    0.5,
                    0.8 - (1.0 - 2.0 * 0.1) * 0.8 * (1.0 - 0.8),
                ],
            ),
        ];
        for (mode, want) in cases {
            assert!(
                close(blend_rgb(mode, b, s), want),
                "{mode:?}: {:?}",
                blend_rgb(mode, b, s)
            );
        }
    }

    #[test]
    fn hsl_modes_keep_the_luma_they_promise() {
        let b = [0.2, 0.5, 0.8];
        let s = [0.9, 0.3, 0.1];
        // Color and hue keep the backdrop's luma; luminosity takes the
        // source's (ClipColor never needs to act here).
        assert!((lum(blend_rgb(Color, b, s)) - lum(b)).abs() < 1e-12);
        assert!((lum(blend_rgb(Hue, b, s)) - lum(b)).abs() < 1e-12);
        assert!((lum(blend_rgb(Luminosity, b, s)) - lum(s)).abs() < 1e-12);
        // Saturation keeps the backdrop's hue order and takes the source's
        // saturation.
        let r = blend_rgb(Saturation, b, s);
        assert!((sat(r) - sat(s)).abs() < 1e-9);
        // A grey stays grey under saturation from a grey.
        assert!(close(blend_rgb(Saturation, b, [0.4; 3]), [lum(b); 3]));
    }

    #[test]
    fn compositing_edges() {
        let dst = [0.1, 0.2, 0.3, 1.0];
        for mode in BlendMode::ALL {
            // A transparent source changes nothing.
            assert_eq!(composite(mode, dst, [0.0; 4]), dst, "{mode:?}");
            // Over a transparent backdrop, every mode is plain "over".
            let src = [0.25, 0.1, 0.05, 0.5];
            assert_eq!(composite(mode, [0.0; 4], src), src, "{mode:?}");
        }
        // Normal is premultiplied over.
        let src = [0.3, 0.3, 0.3, 0.5];
        let over = composite(Normal, dst, src);
        for i in 0..4 {
            assert!((over[i] - (src[i] + dst[i] * 0.5)).abs() < 1e-12);
        }
    }

    #[test]
    fn indices_follow_the_menu_order() {
        for (i, m) in BlendMode::ALL.iter().enumerate() {
            assert_eq!(mode_index(*m), i as u32);
        }
    }
}
