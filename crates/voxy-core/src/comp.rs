//! Compositions.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::id::{CompId, FolderId, LayerId};
use crate::layer::{ForeignData, Layer};
use crate::property::Property;
use crate::time::{FrameRate, Time};

/// Pixel aspect ratio as a fraction: `1/1` is square pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PixelAspect {
    pub num: u32,
    pub den: u32,
}

impl PixelAspect {
    pub const SQUARE: PixelAspect = PixelAspect { num: 1, den: 1 };

    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    pub fn is_square(&self) -> bool {
        self.num == self.den
    }
}

impl Default for PixelAspect {
    fn default() -> Self {
        PixelAspect::SQUARE
    }
}

/// How much smaller than full size the viewport renders a composition.
/// This lowers the cost of previews only: it never changes the composition
/// size, and exports always render at full size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewResolution {
    #[default]
    Full,
    Half,
    Third,
    Quarter,
    /// One in `n` pixels along each side, `2 ..= 64`.
    Custom(u32),
}

impl PreviewResolution {
    /// Largest custom divisor.
    pub const MAX_CUSTOM: u32 = 64;

    /// The divisor: 1 for full, 2 for half, and so on.
    pub fn divisor(self) -> u32 {
        match self {
            PreviewResolution::Full => 1,
            PreviewResolution::Half => 2,
            PreviewResolution::Third => 3,
            PreviewResolution::Quarter => 4,
            PreviewResolution::Custom(n) => n.max(1),
        }
    }

    /// Side length relative to full size, `1/divisor`.
    pub fn scale(self) -> f64 {
        1.0 / f64::from(self.divisor())
    }

    /// The setting for a divisor, using the named steps where they exist.
    pub fn from_divisor(n: u32) -> PreviewResolution {
        match n {
            0 | 1 => PreviewResolution::Full,
            2 => PreviewResolution::Half,
            3 => PreviewResolution::Third,
            4 => PreviewResolution::Quarter,
            n => PreviewResolution::Custom(n),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PreviewResolution::Full => "Full",
            PreviewResolution::Half => "Half",
            PreviewResolution::Third => "Third",
            PreviewResolution::Quarter => "Quarter",
            PreviewResolution::Custom(_) => "Custom",
        }
    }

    pub fn is_default(&self) -> bool {
        *self == PreviewResolution::Full
    }

    /// Size in pixels of a `width` × `height` composition at this setting.
    pub fn size(self, width: u32, height: u32) -> (u32, u32) {
        let d = self.divisor();
        ((width / d).max(1), (height / d).max(1))
    }
}

/// The part of a composition that previews and renders by default,
/// `start ≤ t < end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkArea {
    pub start: Time,
    pub end: Time,
}

/// A composition: a canvas with a stack of layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub id: CompId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "PixelAspect::is_square")]
    pub pixel_aspect: PixelAspect,
    pub frame_rate: FrameRate,
    pub duration: Time,
    /// `None` means the whole composition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_area: Option<WorkArea>,
    pub background: Property,
    /// Project panel folder; `None` is the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<FolderId>,
    /// Timecode of frame 0, in flicks (format 0.5.0). Only the displayed
    /// timecode changes; layers and keyframes stay where they are.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub start_offset: Time,
    /// Show drop-frame timecode; only for 29.97 and 59.94 fps (format 0.5.0).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drop_frame: bool,
    /// Viewport render scale (format 0.5.0); never affects exports.
    #[serde(default, skip_serializing_if = "PreviewResolution::is_default")]
    pub preview_resolution: PreviewResolution,
    /// Layer stack; index 0 is the top layer.
    pub layers: Vec<Arc<Layer>>,
    #[serde(default, skip_serializing_if = "ForeignData::is_empty")]
    pub foreign: ForeignData,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference.
fn is_zero(t: &Time) -> bool {
    *t == Time::ZERO
}

impl Composition {
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id).map(AsRef::as_ref)
    }

    pub fn layer_index(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// Mutable access, copying the layer if a snapshot still shares it.
    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers
            .iter_mut()
            .find(|l| l.id == id)
            .map(Arc::make_mut)
    }

    /// The work area, or the whole composition when none is set.
    pub fn work_range(&self) -> WorkArea {
        self.work_area.unwrap_or(WorkArea {
            start: Time::ZERO,
            end: self.duration,
        })
    }
}
