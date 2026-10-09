//! Stable node identifiers.
//!
//! All IDs in a document come from one per-document counter, so an ID is
//! unique across every node type. IDs never change when nodes are reordered,
//! which lets undo, selection, caches and plugin instances key on them.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            pub const fn raw(self) -> u64 {
                self.0
            }
        }

        impl From<u64> for $name {
            fn from(raw: u64) -> Self {
                $name(raw)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "#{}"), self.0)
            }
        }
    };
}

define_id!(
    /// Identifies a [`crate::Layer`].
    LayerId,
    "layer"
);
define_id!(
    /// Identifies a [`crate::Composition`].
    CompId,
    "comp"
);
define_id!(
    /// Identifies a [`crate::Property`].
    PropId,
    "prop"
);
define_id!(
    /// Identifies an [`crate::Asset`].
    AssetId,
    "asset"
);
