//! Dynamically typed property values.

use std::sync::Arc;

use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::color::LinearRgba;

/// A property value.
///
/// Properties are dynamically typed, so built-in, OpenFX and Alight Motion
/// parameters all share one keyframe, undo and serialization path (design
/// proposal §3.2). Path and gradient values arrive with shapes in Milestone 4.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Bool(bool),
    Int(i64),
    Scalar(f64),
    Vec2(DVec2),
    Vec3(DVec3),
    Color(LinearRgba),
    Enum(u32),
    String(Arc<str>),
}

/// The type of a [`Value`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Bool,
    Int,
    Scalar,
    Vec2,
    Vec3,
    Color,
    Enum,
    String,
}

impl Value {
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Bool(_) => ValueKind::Bool,
            Value::Int(_) => ValueKind::Int,
            Value::Scalar(_) => ValueKind::Scalar,
            Value::Vec2(_) => ValueKind::Vec2,
            Value::Vec3(_) => ValueKind::Vec3,
            Value::Color(_) => ValueKind::Color,
            Value::Enum(_) => ValueKind::Enum,
            Value::String(_) => ValueKind::String,
        }
    }

    /// `false` if any float component is NaN or infinite. Non-finite values
    /// are rejected everywhere: they break determinism and cannot be
    /// represented in JSON.
    pub fn is_finite(&self) -> bool {
        match self {
            Value::Scalar(v) => v.is_finite(),
            Value::Vec2(v) => v.is_finite(),
            Value::Vec3(v) => v.is_finite(),
            Value::Color(c) => c.is_finite(),
            Value::Bool(_) | Value::Int(_) | Value::Enum(_) | Value::String(_) => true,
        }
    }

    pub fn as_scalar(&self) -> Option<f64> {
        match *self {
            Value::Scalar(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_vec2(&self) -> Option<DVec2> {
        match *self {
            Value::Vec2(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_color(&self) -> Option<LinearRgba> {
        match *self {
            Value::Color(c) => Some(c),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }
}
