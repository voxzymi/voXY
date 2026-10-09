//! Composition and project settings (design note §3).

use std::borrow::Cow;
use std::sync::Arc;

use glam::DVec2;

use super::{Command, CommandError, CommandReport, MergeKey, comp_mut};
use crate::comp::{Composition, PreviewResolution, WorkArea};
use crate::document::{CompSettings, Document, ProjectSettings};
use crate::id::CompId;
use crate::layer::Layer;
use crate::property::Property;
use crate::time::{FrameRate, Time};
use crate::value::Value;

/// Smallest composition width or height, in pixels.
pub const MIN_COMP_SIZE: u32 = 16;
/// Largest composition width or height, in pixels. The UI also warns when a
/// composition exceeds the GPU's texture limit, which may be lower.
pub const MAX_COMP_SIZE: u32 = 16_384;
/// Highest frame rate a composition may have.
pub const MAX_FRAME_RATE: f64 = 240.0;
/// Largest distance of a start timecode from zero.
pub const MAX_START_OFFSET: Time = Time::from_secs(24 * 60 * 60);
/// Longest composition.
pub const MAX_COMP_DURATION: Time = Time::from_secs(3 * 60 * 60);

/// Checks settings for a new or edited composition (not the name).
pub fn validate_comp_settings(s: &CompSettings) -> Result<(), CommandError> {
    let invalid = |m: String| Err(CommandError::Invalid(m));
    for (what, v) in [("width", s.width), ("height", s.height)] {
        if !(MIN_COMP_SIZE..=MAX_COMP_SIZE).contains(&v) {
            return invalid(format!(
                "{what} must be between {MIN_COMP_SIZE} and {MAX_COMP_SIZE} pixels"
            ));
        }
    }
    if s.frame_rate.as_f64() > MAX_FRAME_RATE {
        return invalid(format!("frame rate must be at most {MAX_FRAME_RATE} fps"));
    }
    if s.duration < s.frame_rate.frame_to_time(1) {
        return invalid("a composition must be at least one frame long".into());
    }
    if s.duration > MAX_COMP_DURATION {
        return invalid("a composition can be at most 3 hours long".into());
    }
    let pa = s.pixel_aspect;
    if pa.num == 0 || pa.den == 0 || !(0.25..=4.0).contains(&pa.as_f64()) {
        return invalid("pixel aspect ratio must be between 0.25 and 4".into());
    }
    if s.drop_frame && !s.frame_rate.supports_drop_frame() {
        return invalid("drop-frame timecode needs 29.97 or 59.94 fps".into());
    }
    if let PreviewResolution::Custom(n) = s.preview_resolution
        && !(2..=PreviewResolution::MAX_CUSTOM).contains(&n)
    {
        return invalid(format!(
            "custom preview resolution must divide by 2 to {}",
            PreviewResolution::MAX_CUSTOM
        ));
    }
    if s.start_offset.flicks().abs() > MAX_START_OFFSET.flicks() {
        return invalid("the start timecode must be within 24 hours of zero".into());
    }
    if !s.background.is_finite() {
        return invalid("background color must be finite".into());
    }
    Ok(())
}

/// Where the old canvas sits in a resized one: fractions of the size change
/// along x and y. `(0.5, 0.5)` keeps the content centered; `(0, 0)` keeps
/// the top-left corner in place. One of the 3×3 grid positions in the UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor(pub DVec2);

impl Anchor {
    pub const CENTER: Anchor = Anchor(DVec2::new(0.5, 0.5));
    pub const TOP_LEFT: Anchor = Anchor(DVec2::ZERO);

    /// Grid cell `col`, `row` in `0..3`.
    pub fn grid(col: u8, row: u8) -> Anchor {
        Anchor(DVec2::new(f64::from(col.min(2)), f64::from(row.min(2))) / 2.0)
    }
}

/// What resizing a composition does to its layers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResizeMode {
    /// Layers keep their size; top-level layers move by the anchor's share
    /// of the size change (the default).
    KeepLayers(Anchor),
    /// Top-level layers scale uniformly by the smaller of the two size
    /// ratios, and the content is centered.
    ScaleToFit,
}

