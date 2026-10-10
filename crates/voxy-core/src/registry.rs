//! The effect registry: every effect definition this build can run
//! (`docs/design-effects-ofx.md` §1.3).
//!
//! Built-in and OpenFX definitions register into the same registry. It is
//! plain data, kept outside the [`Document`], and passed to whatever needs
//! it (loading, evaluation, planning, the UI). Several versions of one type
//! may be registered, so older projects keep their exact definition.

mod builtin;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::document::Document;
use crate::effect::{EffectInstance, EffectTypeId};
use crate::id::EffectId;
use crate::layer::ForeignData;
use crate::property::{Property, PropertyDesc, Unit};
use crate::value::Value;

pub use builtin::{builtin_definitions, ids};

/// Where an effect appears in the Effects & Presets panel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Blur,
    Stylize,
    Color,
    Keying,
    Distort,
    Transition,
    Generate,
    /// An OpenFX plugin's own grouping, e.g. `"Filter/Color"`.
    Plugin(Arc<str>),
}

impl Category {
    pub fn label(&self) -> &str {
        match self {
            Category::Blur => "Blur",
            Category::Stylize => "Stylize",
            Category::Color => "Color",
            Category::Keying => "Keying and Matte",
            Category::Distort => "Distort",
            Category::Transition => "Transition",
            Category::Generate => "Generate",
            Category::Plugin(group) => group,
        }
    }
}

/// The colour encoding an effect computes in (design §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EffectSpace {
    /// Whatever the project's working space holds, like blending.
    #[default]
    Working,
    /// Always linear light: decoded before, re-encoded after.
    Linear,
}

/// An on-canvas handle for a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GizmoKind {
    /// A draggable point (a `Vec2` in layer pixels).
    Point,
}

/// Whether a parameter is shown and editable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ParamVisibility {
    #[default]
    Editable,
    /// Shown but not editable (for example an unsupported OpenFX type).
    ReadOnly,
    /// Not shown at all (for example plugin state).
    Hidden,
}

/// UI flags of a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ParamFlags {
    /// Show a vector parameter's components as integers.
    pub integer_display: bool,
    /// Show a colour without its alpha.
    pub hide_alpha: bool,
    pub visibility: ParamVisibility,
}

/// A collapsible group of parameters in Effect Controls.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParamGroup {
    pub id: u32,
    pub label: Arc<str>,
    pub parent: Option<u32>,
    pub open_by_default: bool,
}

/// One parameter of an effect definition.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectParamDef {
    /// The parameter's own name, the key used on disk.
    pub name: Arc<str>,
    /// The descriptor shared by every instance; its key is
    /// `type_id.param_key(name)`.
    pub desc: Arc<PropertyDesc>,
    /// Labels of an enum parameter, by index.
    pub choices: Option<Arc<[Arc<str>]>>,
    pub group: Option<u32>,
    pub gizmo: Option<GizmoKind>,
    pub hint: Option<Arc<str>>,
    pub flags: ParamFlags,
}

/// Where a definition comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EffectOrigin {
    Builtin,
    /// Plugin number `index` in the OpenFX binary at `binary`.
    Ofx {
        binary: PathBuf,
        index: u32,
    },
}

/// A complete effect definition.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectDef {
    pub type_id: EffectTypeId,
    pub version: u32,
    pub name: Arc<str>,
    pub category: Category,
    pub description: Arc<str>,
    pub params: Arc<[EffectParamDef]>,
    pub groups: Arc<[ParamGroup]>,
    pub space: EffectSpace,
    /// Uses the deterministic seed (layer, seed parameter, frame), §3.4.
    pub seeded: bool,
    pub origin: EffectOrigin,
}

impl EffectDef {
    pub fn param(&self, name: &str) -> Option<&EffectParamDef> {
        self.params.iter().find(|p| &*p.name == name)
    }

    /// A new instance with every parameter at its default and fresh IDs.
    pub fn instantiate(&self, doc: &mut Document) -> EffectInstance {
        let id: EffectId = doc.alloc_id();
        let params = self
            .params
            .iter()
            .map(|p| Property::with_default(doc.alloc_id(), p.desc.clone()))
            .collect();
        EffectInstance {
            id,
            type_id: self.type_id.clone(),
            version: self.version,
            name: self.name.to_string(),
            enabled: true,
            params,
            matte: None,
            ofx: None,
            extra: ForeignData::default(),
            placeholder: None,
        }
    }

