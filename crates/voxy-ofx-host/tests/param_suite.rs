//! The parameter suite, called through `fetchSuite` the way a plugin calls
//! it: describing parameters, then reading and writing instance values.

#![allow(unsafe_code, clippy::unwrap_used, clippy::many_single_char_names)]

use std::ffi::{CStr, c_char, c_int, c_uint};
use std::sync::Arc;

use voxy_ofx_host::params::{LocalValues, ParamSet, ParamValue, ParamValues};
use voxy_ofx_host::props::{PropSet, Values};
use voxy_ofx_host::suites::{
    HostStruct, MAX_OPAQUE_BYTES, capture_bytes_defaults, param_set_handle,
    release_param_set_handle,
};
use voxy_ofx_sys as ofx;

struct Suites {
    _host: HostStruct,
    p: &'static ofx::OfxParameterSuiteV1,
    props: &'static ofx::OfxPropertySuiteV1,
}

fn suites() -> Suites {
    let host = HostStruct::new(Arc::new(PropSet::new("host")));
    let get = |name: &[u8]| {
        let p = host.fetch_suite(name, 1);
        assert!(!p.is_null());
        p
    };
    // SAFETY: the host returns its static suites of these types.
    let (p, props) = unsafe {
        (
            &*get(ofx::kOfxParameterSuite).cast::<ofx::OfxParameterSuiteV1>(),
            &*get(ofx::kOfxPropertySuite).cast::<ofx::OfxPropertySuiteV1>(),
        )
    };
    Suites {
        _host: host,
        p,
        props,
    }
}

fn c(s: &[u8]) -> *const c_char {
    s.as_ptr().cast()
}

/// Describes the test parameters on a fresh descriptor set.
fn describe(s: &Suites, blob: &ofx::OfxBytes) -> Arc<ParamSet> {
    let set = Arc::new(ParamSet::descriptor(Arc::new(PropSet::new("effect"))));
    let h = param_set_handle(&set);
    let define = s.p.paramDefine.unwrap();
    let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
    // SAFETY: valid handles, type names and out pointers.
    unsafe {
        assert_eq!(
            define(h, c(ofx::kOfxParamTypeDouble), c(b"gain\0"), &raw mut ph),
            0
        );
        assert_eq!(
            s.props.propSetDouble.unwrap()(ph, c(ofx::kOfxParamPropDefault), 0, 0.5),
            0
        );
        assert_eq!(
            s.props.propSetDouble.unwrap()(ph, c(ofx::kOfxParamPropMax), 0, 4.0),
            0
        );
        let mut ty: *mut c_char = std::ptr::null_mut();
        assert_eq!(
            s.props.propGetString.unwrap()(ph, c(ofx::kOfxParamPropDoubleType), 0, &raw mut ty),
            0
        );
        assert_eq!(
            CStr::from_ptr(ty).to_bytes_with_nul(),
            ofx::kOfxParamDoubleTypePlain
        );

        assert_eq!(
            define(h, c(ofx::kOfxParamTypeChoice), c(b"mode\0"), &raw mut ph),
            0
        );
        let options = [c(b"Soft\0"), c(b"Hard\0")];
        assert_eq!(
            s.props.propSetStringN.unwrap()(
                ph,
                c(ofx::kOfxParamPropChoiceOption),
                2,
                options.as_ptr()
            ),
            0
        );
        assert_eq!(
            s.props.propSetInt.unwrap()(ph, c(ofx::kOfxParamPropDefault), 0, 1),
            0
        );

        assert_eq!(
            define(h, c(ofx::kOfxParamTypeRGBA), c(b"tint\0"), &raw mut ph),
            0
        );
        let rgba = [1.0, 0.5, 0.25, 1.0];
        assert_eq!(
            s.props.propSetDoubleN.unwrap()(ph, c(ofx::kOfxParamPropDefault), 4, rgba.as_ptr()),
            0
        );
        let mut dim: c_int = 0;
        assert_eq!(
            s.props.propGetDimension.unwrap()(
                ph,
                c(ofx::kOfxParamPropDimensionLabel),
                &raw mut dim
            ),
            0
        );
        assert_eq!(dim, 4);

        assert_eq!(
            define(h, c(ofx::kOfxParamTypeStrChoice), c(b"look\0"), &raw mut ph),
            0
        );
        let enums = [c(b"warm\0"), c(b"cool\0")];
        assert_eq!(
            s.props.propSetStringN.unwrap()(ph, c(ofx::kOfxParamPropChoiceEnum), 2, enums.as_ptr()),
            0
        );
        assert_eq!(
            s.props.propSetString.unwrap()(ph, c(ofx::kOfxParamPropDefault), 0, c(b"warm\0")),
            0
        );

        assert_eq!(
            define(h, c(ofx::kOfxParamTypeBytes), c(b"blob\0"), &raw mut ph),
            0
        );
        let blob_ptr = std::ptr::from_ref(blob).cast_mut().cast();
        assert_eq!(
            s.props.propSetPointer.unwrap()(ph, c(ofx::kOfxParamPropDefault), 0, blob_ptr),
            0
        );

        assert_eq!(
            define(
                h,
                c(ofx::kOfxParamTypeCustom),
                c(b"state\0"),
                std::ptr::null_mut()
            ),
            0
        );
        assert_eq!(
            define(h, c(ofx::kOfxParamTypeGroup), c(b"grp\0"), &raw mut ph),
            0
        );
        assert_eq!(
            define(h, c(ofx::kOfxParamTypePushButton), c(b"go\0"), &raw mut ph),
            0
        );

        assert_eq!(
            define(h, c(ofx::kOfxParamTypeDouble), c(b"gain\0"), &raw mut ph),
            ofx::kOfxStatErrExists
        );
        assert_eq!(
            define(h, c(b"OfxParamTypeNope\0"), c(b"x\0"), &raw mut ph),
            ofx::kOfxStatErrUnknown
        );
        assert_eq!(
            define(
                h,
                c(b"OfxParamTypeParametric\0"),
                c(b"curve\0"),
                &raw mut ph
            ),
            ofx::kOfxStatErrUnsupported
        );

        // A descriptor has no values.
        let mut gain: ofx::OfxParamHandle = std::ptr::null_mut();
        assert_eq!(
            s.p.paramGetHandle.unwrap()(h, c(b"gain\0"), &raw mut gain, std::ptr::null_mut()),
            0
        );
        let mut v = 0.0;
        assert_eq!(
            s.p.paramGetValue.unwrap()(gain, &raw mut v),
            ofx::kOfxStatErrBadHandle
        );
        capture_bytes_defaults(&set);
    }
    set
}

