//! Parameters: the host's side of `ofxParam.h`, in safe Rust.
//!
//! A [`ParamSet`] belongs to an effect descriptor or an effect instance.
//! Descriptors gain parameters through `paramDefine`, each with the
//! descriptor properties its type has in the spec. An instance copies its
//! descriptor's parameters and reads and writes their values through a
//! [`ParamValues`] store: in the editor that is the document (O-B on); in
//! the in-process harness it is [`LocalValues`].

use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use voxy_ofx_sys as ofx;

use crate::props::{Dim, PropSet, Values};

/// A parameter type (`kOfxParamType*`). Parametric parameters are not
/// supported yet (`docs/ofx-host.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Integer,
    Double,
    Boolean,
    Choice,
    StrChoice,
    Rgba,
    Rgb,
    Double2D,
    Integer2D,
    Double3D,
    Integer3D,
    String,
    Custom,
    Bytes,
    Group,
    Page,
    PushButton,
}

const KINDS: [(ParamKind, &[u8]); 17] = [
    (ParamKind::Integer, ofx::kOfxParamTypeInteger),
    (ParamKind::Double, ofx::kOfxParamTypeDouble),
    (ParamKind::Boolean, ofx::kOfxParamTypeBoolean),
    (ParamKind::Choice, ofx::kOfxParamTypeChoice),
    (ParamKind::StrChoice, ofx::kOfxParamTypeStrChoice),
    (ParamKind::Rgba, ofx::kOfxParamTypeRGBA),
    (ParamKind::Rgb, ofx::kOfxParamTypeRGB),
    (ParamKind::Double2D, ofx::kOfxParamTypeDouble2D),
    (ParamKind::Integer2D, ofx::kOfxParamTypeInteger2D),
    (ParamKind::Double3D, ofx::kOfxParamTypeDouble3D),
    (ParamKind::Integer3D, ofx::kOfxParamTypeInteger3D),
    (ParamKind::String, ofx::kOfxParamTypeString),
    (ParamKind::Custom, ofx::kOfxParamTypeCustom),
    (ParamKind::Bytes, ofx::kOfxParamTypeBytes),
    (ParamKind::Group, ofx::kOfxParamTypeGroup),
    (ParamKind::Page, ofx::kOfxParamTypePage),
    (ParamKind::PushButton, ofx::kOfxParamTypePushButton),
];

/// What a parameter type's values look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `n` C ints (integer, boolean and choice types).
    Ints(usize),
    /// `n` doubles.
    Doubles(usize),
    /// One UTF-8 string (string, custom and string-choice types).
    Text,
    /// An opaque byte buffer.
    Bytes,
    /// No value (group, page, push button).
    None,
}

impl ParamKind {
    /// The type for a `kOfxParamType*` name (without the NUL).
    pub fn from_ofx(name: &[u8]) -> Option<ParamKind> {
        KINDS
            .iter()
            .find(|(_, n)| n.strip_suffix(b"\0") == Some(name))
            .map(|(k, _)| *k)
    }

    /// The `kOfxParamType*` name, without the NUL.
    pub fn ofx_name(self) -> &'static str {
        let n = KINDS
            .iter()
            .find(|(k, _)| *k == self)
            .map_or(&b"\0"[..], |(_, n)| n);
        std::str::from_utf8(n.strip_suffix(b"\0").unwrap_or(n)).unwrap_or_default()
    }

    pub fn shape(self) -> Shape {
        use ParamKind as K;
        match self {
            K::Integer | K::Boolean | K::Choice => Shape::Ints(1),
            K::Integer2D => Shape::Ints(2),
            K::Integer3D => Shape::Ints(3),
            K::Double => Shape::Doubles(1),
            K::Double2D => Shape::Doubles(2),
            K::Double3D | K::Rgb => Shape::Doubles(3),
            K::Rgba => Shape::Doubles(4),
            K::String | K::Custom | K::StrChoice => Shape::Text,
            K::Bytes => Shape::Bytes,
            K::Group | K::Page | K::PushButton => Shape::None,
        }
    }

    fn is_numeric(self) -> bool {
        use ParamKind as K;
        matches!(
            self,
            K::Integer | K::Double | K::Integer2D | K::Double2D | K::Integer3D | K::Double3D
        )
    }

    fn is_double(self) -> bool {
        matches!(
            self,
            ParamKind::Double | ParamKind::Double2D | ParamKind::Double3D
        )
    }

    fn dimension_labels(self) -> Option<&'static [&'static str]> {
        use ParamKind as K;
        match self {
            K::Double2D | K::Integer2D => Some(&["x", "y"]),
            K::Double3D | K::Integer3D => Some(&["x", "y", "z"]),
            K::Rgb => Some(&["r", "g", "b"]),
            K::Rgba => Some(&["r", "g", "b", "a"]),
            _ => None,
        }
    }
}

