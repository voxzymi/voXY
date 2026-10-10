//! Effect instances on layers (`docs/design-effects-ofx.md` §1).
//!
//! An effect is plain data: a type ID and version that name its definition
//! in the effect registry, an enabled flag, a user-visible name, and its
//! parameters as ordinary [`Property`] values, so they keyframe, undo and
//! save exactly like every other property.
//!
//! An effect whose type this build does not know (a plugin that is not
//! installed, or a newer built-in version) is a **placeholder**: the saved
//! JSON object is kept verbatim and written back unchanged, apart from the
//! fields voXY owns (`id`, `name`, `enabled`).

use std::fmt;
use std::sync::Arc;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::id::{EffectId, LayerId, PropId};
use crate::layer::ForeignData;
use crate::property::{Animation, Property};

/// Names an effect definition: `"voxy.blur.gaussian"` for a built-in,
/// `"ofx:<pluginIdentifier>"` for an OpenFX plugin.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectTypeId(pub Arc<str>);

impl EffectTypeId {
    pub const OFX_PREFIX: &'static str = "ofx:";

    pub fn new(id: &str) -> Self {
        EffectTypeId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_ofx(&self) -> bool {
        self.0.starts_with(Self::OFX_PREFIX)
    }

    /// The prefix of every parameter's full property key:
    /// `"fx:<type>/"` for built-ins, `"ofx:<plugin>/"` for OpenFX.
    pub fn param_prefix(&self) -> String {
        if self.is_ofx() {
            format!("{}/", self.0)
        } else {
            format!("fx:{}/", self.0)
        }
    }

    /// The full property key of parameter `name`.
    pub fn param_key(&self, name: &str) -> String {
        format!("{}{name}", self.param_prefix())
    }
}

impl fmt::Display for EffectTypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which channel of the matte layer limits an effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatteChannel {
    #[default]
    Alpha,
    Luma,
}

/// Limits where an effect applies: `out = mix(in, effect(in), coverage)`,
/// with the coverage taken from another layer (design §1.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EffectMatte {
    pub layer: LayerId,
    #[serde(default)]
    pub channel: MatteChannel,
    #[serde(default)]
    pub invert: bool,
}

/// The OpenFX context an instance was created in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfxContext {
    Filter,
    Generator,
    Transition,
    General,
}

/// What an OpenFX effect remembers about its plugin (design §6.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfxInstanceData {
    /// The plugin's `pluginIdentifier`.
    pub plugin: Arc<str>,
    /// Plugin version `[major, minor]`.
    pub version: [u32; 2],
    pub context: OfxContext,
    /// File name of the bundle it came from; a hint for the user only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
}

/// Why an effect is a placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingReason {
    /// No definition with this type ID is installed.
    UnknownType,
    /// The file was written by a newer version of the definition.
    NewerVersion { known: u32 },
    /// The saved parameters could not be read by this build.
    Unreadable(String),
}

impl fmt::Display for MissingReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MissingReason::UnknownType => f.write_str("this effect is not installed"),
            MissingReason::NewerVersion { known } => write!(
                f,
                "this effect was saved by a newer version (this build knows version {known})"
            ),
            MissingReason::Unreadable(e) => write!(f, "its settings could not be read: {e}"),
        }
    }
}

/// The preserved data of a placeholder effect.
#[derive(Debug, Clone, PartialEq)]
pub struct Placeholder {
    /// The effect's JSON object exactly as loaded.
    pub raw: Arc<serde_json::Map<String, serde_json::Value>>,
    pub reason: MissingReason,
    /// Property IDs used inside `raw` that are not in
    /// [`EffectInstance::params`], so they stay unique in the document.
    pub reserved: Vec<PropId>,
}

/// One effect in a layer's stack.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectInstance {
    pub id: EffectId,
    pub type_id: EffectTypeId,
    /// Definition version (built-ins), or the plugin's major version (OFX).
    pub version: u32,
    pub name: String,
    pub enabled: bool,
    /// In definition order. Each key is `type_id.param_key(<name>)`.
    pub params: Vec<Property>,
    pub matte: Option<EffectMatte>,
    pub ofx: Option<OfxInstanceData>,
    /// Fields voXY does not know, written back unchanged.
    pub extra: ForeignData,
    /// `Some` for an effect this build cannot run (design §1.1).
    pub placeholder: Option<Placeholder>,
}

