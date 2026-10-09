//! Pre-compose and unprecompose (design note §4).
//!
//! Keyframes are stored in composition time. Pre-composing keeps every time
//! as it is: the new composition starts where the parent does and the
//! precomp layer maps child time to parent time one to one. Unprecomposing
//! shifts (and, at other speeds, scales) the child's keyframe times and
//! layer edges into parent time.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{DAffine2, DVec2, DVec3};

use super::pool::{
    numbered_name, precomps_folder, remap_parents, renumber_layer, source_duration, source_size,
    validate_name,
};
use super::{Command, CommandError, CommandReport, comp_mut, new_layer};
use crate::color::LinearRgba;
use crate::comp::Composition;
use crate::document::{CompSettings, Document};
use crate::evaluate::{layer_world, local_matrix};
use crate::id::{CompId, LayerId};
use crate::keyframe::SpatialTangents;
use crate::layer::{Layer, LayerKind, SourceRef, Transform};
use crate::property::{Property, keys};
use crate::time::Time;
use crate::value::Value;

/// How [`PrecomposeLayers`] treats the selected layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecomposeMode {
    /// The layers move into the new composition with all their properties;
    /// a new precomp layer replaces them. Works for any selection.
    MoveAttributes,
    /// One media or precomp layer only: its transform, timing and keyframes
    /// stay on the layer, which now shows a new composition holding just its
    /// source.
    LeaveAttributes,
}

/// Moves layers into a new composition and shows that composition in their
/// place. Reports parent links it had to replace. `time` is the time at
/// which a removed parent link's transform is baked (the playhead).
#[derive(Debug, Clone)]
pub struct PrecomposeLayers {
    pub comp: CompId,
    pub layers: Vec<LayerId>,
    pub mode: PrecomposeMode,
    pub name: String,
    pub time: Time,
}

impl Command for PrecomposeLayers {
    fn label(&self) -> Cow<'static, str> {
        "Pre-compose".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        let name = validate_name(&self.name)?;
        let parent = doc
            .comp(self.comp)
            .ok_or_else(|| CommandError::NotFound(self.comp.to_string()))?
            .clone();
        if self.layers.is_empty() {
            return Err(CommandError::Invalid("select at least one layer".into()));
        }
        for id in &self.layers {
            if parent.layer(*id).is_none() {
                return Err(CommandError::NotFound(id.to_string()));
            }
        }
        match self.mode {
            PrecomposeMode::MoveAttributes => {
                move_attributes(doc, &parent, &self.layers, name, self.time, report)
            }
            PrecomposeMode::LeaveAttributes => {
                if self.layers.len() != 1 {
                    return Err(CommandError::Invalid(
                        "“Leave attributes” works on one layer only".into(),
                    ));
                }
                leave_attributes(doc, &parent, self.layers[0], name, report)
            }
        }
    }
}

fn move_attributes(
    doc: &mut Document,
    parent: &Composition,
    ids: &[LayerId],
    name: String,
    time: Time,
    report: &mut CommandReport,
) -> Result<(), CommandError> {
    let moved: HashSet<LayerId> = ids.iter().copied().collect();
    let top = parent
        .layers
        .iter()
        .position(|l| moved.contains(&l.id))
        .expect("checked non-empty and present");
    let child = doc.add_comp(CompSettings {
        name,
        ..CompSettings::of(parent)
    });
    let folder = precomps_folder(doc, report);
    comp_mut(doc, child)?.folder = Some(folder);
    let mut kept = Vec::new();
    let mut inside = Vec::new();
    for l in &parent.layers {
        let mut layer = Layer::clone(l);
        let crosses = layer
            .parent
            .is_some_and(|p| moved.contains(&p) != moved.contains(&layer.id));
        if crosses {
            let p = layer.parent.expect("checked");
            let note = bake_parent(&mut layer, parent, p, time)?;
            let pname = parent.layer(p).map_or("", |l| l.name.as_str());
            let place = if moved.contains(&layer.id) {
                format!("stays in “{}”", parent.name)
            } else {
                "moved into the new composition".to_owned()
            };
            report.warnings.push(format!(
                "“{}” was parented to “{pname}”, which {place}: the parent's transform at {time} \
                 was applied to it and the link removed{note}.",
                layer.name
            ));
            layer.parent = None;
        }
        if moved.contains(&layer.id) {
            inside.push(Arc::new(layer));
        } else {
            kept.push(Arc::new(layer));
        }
    }
    let size = DVec2::new(f64::from(parent.width), f64::from(parent.height));
    let id: LayerId = doc.alloc_id();
    let transform = Transform::new(&mut || doc.alloc_id(), size / 2.0, size / 2.0);
    let child_name = doc.comp(child).map(|c| c.name.clone()).unwrap_or_default();
    let precomp = new_layer(
        id,
        child_name,
        LayerKind::Precomp {
            comp: child,
            speed: 1.0,
            looping: false,
            time_remap: None,
            collapse: false,
        },
        parent.duration,
        transform,
    );
    kept.insert(top, Arc::new(precomp));
    comp_mut(doc, child)?.layers = inside;
    comp_mut(doc, parent.id)?.layers = kept;
    report.created_comps.push(child);
    report.created_layers.push(id);
    Ok(())
}