fn handle(s: &Suites, set: ofx::OfxParamSetHandle, name: &[u8]) -> ofx::OfxParamHandle {
    let mut h: ofx::OfxParamHandle = std::ptr::null_mut();
    // SAFETY: valid handle, name and out pointer.
    unsafe {
        assert_eq!(
            s.p.paramGetHandle.unwrap()(set, c(name), &raw mut h, std::ptr::null_mut()),
            0
        );
    };
    h
}

#[test]
fn describe_then_read_defaults_on_an_instance() {
    let s = suites();
    let data = *b"seed";
    let blob = ofx::OfxBytes {
        data: data.as_ptr(),
        length: data.len(),
    };
    let desc = describe(&s, &blob);
    let values = Arc::new(LocalValues::new());
    let inst = Arc::new(ParamSet::instance(
        &desc,
        Arc::new(PropSet::new("effect")),
        values.clone(),
    ));
    release_param_set_handle(&desc);
    let ih = param_set_handle(&inst);
    // SAFETY: valid handles and out pointers of each parameter's type.
    unsafe {
        let get = s.p.paramGetValue.unwrap();
        let mut d = 0.0;
        assert_eq!(get(handle(&s, ih, b"gain\0"), &raw mut d), 0);
        assert_eq!(d, 0.5);
        let mut i: c_int = 0;
        assert_eq!(get(handle(&s, ih, b"mode\0"), &raw mut i), 0);
        assert_eq!(i, 1);
        let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
        assert_eq!(
            get(
                handle(&s, ih, b"tint\0"),
                &raw mut r,
                &raw mut g,
                &raw mut b,
                &raw mut a
            ),
            0
        );
        assert_eq!([r, g, b, a], [1.0, 0.5, 0.25, 1.0]);
        let mut text: *const c_char = std::ptr::null();
        assert_eq!(get(handle(&s, ih, b"look\0"), &raw mut text), 0);
        assert_eq!(CStr::from_ptr(text).to_str().unwrap(), "warm");
        let mut out = ofx::OfxBytes {
            data: std::ptr::null(),
            length: 0,
        };
        assert_eq!(get(handle(&s, ih, b"blob\0"), &raw mut out), 0);
        assert_eq!(std::slice::from_raw_parts(out.data, out.length), b"seed");
        assert_eq!(
            get(handle(&s, ih, b"grp\0"), &raw mut d),
            ofx::kOfxStatErrUnsupported
        );

        // Instance properties: the label may change, the default may not.
        let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        assert_eq!(
            s.p.paramGetPropertySet.unwrap()(handle(&s, ih, b"gain\0"), &raw mut ph),
            0
        );
        assert_eq!(
            s.props.propSetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, c(b"Gain\0")),
            0
        );
        assert_eq!(
            s.props.propSetDouble.unwrap()(ph, c(ofx::kOfxParamPropDefault), 0, 1.0),
            ofx::kOfxStatErrValue
        );

        // Defining on an instance is refused.
        assert_eq!(
            s.p.paramDefine.unwrap()(
                ih,
                c(ofx::kOfxParamTypeDouble),
                c(b"late\0"),
                std::ptr::null_mut()
            ),
            ofx::kOfxStatErrBadHandle
        );
        assert_eq!(
            s.p.paramGetHandle.unwrap()(
                ih,
                c(b"nope\0"),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            ofx::kOfxStatErrUnknown
        );
    }
    release_param_set_handle(&inst);
}

