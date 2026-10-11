//! `OfxPropertySuiteV1` over [`PropSet`].
//!
//! Host choices where the spec is silent (also in `docs/ofx-host.md`):
//! - getting or setting with the wrong type gives `kOfxStatErrValue`;
//! - setting a property only the host may set gives `kOfxStatErrValue`;
//! - setting past the end of a variable-dimension property grows it, and
//!   `propSet*N` makes it exactly `count` long;
//! - a null name or value pointer gives `kOfxStatErrValue`.

use std::ffi::{CString, c_char, c_int, c_void};

use voxy_ofx_sys as ofx;

use super::{PROP_SETS, c_str, guard};
use crate::props::{Caller, PropError, PropSet, PropType, Values};

pub(super) static SUITE: ofx::OfxPropertySuiteV1 = ofx::OfxPropertySuiteV1 {
    propSetPointer: Some(set_pointer),
    propSetString: Some(set_string),
    propSetDouble: Some(set_double),
    propSetInt: Some(set_int),
    propSetPointerN: Some(set_pointer_n),
    propSetStringN: Some(set_string_n),
    propSetDoubleN: Some(set_double_n),
    propSetIntN: Some(set_int_n),
    propGetPointer: Some(get_pointer),
    propGetString: Some(get_string),
    propGetDouble: Some(get_double),
    propGetInt: Some(get_int),
    propGetPointerN: Some(get_pointer_n),
    propGetStringN: Some(get_string_n),
    propGetDoubleN: Some(get_double_n),
    propGetIntN: Some(get_int_n),
    propReset: Some(reset),
    propGetDimension: Some(get_dimension),
};

fn status(r: Result<(), PropError>) -> ofx::OfxStatus {
    r.map_or_else(PropError::status, |()| ofx::kOfxStatOK)
}

/// Resolves the handle and the name, then runs `f`.
///
/// # Safety
/// `name` is null or a valid C string.
unsafe fn with_set(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    f: impl FnOnce(&PropSet, &std::ffi::CStr) -> Result<(), PropError>,
) -> ofx::OfxStatus {
    guard(|| {
        let Some(set) = PROP_SETS.get(h as usize) else {
            return ofx::kOfxStatErrBadHandle;
        };
        // SAFETY: the caller's contract.
        let Some(name) = (unsafe { c_str(name) }) else {
            return ofx::kOfxStatErrValue;
        };
        status(f(&set, name))
    })
}

/// `count` items at `p`, or `None` for a null pointer or a negative count.
///
/// # Safety
/// `p` is null or points to at least `count` readable items.
unsafe fn items<'a, T>(p: *const T, count: c_int) -> Option<&'a [T]> {
    let n = usize::try_from(count).ok()?;
    if n == 0 {
        return Some(&[]);
    }
    // SAFETY: the caller's contract.
    (!p.is_null()).then(|| unsafe { std::slice::from_raw_parts(p, n) })
}

unsafe extern "C" fn set_pointer(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    index: c_int,
    value: *mut c_void,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes a valid name.
    unsafe {
        with_set(h, name, |s, n| {
            s.set_at(
                n,
                Caller::Plugin,
                index,
                Values::Pointer(vec![value as usize]),
            )
        })
    }
}

unsafe extern "C" fn set_string(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    index: c_int,
    value: *const c_char,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes valid C strings.
    unsafe {
        with_set(h, name, |s, n| {
            let v = c_str(value).ok_or(PropError::Value)?;
            s.set_at(n, Caller::Plugin, index, Values::String(vec![v.to_owned()]))
        })
    }
}

unsafe extern "C" fn set_double(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    index: c_int,
    value: f64,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes a valid name.
    unsafe {
        with_set(h, name, |s, n| {
            s.set_at(n, Caller::Plugin, index, Values::Double(vec![value]))
        })
    }
}

unsafe extern "C" fn set_int(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    index: c_int,
    value: c_int,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes a valid name.
    unsafe {
        with_set(h, name, |s, n| {
            s.set_at(n, Caller::Plugin, index, Values::Int(vec![value]))
        })
    }
}

unsafe extern "C" fn set_pointer_n(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    count: c_int,
    value: *const *mut c_void,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes `count` values.
    unsafe {
        with_set(h, name, |s, n| {
            let v = items(value, count).ok_or(PropError::Value)?;
            let v = v.iter().map(|p| *p as usize).collect();
            s.set_n(n, Caller::Plugin, Values::Pointer(v))
        })
    }
}

unsafe extern "C" fn set_string_n(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    count: c_int,
    value: *const *const c_char,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes `count` valid C strings.
    unsafe {
        with_set(h, name, |s, n| {
            let v = items(value, count).ok_or(PropError::Value)?;
            let v: Option<Vec<CString>> =
                v.iter().map(|p| c_str(*p).map(ToOwned::to_owned)).collect();
            s.set_n(
                n,
                Caller::Plugin,
                Values::String(v.ok_or(PropError::Value)?),
            )
        })
    }
}