/// Applies the world transform of `parent_id` (in `comp`, at `time`) to
/// `layer`'s own transform, so it keeps its place without the link.
/// Returns a note when the result is only approximate.
fn bake_parent(
    layer: &mut Layer,
    comp: &Composition,
    parent_id: LayerId,
    time: Time,
) -> Result<&'static str, CommandError> {
    let m = layer_world(comp, parent_id, time);
    let (s, angle, t) = m.to_scale_angle_translation();
    let skew_free = DAffine2::from_scale_angle_translation(s, angle, t).abs_diff_eq(m, 1e-9);
    let uniform = (s.x - s.y).abs() <= 1e-12 * s.x.abs().max(1.0);
    let mut animated = false;
    let mut cur = Some(parent_id);
    let mut steps = 0;
    while let Some(id) = cur.filter(|_| steps <= comp.layers.len()) {
        steps += 1;
        let Some(p) = comp.layer(id) else { break };
        animated |= p.transform.properties().iter().any(|p| p.is_animated());
        cur = p.parent;
    }
    let tr = &mut layer.transform;
    tr.position.map_values(
        |v| {
            v.as_vec2()
                .map_or_else(|| v.clone(), |p| Value::Vec2(m.transform_point2(p)))
        },
        |tg| SpatialTangents {
            incoming: vec3(m.transform_vector2(tg.incoming.truncate())),
            outgoing: vec3(m.transform_vector2(tg.outgoing.truncate())),
        },
    )?;
    let degrees = angle.to_degrees();
    tr.rotation.map_values(
        |v| {
            v.as_scalar()
                .map_or_else(|| v.clone(), |r| Value::Scalar(r + degrees))
        },
        |tg| tg,
    )?;
    tr.scale.map_values(
        |v| {
            v.as_vec2()
                .map_or_else(|| v.clone(), |sc| Value::Vec2(sc * s))
        },
        |tg| tg,
    )?;
    Ok(if animated {
        " (approximate: the parent is animated)"
    } else if !skew_free || !uniform {
        " (approximate: the parent is scaled unevenly)"
    } else {
        ""
    })
}

fn vec3(v: DVec2) -> DVec3 {
    v.extend(0.0)
}

