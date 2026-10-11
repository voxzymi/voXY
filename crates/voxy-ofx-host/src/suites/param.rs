//! `OfxParameterSuiteV1` over [`ParamSet`] and [`ParamValues`].
//!
//! Host choices where the spec leaves room (also in `docs/ofx-host.md`):
//! - values are read and written only on instances; on a descriptor those
//!   calls get `kOfxStatErrBadHandle`, and `paramDefine` on an instance
//!   gets the same;
//! - `paramSetValue` on a parameter with keys sets a key at the instance's
//!   current time, otherwise the static value;
//! - `paramSetValueAtTime` on a parameter that does not animate gets
//!   `kOfxStatErrValue`;
//! - a choice index outside the options, or a string choice not among its
//!   enum values, gets `kOfxStatErrValue`;
//! - Custom and Bytes values over 1 MiB get `kOfxStatErrMemory` (P-0 Q3);
//! - derivatives use a central difference over 1/1000 frame, integrals
//!   Simpson's rule over 64 steps, on double types only;
//! - "a key at the indicated time" means within 1/10000 frame;
//! - group, page and push-button parameters have no value
//!   (`kOfxStatErrUnsupported`), and parametric parameters are not
//!   supported yet.

use std::collections::HashMap;
use std::ffi::{VaList, c_char, c_int, c_uint};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};

use voxy_ofx_sys as ofx;

use super::{PROP_SETS, Registry, c_str, guard, prop_handle, release_prop_handle};
use crate::params::{DefineError, Param, ParamKind, ParamSet, ParamValue, ParamValues, Shape};
use crate::props::Values;

pub(super) static SUITE: ofx::OfxParameterSuiteV1 = ofx::OfxParameterSuiteV1 {
    paramDefine: Some(define),
    paramGetHandle: Some(get_handle),
    paramSetGetPropertySet: Some(set_props),
    paramGetPropertySet: Some(param_props),
    paramGetValue: Some(get_value),
    paramGetValueAtTime: Some(get_value_at_time),
    paramGetDerivative: Some(get_derivative),
    paramGetIntegral: Some(get_integral),
    paramSetValue: Some(set_value),
    paramSetValueAtTime: Some(set_value_at_time),
    paramGetNumKeys: Some(num_keys),
    paramGetKeyTime: Some(key_time),
    paramGetKeyIndex: Some(key_index),
    paramDeleteKey: Some(delete_key),
    paramDeleteAllKeys: Some(delete_all_keys),
    paramCopy: Some(copy),
    paramEditBegin: Some(edit_begin),
    paramEditEnd: Some(edit_end),
};

/// The largest Custom or Bytes value accepted (P-0 Q3).
pub const MAX_OPAQUE_BYTES: usize = 1 << 20;

/// "A key at the indicated time": within this many frames.
const KEY_EPSILON: f64 = 1e-4;

pub(crate) static PARAM_SETS: LazyLock<Registry<ParamSet>> = LazyLock::new(Registry::new);

/// A live parameter and the set it belongs to.
type Entry = (Arc<Param>, Weak<ParamSet>);

/// What instance calls work on: the parameter, its set and the store.
type Instance = (Arc<Param>, Arc<ParamSet>, Arc<dyn ParamValues>);

/// Live parameters by handle.
static PARAMS: LazyLock<Mutex<HashMap<usize, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn register_param(p: &Arc<Param>, set: &Arc<ParamSet>) -> ofx::OfxParamHandle {
    prop_handle(&p.props);
    let addr = Arc::as_ptr(p) as usize;
    PARAMS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(addr, (Arc::clone(p), Arc::downgrade(set)));
    addr as ofx::OfxParamHandle
}

/// The handle the plugin sees for `set` (and makes its parameters'
/// handles live).
pub fn param_set_handle(set: &Arc<ParamSet>) -> ofx::OfxParamSetHandle {
    for p in set.params() {
        register_param(&p, set);
    }
    PARAM_SETS.register(set) as ofx::OfxParamSetHandle
}