    /// Checks the invariants [`EffectRegistry::register`] relies on.
    fn check(&self) -> Result<(), RegistryError> {
        let bad = |why: String| RegistryError::Invalid {
            type_id: self.type_id.clone(),
            why,
        };
        if self.type_id.as_str().is_empty() {
            return Err(bad("empty type ID".into()));
        }
        let mut names = std::collections::HashSet::new();
        for p in self.params.iter() {
            if p.name.is_empty() || !names.insert(&*p.name) {
                return Err(bad(format!("parameter `{}` is empty or repeated", p.name)));
            }
            if *p.desc.key != self.type_id.param_key(&p.name) {
                return Err(bad(format!(
                    "parameter `{}` has key `{}`",
                    p.name, p.desc.key
                )));
            }
            if p.desc.kind != p.desc.default.kind() || !p.desc.default.is_finite() {
                return Err(bad(format!("parameter `{}` has a bad default", p.name)));
            }
            if let (Some(choices), Value::Enum(i)) = (&p.choices, &p.desc.default)
                && *i as usize >= choices.len()
            {
                return Err(bad(format!(
                    "parameter `{}` defaults past its choices",
                    p.name
                )));
            }
            if p.group
                .is_some_and(|g| !self.groups.iter().any(|grp| grp.id == g))
            {
                return Err(bad(format!("parameter `{}` is in a missing group", p.name)));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RegistryError {
    #[error("{type_id} version {version} is already registered")]
    Duplicate { type_id: EffectTypeId, version: u32 },
    #[error("definition {type_id} is invalid: {why}")]
    Invalid { type_id: EffectTypeId, why: String },
}

/// All effect definitions, by type and version.
#[derive(Debug, Clone, Default)]
pub struct EffectRegistry {
    by_type: BTreeMap<EffectTypeId, Vec<Arc<EffectDef>>>,
}

impl EffectRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry holding every built-in effect.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        for def in builtin_definitions() {
            // Built-ins are checked by tests; a bad one is a programming error.
            r.register(def)
                .unwrap_or_else(|e| panic!("built-in effect: {e}"));
        }
        r
    }

    pub fn register(&mut self, def: EffectDef) -> Result<(), RegistryError> {
        def.check()?;
        let versions = self.by_type.entry(def.type_id.clone()).or_default();
        if versions.iter().any(|d| d.version == def.version) {
            return Err(RegistryError::Duplicate {
                type_id: def.type_id,
                version: def.version,
            });
        }
        versions.push(Arc::new(def));
        versions.sort_by_key(|d| d.version);
        Ok(())
    }

    /// The definition of exactly this version.
    pub fn get(&self, type_id: &EffectTypeId, version: u32) -> Option<&Arc<EffectDef>> {
        self.by_type
            .get(type_id)?
            .iter()
            .find(|d| d.version == version)
    }

    /// The newest registered version of a type.
    pub fn latest(&self, type_id: &EffectTypeId) -> Option<&Arc<EffectDef>> {
        self.by_type.get(type_id)?.last()
    }

    /// The newest version of every type, in type-ID order.
    pub fn latest_all(&self) -> impl Iterator<Item = &Arc<EffectDef>> {
        self.by_type.values().filter_map(|v| v.last())
    }

    pub fn len(&self) -> usize {
        self.by_type.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_type.is_empty()
    }
}

/// Builds parameter definitions tersely (used by the built-ins and tests).
pub(crate) struct ParamBuilder {
    type_id: EffectTypeId,
    params: Vec<EffectParamDef>,
}

impl ParamBuilder {
    pub(crate) fn new(type_id: &EffectTypeId) -> Self {
        ParamBuilder {
            type_id: type_id.clone(),
            params: Vec::new(),
        }
    }

    /// Adds a parameter and returns it for further settings.
    pub(crate) fn add(
        &mut self,
        name: &str,
        label: &str,
        default: Value,
        unit: Unit,
    ) -> &mut EffectParamDef {
        self.params.push(EffectParamDef {
            name: name.into(),
            desc: Arc::new(PropertyDesc {
                key: self.type_id.param_key(name).into(),
                label: label.into(),
                kind: default.kind(),
                default,
                range: None,
                soft_range: None,
                unit,
                animatable: true,
                spatial: false,
            }),
            choices: None,
            group: None,
            gizmo: None,
            hint: None,
            flags: ParamFlags::default(),
        });
        self.params.last_mut().unwrap_or_else(|| unreachable!())
    }

    pub(crate) fn finish(self) -> Arc<[EffectParamDef]> {
        self.params.into()
    }
}

impl EffectParamDef {
    fn desc_mut(&mut self) -> &mut PropertyDesc {
        Arc::make_mut(&mut self.desc)
    }

    /// Hard range (values are clamped into it).
    pub(crate) fn range(&mut self, lo: f64, hi: f64) -> &mut Self {
        self.desc_mut().range = Some((lo, hi));
        self
    }

    /// Suggested slider range.
    pub(crate) fn soft(&mut self, lo: f64, hi: f64) -> &mut Self {
        self.desc_mut().soft_range = Some((lo, hi));
        self
    }

    /// Enum labels; also sets the hard range to the valid indices.
    pub(crate) fn choices(&mut self, labels: &[&str]) -> &mut Self {
        self.choices = Some(labels.iter().map(|&l| Arc::from(l)).collect());
        let last = labels.len().saturating_sub(1) as f64;
        self.desc_mut().range = Some((0.0, last));
        self
    }

    pub(crate) fn hint(&mut self, text: &str) -> &mut Self {
        self.hint = Some(text.into());
        self
    }

    pub(crate) fn hide_alpha(&mut self) -> &mut Self {
        self.flags.hide_alpha = true;
        self
    }
}
