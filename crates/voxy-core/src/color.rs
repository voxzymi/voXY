//! Colors and the project working color space.

use serde::{Deserialize, Serialize};

/// A linear-light RGBA color with straight (non-premultiplied) alpha.
///
/// The document model always stores colors in linear light; the compositor
/// decides how to encode them for the project's [`WorkingSpace`].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LinearRgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl LinearRgba {
    pub const TRANSPARENT: LinearRgba = LinearRgba::new(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: LinearRgba = LinearRgba::new(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: LinearRgba = LinearRgba::new(1.0, 1.0, 1.0, 1.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        LinearRgba { r, g, b, a }
    }

    /// From sRGB-encoded 8-bit components (alpha is linear by definition).
    pub fn from_srgb8(r: u8, g: u8, b: u8, a: u8) -> Self {
        let decode = |v: u8| srgb_to_linear(f32::from(v) / 255.0);
        LinearRgba::new(decode(r), decode(g), decode(b), f32::from(a) / 255.0)
    }

    /// From sRGB-encoded float components in `0..=1`.
    pub fn from_srgb(r: f32, g: f32, b: f32, a: f32) -> Self {
        LinearRgba::new(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a)
    }

    /// sRGB-encoded components, straight alpha.
    pub fn to_srgb(self) -> [f32; 4] {
        [
            linear_to_srgb(self.r),
            linear_to_srgb(self.g),
            linear_to_srgb(self.b),
            self.a,
        ]
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn is_finite(self) -> bool {
        self.to_array().iter().all(|c| c.is_finite())
    }
}

/// The sRGB electro-optical transfer function (IEC 61966-2-1), decode.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The inverse sRGB transfer function, encode.
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// The color space blending, opacity and (later) effects are computed in.
///
/// voXY projects default to [`WorkingSpace::Linear`]. Alight Motion imports
/// will default to whichever space the golden fidelity tests show matches the
/// app's reference renders; that is measured, not assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkingSpace {
    /// Blend in linear light (physically correct; voXY default).
    #[default]
    Linear,
    /// Blend sRGB-encoded values, as many display-referred editors do.
    SrgbCompat,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trip() {
        for i in 0..=255u8 {
            let v = f32::from(i) / 255.0;
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5, "{i}");
        }
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        assert!((srgb_to_linear(0.5) - 0.214_041).abs() < 1e-5);
    }
}
