//! OpenFX plugins as voXY effects (design §6.6, §6.8): a described
//! context becomes an [`EffectDef`], and parameter values cross between
//! voXY's [`Value`]s and the plugin's [`ParamValue`]s.
//!
//! Mapping choices are listed in `docs/ofx-host.md`.

use std::path::Path;
use std::sync::Arc;

use voxy_core::external::{AUTO_PARAM, CLIP_PARAM_PREFIX, RESERVED_PREFIX};
use voxy_core::glam::{DVec2, DVec3};
use voxy_core::registry::{
    Category, EffectDef, EffectOrigin, EffectParamDef, EffectSpace, ParamFlags, ParamGroup,
    ParamVisibility,
};
use voxy_core::{EffectTypeId, LinearRgba, PropertyDesc, Unit, Value, ValueKind};
use voxy_ofx_sys as ofx;

use crate::params::{ParamKind, ParamValue};
use crate::props::ofx_str;
use crate::protocol::{ContextDesc, ParamDesc, PluginDesc, Prop, PropList};

/// The voXY type ID of a plugin: `ofx:<pluginIdentifier>`.
pub fn type_id(identifier: &str) -> EffectTypeId {
    EffectTypeId::new(&format!("{}{identifier}", EffectTypeId::OFX_PREFIX))
}

