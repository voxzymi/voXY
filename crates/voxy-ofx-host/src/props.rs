//! Property sets: the host's side of `ofxProperty.h`.
//!
//! A [`PropSet`] holds typed, multi-valued properties keyed by name. The
//! host defines each property (type, dimension, default, and whether the
//! plugin may set it); the property suite in [`crate::suites`] then reads
//! and writes them for the plugin. Everything here is safe Rust. Strings
//! are kept as C strings so the suite can hand out pointers to them that
//! stay valid until the property changes, as the spec requires
//! (the spec's note on strings).

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::sync::{Mutex, MutexGuard, PoisonError};

use voxy_ofx_sys as ofx;

/// A property's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropType {
    Int,
    Double,
    String,
    Pointer,
}

/// A property's values. Pointers are kept as addresses: the host never
/// dereferences them.
#[derive(Debug, Clone, PartialEq)]
pub enum Values {
    Int(Vec<i32>),
    Double(Vec<f64>),
    String(Vec<CString>),
    Pointer(Vec<usize>),
}

impl Values {
    pub fn ty(&self) -> PropType {
        match self {
            Values::Int(_) => PropType::Int,
            Values::Double(_) => PropType::Double,
            Values::String(_) => PropType::String,
            Values::Pointer(_) => PropType::Pointer,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Values::Int(v) => v.len(),
            Values::Double(v) => v.len(),
            Values::String(v) => v.len(),
            Values::Pointer(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Grows or shrinks to `n` values, filling with zero, `0.0`, `""` or
    /// null.
    fn resize(&mut self, n: usize) {
        match self {
            Values::Int(v) => v.resize(n, 0),
            Values::Double(v) => v.resize(n, 0.0),
            Values::String(v) => v.resize(n, CString::default()),
            Values::Pointer(v) => v.resize(n, 0),
        }
    }

    /// Strings from Rust text. A NUL inside a string ends it.
    pub fn strings<S: AsRef<str>>(items: &[S]) -> Values {
        Values::String(items.iter().map(|s| c_string(s.as_ref())).collect())
    }
}

/// `s` as a C string, cut at the first NUL.
pub fn c_string(s: &str) -> CString {
    let end = s.find('\0').unwrap_or(s.len());
    CString::new(&s[..end]).unwrap_or_default()
}

/// An OpenFX name constant (`b"...\0"`) as text.
pub fn ofx_str(name: &[u8]) -> &str {
    std::str::from_utf8(name.strip_suffix(b"\0").unwrap_or(name)).unwrap_or_default()
}

/// Why a property call failed, as the suite reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropError {
    /// No such property (`kOfxStatErrUnknown`).
    Unknown,
    /// Index or count outside the dimension (`kOfxStatErrBadIndex`).
    BadIndex,
    /// Wrong type, or the plugin may not set it (`kOfxStatErrValue`).
    Value,
}

impl PropError {
    pub fn status(self) -> ofx::OfxStatus {
        match self {
            PropError::Unknown => ofx::kOfxStatErrUnknown,
            PropError::BadIndex => ofx::kOfxStatErrBadIndex,
            PropError::Value => ofx::kOfxStatErrValue,
        }
    }
}

/// How many values a property holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dim {
    /// Exactly this many.
    Fixed(usize),
    /// Any number (the spec writes dimension 0).
    Variable,
}

#[derive(Debug, Clone)]
struct Prop {
    values: Values,
    default: Values,
    dim: Dim,
    /// Whether the plugin may set it. The host always may.
    plugin_writable: bool,
}

#[derive(Debug, Default)]
struct Inner {
    props: BTreeMap<CString, Prop>,
}

/// Who is making a call: the plugin through the suite, or the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Plugin,
    Host,
}

/// A named set of properties. Thread-safe: plugins may call the property
/// suite from several render threads.
#[derive(Debug)]
pub struct PropSet {
    /// What the set belongs to, for messages ("host", "effect descriptor").
    kind: &'static str,
    inner: Mutex<Inner>,
}