/// One parameter value.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Ints(Vec<i32>),
    Doubles(Vec<f64>),
    Text(String),
    Bytes(Vec<u8>),
    None,
}

impl ParamValue {
    /// The zero value of `kind`: 0, `""`, empty bytes.
    pub fn zero(kind: ParamKind) -> ParamValue {
        match kind.shape() {
            Shape::Ints(n) => ParamValue::Ints(vec![0; n]),
            Shape::Doubles(n) => ParamValue::Doubles(vec![0.0; n]),
            Shape::Text => ParamValue::Text(String::new()),
            Shape::Bytes => ParamValue::Bytes(Vec::new()),
            Shape::None => ParamValue::None,
        }
    }

    /// Whether this value fits `kind`.
    pub fn fits(&self, kind: ParamKind) -> bool {
        match (self, kind.shape()) {
            (ParamValue::Ints(v), Shape::Ints(n)) => v.len() == n,
            (ParamValue::Doubles(v), Shape::Doubles(n)) => v.len() == n,
            (ParamValue::Text(_), Shape::Text)
            | (ParamValue::Bytes(_), Shape::Bytes)
            | (ParamValue::None, Shape::None) => true,
            _ => false,
        }
    }
}

/// Where an instance's parameter values live. Times are in frames, as
/// OpenFX counts them. Every method takes the parameter's name.
pub trait ParamValues: Send + Sync + Debug {
    /// The value at `time`, or `None` if the store does not know `name`.
    fn value_at(&self, name: &str, time: f64) -> Option<ParamValue>;
    /// The key times, sorted.
    fn key_times(&self, name: &str) -> Vec<f64>;
    /// Sets the value: a key at `time`, or the static value for `None`.
    fn set(&self, name: &str, time: Option<f64>, value: ParamValue);
    /// Deletes the key at `time`; false if there is none.
    fn delete_key(&self, name: &str, time: f64) -> bool;
    fn delete_all_keys(&self, name: &str);
    /// Starts a group of changes that the host treats as one undo step.
    fn begin_edit(&self, _label: &str) {}
    fn end_edit(&self) {}
}

/// An in-memory store for the in-process harness. Doubles interpolate
/// linearly between keys and hold outside them; every other type holds the
/// previous key. (In the editor, voXY's own keyframes and easings decide.)
#[derive(Debug, Default)]
pub struct LocalValues {
    inner: Mutex<LocalInner>,
}

/// A parameter's keys: `(time, value)`, sorted by time.
type Keys = Vec<(f64, ParamValue)>;

#[derive(Debug, Default)]
struct LocalInner {
    /// `(name, static value, keys)`.
    params: Vec<(String, ParamValue, Keys)>,
    /// Edit groups opened and closed, for tests: (label, depth after).
    edits: Vec<(String, usize)>,
    depth: usize,
}

impl LocalValues {
    pub fn new() -> LocalValues {
        LocalValues::default()
    }

