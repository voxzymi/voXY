//! voXY document model.
//!
//! - [`Document`] → [`Composition`]s → [`Layer`]s → [`Property`]s, all with
//!   stable IDs and copy-on-write sharing between snapshots.
//! - [`Time`] is integer flicks; [`FrameRate`] is rational.
//! - Properties are static or keyframed ([`KeyframeTrack`]): hold, linear or
//!   cubic-bezier timing ([`easing`]) and arc-length motion paths
//!   ([`motion_path`]) for spatial properties.
//! - Every edit is a [`Command`]; [`History`] provides snapshot-based
//!   undo/redo, transactions and drag merging.
//! - [`evaluate()`] resolves a composition at a time for the renderer.
//! - [`persist`] reads and writes versioned `.voxy` JSON.
//!
//! This crate has no GPU, media or UI dependencies.

mod asset;
mod color;
pub mod command;
mod comp;
mod document;
pub mod easing;
mod evaluate;
mod history;
mod id;
pub mod keyframe;
mod layer;
pub mod motion_path;
pub mod persist;
pub mod property;
mod time;
mod value;

pub use asset::{Asset, AssetKind, AssetTable};
pub use color::{LinearRgba, WorkingSpace, linear_to_srgb, srgb_to_linear};
pub use command::{Command, CommandError, MergeKey};
pub use comp::Composition;
pub use document::{CompSettings, Document, ProjectMeta, PropertyOwner, ValidationError};
pub use easing::{CubicEase, EaseHandle, EasePreset};
pub use evaluate::{EvaluatedComp, EvaluatedContent, EvaluatedLayer, evaluate, local_matrix};
pub use history::{History, SharedDocument};
pub use id::{AssetId, CompId, LayerId, PropId};
pub use keyframe::{Interp, Keyframe, KeyframeTrack, SpatialTangents, TrackError};
pub use layer::{BlendMode, ForeignData, Layer, LayerFlags, LayerKind, Transform};
pub use property::{Animation, Property, PropertyDesc, PropertyError, Unit};
pub use time::{FLICKS_PER_SECOND, FrameRate, Time};
pub use value::{Value, ValueKind};

/// Re-exported math types used in the public API.
pub use glam;