fn leave_attributes(
    doc: &mut Document,
    parent: &Composition,
    id: LayerId,
    name: String,
    report: &mut CommandReport,
) -> Result<(), CommandError> {
    let layer = parent.layer(id).expect("checked").clone();
    let Some(source) = layer.kind.source() else {
        return Err(CommandError::Invalid(
            "“Leave attributes” needs a media or precomp layer; use “Move all attributes”".into(),
        ));
    };
    let rate = match source {
        SourceRef::Asset(a) => doc.assets.get(a).and_then(|a| a.frame_rate),
        SourceRef::Comp(c) => doc.comp(c).map(|c| c.frame_rate),
    }
    .unwrap_or(parent.frame_rate);
    let [w, h] = source_size(doc, source).unwrap_or([parent.width, parent.height]);
    let duration = source_duration(doc, source)
        .unwrap_or(parent.duration)
        .max(rate.frame_to_time(1));
    let child = doc.add_comp(CompSettings {
        name,
        width: w.max(1),
        height: h.max(1),
        pixel_aspect: parent.pixel_aspect,
        frame_rate: rate,
        duration,
        background: doc.meta.settings.new_comp.background,
    });
    let folder = precomps_folder(doc, report);
    comp_mut(doc, child)?.folder = Some(folder);
    let (inner_kind, outer_kind) = match layer.kind.clone() {
        LayerKind::Precomp {
            comp,
            speed,
            looping,
            time_remap,
            collapse,
        } => (
            LayerKind::Precomp {
                comp,
                speed: 1.0,
                looping: false,
                time_remap: None,
                collapse: false,
            },
            LayerKind::Precomp {
                comp: child,
                speed,
                looping,
                time_remap,
                collapse,
            },
        ),
        other => (
            other,
            LayerKind::Precomp {
                comp: child,
                speed: 1.0,
                looping: false,
                time_remap: None,
                collapse: false,
            },
        ),
    };
    let size = DVec2::new(f64::from(w), f64::from(h));
    let inner_id: LayerId = doc.alloc_id();
    let transform = Transform::new(&mut || doc.alloc_id(), size / 2.0, size / 2.0);
    let inner = new_layer(
        inner_id,
        layer.name.clone(),
        inner_kind,
        duration,
        transform,
    );
    comp_mut(doc, child)?.layers = vec![Arc::new(inner)];
    comp_mut(doc, parent.id)?
        .layer_mut(id)
        .expect("checked")
        .kind = outer_kind;
    report.created_comps.push(child);
    report.created_layers.push(inner_id);
    Ok(())
}

/// Most parent frames sampled when baking a nonlinear time mapping.
pub const MAX_BAKE_FRAMES: i64 = 100_000;

/// Replaces a precomp layer with copies of its composition's layers,
/// retimed into this composition. The composition stays in the project.
///
/// - Exact when the layer has no time remap and does not loop: keyframe
///   times and layer edges shift by `in_point − start_offset` (and scale by
///   `1/speed`). Layers outside the precomp layer's time range are left out;
///   edges are clipped to it.
/// - With time remap or looping, animated properties are sampled once per
///   frame of this composition and layers keep the span from their first to
///   last appearance; the report says so.
/// - The precomp layer's transform carries over through a new, empty
///   transform layer the copies are parented to, unless that transform is a
///   static identity.
/// - Anything else that cannot carry over exactly is reported.
#[derive(Debug, Clone)]
pub struct UnprecomposeLayer {
    pub layer: LayerId,
}

impl Command for UnprecomposeLayer {
    fn label(&self) -> Cow<'static, str> {
        "Unprecompose".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        let (cid, p) = doc
            .find_layer(self.layer)
            .ok_or_else(|| CommandError::NotFound(self.layer.to_string()))?;
        let p = p.clone();
        let LayerKind::Precomp {
            comp: kid,
            speed,
            looping,
            time_remap,
            collapse,
        } = &p.kind
        else {
            return Err(CommandError::Invalid("not a precomp layer".into()));
        };
        let parent = doc.comp(cid).expect("found above").clone();
        let child = doc
            .comp(*kid)
            .ok_or_else(|| CommandError::NotFound(kid.to_string()))?
            .clone();
        let nonlinear = time_remap.is_some() || *looping;
        let mut notes = Notes::default();

        let mut copies: Vec<Layer> = Vec::new();
        let mut map = HashMap::new();
        for l in &child.layers {
            let mut c = renumber_layer(doc, l);
            map.insert(l.id, c.id);
            let kept = if nonlinear {
                retime_sampled(&mut c, l, &p, &parent, &child, &mut notes)?
            } else {
                retime_linear(&mut c, l, &p, *speed, &mut notes)
            };
            if kept {
                if !p.flags.visible {
                    c.flags.visible = false;
                }
                copies.push(c);
            } else {
                notes.dropped += 1;
            }
        }
        remap_parents(&mut copies, &map);

