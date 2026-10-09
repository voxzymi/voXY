//! Properties: a typed, described, (eventually) animatable value.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::color::LinearRgba;
use crate::id::PropId;
use crate::time::Time;
use crate::value::{Value, ValueKind};

/// Display and editing unit of a property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    None,
    Pixels,
    Degrees,
    /// A multiplier stored as a factor (1.0) and shown as a percentage (100%).
    Factor,
    /// `0..=1` stored, shown as a percentage.
    Normalized,
    Seconds,
}

/// Static description of a property type, shared between all instances.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyDesc {
    /// Stable key, e.g. `"transform.position"` or, later, `"ofx:<plugin>/<param>"`.
    pub key: Arc<str>,
    pub label: Arc<str>,
    pub kind: ValueKind,
    pub default: Value,
    /// Hard range; scalar values are clamped into it.
    pub range: Option<(f64, f64)>,
    /// Suggested UI range for sliders.
    pub soft_range: Option<(f64, f64)>,
    pub unit: Unit,
    pub animatable: bool,
}

impl PropertyDesc {
    /// A descriptor for a key voXY does not know (for example one written by
    /// a newer version). The value is preserved as-is.
    fn unknown(key: &str, value: &Value) -> Self {
        PropertyDesc {
            key: key.into(),
            label: key.into(),
            kind: value.kind(),
            default: value.clone(),
            range: None,
            soft_range: None,
            unit: Unit::None,
            animatable: false,
        }
    }
}

/// How a property's value varies over time.
///
/// Milestone 1 only has static values. `Keyframed(KeyframeTrack)` arrives with
/// the timeline and graph editor in Milestone 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Animation {
    Static(Value),
}

/// Errors from setting a property value.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PropertyError {
    #[error("property `{key}` expects a {expected:?} value, got {got:?}")]
    TypeMismatch {
        key: Arc<str>,
        expected: ValueKind,
        got: ValueKind,
    },
    #[error("property `{key}` rejects non-finite values")]
    NonFinite { key: Arc<str> },
}

/// A property instance on a layer or composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PropertyRepr", into = "PropertyRepr")]
pub struct Property {
    pub id: PropId,
    pub desc: Arc<PropertyDesc>,
    anim: Animation,
}

impl Property {
    /// A new property with a validated static value.
    pub fn new(id: PropId, desc: Arc<PropertyDesc>, value: Value) -> Result<Self, PropertyError> {
        let value = validate(&desc, value)?;
        Ok(Property {
            id,
            desc,
            anim: Animation::Static(value),
        })
    }

    /// A new property from a built-in descriptor key.
    ///
    /// # Panics
    /// If `key` is not a built-in property or `value` does not fit it; both
    /// are programming errors.
    pub fn builtin(id: PropId, key: &str, value: Value) -> Self {
        let desc = builtin_desc(key).unwrap_or_else(|| panic!("unknown built-in property {key}"));
        Property::new(id, desc, value).unwrap_or_else(|e| panic!("{e}"))
    }

    pub fn key(&self) -> &str {
        &self.desc.key
    }

    pub fn animation(&self) -> &Animation {
        &self.anim
    }

    /// The value at `time`.
    pub fn value_at(&self, _time: Time) -> &Value {
        match &self.anim {
            Animation::Static(v) => v,
        }
    }

    /// Replaces the property with a static value (validated, scalars clamped).
    pub fn set_static(&mut self, value: Value) -> Result<(), PropertyError> {
        self.anim = Animation::Static(validate(&self.desc, value)?);
        Ok(())
    }
}

fn validate(desc: &PropertyDesc, value: Value) -> Result<Value, PropertyError> {
    if value.kind() != desc.kind {
        return Err(PropertyError::TypeMismatch {
            key: desc.key.clone(),
            expected: desc.kind,
            got: value.kind(),
        });
    }
    if !value.is_finite() {
        return Err(PropertyError::NonFinite {
            key: desc.key.clone(),
        });
    }
    Ok(match (value, desc.range) {
        (Value::Scalar(v), Some((lo, hi))) => Value::Scalar(v.clamp(lo, hi)),
        (Value::Int(v), Some((lo, hi))) => Value::Int(v.clamp(lo as i64, hi as i64)),
        (v, _) => v,
    })
}

/// On-disk form: the descriptor is referenced by key, not duplicated.
#[derive(Serialize, Deserialize)]
struct PropertyRepr {
    id: PropId,
    key: Arc<str>,
    #[serde(flatten)]
    anim: Animation,
}

