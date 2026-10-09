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
    /// Layer stack; index 0 is the top layer.
    pub layers: Vec<Arc<Layer>>,
    #[serde(default, skip_serializing_if = "ForeignData::is_empty")]
    pub foreign: ForeignData,
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
