//! Dropping project items (or files) onto a timeline, the viewport, a layer
//! or "nowhere" (a new composition) — `docs/design-media-comps.md` §5.
//!
//! [`resolve_drop`] is a pure function. The drag preview and the
//! [`DropItems`] command both call it, so the preview always matches the
//! result and the rules are tested without a window.
//!
//! | Rule | Behavior |
//! |---|---|
//! | Several items | **Stack**: all start at the drop time, the first on top. With `sequence`, each starts where the previous one ends. |
//! | Length | The source's own duration (not trimmed at the composition end); a still uses the default still duration or lasts to the end; never under one frame. |
//! | Layer index | Inserted at the drop index; the first item is the topmost of the group. |
//! | Drop time | Snapped to the composition's frame grid and kept inside it. |
//! | Viewport | Centered at the drop point, on top, at the drop time. |
//! | Replace | Uses the first item only (and says so). |
//! | New composition | Size, frame rate and duration come from the chosen item; stills keep the project defaults. One composition, or one per item. |
//! | Cycles | A composition that would contain itself blocks the drop with the reason. |

use std::borrow::Cow;

use glam::DVec2;

use super::pool::{
    check_nesting, insert_source_layer, numbered_name, replace_layer_source, source_duration,
    source_exists, source_layer_length, validate_name,
};
use super::settings::{nearest_frame_time, validate_comp_settings};
use super::{Command, CommandError, CommandReport, comp_mut};
use crate::asset::{Asset, AssetKind};
use crate::document::{CompSettings, Document};
use crate::id::{AssetId, CompId, FolderId, LayerId};
use crate::layer::SourceRef;
use crate::time::{FrameRate, Time};

/// One thing being dropped.
#[derive(Debug, Clone)]
pub enum DropItem {
    /// Footage already in the project.
    Asset(AssetId),
    /// A composition (becomes a precomp layer).
    Comp(CompId),
    /// A file that is not in the project yet, already probed by the caller
    /// (core does no I/O). The command imports it as part of the drop.
    File(Asset),
}

/// Where the items land.
#[derive(Debug, Clone)]
pub enum DropTarget {
    /// On a timeline at `time`, inserted at layer `index` (0 is the top).
    Timeline {
        comp: CompId,
        time: Time,
        index: usize,
    },
    /// On the viewport: centered at `pos` (composition pixels), on top.
    Viewport {
        comp: CompId,
        time: Time,
        pos: DVec2,
    },
    /// Onto a layer, to replace its source.
    ReplaceLayer { layer: LayerId },
    /// Onto nothing: make a composition from the items. With `per_item`,
    /// one composition per item with its own settings; otherwise a single
    /// one whose settings come from item `source`.
    NewComp { per_item: bool, source: usize },
}

/// Modifier-dependent options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Placement {
    /// Ctrl/Cmd held: each item starts where the previous one ends.
    pub sequence: bool,
}

/// A layer the drop will add.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlannedLayer {
    /// Index into the dropped items.
    pub item: usize,
    pub in_point: Time,
    pub length: Time,
    /// Where it is inserted in the composition's stack.
    pub index: usize,
    /// Where the layer's center goes, if not the composition's center.
    pub position: Option<DVec2>,
}

/// A composition the drop will create, with the layers it starts with.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedComp {
    pub settings: CompSettings,
    pub layers: Vec<PlannedLayer>,
}

/// What a drop would do.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Refused, with the reason to show (the cursor becomes "not allowed").
    Blocked(String),
    Layers {
        comp: CompId,
        layers: Vec<PlannedLayer>,
        warnings: Vec<String>,
    },
    Replace {
        layer: LayerId,
        item: usize,
        warnings: Vec<String>,
    },
    NewComps {
        comps: Vec<PlannedComp>,
        warnings: Vec<String>,
    },
}

/// What the planner needs to know about an item, whether or not it is in the
/// project yet.
struct Info {
    source: Option<SourceRef>,
    name: String,
    is_asset: bool,
    size: Option<[u32; 2]>,
    duration: Option<Time>,
    frame_rate: Option<FrameRate>,
}

fn info_of(doc: &Document, item: &DropItem) -> Result<Info, String> {
    match item {
        DropItem::Asset(id) => {
            let s = SourceRef::Asset(*id);
            let a = doc
                .assets
                .get(*id)
                .ok_or_else(|| "an item is no longer in the project".to_owned())?;
            Ok(Info {
                source: Some(s),
                name: a.name.clone(),
                is_asset: true,
                size: a.pixel_size,
                duration: source_duration(doc, s),
                frame_rate: a.frame_rate,
            })
        }
        DropItem::Comp(id) => {
            let c = doc
                .comp(*id)
                .ok_or_else(|| "an item is no longer in the project".to_owned())?;
            Ok(Info {
                source: Some(SourceRef::Comp(*id)),
                name: c.name.clone(),
                is_asset: false,
                size: Some([c.width, c.height]),
                duration: Some(c.duration),
                frame_rate: Some(c.frame_rate),
            })
        }
        DropItem::File(a) => Ok(Info {
            source: None,
            name: a.name.clone(),
            is_asset: true,
            size: a.pixel_size,
            duration: a
                .duration
                .filter(|d| a.kind == AssetKind::Video && *d > Time::ZERO),
            frame_rate: a.frame_rate,
        }),
    }
}

