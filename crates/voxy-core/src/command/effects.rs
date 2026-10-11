//! Effect-stack commands (`docs/design-effects-ofx.md` §1.6). Each one is a
//! single undo step. Parameter edits use the ordinary property commands.

use std::borrow::Cow;
use std::sync::Arc;

use super::{Command, CommandError, CommandReport, insert_layer, new_layer};
use crate::document::Document;
use crate::effect::{EffectInstance, EffectMatte};
use crate::id::{CompId, EffectId, LayerId};
use crate::keyframe::SpatialTangents;
use crate::layer::{BlendMode, Layer, LayerKind, Transform};
use crate::registry::EffectDef;
use crate::value::Value;

fn layer_mut(doc: &mut Document, id: LayerId) -> Result<&mut Layer, CommandError> {
    doc.layer_mut(id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))
}

fn effect_index(layer: &Layer, id: EffectId) -> Result<usize, CommandError> {
    layer
        .effects
        .iter()
        .position(|e| e.id == id)
        .ok_or_else(|| CommandError::NotFound(format!("{id} on {}", layer.id)))
}

fn plural(n: usize, one: &'static str, many: &'static str) -> Cow<'static, str> {
    if n == 1 { one } else { many }.into()
}

/// Adds a new effect at its defaults. `index` `None` appends it to the end
/// of the stack (applied last).
#[derive(Debug, Clone)]
pub struct AddEffect {
    pub layer: LayerId,
    pub index: Option<usize>,
    pub def: Arc<EffectDef>,
}

impl Command for AddEffect {
    fn label(&self) -> Cow<'static, str> {
        format!("Add {}", self.def.name).into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        doc.find_layer(self.layer)
            .ok_or_else(|| CommandError::NotFound(self.layer.to_string()))?;
        let mut fx = self.def.instantiate(doc);
        // Point parameters start at the centre of the layer's content.
        if let Some(size) = doc.content_size(self.layer) {
            for (p, d) in fx.params.iter_mut().zip(self.def.params.iter()) {
                if d.gizmo == Some(crate::registry::GizmoKind::Point) {
                    // A Vec2 always fits a point parameter.
                    let _ = p.set_static(Value::Vec2(size / 2.0));
                }
            }
        }
        report.created_effects.push(fx.id);
        let layer = layer_mut(doc, self.layer)?;
        let at = self
            .index
            .unwrap_or(layer.effects.len())
            .min(layer.effects.len());
        layer.effects.insert(at, fx);
        Ok(())
    }
}

/// Removes effects from a layer.
#[derive(Debug, Clone)]
pub struct RemoveEffects {
    pub layer: LayerId,
    pub ids: Vec<EffectId>,
}

impl Command for RemoveEffects {
    fn label(&self) -> Cow<'static, str> {
        plural(self.ids.len(), "Remove Effect", "Remove Effects")
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let layer = layer_mut(doc, self.layer)?;
        for &id in &self.ids {
            effect_index(layer, id)?;
        }
        layer.effects.retain(|e| !self.ids.contains(&e.id));
        Ok(())
    }
}

/// Moves one effect to `to_index` in its layer's stack (drag-reorder).
#[derive(Debug, Clone)]
pub struct MoveEffect {
    pub layer: LayerId,
    pub id: EffectId,
    pub to_index: usize,
}

impl Command for MoveEffect {
    fn label(&self) -> Cow<'static, str> {
        "Reorder Effect".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let layer = layer_mut(doc, self.layer)?;
        let from = effect_index(layer, self.id)?;
        let fx = layer.effects.remove(from);
        let to = self.to_index.min(layer.effects.len());
        layer.effects.insert(to, fx);
        Ok(())
    }
}

/// Gives an effect, and all its parameters, fresh IDs.
fn renumber(doc: &mut Document, fx: &mut EffectInstance) {
    fx.renumber(&mut || doc.alloc_id());
}