impl Default for ResizeMode {
    fn default() -> Self {
        ResizeMode::KeepLayers(Anchor::CENTER)
    }
}

/// Keyframes and layer edges of a composition that are not on a frame
/// boundary of `rate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OffGrid {
    pub keyframes: usize,
    pub layer_edges: usize,
}

fn on_grid(rate: FrameRate, t: Time) -> bool {
    rate.frame_to_time(rate.time_to_frame(t)) == t
}

/// The frame boundary of `rate` nearest to `t` (ties go to the earlier one).
pub fn nearest_frame_time(rate: FrameRate, t: Time) -> Time {
    let f = rate.time_to_frame(t);
    let (a, b) = (rate.frame_to_time(f), rate.frame_to_time(f + 1));
    if t - a <= b - t { a } else { b }
}

fn comp_properties(c: &Composition) -> impl Iterator<Item = &Property> {
    std::iter::once(&c.background).chain(c.layers.iter().flat_map(|l| l.properties()))
}

/// Counts what changing `comp` to frame rate `rate` would leave between
/// frames. The settings dialog shows this before applying.
pub fn off_grid(comp: &Composition, rate: FrameRate) -> OffGrid {
    let keyframes = comp_properties(comp)
        .filter_map(Property::track)
        .flat_map(crate::keyframe::KeyframeTrack::keys)
        .filter(|k| !on_grid(rate, k.time))
        .count();
    let layer_edges = comp
        .layers
        .iter()
        .flat_map(|l| [l.in_point, l.out_point])
        .filter(|&t| !on_grid(rate, t))
        .count();
    OffGrid {
        keyframes,
        layer_edges,
    }
}

/// Edits a composition's settings.
///
/// - Keyframes and layer timing never move in time when the frame rate
///   changes (time is in flicks); only the frame grid does. With
///   `snap_to_frames`, keys and layer edges move to the nearest new frame
///   (keys landing on the same frame merge, the later one winning) and the
///   report says how many; otherwise it reports how many are off-grid.
/// - A size change treats layers per `resize`.
/// - The work area is clamped to the new duration.
/// - An animated background keeps its keyframes; a static one takes
///   `settings.background`.
///
/// Edits of one composition merge into one undo step while an interaction
/// is open (live dialog edits).
#[derive(Debug, Clone)]
pub struct SetCompositionSettings {
    pub comp: CompId,
    pub settings: CompSettings,
    pub resize: ResizeMode,
    pub snap_to_frames: bool,
}

impl Command for SetCompositionSettings {
    fn label(&self) -> Cow<'static, str> {
        "Composition Settings".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        let s = &self.settings;
        let name = super::validate_name(&s.name)?;
        validate_comp_settings(s)?;
        let c = comp_mut(doc, self.comp)?;
        let old = DVec2::new(f64::from(c.width), f64::from(c.height));
        let new = DVec2::new(f64::from(s.width), f64::from(s.height));
        if old != new {
            resize_layers(c, old, new, self.resize)?;
        }
        c.name = name;
        c.width = s.width;
        c.height = s.height;
        c.pixel_aspect = s.pixel_aspect;
        c.duration = s.duration;
        c.start_offset = s.start_offset;
        c.drop_frame = s.drop_frame;
        c.preview_resolution = s.preview_resolution;
        if !c.background.is_animated() {
            c.background.set_static(Value::Color(s.background))?;
        }
        c.work_area = c.work_area.and_then(|w| {
            let end = w.end.min(s.duration);
            (w.start < end).then_some(WorkArea {
                start: w.start,
                end,
            })
        });
        if c.frame_rate != s.frame_rate {
            c.frame_rate = s.frame_rate;
            if self.snap_to_frames {
                let (keys, merged, edges) = snap_comp(c, s.frame_rate);
                if keys + edges > 0 {
                    report.warnings.push(format!(
                        "Moved {keys} keyframes and {edges} layer edges to the nearest frame."
                    ));
                }
                if merged > 0 {
                    report.warnings.push(format!(
                        "{merged} keyframes landed on the same frame as another and were merged."
                    ));
                }
            } else {
                let og = off_grid(c, s.frame_rate);
                if og.keyframes + og.layer_edges > 0 {
                    report.warnings.push(format!(
                        "{} keyframes and {} layer edges fall between frames at {} fps.",
                        og.keyframes, og.layer_edges, s.frame_rate
                    ));
                }
            }
        }
        Ok(())
    }

    fn merge_key(&self) -> Option<MergeKey> {
        Some(MergeKey {
            kind: "comp-settings",
            target: self.comp.raw(),
        })
    }
}