    /// Adds `name` with a static value.
    pub fn insert(&self, name: &str, value: ParamValue) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.params.retain(|(n, ..)| n != name);
        inner.params.push((name.to_owned(), value, Vec::new()));
    }

    /// The edit groups seen so far: `(label, depth)` on begin, `("", depth)`
    /// on end.
    pub fn edit_log(&self) -> Vec<(String, usize)> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .edits
            .clone()
    }

    fn with<R>(
        &self,
        name: &str,
        f: impl FnOnce(&mut ParamValue, &mut Vec<(f64, ParamValue)>) -> R,
    ) -> Option<R> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner
            .params
            .iter_mut()
            .find(|(n, ..)| n == name)
            .map(|(_, v, keys)| f(v, keys))
    }
}

fn lerp(from: &ParamValue, to: &ParamValue, amount: f64) -> ParamValue {
    match (from, to) {
        (ParamValue::Doubles(lo), ParamValue::Doubles(hi)) => ParamValue::Doubles(
            lo.iter()
                .zip(hi)
                .map(|(lo, hi)| lo + (hi - lo) * amount)
                .collect(),
        ),
        _ => from.clone(),
    }
}

impl ParamValues for LocalValues {
    fn value_at(&self, name: &str, time: f64) -> Option<ParamValue> {
        self.with(name, |v, keys| {
            let Some(first) = keys.first() else {
                return v.clone();
            };
            if time <= first.0 {
                return first.1.clone();
            }
            for w in keys.windows(2) {
                let ((t0, a), (t1, b)) = (&w[0], &w[1]);
                if time < *t1 {
                    return lerp(a, b, (time - t0) / (t1 - t0));
                }
            }
            keys.last().map_or_else(|| v.clone(), |k| k.1.clone())
        })
    }

    fn key_times(&self, name: &str) -> Vec<f64> {
        self.with(name, |_, keys| keys.iter().map(|k| k.0).collect())
            .unwrap_or_default()
    }

    fn set(&self, name: &str, time: Option<f64>, value: ParamValue) {
        if self.with(name, |_, _| ()).is_none() {
            self.insert(name, value.clone());
        }
        self.with(name, |v, keys| match time {
            None => *v = value,
            Some(t) => match keys.iter().position(|k| k.0 >= t) {
                Some(i) if keys[i].0 == t => keys[i].1 = value,
                Some(i) => keys.insert(i, (t, value)),
                None => keys.push((t, value)),
            },
        });
    }

    fn delete_key(&self, name: &str, time: f64) -> bool {
        self.with(name, |_, keys| {
            let before = keys.len();
            keys.retain(|k| k.0 != time);
            keys.len() != before
        })
        .unwrap_or(false)
    }

    fn delete_all_keys(&self, name: &str) {
        self.with(name, |_, keys| keys.clear());
    }

    fn begin_edit(&self, label: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.depth += 1;
        let d = inner.depth;
        inner.edits.push((label.to_owned(), d));
    }

    fn end_edit(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.depth = inner.depth.saturating_sub(1);
        let d = inner.depth;
        inner.edits.push((String::new(), d));
    }
}

/// One parameter: its name, type and property set.
#[derive(Debug)]
pub struct Param {
    pub name: String,
    pub kind: ParamKind,
    pub props: Arc<PropSet>,
    /// Strings and byte buffers handed to the plugin, kept alive until the
    /// next few reads (the spec's note on strings).
    pub(crate) handed_out: Mutex<Vec<Box<[u8]>>>,
}

/// How many returned strings or buffers a parameter keeps alive.
pub(crate) const HANDED_OUT: usize = 16;

impl Param {
    fn new(name: &str, kind: ParamKind, props: Arc<PropSet>) -> Param {
        Param {
            name: name.to_owned(),
            kind,
            props,
            handed_out: Mutex::new(Vec::new()),
        }
    }

    /// Keeps `buf` alive for a while and returns a pointer to it.
    pub(crate) fn hand_out(&self, buf: Box<[u8]>) -> *const u8 {
        let mut held = self
            .handed_out
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let p = buf.as_ptr();
        held.push(buf);
        if held.len() > HANDED_OUT {
            held.remove(0);
        }
        p
    }