/// Withdraws `set`'s handle and its parameters' handles.
pub fn release_param_set_handle(set: &Arc<ParamSet>) {
    let mut params = PARAMS.lock().unwrap_or_else(PoisonError::into_inner);
    for p in set.params() {
        params.remove(&(Arc::as_ptr(&p) as usize));
        release_prop_handle(&p.props);
    }
    drop(params);
    PARAM_SETS.unregister(set);
}

fn lookup(h: ofx::OfxParamHandle) -> Option<(Arc<Param>, Arc<ParamSet>)> {
    let params = PARAMS.lock().unwrap_or_else(PoisonError::into_inner);
    let (p, set) = params.get(&(h as usize))?;
    Some((Arc::clone(p), set.upgrade()?))
}

/// The instance store behind a parameter, or the status to return.
fn instance(h: ofx::OfxParamHandle) -> Result<Instance, ofx::OfxStatus> {
    let (p, set) = lookup(h).ok_or(ofx::kOfxStatErrBadHandle)?;
    let values = set.values.clone().ok_or(ofx::kOfxStatErrBadHandle)?;
    if p.kind.shape() == Shape::None {
        return Err(ofx::kOfxStatErrUnsupported);
    }
    Ok((p, set, values))
}

fn status(r: Result<(), ofx::OfxStatus>) -> ofx::OfxStatus {
    r.err().unwrap_or(ofx::kOfxStatOK)
}

/// Reads every Bytes parameter's default (`kOfxParamPropDefault`, an
/// `OfxBytes *` the plugin set while describing) into the set, so an
/// instance can start from it after the plugin's buffer is gone. A default
/// over the cap is ignored.
///
/// # Safety
/// Each non-null default points to a valid `OfxBytes` whose `data` has
/// `length` readable bytes, as the spec requires during Describe.
pub unsafe fn capture_bytes_defaults(set: &ParamSet) {
    let name = String::from_utf8_lossy(
        ofx::kOfxParamPropDefault
            .strip_suffix(b"\0")
            .unwrap_or_default(),
    )
    .into_owned();
    let mut captured = Vec::new();
    for p in set.params() {
        if p.kind != ParamKind::Bytes {
            continue;
        }
        let Some(Values::Pointer(v)) = p.props.get(&name) else {
            continue;
        };
        let Some(&addr) = v.first().filter(|a| **a != 0) else {
            continue;
        };
        // SAFETY: the caller's contract.
        let bytes = unsafe { &*(addr as *const ofx::OfxBytes) };
        if bytes.length > MAX_OPAQUE_BYTES || (bytes.data.is_null() && bytes.length > 0) {
            continue;
        }
        let data = if bytes.length == 0 {
            Vec::new()
        } else {
            // SAFETY: the caller's contract.
            unsafe { std::slice::from_raw_parts(bytes.data, bytes.length) }.to_vec()
        };
        captured.push((p.name.clone(), data));
    }
    *set.bytes_defaults
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = captured;
}