impl PropSet {
    pub fn new(kind: &'static str) -> PropSet {
        PropSet {
            kind,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Defines (or redefines) `name` with `default` as its value. A fixed
    /// dimension is the default's length.
    pub fn define(&self, name: &str, default: Values, dim: Dim, plugin_writable: bool) {
        let dim = match dim {
            Dim::Fixed(_) => Dim::Fixed(default.len()),
            Dim::Variable => Dim::Variable,
        };
        self.lock().props.insert(
            c_string(name),
            Prop {
                values: default.clone(),
                default,
                dim,
                plugin_writable,
            },
        );
    }

    /// Shorthand for a fixed-dimension property the plugin may set.
    pub fn define_rw(&self, name: &str, default: Values) {
        self.define(name, default, Dim::Fixed(0), true);
    }

    /// Shorthand for a fixed-dimension property only the host sets.
    pub fn define_ro(&self, name: &str, default: Values) {
        self.define(name, default, Dim::Fixed(0), false);
    }

    /// A copy of every property (values, defaults, dimensions and
    /// writability) under a new kind.
    #[must_use]
    pub fn duplicate(&self, kind: &'static str) -> PropSet {
        let props = self.lock().props.clone();
        PropSet {
            kind,
            inner: Mutex::new(Inner { props }),
        }
    }

    /// Changes whether the plugin may set `name`.
    pub fn set_plugin_writable(&self, name: &str, writable: bool) {
        if let Some(p) = self.lock().props.get_mut(c_string(name).as_c_str()) {
            p.plugin_writable = writable;
        }
    }

    /// Defines a fixed-dimension property the plugin may set, by its
    /// OpenFX constant.
    pub fn rw(&self, name: &[u8], default: Values) {
        self.define(ofx_str(name), default, Dim::Fixed(0), true);
    }

    /// Defines a fixed-dimension property only the host sets.
    pub fn ro(&self, name: &[u8], default: Values) {
        self.define(ofx_str(name), default, Dim::Fixed(0), false);
    }

    /// Defines a variable-dimension property the plugin may set.
    pub fn var(&self, name: &[u8], default: Values) {
        self.define(ofx_str(name), default, Dim::Variable, true);
    }

    /// Defines a variable-dimension property only the host sets.
    pub fn var_ro(&self, name: &[u8], default: Values) {
        self.define(ofx_str(name), default, Dim::Variable, false);
    }

    /// Strings from OpenFX constants.
    pub fn consts(names: &[&[u8]]) -> Values {
        Values::String(names.iter().map(|n| c_string(ofx_str(n))).collect())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.lock().props.contains_key(c_string(name).as_c_str())
    }

    /// The property names, sorted.
    pub fn names(&self) -> Vec<String> {
        self.lock()
            .props
            .keys()
            .map(|k| k.to_string_lossy().into_owned())
            .collect()
    }

    /// A copy of all values of `name`.
    pub fn get(&self, name: &str) -> Option<Values> {
        self.lock()
            .props
            .get(c_string(name).as_c_str())
            .map(|p| p.values.clone())
    }

    /// Sets all values of `name` as the host (ignores `plugin_writable`).
    pub fn set(&self, name: &str, values: Values) -> Result<(), PropError> {
        let n = values.len();
        self.with(c_string(name).as_c_str(), Caller::Host, values.ty(), |p| {
            if let Dim::Fixed(d) = p.dim
                && n != d
            {
                return Err(PropError::BadIndex);
            }
            p.values = values;
            Ok(())
        })
    }

    pub fn int(&self, name: &str, index: usize) -> Option<i32> {
        match self.get(name)? {
            Values::Int(v) => v.get(index).copied(),
            _ => None,
        }
    }

    pub fn double(&self, name: &str, index: usize) -> Option<f64> {
        match self.get(name)? {
            Values::Double(v) => v.get(index).copied(),
            _ => None,
        }
    }

    pub fn string(&self, name: &str, index: usize) -> Option<String> {
        match self.get(name)? {
            Values::String(v) => v.get(index).map(|s| s.to_string_lossy().into_owned()),
            _ => None,
        }
    }

    /// All strings of `name` (empty if missing or not a string property).
    pub fn strings(&self, name: &str) -> Vec<String> {
        match self.get(name) {
            Some(Values::String(v)) => v.iter().map(|s| s.to_string_lossy().into_owned()).collect(),
            _ => Vec::new(),
        }
    }

    fn with<R>(
        &self,
        name: &CStr,
        caller: Caller,
        ty: PropType,
        f: impl FnOnce(&mut Prop) -> Result<R, PropError>,
    ) -> Result<R, PropError> {
        let mut inner = self.lock();
        let p = inner.props.get_mut(name).ok_or(PropError::Unknown)?;
        if p.values.ty() != ty {
            return Err(PropError::Value);
        }
        if caller == Caller::Plugin && !p.plugin_writable {
            return Err(PropError::Value);
        }
        f(p)
    }

    /// Sets value `index` of `name` (`propSet*`). A variable-dimension
    /// property grows to fit; a fixed one refuses an index outside it.
    pub fn set_at(
        &self,
        name: &CStr,
        caller: Caller,
        index: i32,
        value: Values,
    ) -> Result<(), PropError> {
        let index = usize::try_from(index).map_err(|_| PropError::BadIndex)?;
        self.with(name, caller, value.ty(), |p| {
            match p.dim {
                Dim::Fixed(d) if index >= d => return Err(PropError::BadIndex),
                Dim::Variable if index >= p.values.len() => p.values.resize(index + 1),
                _ => {}
            }
            match (&mut p.values, value) {
                (Values::Int(v), Values::Int(x)) => v[index] = x[0],
                (Values::Double(v), Values::Double(x)) => v[index] = x[0],
                (Values::Pointer(v), Values::Pointer(x)) => v[index] = x[0],
                (Values::String(v), Values::String(mut x)) => v[index] = x.swap_remove(0),
                _ => return Err(PropError::Value),
            }
            Ok(())
        })
    }

    /// Sets values `0..count` of `name` (`propSet*N`). A variable-dimension
    /// property becomes exactly `count` long.
    pub fn set_n(&self, name: &CStr, caller: Caller, values: Values) -> Result<(), PropError> {
        let n = values.len();
        self.with(name, caller, values.ty(), |p| {
            match p.dim {
                Dim::Fixed(d) if n > d => return Err(PropError::BadIndex),
                Dim::Variable => p.values.resize(n),
                Dim::Fixed(_) => {}
            }
            match (&mut p.values, values) {
                (Values::Int(v), Values::Int(x)) => v[..n].copy_from_slice(&x),
                (Values::Double(v), Values::Double(x)) => v[..n].copy_from_slice(&x),
                (Values::Pointer(v), Values::Pointer(x)) => v[..n].copy_from_slice(&x),
                (Values::String(v), Values::String(x)) => {
                    for (slot, s) in v.iter_mut().zip(x) {
                        *slot = s;
                    }
                }
                _ => return Err(PropError::Value),
            }
            Ok(())
        })
    }

    /// Calls `f` with values `0..count` of `name` (`propGet*` and
    /// `propGet*N`), checking the type and the count. The values stay
    /// borrowed only for the call.
    pub fn read<R>(
        &self,
        name: &CStr,
        ty: PropType,
        count: i32,
        f: impl FnOnce(&Values) -> R,
    ) -> Result<R, PropError> {
        let count = usize::try_from(count).map_err(|_| PropError::BadIndex)?;
        let inner = self.lock();
        let p = inner.props.get(name).ok_or(PropError::Unknown)?;
        if p.values.ty() != ty {
            return Err(PropError::Value);
        }
        if count > p.values.len() {
            return Err(PropError::BadIndex);
        }
        Ok(f(&p.values))
    }

    /// `propReset`: back to the default value.
    pub fn reset(&self, name: &CStr, caller: Caller) -> Result<(), PropError> {
        let mut inner = self.lock();
        let p = inner.props.get_mut(name).ok_or(PropError::Unknown)?;
        if caller == Caller::Plugin && !p.plugin_writable {
            return Err(PropError::Value);
        }
        p.values = p.default.clone();
        Ok(())
    }

    /// `propGetDimension`: the current number of values.
    pub fn dimension(&self, name: &CStr) -> Result<usize, PropError> {
        let inner = self.lock();
        let p = inner.props.get(name).ok_or(PropError::Unknown)?;
        Ok(p.values.len())
    }
}