    /// The default from `kOfxParamPropDefault`.
    pub fn default_value(&self) -> ParamValue {
        let get = |name: &str| self.props.get(name);
        let default = String::from_utf8_lossy(
            ofx::kOfxParamPropDefault
                .strip_suffix(b"\0")
                .unwrap_or_default(),
        )
        .into_owned();
        match (self.kind.shape(), get(&default)) {
            (Shape::Ints(_), Some(Values::Int(v))) => ParamValue::Ints(v),
            (Shape::Doubles(_), Some(Values::Double(v))) => ParamValue::Doubles(v),
            (Shape::Text, Some(Values::String(v))) => ParamValue::Text(
                v.first()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            // Bytes defaults are copied in when the plugin sets them.
            _ => ParamValue::zero(self.kind),
        }
    }
}

/// Whether a parameter set belongs to a descriptor or an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Descriptor,
    Instance,
}

/// Why `define` failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefineError {
    /// A parameter of that name exists (`kOfxStatErrExists`).
    Exists,
    /// Not a descriptor's set (`kOfxStatErrBadHandle`).
    NotDescriptor,
}

/// The parameters of one descriptor or instance.
#[derive(Debug)]
pub struct ParamSet {
    pub role: Role,
    /// The set's own property set (`paramSetGetPropertySet`): the effect's.
    pub props: Arc<PropSet>,
    params: Mutex<Vec<Arc<Param>>>,
    /// Bytes defaults set on descriptors, by parameter name.
    pub(crate) bytes_defaults: Mutex<Vec<(String, Vec<u8>)>>,
    /// The instance's value store (unused on descriptors).
    pub values: Option<Arc<dyn ParamValues>>,
    /// The instance's current time, in frames, as `f64` bits.
    time: AtomicU64,
}

impl ParamSet {
    pub fn descriptor(props: Arc<PropSet>) -> ParamSet {
        ParamSet {
            role: Role::Descriptor,
            props,
            params: Mutex::new(Vec::new()),
            bytes_defaults: Mutex::new(Vec::new()),
            values: None,
            time: AtomicU64::new(0),
        }
    }

    /// An instance of `descriptor`: the same parameters with copies of
    /// their properties (the instance-only ones made read-only), and every
    /// value in `values` starting at its default unless the store already
    /// has it.
    pub fn instance(
        descriptor: &ParamSet,
        props: Arc<PropSet>,
        values: Arc<dyn ParamValues>,
    ) -> ParamSet {
        let params: Vec<Arc<Param>> = descriptor
            .params()
            .iter()
            .map(|p| {
                let copy = Arc::new(p.props.duplicate("parameter instance"));
                for name in copy.names() {
                    if !INSTANCE_WRITABLE
                        .iter()
                        .any(|w| w.strip_suffix(b"\0") == Some(name.as_bytes()))
                    {
                        copy.set_plugin_writable(&name, false);
                    }
                }
                Arc::new(Param::new(&p.name, p.kind, copy))
            })
            .collect();
        let bytes = descriptor
            .bytes_defaults
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for p in &params {
            if p.kind.shape() == Shape::None || values.value_at(&p.name, 0.0).is_some() {
                continue;
            }
            let default = match p.kind {
                ParamKind::Bytes => ParamValue::Bytes(
                    bytes
                        .iter()
                        .find(|(n, _)| *n == p.name)
                        .map(|(_, b)| b.clone())
                        .unwrap_or_default(),
                ),
                _ => p.default_value(),
            };
            values.set(&p.name, None, default);
        }
        ParamSet {
            role: Role::Instance,
            props,
            params: Mutex::new(params),
            bytes_defaults: Mutex::new(bytes),
            values: Some(values),
            time: AtomicU64::new(0),
        }
    }

    pub fn params(&self) -> Vec<Arc<Param>> {
        self.params
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn get(&self, name: &str) -> Option<Arc<Param>> {
        self.params
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|p| p.name == name)
            .cloned()
    }