#[test]
fn keys_interpolation_derivative_and_integral() {
    let s = suites();
    let blob = ofx::OfxBytes {
        data: std::ptr::null(),
        length: 0,
    };
    let desc = describe(&s, &blob);
    let values = Arc::new(LocalValues::new());
    let inst = Arc::new(ParamSet::instance(
        &desc,
        Arc::new(PropSet::new("effect")),
        values.clone(),
    ));
    let ih = param_set_handle(&inst);
    let gain = handle(&s, ih, b"gain\0");
    // SAFETY: valid handles; doubles in, double pointers out.
    unsafe {
        let p = s.p;
        assert_eq!(p.paramSetValueAtTime.unwrap()(gain, 0.0, 0.0f64), 0);
        assert_eq!(p.paramSetValueAtTime.unwrap()(gain, 10.0, 1.0f64), 0);
        let mut v = 0.0_f64;
        assert_eq!(p.paramGetValueAtTime.unwrap()(gain, 5.0, &raw mut v), 0);
        assert_eq!(v, 0.5);
        assert_eq!(p.paramGetDerivative.unwrap()(gain, 5.0, &raw mut v), 0);
        assert!((v - 0.1).abs() < 1e-9, "{v}");
        assert_eq!(p.paramGetIntegral.unwrap()(gain, 0.0, 10.0, &raw mut v), 0);
        assert!((v - 5.0).abs() < 1e-9, "{v}");

        let mut n: c_uint = 0;
        assert_eq!(p.paramGetNumKeys.unwrap()(gain, &raw mut n), 0);
        assert_eq!(n, 2);
        let mut t = 0.0;
        assert_eq!(p.paramGetKeyTime.unwrap()(gain, 1, &raw mut t), 0);
        assert_eq!(t, 10.0);
        assert_eq!(
            p.paramGetKeyTime.unwrap()(gain, 2, &raw mut t),
            ofx::kOfxStatErrBadIndex
        );
        let mut i: c_int = 9;
        assert_eq!(p.paramGetKeyIndex.unwrap()(gain, 10.0, 0, &raw mut i), 0);
        assert_eq!(i, 1);
        assert_eq!(p.paramGetKeyIndex.unwrap()(gain, 3.0, 1, &raw mut i), 0);
        assert_eq!(i, 1);
        assert_eq!(p.paramGetKeyIndex.unwrap()(gain, 3.0, -1, &raw mut i), 0);
        assert_eq!(i, 0);
        assert_eq!(
            p.paramGetKeyIndex.unwrap()(gain, 10.0, 1, &raw mut i),
            ofx::kOfxStatFailed
        );
        assert_eq!(i, -1);

        // With keys, setValue keys the current time.
        inst.set_time(4.0);
        assert_eq!(p.paramSetValue.unwrap()(gain, 2.0f64), 0);
        assert_eq!(values.key_times("gain"), [0.0, 4.0, 10.0]);
        assert_eq!(p.paramGetValue.unwrap()(gain, &raw mut v), 0);
        assert_eq!(v, 2.0);

        assert_eq!(p.paramDeleteKey.unwrap()(gain, 4.0), 0);
        assert_eq!(
            p.paramDeleteKey.unwrap()(gain, 4.0),
            ofx::kOfxStatErrBadIndex
        );
        assert_eq!(p.paramDeleteAllKeys.unwrap()(gain), 0);
        assert_eq!(values.key_times("gain"), Vec::<f64>::new());
        assert_eq!(
            values.value_at("gain", 99.0),
            Some(ParamValue::Doubles(vec![0.4])),
            "the value shown at t=4 stays"
        );

        // Derivatives are for doubles only.
        let mut x = 0.0;
        assert_eq!(
            p.paramGetDerivative.unwrap()(handle(&s, ih, b"mode\0"), 0.0, &raw mut x),
            ofx::kOfxStatErrUnsupported
        );
    }
    release_param_set_handle(&inst);
    release_param_set_handle(&desc);
    // SAFETY: a released handle is refused.
    unsafe {
        let mut v = 0.0;
        assert_eq!(
            s.p.paramGetValue.unwrap()(gain, &raw mut v),
            ofx::kOfxStatErrBadHandle
        );
    }
}