impl EffectInstance {
    pub fn is_placeholder(&self) -> bool {
        self.placeholder.is_some()
    }

    /// The parameter's own name (the key without the type prefix).
    pub fn param_name<'a>(&self, p: &'a Property) -> &'a str {
        let prefix = self.type_id.param_prefix();
        p.key().strip_prefix(prefix.as_str()).unwrap_or(p.key())
    }

    pub fn param(&self, name: &str) -> Option<&Property> {
        let key = self.type_id.param_key(name);
        self.params.iter().find(|p| p.key() == key)
    }

    pub fn param_mut(&mut self, name: &str) -> Option<&mut Property> {
        let key = self.type_id.param_key(name);
        self.params.iter_mut().find(|p| p.key() == key)
    }

    /// Parameters that may be edited: none for a placeholder.
    pub fn editable_params(&self) -> &[Property] {
        if self.is_placeholder() {
            &[]
        } else {
            &self.params
        }
    }

    pub fn editable_params_mut(&mut self) -> &mut [Property] {
        if self.is_placeholder() {
            &mut []
        } else {
            &mut self.params
        }
    }

    /// Every document ID this effect uses: its own, its parameters' and
    /// those reserved by a placeholder.
    pub fn ids(&self) -> impl Iterator<Item = u64> + '_ {
        std::iter::once(self.id.raw())
            .chain(self.params.iter().map(|p| p.id.raw()))
            .chain(
                self.placeholder
                    .iter()
                    .flat_map(|ph| ph.reserved.iter().map(|id| id.raw())),
            )
    }

    /// Gives the effect and every ID it uses fresh values from `alloc`, also
    /// inside a placeholder's preserved JSON, so a copy never shares an ID
    /// with its original.
    pub fn renumber(&mut self, alloc: &mut dyn FnMut() -> u64) {
        self.id = EffectId(alloc());
        let mut map = std::collections::HashMap::new();
        for p in &mut self.params {
            let new = PropId(alloc());
            map.insert(p.id.raw(), new.raw());
            p.id = new;
        }
        if let Some(ph) = &mut self.placeholder {
            for id in &mut ph.reserved {
                let new = PropId(alloc());
                map.insert(id.raw(), new.raw());
                *id = new;
            }
            let raw = Arc::make_mut(&mut ph.raw);
            raw.insert("id".into(), self.id.raw().into());
            if let Some(params) = raw.get_mut("params").and_then(|p| p.as_array_mut()) {
                for p in params {
                    if let Some(slot) = p.get_mut("id")
                        && let Some(new) = slot.as_u64().and_then(|old| map.get(&old))
                    {
                        *slot = (*new).into();
                    }
                }
            }
        }
    }

    /// Turns this effect into a placeholder for `reason`, keeping its data
    /// (as it would be saved) verbatim.
    pub fn mark_missing(&mut self, reason: MissingReason) {
        if let Some(ph) = &mut self.placeholder {
            ph.reason = reason;
            return;
        }
        // Through text, not `to_value`: f32 values (colours) are written in
        // their shortest form, exactly as in a saved file, instead of being
        // widened to f64 digits.
        let raw = serde_json::to_string(&self.to_repr())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        // The parameters stay (read-only), so their IDs are already counted.
        self.placeholder = Some(Placeholder {
            raw: Arc::new(raw),
            reason,
            reserved: Vec::new(),
        });
    }

    fn to_repr(&self) -> EffectRepr {
        let prefix = self.type_id.param_prefix();
        EffectRepr {
            id: self.id,
            type_id: self.type_id.clone(),
            version: self.version,
            name: self.name.clone(),
            enabled: self.enabled,
            params: self
                .params
                .iter()
                .map(|p| ParamRepr {
                    key: p
                        .key()
                        .strip_prefix(prefix.as_str())
                        .unwrap_or(p.key())
                        .into(),
                    id: p.id,
                    anim: p.animation().clone(),
                })
                .collect(),
            matte: self.matte,
            ofx: self.ofx.clone(),
            extra: self.extra.clone(),
        }
    }
}