        // The precomp layer's own transform, kept through a transform layer.
        let identity = p.parent.is_none()
            && p.transform.properties().iter().all(|q| !q.is_animated())
            && local_matrix(&p.transform, p.in_point).abs_diff_eq(DAffine2::IDENTITY, 1e-12);
        let holder = if identity {
            None
        } else {
            Some(transform_layer(doc, &p))
        };
        if let Some(h) = &holder {
            for c in &mut copies {
                if c.parent.is_none() {
                    c.parent = Some(h.id);
                }
            }
        }

        let o = &p.transform.opacity;
        if o.is_animated() || o.value_at(p.in_point).as_scalar() != Some(1.0) {
            report
                .warnings
                .push(format!("The opacity of “{}” was not carried over.", p.name));
        }
        if *collapse {
            report.warnings.push(
                "“Collapse transformations” was on; blending with layers below may differ.".into(),
            );
        }
        if child.frame_rate != parent.frame_rate {
            report.warnings.push(format!(
                "“{}” runs at {} fps; its layers now follow the {} fps frame grid of “{}”.",
                child.name, child.frame_rate, parent.frame_rate, parent.name
            ));
        }
        notes.report(report, &p.name);

        let index = parent.layer_index(p.id).expect("found above");
        let holder_id = holder.as_ref().map(|h| h.id);
        report.created_layers.extend(copies.iter().map(|c| c.id));
        report.created_layers.extend(holder_id);
        let c = comp_mut(doc, cid)?;
        c.layers.remove(index);
        for l in &mut c.layers {
            if l.parent == Some(p.id) {
                Arc::make_mut(l).parent = holder_id;
            }
        }
        let new: Vec<Arc<Layer>> = copies.into_iter().chain(holder).map(Arc::new).collect();
        c.layers.splice(index..index, new);
        Ok(())
    }
}

#[derive(Default)]
struct Notes {
    dropped: usize,
    merged_keys: usize,
    rounded: bool,
    video_speed: bool,
    sampled: bool,
    gaps: Vec<String>,
}

impl Notes {
    fn report(&self, report: &mut CommandReport, precomp: &str) {
        if self.dropped > 0 {
            report.warnings.push(format!(
                "{} layers were outside the time range of “{precomp}” and were left out.",
                self.dropped
            ));
        }
        if self.sampled {
            report.warnings.push(
                "Time remapping or looping was baked by sampling animated properties once per frame."
                    .into(),
            );
        }
        for name in &self.gaps {
            report.warnings.push(format!(
                "“{name}” appeared more than once or with gaps; it now spans its first to last appearance."
            ));
        }
        if self.video_speed {
            report.warnings.push(
                "Video inside the precomp played at a changed speed or remapped time; it now plays at normal speed."
                    .into(),
            );
        }
        if self.rounded {
            report
                .warnings
                .push("Some times were rounded to the nearest flick when changing speed.".into());
        }
        if self.merged_keys > 0 {
            report.warnings.push(format!(
                "{} keyframes landed on the same time as another and were merged.",
                self.merged_keys
            ));
        }
    }
}

/// Linear mapping child → parent time. Returns whether the layer is kept.
fn retime_linear(c: &mut Layer, orig: &Layer, p: &Layer, speed: f64, notes: &mut Notes) -> bool {
    let f = |tc: Time, rounded: &mut bool| -> Time {
        let local = tc - p.start_offset;
        if speed == 1.0 {
            p.in_point + local
        } else {
            let exact = local.flicks() as f64 / speed;
            let r = exact.round();
            *rounded |= r != exact;
            p.in_point + Time::from_flicks(r as i64)
        }
    };
    let mut rounded = false;
    let start = f(orig.in_point, &mut rounded);
    let end = f(orig.out_point, &mut rounded);
    let (cin, cout) = (start.max(p.in_point), end.min(p.out_point));
    if cin >= cout {
        notes.rounded |= rounded;
        return false;
    }
    for prop in c.properties_mut() {
        notes.merged_keys += prop.retime_keys(|t| f(t, &mut false));
    }
    if speed != 1.0 {
        for k in orig.properties() {
            if let Some(t) = k.track() {
                for key in t.keys() {
                    f(key.time, &mut rounded);
                }
            }
        }
    }
    // Clipping the start is a trim: the source stays where it was.
    let trim = cin - start;
    c.in_point = cin;
    c.out_point = cout;
    c.start_offset =
        orig.start_offset + Time::from_flicks((trim.flicks() as f64 * speed).round() as i64);
    if speed != 1.0 {
        match &mut c.kind {
            LayerKind::Precomp { speed: s, .. } => *s *= speed,
            LayerKind::Video { .. } => notes.video_speed = true,
            _ => {}
        }
    }
    notes.rounded |= rounded;
    true
}