    /// `paramDefine`: a new parameter with its type's descriptor properties.
    pub fn define(&self, kind: ParamKind, name: &str) -> Result<Arc<Param>, DefineError> {
        if self.role != Role::Descriptor {
            return Err(DefineError::NotDescriptor);
        }
        let mut params = self.params.lock().unwrap_or_else(PoisonError::into_inner);
        if params.iter().any(|p| p.name == name) {
            return Err(DefineError::Exists);
        }
        let props = Arc::new(PropSet::new("parameter descriptor"));
        define_props(&props, kind, name);
        let p = Arc::new(Param::new(name, kind, props));
        params.push(Arc::clone(&p));
        Ok(p)
    }

    pub fn set_time(&self, frames: f64) {
        self.time.store(frames.to_bits(), Ordering::Relaxed);
    }

    /// The time "current value" calls use.
    pub fn time(&self) -> f64 {
        f64::from_bits(self.time.load(Ordering::Relaxed))
    }
}

/// Parameter properties the plugin may still change on an instance (the
/// spec marks them "instance (read/write)").
const INSTANCE_WRITABLE: &[&[u8]] = &[
    ofx::kOfxPropLabel,
    ofx::kOfxPropShortLabel,
    ofx::kOfxPropLongLabel,
    ofx::kOfxParamPropHint,
    ofx::kOfxParamPropEnabled,
    ofx::kOfxParamPropSecret,
    ofx::kOfxParamPropDataPtr,
    ofx::kOfxParamPropEvaluateOnChange,
    ofx::kOfxParamPropMin,
    ofx::kOfxParamPropMax,
    ofx::kOfxParamPropDisplayMin,
    ofx::kOfxParamPropDisplayMax,
    ofx::kOfxParamPropIncrement,
    ofx::kOfxParamPropDigits,
    ofx::kOfxParamPropChoiceOption,
    ofx::kOfxParamPropChoiceOrder,
    ofx::kOfxParamPropChoiceEnum,
    ofx::kOfxParamPropGroupOpen,
    ofx::kOfxPropIcon,
];

fn key(name: &[u8]) -> &str {
    crate::props::ofx_str(name)
}