/// `"clip.mov"` becomes `"clip"`.
fn stem(name: &str) -> &str {
    std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
}

fn settings_for(
    info: &Info,
    defaults: &CompSettings,
    name: String,
) -> Result<CompSettings, String> {
    let Some([w, h]) = info.size.filter(|s| s[0] > 0 && s[1] > 0) else {
        return Err(format!("{}: its size is unknown", info.name));
    };
    Ok(CompSettings {
        name,
        width: w,
        height: h,
        frame_rate: info.frame_rate.unwrap_or(defaults.frame_rate),
        duration: info.duration.unwrap_or(defaults.duration),
        ..defaults.clone()
    })
}

/// How layers are laid out in one composition.
#[derive(Clone, Copy)]
struct Layout {
    /// The composition's duration and frame rate.
    duration: Time,
    rate: FrameRate,
    default_still: Option<Time>,
    /// Stack index of the first item.
    base: usize,
    /// Where the group starts.
    at: Time,
    position: Option<DVec2>,
    sequence: bool,
}

/// Lays `infos` out as layers of a composition: stacked at `at`, or in
/// sequence.
fn layout(infos: &[(usize, &Info)], l: &Layout) -> Vec<PlannedLayer> {
    let Layout {
        duration,
        rate,
        default_still,
        base,
        at,
        position,
        sequence,
    } = *l;
    let frame = rate.frame_to_time(1);
    let mut cursor = at;
    infos
        .iter()
        .enumerate()
        .map(|(k, (item, info))| {
            let in_point = if sequence { cursor } else { at };
            let length = source_layer_length(
                info.duration,
                info.is_asset,
                default_still,
                duration - in_point,
                frame,
            );
            cursor = in_point + length;
            PlannedLayer {
                item: *item,
                in_point,
                length,
                index: base + k,
                position,
            }
        })
        .collect()
}

/// Decides what dropping `items` on `target` would do. Pure: it reads the
/// document and changes nothing.
#[must_use]
pub fn resolve_drop(
    doc: &Document,
    target: &DropTarget,
    items: &[DropItem],
    placement: Placement,
) -> Plan {
    if items.is_empty() {
        return Plan::Blocked("Nothing to drop".into());
    }
    let mut infos = Vec::with_capacity(items.len());
    for item in items {
        match info_of(doc, item) {
            Ok(i) => infos.push(i),
            Err(why) => return Plan::Blocked(why),
        }
    }
    let default_still = doc.meta.settings.default_still_duration;
    match target {
        DropTarget::Timeline { comp, .. } | DropTarget::Viewport { comp, .. } => {
            let Some(c) = doc.comp(*comp) else {
                return Plan::Blocked("That composition is not open".into());
            };
            for info in &infos {
                if let Some(SourceRef::Comp(inner)) = info.source
                    && let Some(why) = doc.nesting_error(*comp, inner)
                {
                    return Plan::Blocked(why);
                }
            }
            let (time, base, position) = match target {
                DropTarget::Timeline { time, index, .. } => {
                    (*time, (*index).min(c.layers.len()), None)
                }
                DropTarget::Viewport { time, pos, .. } => (*time, 0, Some(*pos)),
                _ => unreachable!("matched above"),
            };
            let frame = c.frame_rate.frame_to_time(1);
            let latest = (c.duration - frame).max(Time::ZERO);
            let at = nearest_frame_time(c.frame_rate, time.max(Time::ZERO)).min(latest);
            let pairs: Vec<(usize, &Info)> = infos.iter().enumerate().collect();
            Plan::Layers {
                comp: *comp,
                layers: layout(
                    &pairs,
                    &Layout {
                        duration: c.duration,
                        rate: c.frame_rate,
                        default_still,
                        base,
                        at,
                        position,
                        sequence: placement.sequence,
                    },
                ),
                warnings: Vec::new(),
            }
        }
        DropTarget::ReplaceLayer { layer } => {
            let Some((comp, _)) = doc.find_layer(*layer) else {
                return Plan::Blocked("That layer no longer exists".into());
            };
            if let Some(SourceRef::Comp(inner)) = infos[0].source
                && let Some(why) = doc.nesting_error(comp, inner)
            {
                return Plan::Blocked(why);
            }
            let warnings = if items.len() > 1 {
                vec!["Only the first item was used to replace the layer.".to_owned()]
            } else {
                Vec::new()
            };
            Plan::Replace {
                layer: *layer,
                item: 0,
                warnings,
            }
        }
        DropTarget::NewComp { per_item, source } => {
            let defaults = &doc.meta.settings.new_comp;
            let mut taken: Vec<String> = doc.comps.values().map(|c| c.name.clone()).collect();
            let mut name_for = |info: &Info| {
                let base = validate_name(stem(&info.name)).unwrap_or_else(|_| "Comp".to_owned());
                let name = if taken.contains(&base) {
                    numbered_name(&base, &|n| taken.iter().any(|t| t == n))
                } else {
                    base
                };
                taken.push(name.clone());
                name
            };
            let groups: Vec<(usize, Vec<usize>)> = if *per_item {
                (0..infos.len()).map(|i| (i, vec![i])).collect()
            } else {
                vec![((*source).min(infos.len() - 1), (0..infos.len()).collect())]
            };
            let mut comps = Vec::new();
            for (from, members) in groups {
                let settings = match settings_for(&infos[from], defaults, name_for(&infos[from])) {
                    Ok(s) => s,
                    Err(why) => return Plan::Blocked(why),
                };
                if let Err(e) = validate_comp_settings(&settings) {
                    return Plan::Blocked(e.to_string());
                }
                let pairs: Vec<(usize, &Info)> = members.iter().map(|&i| (i, &infos[i])).collect();
                let layers = layout(
                    &pairs,
                    &Layout {
                        duration: settings.duration,
                        rate: settings.frame_rate,
                        default_still,
                        base: 0,
                        at: Time::ZERO,
                        position: None,
                        sequence: placement.sequence,
                    },
                );
                comps.push(PlannedComp { settings, layers });
            }
            Plan::NewComps {
                comps,
                warnings: Vec::new(),
            }
        }
    }
}