unsafe extern "C" fn set_double_n(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    count: c_int,
    value: *const f64,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes `count` values.
    unsafe {
        with_set(h, name, |s, n| {
            let v = items(value, count).ok_or(PropError::Value)?;
            s.set_n(n, Caller::Plugin, Values::Double(v.to_vec()))
        })
    }
}

unsafe extern "C" fn set_int_n(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    count: c_int,
    value: *const c_int,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes `count` values.
    unsafe {
        with_set(h, name, |s, n| {
            let v = items(value, count).ok_or(PropError::Value)?;
            s.set_n(n, Caller::Plugin, Values::Int(v.to_vec()))
        })
    }
}

/// Reads values `index..index + count` into `out` with `put`.
///
/// # Safety
/// `name` is null or a valid C string; `out` is null or has room for
/// `count` items.
unsafe fn get<T>(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    ty: PropType,
    index: c_int,
    count: c_int,
    out: *mut T,
    put: impl Fn(&Values, usize) -> T,
) -> ofx::OfxStatus {
    // SAFETY: the caller's contract.
    unsafe {
        with_set(h, name, |s, n| {
            if out.is_null() {
                return Err(PropError::Value);
            }
            let first = usize::try_from(index).map_err(|_| PropError::BadIndex)?;
            let n_items = usize::try_from(count).map_err(|_| PropError::BadIndex)?;
            let end = index.checked_add(count).ok_or(PropError::BadIndex)?;
            s.read(n, ty, end, |v| {
                for i in 0..n_items {
                    // SAFETY: `out` has room for `count` items.
                    out.add(i).write(put(v, first + i));
                }
            })
        })
    }
}

fn int_at(v: &Values, i: usize) -> c_int {
    match v {
        Values::Int(v) => v[i],
        _ => 0,
    }
}

fn double_at(v: &Values, i: usize) -> f64 {
    match v {
        Values::Double(v) => v[i],
        _ => 0.0,
    }
}

fn pointer_at(v: &Values, i: usize) -> *mut c_void {
    match v {
        Values::Pointer(v) => v[i] as *mut c_void,
        _ => std::ptr::null_mut(),
    }
}

/// The stored string's own buffer: valid until the property changes.
fn string_at(v: &Values, i: usize) -> *mut c_char {
    match v {
        Values::String(v) => v[i].as_ptr().cast_mut(),
        _ => std::ptr::null_mut(),
    }
}

unsafe extern "C" fn get_pointer(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    index: c_int,
    out: *mut *mut c_void,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes a valid name and out pointer.
    unsafe { get(h, name, PropType::Pointer, index, 1, out, pointer_at) }
}

unsafe extern "C" fn get_string(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    index: c_int,
    out: *mut *mut c_char,
) -> ofx::OfxStatus {
    // SAFETY: as above.
    unsafe { get(h, name, PropType::String, index, 1, out, string_at) }
}

unsafe extern "C" fn get_double(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    index: c_int,
    out: *mut f64,
) -> ofx::OfxStatus {
    // SAFETY: as above.
    unsafe { get(h, name, PropType::Double, index, 1, out, double_at) }
}

unsafe extern "C" fn get_int(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    index: c_int,
    out: *mut c_int,
) -> ofx::OfxStatus {
    // SAFETY: as above.
    unsafe { get(h, name, PropType::Int, index, 1, out, int_at) }
}

unsafe extern "C" fn get_pointer_n(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    count: c_int,
    out: *mut *mut c_void,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes room for `count` values.
    unsafe { get(h, name, PropType::Pointer, 0, count, out, pointer_at) }
}

unsafe extern "C" fn get_string_n(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    count: c_int,
    out: *mut *mut c_char,
) -> ofx::OfxStatus {
    // SAFETY: as above.
    unsafe { get(h, name, PropType::String, 0, count, out, string_at) }
}

unsafe extern "C" fn get_double_n(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    count: c_int,
    out: *mut f64,
) -> ofx::OfxStatus {
    // SAFETY: as above.
    unsafe { get(h, name, PropType::Double, 0, count, out, double_at) }
}

unsafe extern "C" fn get_int_n(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    count: c_int,
    out: *mut c_int,
) -> ofx::OfxStatus {
    // SAFETY: as above.
    unsafe { get(h, name, PropType::Int, 0, count, out, int_at) }
}

unsafe extern "C" fn reset(h: ofx::OfxPropertySetHandle, name: *const c_char) -> ofx::OfxStatus {
    // SAFETY: OFX passes a valid name.
    unsafe { with_set(h, name, |s, n| s.reset(n, Caller::Plugin)) }
}

unsafe extern "C" fn get_dimension(
    h: ofx::OfxPropertySetHandle,
    name: *const c_char,
    out: *mut c_int,
) -> ofx::OfxStatus {
    // SAFETY: OFX passes a valid name and out pointer.
    unsafe {
        with_set(h, name, |s, n| {
            if out.is_null() {
                return Err(PropError::Value);
            }
            let d = s.dimension(n)?;
            out.write(c_int::try_from(d).unwrap_or(c_int::MAX));
            Ok(())
        })
    }
}