unsafe extern "C" fn define(
    set: ofx::OfxParamSetHandle,
    kind: *const c_char,
    name: *const c_char,
    props: *mut ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    guard(|| {
        let Some(s) = PARAM_SETS.get(set as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        // SAFETY: the plugin passes valid strings.
        let (Some(kind), Some(name)) = (unsafe { c_str(kind) }, unsafe { c_str(name) }) else {
            return ofx::kOfxStatErrValue;
        };
        let Some(kind) = ParamKind::from_ofx(kind.to_bytes()) else {
            let known = kind.to_bytes() == b"OfxParamTypeParametric";
            return if known {
                ofx::kOfxStatErrUnsupported
            } else {
                ofx::kOfxStatErrUnknown
            };
        };
        match s.define(kind, &name.to_string_lossy()) {
            Ok(p) => {
                register_param(&p, &s);
                if !props.is_null() {
                    // SAFETY: a valid out pointer from the plugin.
                    unsafe { props.write(prop_handle(&p.props)) };
                }
                ofx::kOfxStatOK
            }
            Err(DefineError::Exists) => ofx::kOfxStatErrExists,
            Err(DefineError::NotDescriptor) => ofx::kOfxStatErrBadHandle,
        }
    })
}

unsafe extern "C" fn get_handle(
    set: ofx::OfxParamSetHandle,
    name: *const c_char,
    out: *mut ofx::OfxParamHandle,
    props: *mut ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    guard(|| {
        let Some(s) = PARAM_SETS.get(set as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        // SAFETY: the plugin passes a valid name.
        let Some(name) = (unsafe { c_str(name) }) else {
            return ofx::kOfxStatErrValue;
        };
        let Some(p) = s.get(&name.to_string_lossy()) else {
            return ofx::kOfxStatErrUnknown;
        };
        let h = register_param(&p, &s);
        // SAFETY: valid out pointers from the plugin (either may be null).
        unsafe {
            if !out.is_null() {
                out.write(h);
            }
            if !props.is_null() {
                props.write(prop_handle(&p.props));
            }
        }
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn set_props(
    set: ofx::OfxParamSetHandle,
    out: *mut ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    guard(|| {
        let Some(s) = PARAM_SETS.get(set as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        if out.is_null() {
            return ofx::kOfxStatErrValue;
        }
        // SAFETY: a valid out pointer.
        unsafe { out.write(prop_handle(&s.props)) };
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn param_props(
    h: ofx::OfxParamHandle,
    out: *mut ofx::OfxPropertySetHandle,
) -> ofx::OfxStatus {
    guard(|| {
        let Some((p, _)) = lookup(h) else {
            return ofx::kOfxStatErrBadHandle;
        };
        if out.is_null() {
            return ofx::kOfxStatErrValue;
        }
        debug_assert!(PROP_SETS.get(Arc::as_ptr(&p.props) as usize).is_some());
        // SAFETY: a valid out pointer.
        unsafe { out.write(prop_handle(&p.props)) };
        ofx::kOfxStatOK
    })
}

/// Writes `value` through the out pointers in `args`.
///
/// # Safety
/// `args` holds out pointers of the parameter type's kind and count.
unsafe fn write_out(
    p: &Param,
    value: &ParamValue,
    args: &mut VaList<'_>,
) -> Result<(), ofx::OfxStatus> {
    // SAFETY (whole body): the caller's contract; each pointer is checked
    // for null before writing.
    unsafe {
        match value {
            ParamValue::Ints(v) => {
                for x in v {
                    let out = args.next_arg::<*mut c_int>();
                    if out.is_null() {
                        return Err(ofx::kOfxStatErrValue);
                    }
                    out.write(*x);
                }
            }
            ParamValue::Doubles(v) => {
                for x in v {
                    let out = args.next_arg::<*mut f64>();
                    if out.is_null() {
                        return Err(ofx::kOfxStatErrValue);
                    }
                    out.write(*x);
                }
            }
            ParamValue::Text(s) => {
                let out = args.next_arg::<*mut *const c_char>();
                if out.is_null() {
                    return Err(ofx::kOfxStatErrValue);
                }
                let c = crate::props::c_string(s).into_bytes_with_nul();
                out.write(p.hand_out(c.into_boxed_slice()).cast());
            }
            ParamValue::Bytes(b) => {
                let out = args.next_arg::<*mut ofx::OfxBytes>();
                if out.is_null() {
                    return Err(ofx::kOfxStatErrValue);
                }
                let len = b.len();
                let data = p.hand_out(b.clone().into_boxed_slice());
                out.write(ofx::OfxBytes {
                    data: if len == 0 { std::ptr::null() } else { data },
                    length: len,
                });
            }
            ParamValue::None => return Err(ofx::kOfxStatErrUnsupported),
        }
    }
    Ok(())
}

fn value_at(p: &Param, values: &dyn ParamValues, time: f64) -> Result<ParamValue, ofx::OfxStatus> {
    values
        .value_at(&p.name, time)
        .filter(|v| v.fits(p.kind))
        .ok_or(ofx::kOfxStatErrUnknown)
}

/// # Safety
/// `args` matches the parameter type.
unsafe fn get_at(
    h: ofx::OfxParamHandle,
    time: Option<f64>,
    args: &mut VaList<'_>,
) -> ofx::OfxStatus {
    guard(|| {
        status((|| {
            let (p, set, values) = instance(h)?;
            let v = value_at(&p, values.as_ref(), time.unwrap_or_else(|| set.time()))?;
            // SAFETY: the caller's contract.
            unsafe { write_out(&p, &v, args) }
        })())
    })
}

unsafe extern "C" fn get_value(h: ofx::OfxParamHandle, mut args: ...) -> ofx::OfxStatus {
    // SAFETY: the plugin passes out pointers of the parameter's type.
    unsafe { get_at(h, None, &mut args) }
}

unsafe extern "C" fn get_value_at_time(
    h: ofx::OfxParamHandle,
    time: f64,
    mut args: ...
) -> ofx::OfxStatus {
    // SAFETY: as above.
    unsafe { get_at(h, Some(time), &mut args) }
}

/// A double parameter's samples combined by `f`, written out.
///
/// # Safety
/// `args` holds one `double *` per dimension.
unsafe fn doubles_out(
    h: ofx::OfxParamHandle,
    args: &mut VaList<'_>,
    f: impl Fn(&dyn Fn(f64) -> Option<Vec<f64>>) -> Option<Vec<f64>>,
) -> ofx::OfxStatus {
    guard(|| {
        status((|| {
            let (p, _, values) = instance(h)?;
            if !matches!(p.kind.shape(), Shape::Doubles(_)) {
                return Err(ofx::kOfxStatErrUnsupported);
            }
            let sample = |t: f64| match value_at(&p, values.as_ref(), t) {
                Ok(ParamValue::Doubles(v)) => Some(v),
                _ => None,
            };
            let v = f(&sample).ok_or(ofx::kOfxStatErrUnknown)?;
            // SAFETY: the caller's contract.
            unsafe { write_out(&p, &ParamValue::Doubles(v), args) }
        })())
    })
}

unsafe extern "C" fn get_derivative(
    h: ofx::OfxParamHandle,
    time: f64,
    mut args: ...
) -> ofx::OfxStatus {
    const H: f64 = 1e-3;
    // SAFETY: the plugin passes one double pointer per dimension.
    unsafe {
        doubles_out(h, &mut args, |s| {
            let (a, b) = (s(time - H)?, s(time + H)?);
            Some(a.iter().zip(&b).map(|(a, b)| (b - a) / (2.0 * H)).collect())
        })
    }
}

unsafe extern "C" fn get_integral(
    h: ofx::OfxParamHandle,
    t1: f64,
    t2: f64,
    mut args: ...
) -> ofx::OfxStatus {
    const STEPS: u32 = 64;
    // SAFETY: as above.
    unsafe {
        doubles_out(h, &mut args, |s| {
            let step = (t2 - t1) / f64::from(STEPS);
            let mut sum = s(t1)?;
            for (i, x) in s(t2)?.iter().enumerate() {
                sum[i] += x;
            }
            for k in 1..STEPS {
                let w = if k % 2 == 1 { 4.0 } else { 2.0 };
                for (i, x) in s(t1 + step * f64::from(k))?.iter().enumerate() {
                    sum[i] += w * x;
                }
            }
            Some(sum.iter().map(|x| x * step / 3.0).collect())
        })
    }
}

/// Reads a value of `p`'s type from `args`, checking it.
///
/// # Safety
/// `args` holds values of the parameter type's kind and count.
unsafe fn read_in(p: &Param, args: &mut VaList<'_>) -> Result<ParamValue, ofx::OfxStatus> {
    let choices = |prop: &[u8]| {
        p.props.strings(
            std::str::from_utf8(prop.strip_suffix(b"\0").unwrap_or(prop)).unwrap_or_default(),
        )
    };
    // SAFETY (whole body): the caller's contract.
    unsafe {
        Ok(match p.kind.shape() {
            Shape::Ints(n) => {
                let mut v: Vec<i32> = (0..n).map(|_| args.next_arg::<c_int>()).collect();
                match p.kind {
                    ParamKind::Boolean => v[0] = i32::from(v[0] != 0),
                    ParamKind::Choice => {
                        let options = choices(ofx::kOfxParamPropChoiceOption).len();
                        if usize::try_from(v[0]).map_or(true, |i| options > 0 && i >= options) {
                            return Err(ofx::kOfxStatErrValue);
                        }
                    }
                    _ => {}
                }
                ParamValue::Ints(v)
            }
            Shape::Doubles(n) => {
                ParamValue::Doubles((0..n).map(|_| args.next_arg::<f64>()).collect())
            }
            Shape::Text => {
                let s = c_str(args.next_arg::<*const c_char>()).ok_or(ofx::kOfxStatErrValue)?;
                if p.kind == ParamKind::Custom && s.to_bytes().len() > MAX_OPAQUE_BYTES {
                    return Err(ofx::kOfxStatErrMemory);
                }
                let s = s.to_string_lossy().into_owned();
                if p.kind == ParamKind::StrChoice {
                    let allowed = choices(ofx::kOfxParamPropChoiceEnum);
                    if !allowed.is_empty() && !allowed.contains(&s) {
                        return Err(ofx::kOfxStatErrValue);
                    }
                }
                ParamValue::Text(s)
            }
            Shape::Bytes => {
                let b = args.next_arg::<*const ofx::OfxBytes>();
                if b.is_null() {
                    return Err(ofx::kOfxStatErrValue);
                }
                let b = &*b;
                if b.length > MAX_OPAQUE_BYTES {
                    return Err(ofx::kOfxStatErrMemory);
                }
                if b.length == 0 {
                    ParamValue::Bytes(Vec::new())
                } else if b.data.is_null() {
                    return Err(ofx::kOfxStatErrValue);
                } else {
                    ParamValue::Bytes(std::slice::from_raw_parts(b.data, b.length).to_vec())
                }
            }
            Shape::None => return Err(ofx::kOfxStatErrUnsupported),
        })
    }
}

fn animates(p: &Param) -> bool {
    let name = std::str::from_utf8(
        ofx::kOfxParamPropAnimates
            .strip_suffix(b"\0")
            .unwrap_or_default(),
    )
    .unwrap_or_default();
    p.props.int(name, 0).unwrap_or(0) != 0
}

/// # Safety
/// `args` matches the parameter type.
unsafe fn set_at(
    h: ofx::OfxParamHandle,
    time: Option<f64>,
    args: &mut VaList<'_>,
) -> ofx::OfxStatus {
    guard(|| {
        status((|| {
            let (p, set, values) = instance(h)?;
            // SAFETY: the caller's contract.
            let v = unsafe { read_in(&p, args) }?;
            let at = match time {
                Some(_) if !animates(&p) => return Err(ofx::kOfxStatErrValue),
                Some(t) => Some(t),
                None if values.key_times(&p.name).is_empty() => None,
                None => Some(set.time()),
            };
            values.set(&p.name, at, v);
            Ok(())
        })())
    })
}

unsafe extern "C" fn set_value(h: ofx::OfxParamHandle, mut args: ...) -> ofx::OfxStatus {
    // SAFETY: the plugin passes values of the parameter's type.
    unsafe { set_at(h, None, &mut args) }
}

unsafe extern "C" fn set_value_at_time(
    h: ofx::OfxParamHandle,
    time: f64,
    mut args: ...
) -> ofx::OfxStatus {
    // SAFETY: as above.
    unsafe { set_at(h, Some(time), &mut args) }
}

fn keys(h: ofx::OfxParamHandle) -> Result<(Instance, Vec<f64>), ofx::OfxStatus> {
    let inst = instance(h)?;
    let k = inst.2.key_times(&inst.0.name);
    Ok((inst, k))
}

unsafe extern "C" fn num_keys(h: ofx::OfxParamHandle, out: *mut c_uint) -> ofx::OfxStatus {
    guard(|| {
        status((|| {
            let (_, k) = keys(h)?;
            if out.is_null() {
                return Err(ofx::kOfxStatErrValue);
            }
            // SAFETY: a valid out pointer.
            unsafe { out.write(c_uint::try_from(k.len()).unwrap_or(c_uint::MAX)) };
            Ok(())
        })())
    })
}

unsafe extern "C" fn key_time(
    h: ofx::OfxParamHandle,
    nth: c_uint,
    out: *mut f64,
) -> ofx::OfxStatus {
    guard(|| {
        status((|| {
            let (_, k) = keys(h)?;
            let t = k.get(nth as usize).ok_or(ofx::kOfxStatErrBadIndex)?;
            if out.is_null() {
                return Err(ofx::kOfxStatErrValue);
            }
            // SAFETY: a valid out pointer.
            unsafe { out.write(*t) };
            Ok(())
        })())
    })
}

unsafe extern "C" fn key_index(
    h: ofx::OfxParamHandle,
    time: f64,
    direction: c_int,
    out: *mut c_int,
) -> ofx::OfxStatus {
    guard(|| {
        status((|| {
            let (_, k) = keys(h)?;
            if out.is_null() {
                return Err(ofx::kOfxStatErrValue);
            }
            let found = match direction.signum() {
                0 => k.iter().position(|t| (t - time).abs() < KEY_EPSILON),
                1 => k.iter().position(|t| *t > time + KEY_EPSILON),
                _ => k.iter().rposition(|t| *t < time - KEY_EPSILON),
            };
            let index = found.map_or(-1, |i| c_int::try_from(i).unwrap_or(c_int::MAX));
            // SAFETY: a valid out pointer.
            unsafe { out.write(index) };
            if found.is_some() {
                Ok(())
            } else {
                Err(ofx::kOfxStatFailed)
            }
        })())
    })
}

unsafe extern "C" fn delete_key(h: ofx::OfxParamHandle, time: f64) -> ofx::OfxStatus {
    guard(|| {
        status((|| {
            let ((p, _, values), k) = keys(h)?;
            let t = k
                .iter()
                .find(|t| (*t - time).abs() < KEY_EPSILON)
                .ok_or(ofx::kOfxStatErrBadIndex)?;
            if values.delete_key(&p.name, *t) {
                Ok(())
            } else {
                Err(ofx::kOfxStatErrBadIndex)
            }
        })())
    })
}

unsafe extern "C" fn delete_all_keys(h: ofx::OfxParamHandle) -> ofx::OfxStatus {
    guard(|| {
        status((|| {
            let (p, set, values) = instance(h)?;
            // Keep the value the parameter shows now as its static value.
            let now = value_at(&p, values.as_ref(), set.time())?;
            values.delete_all_keys(&p.name);
            values.set(&p.name, None, now);
            Ok(())
        })())
    })
}

unsafe extern "C" fn copy(
    to: ofx::OfxParamHandle,
    from: ofx::OfxParamHandle,
    offset: f64,
    range: *const ofx::OfxRangeD,
) -> ofx::OfxStatus {
    guard(|| {
        status((|| {
            let (dst, _, dst_values) = instance(to)?;
            let (src, src_set, src_values) = instance(from)?;
            if dst.kind != src.kind {
                return Err(ofx::kOfxStatErrValue);
            }
            // SAFETY: null or a valid range from the plugin.
            let range = unsafe { range.as_ref() }.map(|r| (r.min, r.max));
            let static_value = value_at(&src, src_values.as_ref(), src_set.time())?;
            dst_values.delete_all_keys(&dst.name);
            dst_values.set(&dst.name, None, static_value);
            for t in src_values.key_times(&src.name) {
                if range.is_some_and(|(lo, hi)| t < lo || t > hi) {
                    continue;
                }
                let v = value_at(&src, src_values.as_ref(), t)?;
                dst_values.set(&dst.name, Some(t + offset), v);
            }
            Ok(())
        })())
    })
}

unsafe extern "C" fn edit_begin(
    set: ofx::OfxParamSetHandle,
    name: *const c_char,
) -> ofx::OfxStatus {
    guard(|| {
        let Some(s) = PARAM_SETS.get(set as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        let Some(values) = &s.values else {
            return ofx::kOfxStatErrBadHandle;
        };
        // SAFETY: null or a valid name from the plugin.
        let label = unsafe { c_str(name) }.map(|n| n.to_string_lossy().into_owned());
        values.begin_edit(label.as_deref().unwrap_or_default());
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn edit_end(set: ofx::OfxParamSetHandle) -> ofx::OfxStatus {
    guard(|| {
        let Some(s) = PARAM_SETS.get(set as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        let Some(values) = &s.values else {
            return ofx::kOfxStatErrBadHandle;
        };
        values.end_edit();
        ofx::kOfxStatOK
    })
}
