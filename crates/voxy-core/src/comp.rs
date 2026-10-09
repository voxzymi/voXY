//! Compositions.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::id::{CompId, LayerId};
use crate::layer::{ForeignData, Layer};
use crate::property::Property;
use crate::time::{FrameRate, Time};

/// A composition: a canvas with a stack of layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub id: CompId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub duration: Time,
    pub background: Property,
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
}