/// Drops items: imports any files, then does what [`resolve_drop`] planned,
/// as one undo step. Nothing changes if the drop is blocked or anything
/// fails.
#[derive(Debug, Clone)]
pub struct DropItems {
    pub target: DropTarget,
    pub items: Vec<DropItem>,
    pub placement: Placement,
    /// Where imported files and new compositions go in the project.
    pub folder: Option<FolderId>,
}

impl Command for DropItems {
    fn label(&self) -> Cow<'static, str> {
        match self.target {
            DropTarget::NewComp { .. } => "New Composition From Footage".into(),
            DropTarget::ReplaceLayer { .. } => "Replace Layer Source".into(),
            _ => "Drop".into(),
        }
    }

    fn apply(&self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply_reported(doc, &mut CommandReport::default())
    }

    fn apply_reported(
        &self,
        doc: &mut Document,
        report: &mut CommandReport,
    ) -> Result<(), CommandError> {
        if let Some(f) = self.folder {
            doc.folders
                .contains_key(&f)
                .then_some(())
                .ok_or_else(|| CommandError::NotFound(f.to_string()))?;
        }
        let plan = resolve_drop(doc, &self.target, &self.items, self.placement);
        if let Plan::Blocked(why) = &plan {
            return Err(CommandError::Invalid(why.clone()));
        }
        // Import the files first, so every item has a source.
        let mut sources: Vec<SourceRef> = Vec::with_capacity(self.items.len());
        for item in &self.items {
            sources.push(match item {
                DropItem::Asset(a) => SourceRef::Asset(*a),
                DropItem::Comp(c) => SourceRef::Comp(*c),
                DropItem::File(asset) => {
                    if asset.path.trim().is_empty() {
                        return Err(CommandError::Invalid("a media path cannot be empty".into()));
                    }
                    let mut asset = asset.clone();
                    asset.name = validate_name(&asset.name)?;
                    asset.folder = self.folder;
                    let id = doc.add_asset(asset);
                    report.created_assets.push(id);
                    SourceRef::Asset(id)
                }
            });
        }
        for &s in &sources {
            source_exists(doc, s)?;
        }
        match plan {
            Plan::Blocked(_) => unreachable!("returned above"),
            Plan::Layers {
                comp,
                layers,
                warnings,
            } => {
                for l in layers {
                    let source = sources[l.item];
                    check_nesting(doc, comp, source)?;
                    let id = insert_source_layer(
                        doc, comp, source, l.index, l.in_point, l.length, l.position,
                    )?;
                    report.created_layers.push(id);
                }
                report.warnings.extend(warnings);
            }
            Plan::Replace {
                layer,
                item,
                warnings,
            } => {
                replace_layer_source(doc, layer, sources[item])?;
                report.warnings.extend(warnings);
            }
            Plan::NewComps { comps, warnings } => {
                for planned in comps {
                    let id = doc.add_comp(planned.settings);
                    comp_mut(doc, id)?.folder = self.folder;
                    if doc.main_comp.is_none() {
                        doc.main_comp = Some(id);
                    }
                    report.created_comps.push(id);
                    for l in planned.layers {
                        let layer_id = insert_source_layer(
                            doc,
                            id,
                            sources[l.item],
                            l.index,
                            l.in_point,
                            l.length,
                            l.position,
                        )?;
                        report.created_layers.push(layer_id);
                    }
                }
                report.warnings.extend(warnings);
            }
        }
        Ok(())
    }
}