/// After `layers` were copied as a group with new IDs (`map`: old to new),
/// points their effect mattes and layer references at the copies; links to
/// layers outside the group are cleared.
pub(crate) fn remap_layer_refs(
    layers: &mut [Layer],
    map: &std::collections::HashMap<LayerId, LayerId>,
) {
    let remap = |v: &Value| match v {
        Value::Layer(Some(id)) => Value::Layer(map.get(id).copied()),
        other => other.clone(),
    };
    for l in layers {
        for fx in &mut l.effects {
            fx.matte = fx
                .matte
                .and_then(|m| map.get(&m.layer).map(|&layer| EffectMatte { layer, ..m }));
            for p in fx.editable_params_mut() {
                if p.desc.kind == crate::value::ValueKind::Layer {
                    // Mapping layer references never fails validation.
                    let _ = p.map_values(remap, |t: SpatialTangents| t);
                }
            }
        }
    }
}

/// Duplicates effects; each copy goes right after its original.
#[derive(Debug, Clone)]
pub struct DuplicateEffects {
    pub layer: LayerId,
    pub ids: Vec<EffectId>,
}

impl Command for DuplicateEffects {
    fn label(&self) -> Cow<'static, str> {
        plural(self.ids.len(), "Duplicate Effect", "Duplicate Effects")
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        let layer = doc
            .find_layer(self.layer)
            .ok_or_else(|| CommandError::NotFound(self.layer.to_string()))?
            .1;
        let mut copies = Vec::new();
        for &id in &self.ids {
            let fx = &layer.effects[effect_index(layer, id)?];
            if fx.is_placeholder() {
                return Err(CommandError::Invalid(format!(
                    "{} is not installed and cannot be duplicated",
                    fx.name
                )));
            }
            copies.push((id, fx.clone()));
        }
        for (after, mut fx) in copies {
            renumber(doc, &mut fx);
            report.created_effects.push(fx.id);
            let layer = layer_mut(doc, self.layer)?;
            let at = effect_index(layer, after)? + 1;
            layer.effects.insert(at, fx);
        }
        Ok(())
    }
}

/// Turns effects on or off.
#[derive(Debug, Clone)]
pub struct SetEffectsEnabled {
    pub layer: LayerId,
    pub ids: Vec<EffectId>,
    pub enabled: bool,
}

impl Command for SetEffectsEnabled {
    fn label(&self) -> Cow<'static, str> {
        match (self.enabled, self.ids.len()) {
            (true, 1) => "Enable Effect",
            (true, _) => "Enable Effects",
            (false, 1) => "Disable Effect",
            (false, _) => "Disable Effects",
        }
        .into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let layer = layer_mut(doc, self.layer)?;
        for &id in &self.ids {
            let i = effect_index(layer, id)?;
            layer.effects[i].enabled = self.enabled;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RenameEffect {
    pub layer: LayerId,
    pub id: EffectId,
    pub name: String,
}

impl Command for RenameEffect {
    fn label(&self) -> Cow<'static, str> {
        "Rename Effect".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(CommandError::Invalid("an effect needs a name".into()));
        }
        let layer = layer_mut(doc, self.layer)?;
        let i = effect_index(layer, self.id)?;
        name.clone_into(&mut layer.effects[i].name);
        Ok(())
    }
}

/// Sets or clears an effect's matte. The matte layer must be another layer
/// in the same composition.
#[derive(Debug, Clone)]
pub struct SetEffectMatte {
    pub layer: LayerId,
    pub id: EffectId,
    pub matte: Option<EffectMatte>,
}

impl Command for SetEffectMatte {
    fn label(&self) -> Cow<'static, str> {
        "Set Effect Matte".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let (comp, _) = doc
            .find_layer(self.layer)
            .ok_or_else(|| CommandError::NotFound(self.layer.to_string()))?;
        if let Some(m) = self.matte {
            let same_comp = doc.find_layer(m.layer).is_some_and(|(c, _)| c == comp);
            if m.layer == self.layer || !same_comp {
                return Err(CommandError::Invalid(
                    "the matte must be another layer in the same composition".into(),
                ));
            }
        }
        let layer = layer_mut(doc, self.layer)?;
        let i = effect_index(layer, self.id)?;
        if layer.effects[i].is_placeholder() {
            return Err(CommandError::Invalid(
                "a missing effect's settings cannot be changed".into(),
            ));
        }
        layer.effects[i].matte = self.matte;
        Ok(())
    }
}