impl TryFrom<PropertyRepr> for Property {
    type Error = PropertyError;
    fn try_from(r: PropertyRepr) -> Result<Self, PropertyError> {
        let Animation::Static(value) = r.anim;
        let desc =
            builtin_desc(&r.key).unwrap_or_else(|| Arc::new(PropertyDesc::unknown(&r.key, &value)));
        // Load exactly what was saved: kind and finiteness are checked, but
        // values are not re-clamped, so a load/save cycle never alters data.
        if value.kind() != desc.kind {
            return Err(PropertyError::TypeMismatch {
                key: desc.key.clone(),
                expected: desc.kind,
                got: value.kind(),
            });
        }
        if !value.is_finite() {
            return Err(PropertyError::NonFinite {
                key: desc.key.clone(),
            });
        }
        Ok(Property {
            id: r.id,
            desc,
            anim: Animation::Static(value),
        })
    }
}

impl From<Property> for PropertyRepr {
    fn from(p: Property) -> Self {
        PropertyRepr {
            id: p.id,
            key: p.desc.key.clone(),
            anim: p.anim,
        }
    }
}

/// Keys of the built-in properties.
pub mod keys {
    pub const ANCHOR: &str = "transform.anchor";
    pub const POSITION: &str = "transform.position";
    pub const SCALE: &str = "transform.scale";
    pub const ROTATION: &str = "transform.rotation";
    pub const OPACITY: &str = "transform.opacity";
    pub const SOLID_COLOR: &str = "solid.color";
    pub const SOLID_SIZE: &str = "solid.size";
    pub const COMP_BACKGROUND: &str = "comp.background";
}

/// Looks up a built-in property descriptor.
pub fn builtin_desc(key: &str) -> Option<Arc<PropertyDesc>> {
    BUILTINS.get(key).cloned()
}

static BUILTINS: LazyLock<HashMap<&'static str, Arc<PropertyDesc>>> = LazyLock::new(|| {
    let d = |key: &'static str,
             label: &'static str,
             default: Value,
             range: Option<(f64, f64)>,
             soft_range: Option<(f64, f64)>,
             unit: Unit| {
        (
            key,
            Arc::new(PropertyDesc {
                key: key.into(),
                label: label.into(),
                kind: default.kind(),
                default,
                range,
                soft_range,
                unit,
                animatable: true,
            }),
        )
    };
    HashMap::from([
        d(
            keys::ANCHOR,
            "Anchor Point",
            Value::Vec2(DVec2::ZERO),
            None,
            None,
            Unit::Pixels,
        ),
        d(
            keys::POSITION,
            "Position",
            Value::Vec2(DVec2::ZERO),
            None,
            None,
            Unit::Pixels,
        ),
        d(
            keys::SCALE,
            "Scale",
            Value::Vec2(DVec2::ONE),
            None,
            Some((0.0, 4.0)),
            Unit::Factor,
        ),
        d(
            keys::ROTATION,
            "Rotation",
            Value::Scalar(0.0),
            None,
            None,
            Unit::Degrees,
        ),
        d(
            keys::OPACITY,
            "Opacity",
            Value::Scalar(1.0),
            Some((0.0, 1.0)),
            None,
            Unit::Normalized,
        ),
        d(
            keys::SOLID_COLOR,
            "Color",
            Value::Color(LinearRgba::WHITE),
            None,
            None,
            Unit::None,
        ),
        d(
            keys::SOLID_SIZE,
            "Size",
            Value::Vec2(DVec2::new(100.0, 100.0)),
            None,
            Some((1.0, 8192.0)),
            Unit::Pixels,
        ),
        d(
            keys::COMP_BACKGROUND,
            "Background",
            Value::Color(LinearRgba::BLACK),
            None,
            None,
            Unit::None,
        ),
    ])
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_validates_and_clamps() {
        let mut p = Property::builtin(PropId(1), keys::OPACITY, Value::Scalar(0.5));
        p.set_static(Value::Scalar(2.0)).unwrap();
        assert_eq!(p.value_at(Time::ZERO), &Value::Scalar(1.0));
        assert!(matches!(
            p.set_static(Value::Bool(true)),
            Err(PropertyError::TypeMismatch { .. })
        ));
        assert!(matches!(
            p.set_static(Value::Scalar(f64::NAN)),
            Err(PropertyError::NonFinite { .. })
        ));
    }

    #[test]
    fn serde_by_key() {
        let p = Property::builtin(PropId(7), keys::ROTATION, Value::Scalar(45.0));
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(
            json,
            r#"{"id":7,"key":"transform.rotation","static":{"type":"scalar","value":45.0}}"#
        );
        let back: Property = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert!(Arc::ptr_eq(
            &back.desc,
            &builtin_desc(keys::ROTATION).unwrap()
        ));
    }

    #[test]
    fn unknown_keys_are_preserved() {
        let json = r#"{"id":3,"key":"future.thing","static":{"type":"int","value":4}}"#;
        let p: Property = serde_json::from_str(json).unwrap();
        assert_eq!(p.key(), "future.thing");
        assert_eq!(serde_json::to_string(&p).unwrap(), json);
    }

    #[test]
    fn wrong_kind_on_load_is_an_error() {
        let json = r#"{"id":3,"key":"transform.rotation","static":{"type":"bool","value":true}}"#;
        assert!(serde_json::from_str::<Property>(json).is_err());
    }
}