/// On-disk parameter: the key is the parameter's own name.
#[derive(Serialize, Deserialize)]
struct ParamRepr {
    key: Arc<str>,
    id: PropId,
    #[serde(flatten)]
    anim: Animation,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct EffectRepr {
    id: EffectId,
    #[serde(rename = "type")]
    type_id: EffectTypeId,
    version: u32,
    name: String,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    params: Vec<ParamRepr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    matte: Option<EffectMatte>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ofx: Option<OfxInstanceData>,
    #[serde(flatten)]
    extra: ForeignData,
}

/// The fields every effect must have, even one this build cannot read.
#[derive(Deserialize)]
struct EffectHeader {
    id: EffectId,
    #[serde(rename = "type")]
    type_id: EffectTypeId,
    version: u32,
    name: String,
    #[serde(default = "yes")]
    enabled: bool,
}

impl Serialize for EffectInstance {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.placeholder {
            Some(ph) => {
                let mut raw = (*ph.raw).clone();
                raw.insert("id".into(), self.id.raw().into());
                raw.insert("name".into(), self.name.clone().into());
                raw.insert("enabled".into(), self.enabled.into());
                raw.serialize(s)
            }
            None => self.to_repr().serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for EffectInstance {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let serde_json::Value::Object(map) = value else {
            return Err(D::Error::custom("an effect must be a JSON object"));
        };
        let header: EffectHeader = serde_json::from_value(serde_json::Value::Object(map.clone()))
            .map_err(D::Error::custom)?;
        match parse_full(&map) {
            Ok(effect) => Ok(effect),
            Err(e) => Ok(EffectInstance {
                id: header.id,
                type_id: header.type_id,
                version: header.version,
                name: header.name,
                enabled: header.enabled,
                params: Vec::new(),
                matte: None,
                ofx: None,
                extra: ForeignData::default(),
                placeholder: Some(Placeholder {
                    reserved: reserved_ids(&map),
                    raw: Arc::new(map),
                    reason: MissingReason::Unreadable(e),
                }),
            }),
        }
    }
}

impl EffectInstance {
    /// Re-reads a placeholder's preserved JSON, keeping the fields voXY owns
    /// (`id`, `name`, `enabled`) from `self`. `None` if it is not a
    /// placeholder or its data still cannot be read.
    pub(crate) fn reparse_placeholder(&self) -> Option<EffectInstance> {
        let ph = self.placeholder.as_ref()?;
        let mut fx = parse_full(&ph.raw).ok()?;
        fx.id = self.id;
        fx.name.clone_from(&self.name);
        fx.enabled = self.enabled;
        Some(fx)
    }
}

/// Reads a whole effect. Parameters get descriptors for unknown keys; the
/// registry supplies the real ones when the project is resolved.
fn parse_full(map: &serde_json::Map<String, serde_json::Value>) -> Result<EffectInstance, String> {
    let repr: EffectRepr = serde_json::from_value(serde_json::Value::Object(map.clone()))
        .map_err(|e| e.to_string())?;
    let mut params = Vec::with_capacity(repr.params.len());
    for p in repr.params {
        let key = repr.type_id.param_key(&p.key);
        params.push(Property::from_saved(p.id, &key, p.anim, None).map_err(|e| e.to_string())?);
    }
    Ok(EffectInstance {
        id: repr.id,
        type_id: repr.type_id,
        version: repr.version,
        name: repr.name,
        enabled: repr.enabled,
        params,
        matte: repr.matte,
        ofx: repr.ofx,
        extra: repr.extra,
        placeholder: None,
    })
}

/// Parameter IDs inside an effect object that could not be read, so that
/// they are never handed out again.
fn reserved_ids(map: &serde_json::Map<String, serde_json::Value>) -> Vec<PropId> {
    map.get("params")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("id")?.as_u64())
        .map(PropId)
        .collect()
}