/// Inserts copies of effects (from the clipboard or a preset) at `index`
/// (`None`: the end) of each target layer, in one step. The copies get new
/// IDs; references to layers outside the target's composition are cleared
/// and reported.
#[derive(Debug, Clone)]
pub struct PasteEffects {
    pub layers: Vec<LayerId>,
    pub index: Option<usize>,
    pub items: Vec<EffectInstance>,
}

impl Command for PasteEffects {
    fn label(&self) -> Cow<'static, str> {
        plural(self.items.len(), "Paste Effect", "Paste Effects")
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        if let Some(fx) = self.items.iter().find(|f| f.is_placeholder()) {
            return Err(CommandError::Invalid(format!(
                "{} is not installed and cannot be pasted",
                fx.name
            )));
        }
        for &target in &self.layers {
            let (comp, _) = doc
                .find_layer(target)
                .ok_or_else(|| CommandError::NotFound(target.to_string()))?;
            let mut copies = Vec::with_capacity(self.items.len());
            for item in &self.items {
                let mut fx = item.clone();
                renumber(doc, &mut fx);
                let cleared = keep_refs_within(doc, comp, target, &mut fx)?;
                if cleared > 0 {
                    report.warnings.push(format!(
                        "{}: {cleared} layer reference(s) pointed outside this composition and were cleared",
                        fx.name
                    ));
                }
                report.created_effects.push(fx.id);
                copies.push(fx);
            }
            let layer = layer_mut(doc, target)?;
            let at = self
                .index
                .unwrap_or(layer.effects.len())
                .min(layer.effects.len());
            layer.effects.splice(at..at, copies);
        }
        Ok(())
    }
}

/// Clears the matte and layer-reference values of `fx` that do not name
/// another layer of `comp`. Returns how many were cleared.
fn keep_refs_within(
    doc: &Document,
    comp: CompId,
    owner: LayerId,
    fx: &mut EffectInstance,
) -> Result<usize, CommandError> {
    let valid = |id: LayerId| id != owner && doc.find_layer(id).is_some_and(|(c, _)| c == comp);
    let mut cleared = 0;
    if fx.matte.is_some_and(|m| !valid(m.layer)) {
        fx.matte = None;
        cleared += 1;
    }
    for p in &mut fx.params {
        cleared += clear_refs(p, &|id| !valid(id))?;
    }
    Ok(cleared)
}

/// Sets every `Value::Layer` in `p` for which `drop` is true to `None`.
fn clear_refs(
    p: &mut crate::property::Property,
    drop: &dyn Fn(LayerId) -> bool,
) -> Result<usize, CommandError> {
    let hits = |v: &Value| matches!(v, Value::Layer(Some(id)) if drop(*id));
    let count = match p.animation() {
        crate::property::Animation::Static(v) => usize::from(hits(v)),
        crate::property::Animation::Keyframed(t) => {
            t.keys().iter().filter(|k| hits(&k.value)).count()
        }
    };
    if count > 0 {
        p.map_values(
            |v| {
                if hits(v) {
                    Value::Layer(None)
                } else {
                    v.clone()
                }
            },
            |t: SpatialTangents| t,
        )?;
    }
    Ok(count)
}

/// Clears effect mattes and layer references to `removed` layers in
/// `layers` (called by every command that deletes layers).
pub(crate) fn clear_layer_refs(layers: &mut [Arc<Layer>], removed: &[LayerId]) {
    if removed.is_empty() {
        return;
    }
    let gone = |id: LayerId| removed.contains(&id);
    for l in layers.iter_mut() {
        let refers = l.effects.iter().any(|fx| {
            fx.matte.is_some_and(|m| gone(m.layer))
                || fx.params.iter().any(|p| match p.animation() {
                    crate::property::Animation::Static(v) => {
                        matches!(v, Value::Layer(Some(id)) if gone(*id))
                    }
                    crate::property::Animation::Keyframed(t) => t
                        .keys()
                        .iter()
                        .any(|k| matches!(k.value, Value::Layer(Some(id)) if gone(id))),
                })
        });
        if !refers {
            continue;
        }
        for fx in &mut Arc::make_mut(l).effects {
            if fx.matte.is_some_and(|m| gone(m.layer)) {
                fx.matte = None;
            }
            // Placeholders keep their data verbatim; only resolved effects
            // are edited.
            for p in fx.editable_params_mut() {
                // Mapping layer references to `None` never fails validation.
                let _ = clear_refs(p, &gone);
            }
        }
    }
}