/// The descriptor properties of a parameter of type `kind` named `name`,
/// with the spec's defaults.
fn define_props(p: &PropSet, kind: ParamKind, name: &str) {
    let rw = |n: &[u8], v: Values| p.define_rw(key(n), v);
    let ro = |n: &[u8], v: Values| p.define_ro(key(n), v);
    let var = |n: &[u8], v: Values| p.define(key(n), v, Dim::Variable, true);

    ro(ofx::kOfxParamPropType, Values::strings(&[kind.ofx_name()]));
    ro(ofx::kOfxPropName, Values::strings(&[name]));
    rw(ofx::kOfxPropLabel, Values::strings(&[name]));
    rw(ofx::kOfxPropShortLabel, Values::strings(&[name]));
    rw(ofx::kOfxPropLongLabel, Values::strings(&[name]));
    rw(ofx::kOfxParamPropSecret, Values::Int(vec![0]));
    rw(ofx::kOfxParamPropCanUndo, Values::Int(vec![1]));
    rw(ofx::kOfxParamPropHint, Values::strings(&[""]));
    rw(ofx::kOfxParamPropScriptName, Values::strings(&[name]));
    rw(ofx::kOfxParamPropParent, Values::strings(&[""]));
    rw(ofx::kOfxParamPropEnabled, Values::Int(vec![1]));
    rw(ofx::kOfxParamPropDataPtr, Values::Pointer(vec![0]));
    rw(ofx::kOfxPropIcon, Values::strings(&["", ""]));

    match kind {
        ParamKind::Group => {
            rw(ofx::kOfxParamPropGroupOpen, Values::Int(vec![1]));
            return;
        }
        ParamKind::Page => {
            var(ofx::kOfxParamPropPageChild, Values::String(Vec::new()));
            return;
        }
        ParamKind::PushButton => return,
        _ => {}
    }

    // Valued parameters.
    rw(ofx::kOfxParamPropInteractV1, Values::Pointer(vec![0]));
    rw(
        ofx::kOfxParamPropInteractSize,
        Values::Double(vec![0.0, 0.0]),
    );
    rw(
        ofx::kOfxParamPropInteractSizeAspect,
        Values::Double(vec![1.0]),
    );
    rw(
        ofx::kOfxParamPropInteractMinimumSize,
        Values::Int(vec![10, 10]),
    );
    rw(
        ofx::kOfxParamPropInteractPreferedSize,
        Values::Int(vec![10, 10]),
    );
    ro(ofx::kOfxParamPropHasHostOverlayHandle, Values::Int(vec![0]));
    rw(ofx::kOfxParamPropUseHostOverlayHandle, Values::Int(vec![0]));
    rw(ofx::kOfxParamPropAnimates, Values::Int(vec![1]));
    ro(ofx::kOfxParamPropIsAnimating, Values::Int(vec![0]));
    ro(ofx::kOfxParamPropIsAutoKeying, Values::Int(vec![0]));
    rw(ofx::kOfxParamPropPersistant, Values::Int(vec![1]));
    rw(ofx::kOfxParamPropEvaluateOnChange, Values::Int(vec![1]));
    rw(ofx::kOfxParamPropPluginMayWrite, Values::Int(vec![0]));
    rw(
        ofx::kOfxParamPropCacheInvalidation,
        Values::strings(&[key(ofx::kOfxParamInvalidateValueChange)]),
    );
    let default = match kind.shape() {
        Shape::Ints(n) => Values::Int(vec![0; n]),
        Shape::Doubles(n) => Values::Double(vec![0.0; n]),
        Shape::Text => Values::strings(&[""]),
        Shape::Bytes | Shape::None => Values::Pointer(vec![0]),
    };
    rw(ofx::kOfxParamPropDefault, default);

    if kind.is_numeric() {
        let (lo, hi) = match kind.shape() {
            Shape::Ints(n) => (
                Values::Int(vec![i32::MIN; n]),
                Values::Int(vec![i32::MAX; n]),
            ),
            Shape::Doubles(n) => (
                Values::Double(vec![-f64::MAX; n]),
                Values::Double(vec![f64::MAX; n]),
            ),
            _ => unreachable!("numeric kinds hold ints or doubles"),
        };
        rw(ofx::kOfxParamPropMin, lo.clone());
        rw(ofx::kOfxParamPropDisplayMin, lo);
        rw(ofx::kOfxParamPropMax, hi.clone());
        rw(ofx::kOfxParamPropDisplayMax, hi);
    }
    if kind.is_double() {
        rw(ofx::kOfxParamPropIncrement, Values::Double(vec![1.0]));
        rw(ofx::kOfxParamPropDigits, Values::Int(vec![2]));
        rw(
            ofx::kOfxParamPropDoubleType,
            Values::strings(&[key(ofx::kOfxParamDoubleTypePlain)]),
        );
        rw(
            ofx::kOfxParamPropDefaultCoordinateSystem,
            Values::strings(&[key(ofx::kOfxParamCoordinatesCanonical)]),
        );
        if kind == ParamKind::Double {
            rw(ofx::kOfxParamPropShowTimeMarker, Values::Int(vec![0]));
        }
    }
    if let Some(labels) = kind.dimension_labels() {
        rw(ofx::kOfxParamPropDimensionLabel, Values::strings(labels));
    }
    match kind {
        ParamKind::Choice => {
            var(ofx::kOfxParamPropChoiceOption, Values::String(Vec::new()));
            var(ofx::kOfxParamPropChoiceOrder, Values::Int(Vec::new()));
        }
        ParamKind::StrChoice => {
            var(ofx::kOfxParamPropChoiceOption, Values::String(Vec::new()));
            var(ofx::kOfxParamPropChoiceEnum, Values::String(Vec::new()));
        }
        ParamKind::String => {
            rw(
                ofx::kOfxParamPropStringMode,
                Values::strings(&[key(ofx::kOfxParamStringIsSingleLine)]),
            );
            rw(ofx::kOfxParamPropStringFilePathExists, Values::Int(vec![1]));
        }
        ParamKind::Custom => {
            rw(
                ofx::kOfxParamPropCustomInterpCallbackV1,
                Values::Pointer(vec![0]),
            );
        }
        _ => {}
    }
}
