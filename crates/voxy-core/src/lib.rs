//! voXY document model.
//!
//! - [`Document`] → [`Composition`]s → [`Layer`]s → [`Property`]s, all with
//!   stable IDs and copy-on-write sharing between snapshots. Assets,
//!   compositions and [`Folder`]s form the project pool; precomp layers
//!   nest compositions.
//! - [`Time`] is integer flicks; [`FrameRate`] is rational.
//! - Properties are static or keyframed ([`KeyframeTrack`]): hold, linear or
//!   cubic-bezier timing ([`easing`]), analytic spring, bounce, oscillation,
//!   steps and stepped-ramp easings ([`shapes`]) and arc-length motion paths
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
mod pool;
pub mod property;
pub mod shapes;
mod time;
pub mod timecode;
mod value;

pub use asset::{Asset, AssetKind, AssetTable, AudioInfo, ContentHash};
pub use color::{LinearRgba, WorkingSpace, linear_to_srgb, srgb_to_linear};
pub use command::{Command, CommandError, CommandReport, MergeKey};
pub use comp::{Composition, PixelAspect, PreviewResolution, WorkArea};
pub use document::{
    CompSettings, Document, ProjectMeta, ProjectSettings, PropertyOwner, ValidationError,
};
pub use easing::{CubicEase, EaseHandle, EasePreset};
pub use evaluate::{
    EvaluatedComp, EvaluatedContent, EvaluatedLayer, MAX_NESTING, Nested, evaluate, layer_world,
    local_matrix,
};
pub use history::{History, SharedDocument};
pub use id::{AssetId, CompId, FolderId, LayerId, PropId};
pub use keyframe::{
    Interp, InterpKind, Keyframe, KeyframeTrack, SegmentEase, SpatialTangents, TrackError,
};
pub use layer::{
    BlendMode, ForeignData, Layer, LayerFlags, LayerKind, PlaceholderInfo, PlaceholderKind,
    SourceRef, Transform,
};
pub use pool::{ExpandedItems, Folder, PoolItem, Usage};
pub use property::{Animation, Property, PropertyDesc, PropertyError, Unit};
pub use shapes::{
    BounceParams, Jump, JumpMode, OscillationParams, SpringParams, SteppedRampParams, StepsParams,
};
pub use time::{FLICKS_PER_SECOND, FrameRate, Time};
pub use value::{Curve, CurveError, Value, ValueKind};

/// Re-exported math types used in the public API.
pub use glam;