/// Sets every effect parameter that refers to a removed asset (a LUT) to
/// `None`, in every composition. Placeholders keep their data verbatim.
pub(crate) fn clear_asset_refs(doc: &mut Document, removed: &[crate::AssetId]) {
    if removed.is_empty() {
        return;
    }
    let hits = |v: &Value| matches!(v, Value::Asset(Some(a)) if removed.contains(a));
    let refers = |l: &Layer| {
        l.effects.iter().any(|fx| {
            fx.params.iter().any(|p| match p.animation() {
                crate::property::Animation::Static(v) => hits(v),
                crate::property::Animation::Keyframed(t) => t.keys().iter().any(|k| hits(&k.value)),
            })
        })
    };
    let comps: Vec<CompId> = doc
        .comps
        .values()
        .filter(|c| c.layers.iter().any(|l| refers(l)))
        .map(|c| c.id)
        .collect();
    for cid in comps {
        let Some(c) = doc.comp_mut(cid) else { continue };
        for l in c.layers.iter_mut().filter(|l| refers(l)) {
            for fx in &mut Arc::make_mut(l).effects {
                for p in fx.editable_params_mut() {
                    // Mapping asset references to `None` never fails validation.
                    let _ = p.map_values(
                        |v| {
                            if hits(v) {
                                Value::Asset(None)
                            } else {
                                v.clone()
                            }
                        },
                        |t: SpatialTangents| t,
                    );
                }
            }
        }
    }
}

/// Sets the blend mode of layers.
#[derive(Debug, Clone)]
pub struct SetBlendMode {
    pub layers: Vec<LayerId>,
    pub mode: BlendMode,
}

impl Command for SetBlendMode {
    fn label(&self) -> Cow<'static, str> {
        "Set Blend Mode".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        for &id in &self.layers {
            layer_mut(doc, id)?.blend_mode = self.mode;
        }
        Ok(())
    }
}

/// Adds an adjustment layer covering the composition. `index` 0 puts it on
/// top.
#[derive(Debug, Clone)]
pub struct AddAdjustmentLayer {
    pub comp: CompId,
    pub name: Option<String>,
    pub index: usize,
}

impl Command for AddAdjustmentLayer {
    fn label(&self) -> Cow<'static, str> {
        "Add Adjustment Layer".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        let c = doc
            .comp(self.comp)
            .ok_or_else(|| CommandError::NotFound(self.comp.to_string()))?;
        let duration = c.duration;
        let n = c
            .layers
            .iter()
            .filter(|l| matches!(l.kind, LayerKind::Adjustment))
            .count();
        let name = self
            .name
            .clone()
            .unwrap_or_else(|| format!("Adjustment Layer {}", n + 1));
        let id = doc.alloc_id();
        let transform =
            Transform::new(&mut || doc.alloc_id(), glam::DVec2::ZERO, glam::DVec2::ZERO);
        let layer = new_layer(id, name, LayerKind::Adjustment, duration, transform);
        report.created_layers.push(id);
        insert_layer(doc, self.comp, self.index, layer)
    }
}

/// Puts every parameter of an effect back to its default, static (its
/// keyframes are removed).
#[derive(Debug, Clone)]
pub struct ResetEffect {
    pub layer: LayerId,
    pub id: EffectId,
}

impl Command for ResetEffect {
    fn label(&self) -> Cow<'static, str> {
        "Reset Effect".into()
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        let layer = layer_mut(doc, self.layer)?;
        let i = effect_index(layer, self.id)?;
        if layer.effects[i].is_placeholder() {
            return Err(CommandError::Invalid(
                "a missing effect's settings cannot be changed".into(),
            ));
        }
        for p in &mut layer.effects[i].params {
            *p = crate::property::Property::with_default(p.id, p.desc.clone());
        }
        Ok(())
    }
}