#[test]
fn values_are_checked_and_capped() {
    let s = suites();
    let blob = ofx::OfxBytes {
        data: std::ptr::null(),
        length: 0,
    };
    let desc = describe(&s, &blob);
    desc.get("state")
        .unwrap()
        .props
        .set("OfxParamPropAnimates", Values::Int(vec![0]))
        .unwrap();
    let values = Arc::new(LocalValues::new());
    let inst = Arc::new(ParamSet::instance(
        &desc,
        Arc::new(PropSet::new("effect")),
        values.clone(),
    ));
    let ih = param_set_handle(&inst);
    // SAFETY: valid handles; values of each parameter's type.
    unsafe {
        let set = s.p.paramSetValue.unwrap();
        let mode = handle(&s, ih, b"mode\0");
        assert_eq!(set(mode, 1 as c_int), 0);
        assert_eq!(set(mode, 2 as c_int), ofx::kOfxStatErrValue, "two options");
        assert_eq!(set(mode, -1 as c_int), ofx::kOfxStatErrValue);
        let look = handle(&s, ih, b"look\0");
        assert_eq!(set(look, c(b"cool\0")), 0);
        assert_eq!(set(look, c(b"hot\0")), ofx::kOfxStatErrValue);

        let big = vec![b'x'; MAX_OPAQUE_BYTES + 1];
        let blob_h = handle(&s, ih, b"blob\0");
        let over = ofx::OfxBytes {
            data: big.as_ptr(),
            length: big.len(),
        };
        assert_eq!(set(blob_h, &raw const over), ofx::kOfxStatErrMemory);
        let ok = ofx::OfxBytes {
            data: big.as_ptr(),
            length: 3,
        };
        assert_eq!(set(blob_h, &raw const ok), 0);
        assert_eq!(
            values.value_at("blob", 0.0),
            Some(ParamValue::Bytes(b"xxx".to_vec()))
        );

        let mut text = big.clone();
        text.push(0);
        let state = handle(&s, ih, b"state\0");
        assert_eq!(
            set(state, text.as_ptr().cast::<c_char>()),
            ofx::kOfxStatErrMemory
        );
        assert_eq!(set(state, c(b"{\"k\":1}\0")), 0);

        // A parameter that does not animate cannot be keyed; one that
        // does can.
        assert_eq!(
            s.p.paramSetValueAtTime.unwrap()(state, 1.0, c(b"x\0")),
            ofx::kOfxStatErrValue
        );
        assert_eq!(s.p.paramSetValueAtTime.unwrap()(look, 1.0, c(b"cool\0")), 0);
    }
    release_param_set_handle(&inst);
    release_param_set_handle(&desc);
}

#[test]
fn copy_and_edit_groups() {
    let s = suites();
    let blob = ofx::OfxBytes {
        data: std::ptr::null(),
        length: 0,
    };
    let desc = describe(&s, &blob);
    let a_values = Arc::new(LocalValues::new());
    let b_values = Arc::new(LocalValues::new());
    let a = Arc::new(ParamSet::instance(
        &desc,
        Arc::new(PropSet::new("effect")),
        a_values.clone(),
    ));
    let b = Arc::new(ParamSet::instance(
        &desc,
        Arc::new(PropSet::new("effect")),
        b_values.clone(),
    ));
    let (ah, bh) = (param_set_handle(&a), param_set_handle(&b));
    // SAFETY: valid handles.
    unsafe {
        let p = s.p;
        let from = handle(&s, ah, b"gain\0");
        assert_eq!(p.paramSetValueAtTime.unwrap()(from, 1.0, 1.0f64), 0);
        assert_eq!(p.paramSetValueAtTime.unwrap()(from, 5.0, 5.0f64), 0);
        assert_eq!(p.paramSetValueAtTime.unwrap()(from, 9.0, 9.0f64), 0);
        let range = ofx::OfxRangeD { min: 0.0, max: 6.0 };
        assert_eq!(
            p.paramCopy.unwrap()(handle(&s, bh, b"gain\0"), from, 10.0, &raw const range),
            0
        );
        assert_eq!(b_values.key_times("gain"), [11.0, 15.0]);
        assert_eq!(
            p.paramCopy.unwrap()(handle(&s, bh, b"mode\0"), from, 0.0, std::ptr::null()),
            ofx::kOfxStatErrValue,
            "different types"
        );

        assert_eq!(p.paramEditBegin.unwrap()(ah, c(b"Reset\0")), 0);
        assert_eq!(p.paramEditEnd.unwrap()(ah), 0);
        assert_eq!(
            a_values.edit_log(),
            [("Reset".to_owned(), 1), (String::new(), 0)]
        );
        let mut desc_h: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        assert_eq!(p.paramSetGetPropertySet.unwrap()(ah, &raw mut desc_h), 0);
        assert!(!desc_h.is_null());
    }
    for set in [&a, &b, &desc] {
        release_param_set_handle(set);
    }
}