/// Nonlinear mapping: sample per parent frame. Returns whether the layer is
/// kept.
fn retime_sampled(
    c: &mut Layer,
    orig: &Layer,
    p: &Layer,
    parent: &Composition,
    child: &Composition,
    notes: &mut Notes,
) -> Result<bool, CommandError> {
    notes.sampled = true;
    let rate = parent.frame_rate;
    let first = rate.time_to_frame(p.in_point);
    let last = rate.time_to_frame(p.out_point - Time::from_flicks(1));
    if last - first >= MAX_BAKE_FRAMES {
        return Err(CommandError::Invalid(format!(
            "the precomp layer is too long to bake (more than {MAX_BAKE_FRAMES} frames)"
        )));
    }
    let map = |t: Time| p.child_time(t, child.duration).unwrap_or(Time::ZERO);
    let frame = |f: i64| rate.frame_to_time(f).max(p.in_point);
    let active: Vec<i64> = (first..=last)
        .filter(|&f| {
            let tc = map(frame(f));
            orig.in_point <= tc && tc < orig.out_point
        })
        .collect();
    let (Some(&a), Some(&b)) = (active.first(), active.last()) else {
        return Ok(false);
    };
    if usize::try_from(b - a + 1).ok() != Some(active.len()) {
        notes.gaps.push(orig.name.clone());
    }
    let times: Vec<Time> = (a..=b).map(frame).collect();
    for prop in c.properties_mut() {
        if prop.is_animated() {
            let src: Property = prop.clone();
            prop.bake_samples(&times, |t| src.value_at(map(t)))?;
        }
    }
    c.in_point = times[0];
    c.out_point = rate.frame_to_time(b + 1).min(p.out_point);
    c.start_offset = map(c.in_point) - orig.in_point + orig.start_offset;
    if matches!(c.kind, LayerKind::Video { .. }) {
        notes.video_speed = true;
    }
    Ok(true)
}

/// An empty layer carrying `p`'s transform, parent and timing.
fn transform_layer(doc: &mut Document, p: &Layer) -> Layer {
    let color = Property::builtin(
        doc.alloc_id(),
        keys::SOLID_COLOR,
        Value::Color(LinearRgba::new(0.0, 0.0, 0.0, 0.0)),
    );
    let size = Property::builtin(doc.alloc_id(), keys::SOLID_SIZE, Value::Vec2(DVec2::ZERO));
    let mut transform = p.transform.clone();
    for q in transform.properties_mut() {
        q.id = doc.alloc_id();
    }
    let id: LayerId = doc.alloc_id();
    let name = format!("{} transform", p.name);
    let mut l = new_layer(
        id,
        name,
        LayerKind::Solid { color, size },
        Time::ZERO,
        transform,
    );
    l.in_point = p.in_point;
    l.out_point = p.out_point;
    l.parent = p.parent;
    l.flags = p.flags;
    l
}

/// Default name for a precomp made in `comp`: `"<comp name> Precomp 1"`, or the
/// next free number.
pub fn precomp_name(doc: &Document, comp: CompId) -> String {
    let base = doc.comp(comp).map_or("Comp", |c| c.name.as_str());
    let first = format!("{base} Precomp 1");
    let taken = |n: &str| doc.comps.values().any(|c| c.name == n);
    if taken(&first) {
        numbered_name(&first, &taken)
    } else {
        first
    }
}