fn ints(p: &PropList, name: &[u8]) -> Vec<i32> {
    match p.get(ofx_str(name)) {
        Some(Prop::Int(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn doubles(p: &PropList, name: &[u8]) -> Vec<f64> {
    match p.get(ofx_str(name)) {
        Some(Prop::Double(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn strings(p: &PropList, name: &[u8]) -> Vec<String> {
    match p.get(ofx_str(name)) {
        Some(Prop::String(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn string(p: &PropList, name: &[u8]) -> String {
    strings(p, name).into_iter().next().unwrap_or_default()
}

fn flag(p: &PropList, name: &[u8]) -> bool {
    ints(p, name).first().copied().unwrap_or(0) != 0
}

/// A range from `min`/`max`, unless it is the "no limit" default.
fn range(lo: &[f64], hi: &[f64]) -> Option<(f64, f64)> {
    let lo = lo.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = hi.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let unlimited =
        |v: f64| !v.is_finite() || v.abs() >= f64::MAX / 2.0 || v.abs() >= f64::from(i32::MAX);
    (lo < hi && !(unlimited(lo) && unlimited(hi))).then(|| {
        (
            if unlimited(lo) { f64::MIN } else { lo },
            if unlimited(hi) { f64::MAX } else { hi },
        )
    })
}

fn numeric_range(p: &PropList, lo: &[u8], hi: &[u8]) -> Option<(f64, f64)> {
    let as_f = |n: &[u8]| {
        let d = doubles(p, n);
        if d.is_empty() {
            ints(p, n).into_iter().map(f64::from).collect()
        } else {
            d
        }
    };
    range(&as_f(lo), &as_f(hi))
}

/// The unit a double parameter's type implies.
fn double_unit(p: &PropList) -> Unit {
    if string(p, ofx::kOfxParamPropDefaultCoordinateSystem)
        == ofx_str(ofx::kOfxParamCoordinatesNormalised)
    {
        return Unit::Normalized;
    }
    let t = string(p, ofx::kOfxParamPropDoubleType);
    let is = |c: &[u8]| t == ofx_str(c);
    if is(ofx::kOfxParamDoubleTypeAngle) {
        Unit::Degrees
    } else if is(ofx::kOfxParamDoubleTypeScale) {
        Unit::Factor
    } else if [
        ofx::kOfxParamDoubleTypeX.as_slice(),
        ofx::kOfxParamDoubleTypeY,
        ofx::kOfxParamDoubleTypeXAbsolute,
        ofx::kOfxParamDoubleTypeYAbsolute,
        ofx::kOfxParamDoubleTypeXY,
        ofx::kOfxParamDoubleTypeXYAbsolute,
    ]
    .iter()
    .any(|c| is(c))
    {
        Unit::Pixels
    } else {
        Unit::None
    }
}

/// The parameter's default as a plugin value.
fn default_param_value(d: &ParamDesc) -> ParamValue {
    let p = &d.props;
    let def = ofx::kOfxParamPropDefault;
    match d.kind {
        ParamKind::Integer
        | ParamKind::Boolean
        | ParamKind::Choice
        | ParamKind::Integer2D
        | ParamKind::Integer3D => ParamValue::Ints(ints(p, def)),
        ParamKind::Double
        | ParamKind::Double2D
        | ParamKind::Double3D
        | ParamKind::Rgb
        | ParamKind::Rgba => ParamValue::Doubles(doubles(p, def)),
        ParamKind::String | ParamKind::Custom | ParamKind::StrChoice => {
            ParamValue::Text(string(p, def))
        }
        ParamKind::Bytes => ParamValue::Bytes(Vec::new()),
        ParamKind::Group | ParamKind::Page | ParamKind::PushButton => ParamValue::None,
    }
}

/// The voXY value kind a parameter type maps to, or `None` for types with
/// no value (groups, pages).
fn value_kind(kind: ParamKind) -> Option<ValueKind> {
    use ParamKind as K;
    Some(match kind {
        K::Integer => ValueKind::Int,
        K::Double => ValueKind::Scalar,
        K::Boolean | K::PushButton => ValueKind::Bool,
        K::Choice => ValueKind::Enum,
        K::StrChoice | K::String | K::Custom | K::Bytes => ValueKind::String,
        K::Rgba | K::Rgb => ValueKind::Color,
        K::Double2D | K::Integer2D => ValueKind::Vec2,
        K::Double3D | K::Integer3D => ValueKind::Vec3,
        K::Group | K::Page => return None,
    })
}

/// A plugin value as a voXY value of `kind` (missing components are 0).
pub fn to_value(kind: ParamKind, v: &ParamValue) -> Option<Value> {
    let d = |v: &[f64], i: usize| v.get(i).copied().unwrap_or(0.0);
    let n = |v: &[i32], i: usize| f64::from(v.get(i).copied().unwrap_or(0));
    Some(match (kind, v) {
        (ParamKind::Integer, ParamValue::Ints(v)) => {
            Value::Int(i64::from(v.first().copied().unwrap_or(0)))
        }
        (ParamKind::Boolean | ParamKind::PushButton, ParamValue::Ints(v)) => {
            Value::Bool(v.first().copied().unwrap_or(0) != 0)
        }
        (ParamKind::PushButton, ParamValue::None) => Value::Bool(false),
        (ParamKind::Choice, ParamValue::Ints(v)) => {
            Value::Enum(u32::try_from(v.first().copied().unwrap_or(0)).unwrap_or(0))
        }
        (ParamKind::Integer2D, ParamValue::Ints(v)) => Value::Vec2(DVec2::new(n(v, 0), n(v, 1))),
        (ParamKind::Integer3D, ParamValue::Ints(v)) => {
            Value::Vec3(DVec3::new(n(v, 0), n(v, 1), n(v, 2)))
        }
        (ParamKind::Double, ParamValue::Doubles(v)) => Value::Scalar(d(v, 0)),
        (ParamKind::Double2D, ParamValue::Doubles(v)) => Value::Vec2(DVec2::new(d(v, 0), d(v, 1))),
        (ParamKind::Double3D, ParamValue::Doubles(v)) => {
            Value::Vec3(DVec3::new(d(v, 0), d(v, 1), d(v, 2)))
        }
        (ParamKind::Rgb, ParamValue::Doubles(v)) => Value::Color(LinearRgba::new(
            d(v, 0) as f32,
            d(v, 1) as f32,
            d(v, 2) as f32,
            1.0,
        )),
        (ParamKind::Rgba, ParamValue::Doubles(v)) => Value::Color(LinearRgba::new(
            d(v, 0) as f32,
            d(v, 1) as f32,
            d(v, 2) as f32,
            v.get(3).copied().unwrap_or(1.0) as f32,
        )),
        (ParamKind::String | ParamKind::Custom | ParamKind::StrChoice, ParamValue::Text(s)) => {
            Value::String(s.as_str().into())
        }
        (ParamKind::Bytes, ParamValue::Bytes(b)) => Value::String(base64_encode(b).into()),
        _ => return None,
    })
}

/// A voXY value as the plugin value of `kind`, or `None` if it does not
/// fit (including Bytes that are not valid base64 or over the cap).
pub fn to_param_value(kind: ParamKind, v: &Value) -> Option<ParamValue> {
    let i = |x: f64| x.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    Some(match (kind, v) {
        (ParamKind::Integer, Value::Int(x)) => ParamValue::Ints(vec![i32::try_from(*x).ok()?]),
        (ParamKind::Boolean | ParamKind::PushButton, Value::Bool(b)) => {
            ParamValue::Ints(vec![i32::from(*b)])
        }
        (ParamKind::Choice, Value::Enum(x)) => ParamValue::Ints(vec![i32::try_from(*x).ok()?]),
        (ParamKind::Integer2D, Value::Vec2(p)) => ParamValue::Ints(vec![i(p.x), i(p.y)]),
        (ParamKind::Integer3D, Value::Vec3(p)) => ParamValue::Ints(vec![i(p.x), i(p.y), i(p.z)]),
        (ParamKind::Double, Value::Scalar(x)) => ParamValue::Doubles(vec![*x]),
        (ParamKind::Double2D, Value::Vec2(p)) => ParamValue::Doubles(vec![p.x, p.y]),
        (ParamKind::Double3D, Value::Vec3(p)) => ParamValue::Doubles(vec![p.x, p.y, p.z]),
        (ParamKind::Rgb, Value::Color(c)) => {
            ParamValue::Doubles(vec![f64::from(c.r), f64::from(c.g), f64::from(c.b)])
        }
        (ParamKind::Rgba, Value::Color(c)) => ParamValue::Doubles(vec![
            f64::from(c.r),
            f64::from(c.g),
            f64::from(c.b),
            f64::from(c.a),
        ]),
        (ParamKind::String | ParamKind::StrChoice, Value::String(s)) => {
            ParamValue::Text(s.to_string())
        }
        (ParamKind::Custom, Value::String(s)) if s.len() <= crate::suites::MAX_OPAQUE_BYTES => {
            ParamValue::Text(s.to_string())
        }
        (ParamKind::Bytes, Value::String(s)) => {
            let b = base64_decode(s)?;
            (b.len() <= crate::suites::MAX_OPAQUE_BYTES).then_some(ParamValue::Bytes(b))?
        }
        _ => return None,
    })
}

/// Why a context could not become an effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// The plugin has no context voXY maps (filter or generator for now).
    NoContext,
    /// A parameter could not be mapped.
    Param { name: String, why: String },
}

impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MapError::NoContext => f.write_str("it has no filter or generator context"),
            MapError::Param { name, why } => write!(f, "parameter {name}: {why}"),
        }
    }
}

impl std::error::Error for MapError {}

/// The contexts voXY maps, in order of preference.
pub const MAPPED_CONTEXTS: [&[u8]; 4] = [
    ofx::kOfxImageEffectContextFilter,
    ofx::kOfxImageEffectContextTransition,
    ofx::kOfxImageEffectContextGenerator,
    ofx::kOfxImageEffectContextGeneral,
];

/// The context a plugin runs in as a voXY effect: the first it has of
/// filter, transition, generator (whose output replaces the effect's
/// input) and general.
pub fn mapped_context(plugin: &PluginDesc) -> Option<&ContextDesc> {
    MAPPED_CONTEXTS
        .iter()
        .find_map(|want| plugin.contexts.iter().find(|c| c.context == ofx_str(want)))
}

/// The mapped context of `plugin` (see [`mapped_context`]) as an effect
/// definition. `binary` and `index` say where the plugin lives
/// (`EffectOrigin::Ofx`).
pub fn effect_def(plugin: &PluginDesc, binary: &Path, index: u32) -> Result<EffectDef, MapError> {
    let ctx = mapped_context(plugin).ok_or(MapError::NoContext)?;
    let tid = type_id(&plugin.identifier);
    let label = string(&plugin.props, ofx::kOfxPropLabel);
    let name = if label.is_empty() {
        plugin.identifier.clone()
    } else {
        label
    };
    let grouping = string(&plugin.props, ofx::kOfxImageEffectPluginPropGrouping);
    let category = Category::Plugin(if grouping.is_empty() {
        "OpenFX".into()
    } else {
        grouping.into()
    });

    let (groups, group_of) = groups(ctx);
    let mut params = Vec::new();
    for d in &ctx.params {
        if d.name.starts_with(RESERVED_PREFIX) {
            return Err(MapError::Param {
                name: d.name.clone(),
                why: format!("names starting with {RESERVED_PREFIX} are voXY's"),
            });
        }
        if let Some(def) = param_def(&tid, d, &group_of)? {
            params.push(def);
        }
    }
    let transition = ctx.context == ofx_str(ofx::kOfxImageEffectContextTransition);
    if transition {
        params.extend(transition_params(&tid));
    } else {
        params.extend(extra_clips(ctx).map(|c| clip_param(&tid, c)));
    }
    let category = if transition {
        Category::Transition
    } else {
        category
    };
    Ok(EffectDef {
        type_id: tid,
        version: plugin.version.0,
        name: name.into(),
        category,
        description: string(&plugin.props, ofx::kOfxPropPluginDescription).into(),
        params: params.into(),
        groups: groups.into(),
        space: EffectSpace::Working,
        seeded: false,
        origin: EffectOrigin::Ofx {
            binary: binary.to_path_buf(),
            index,
        },
    })
}

/// A context's input clips other than the effect's own input and masks:
/// each is fed by a layer-reference parameter (design §6.4). Masks are
/// left unconnected; voXY's effect matte masks the result instead.
pub fn extra_clips(ctx: &ContextDesc) -> impl Iterator<Item = &crate::protocol::ClipDesc> {
    let main = [
        ofx::kOfxImageEffectSimpleSourceClipName.as_slice(),
        ofx::kOfxImageEffectTransitionSourceFromClipName,
        ofx::kOfxImageEffectOutputClipName,
    ];
    ctx.clips
        .iter()
        .filter(move |c| !main.iter().any(|m| c.name == ofx_str(m)))
        .filter(|c| !flag(&c.props, ofx::kOfxImageClipPropIsMask))
}

/// The layer-reference parameter feeding an extra clip.
fn clip_param(tid: &EffectTypeId, clip: &crate::protocol::ClipDesc) -> EffectParamDef {
    let name = format!("{CLIP_PARAM_PREFIX}{}", clip.name);
    let label = string(&clip.props, ofx::kOfxPropLabel);
    let optional = flag(&clip.props, ofx::kOfxImageClipPropOptional);
    EffectParamDef {
        desc: Arc::new(PropertyDesc {
            key: tid.param_key(&name).into(),
            label: if label.is_empty() {
                clip.name.clone()
            } else {
                label
            }
            .into(),
            kind: ValueKind::Layer,
            default: Value::Layer(None),
            range: None,
            soft_range: None,
            unit: Unit::None,
            animatable: true,
            spatial: false,
        }),
        name: name.into(),
        choices: None,
        group: None,
        gizmo: None,
        hint: Some(
            format!(
                "The layer for the plugin's {} input{}. Its own effects are not used.",
                clip.name,
                if optional { " (optional)" } else { "" }
            )
            .into(),
        ),
        flags: ParamFlags::default(),
    }
}

/// The parameters voXY adds to a transition (design §5, §6.4): Auto, and
/// the layer that is `SourceTo`.
fn transition_params(tid: &EffectTypeId) -> [EffectParamDef; 2] {
    let def = |name: &str, label: &str, kind, default, hint: &str| EffectParamDef {
        name: name.into(),
        desc: Arc::new(PropertyDesc {
            key: tid.param_key(name).into(),
            label: label.into(),
            kind,
            default,
            range: None,
            soft_range: None,
            unit: Unit::None,
            animatable: true,
            spatial: false,
        }),
        choices: None,
        group: None,
        gizmo: None,
        hint: Some(hint.into()),
        flags: ParamFlags::default(),
    };
    [
        def(
            AUTO_PARAM,
            "Auto",
            ValueKind::Bool,
            Value::Bool(true),
            "Run the transition over the layer's whole duration, instead of following Transition.",
        ),
        def(
            &format!(
                "{CLIP_PARAM_PREFIX}{}",
                ofx_str(ofx::kOfxImageEffectTransitionSourceToClipName)
            ),
            "Transition To",
            ValueKind::Layer,
            Value::Layer(None),
            "The layer to show at the end. Its own effects are not used.",
        ),
    ]
}

/// Group parameters become collapsible groups, in definition order (a
/// parent always precedes its children in OpenFX). Pages are flattened.
fn groups(ctx: &ContextDesc) -> (Vec<ParamGroup>, std::collections::HashMap<String, u32>) {
    let mut groups = Vec::new();
    let mut ids = std::collections::HashMap::new();
    for d in ctx.params.iter().filter(|d| d.kind == ParamKind::Group) {
        let id = u32::try_from(groups.len()).unwrap_or(u32::MAX);
        let parent = ids
            .get(&string(&d.props, ofx::kOfxParamPropParent))
            .copied();
        let label = string(&d.props, ofx::kOfxPropLabel);
        groups.push(ParamGroup {
            id,
            label: if label.is_empty() {
                d.name.clone()
            } else {
                label
            }
            .into(),
            parent,
            open_by_default: flag(&d.props, ofx::kOfxParamPropGroupOpen),
        });
        ids.insert(d.name.clone(), id);
    }
    (groups, ids)
}

fn param_def(
    tid: &EffectTypeId,
    d: &ParamDesc,
    group_of: &std::collections::HashMap<String, u32>,
) -> Result<Option<EffectParamDef>, MapError> {
    let Some(kind) = value_kind(d.kind) else {
        return Ok(None);
    };
    let bad = |why: &str| MapError::Param {
        name: d.name.clone(),
        why: why.to_owned(),
    };
    let p = &d.props;
    let default = to_value(d.kind, &default_param_value(d))
        .ok_or_else(|| bad("its default does not fit its type"))?;
    let label = string(p, ofx::kOfxPropLabel);
    let choices: Option<Arc<[Arc<str>]>> = (d.kind == ParamKind::Choice).then(|| {
        strings(p, ofx::kOfxParamPropChoiceOption)
            .into_iter()
            .map(Into::into)
            .collect()
    });
    let default = match (&default, &choices) {
        (Value::Enum(i), Some(c)) if *i as usize >= c.len() => Value::Enum(0),
        _ => default,
    };
    let (unit, hard, soft) = match d.kind {
        ParamKind::Double | ParamKind::Double2D | ParamKind::Double3D => (
            double_unit(p),
            numeric_range(p, ofx::kOfxParamPropMin, ofx::kOfxParamPropMax),
            numeric_range(
                p,
                ofx::kOfxParamPropDisplayMin,
                ofx::kOfxParamPropDisplayMax,
            ),
        ),
        ParamKind::Integer | ParamKind::Integer2D | ParamKind::Integer3D => (
            Unit::None,
            numeric_range(p, ofx::kOfxParamPropMin, ofx::kOfxParamPropMax),
            numeric_range(
                p,
                ofx::kOfxParamPropDisplayMin,
                ofx::kOfxParamPropDisplayMax,
            ),
        ),
        _ => (Unit::None, None, None),
    };
    let visibility = if flag(p, ofx::kOfxParamPropSecret) || d.kind == ParamKind::Bytes {
        ParamVisibility::Hidden
    } else if d.kind == ParamKind::PushButton {
        // Buttons arrive with the parameter UI in a later step.
        ParamVisibility::ReadOnly
    } else {
        ParamVisibility::Editable
    };
    let hint = string(p, ofx::kOfxParamPropHint);
    let hint = match d.kind {
        ParamKind::StrChoice => {
            let opts = strings(p, ofx::kOfxParamPropChoiceEnum).join(", ");
            Some(if hint.is_empty() {
                format!("One of: {opts}")
            } else {
                format!("{hint} (one of: {opts})")
            })
        }
        _ => (!hint.is_empty()).then_some(hint),
    };
    let opaque = matches!(d.kind, ParamKind::Custom | ParamKind::Bytes);
    Ok(Some(EffectParamDef {
        name: d.name.as_str().into(),
        desc: Arc::new(PropertyDesc {
            key: tid.param_key(&d.name).into(),
            label: if label.is_empty() {
                d.name.clone()
            } else {
                label
            }
            .into(),
            kind,
            default,
            range: hard,
            // A display range is kept inside the hard one.
            soft_range: match (soft, hard) {
                (Some((a, b)), Some((lo, hi))) => {
                    Some((a.max(lo), b.min(hi))).filter(|(a, b)| a < b)
                }
                (s, h) => s.or(h),
            },
            unit,
            animatable: !opaque
                && d.kind != ParamKind::PushButton
                && flag(p, ofx::kOfxParamPropAnimates),
            spatial: false,
        }),
        choices,
        group: group_of.get(&string(p, ofx::kOfxParamPropParent)).copied(),
        gizmo: None,
        hint: hint.map(Into::into),
        flags: ParamFlags {
            integer_display: matches!(d.kind, ParamKind::Integer2D | ParamKind::Integer3D),
            hide_alpha: d.kind == ParamKind::Rgb,
            visibility,
        },
    }))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (Bytes parameters on disk, P-0 Q3).
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(B64[(n >> shift) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard base64 with padding; `None` for anything else.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let t = text.as_bytes();
    if !t.len().is_multiple_of(4) {
        return None;
    }
    let val = |c: u8| B64.iter().position(|b| *b == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(t.len() / 4 * 3);
    for (k, q) in t.chunks(4).enumerate() {
        let last = k + 1 == t.len() / 4;
        let pad = q.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (i, c) in q.iter().enumerate() {
            let v = if i >= 4 - pad { 0 } else { val(*c)? };
            n = (n << 6) | v;
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}