fn resize_layers(
    c: &mut Composition,
    old: DVec2,
    new: DVec2,
    mode: ResizeMode,
) -> Result<(), CommandError> {
    let (scale, offset) = match mode {
        ResizeMode::KeepLayers(Anchor(a)) => (1.0, (new - old) * a),
        ResizeMode::ScaleToFit => {
            let s = (new.x / old.x).min(new.y / old.y);
            (s, (new - old * s) / 2.0)
        }
    };
    for l in &mut c.layers {
        // Children follow their parents.
        if l.parent.is_some() {
            continue;
        }
        let l: &mut Layer = Arc::make_mut(l);
        l.transform.position.map_values(
            |v| {
                v.as_vec2()
                    .map_or_else(|| v.clone(), |p| Value::Vec2(p * scale + offset))
            },
            |t| crate::SpatialTangents {
                incoming: t.incoming * scale,
                outgoing: t.outgoing * scale,
            },
        )?;
        if scale != 1.0 {
            l.transform.scale.map_values(
                |v| {
                    v.as_vec2()
                        .map_or_else(|| v.clone(), |s| Value::Vec2(s * scale))
                },
                |t| t,
            )?;
        }
    }
    Ok(())
}

/// Snaps keys and layer edges to `rate`. Returns (keys moved, keys merged,
/// edges moved).
fn snap_comp(c: &mut Composition, rate: FrameRate) -> (usize, usize, usize) {
    let mut moved = 0;
    let mut merged = 0;
    let mut edges = 0;
    let mut snap_prop = |p: &mut Property| {
        if let Some(t) = p.track() {
            moved += t.keys().iter().filter(|k| !on_grid(rate, k.time)).count();
            merged += p.retime_keys(|t| nearest_frame_time(rate, t));
        }
    };
    snap_prop(&mut c.background);
    for l in &mut c.layers {
        let l: &mut Layer = Arc::make_mut(l);
        for p in l.properties_mut() {
            snap_prop(p);
        }
        let new_in = nearest_frame_time(rate, l.in_point);
        let new_out = nearest_frame_time(rate, l.out_point).max(new_in);
        edges += usize::from(new_in != l.in_point) + usize::from(new_out != l.out_point);
        // Moving the in-point is a trim: the source stays where it was.
        l.start_offset += new_in - l.in_point;
        l.in_point = new_in;
        l.out_point = new_out;
    }
    (moved, merged, edges)
}

#[derive(Debug, Clone)]
pub struct SetProjectSettings(pub ProjectSettings);

impl Command for SetProjectSettings {
    fn label(&self) -> Cow<'static, str> {
        "Project Settings".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let mut s = self.0.clone();
        s.new_comp.name = super::validate_name(&s.new_comp.name)?;
        validate_comp_settings(&s.new_comp)?;
        if s.default_still_duration.is_some_and(|d| d <= Time::ZERO) {
            return Err(CommandError::Invalid(
                "the default still duration must be positive".into(),
            ));
        }
        doc.meta.settings = s;
        Ok(())
    }
}

/// Sets or clears a composition's work area. Merges while dragging.
#[derive(Debug, Clone)]
pub struct SetWorkArea {
    pub comp: CompId,
    pub area: Option<WorkArea>,
}

impl Command for SetWorkArea {
    fn label(&self) -> Cow<'static, str> {
        "Set Work Area".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let c = comp_mut(doc, self.comp)?;
        if let Some(w) = self.area
            && !(Time::ZERO <= w.start && w.start < w.end && w.end <= c.duration)
        {
            return Err(CommandError::Invalid(
                "the work area must lie inside the composition and be at least one flick long"
                    .into(),
            ));
        }
        c.work_area = self.area;
        Ok(())
    }

    fn merge_key(&self) -> Option<MergeKey> {
        Some(MergeKey {
            kind: "work-area",
            target: self.comp.raw(),
        })
    }
}
