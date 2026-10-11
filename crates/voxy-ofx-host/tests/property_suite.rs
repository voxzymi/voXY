//! The property and memory suites, called through `fetchSuite` the way a
//! plugin calls them.

#![allow(unsafe_code, clippy::unwrap_used)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::Arc;

use voxy_ofx_host::props::{Dim, PropSet, Values};
use voxy_ofx_host::suites::{HostStruct, live_blocks, prop_handle, release_prop_handle};
use voxy_ofx_sys as ofx;

fn host() -> HostStruct {
    let props = Arc::new(PropSet::new("host"));
    props.define_ro("OfxPropName", Values::strings(&["org.voxy.test"]));
    HostStruct::new(props)
}

fn prop_suite(h: &HostStruct) -> &'static ofx::OfxPropertySuiteV1 {
    let p = h.fetch_suite(ofx::kOfxPropertySuite, 1);
    assert!(!p.is_null());
    // SAFETY: the host returns its static suite.
    unsafe { &*p.cast::<ofx::OfxPropertySuiteV1>() }
}

fn n(s: &[u8]) -> *const c_char {
    CStr::from_bytes_with_nul(s).unwrap().as_ptr()
}

#[test]
fn unknown_suites_and_versions_are_null() {
    let h = host();
    assert!(h.fetch_suite(ofx::kOfxPropertySuite, 2).is_null());
    assert!(h.fetch_suite(b"OfxNoSuchSuite\0", 1).is_null());
    assert!(
        h.fetch_suite(b"OfxImageEffectOpenGLRenderSuite\0", 1)
            .is_null(),
        "D5"
    );
}

#[test]
fn get_and_set_follow_type_dimension_and_writability() {
    let h = host();
    let s = prop_suite(&h);
    let set = Arc::new(PropSet::new("test"));
    set.define_rw("size", Values::Double(vec![1.0, 2.0]));
    set.define_ro("locked", Values::Int(vec![7]));
    set.define("list", Values::Int(vec![]), Dim::Variable, true);
    set.define_rw("label", Values::strings(&["hello"]));
    let ph = prop_handle(&set);
    // SAFETY: valid handles, names and out pointers throughout.
    unsafe {
        let mut d = 0.0;
        assert_eq!(s.propGetDouble.unwrap()(ph, n(b"size\0"), 1, &raw mut d), 0);
        assert_eq!(d, 2.0);
        assert_eq!(
            s.propGetDouble.unwrap()(ph, n(b"size\0"), 2, &raw mut d),
            ofx::kOfxStatErrBadIndex
        );
        assert_eq!(
            s.propGetDouble.unwrap()(ph, n(b"size\0"), -1, &raw mut d),
            ofx::kOfxStatErrBadIndex
        );
        assert_eq!(
            s.propGetDouble.unwrap()(ph, n(b"nope\0"), 0, &raw mut d),
            ofx::kOfxStatErrUnknown
        );
        let mut i: c_int = 0;
        assert_eq!(
            s.propGetInt.unwrap()(ph, n(b"size\0"), 0, &raw mut i),
            ofx::kOfxStatErrValue,
            "wrong type"
        );
        assert_eq!(s.propSetDouble.unwrap()(ph, n(b"size\0"), 0, 5.0), 0);
        let mut both = [0.0; 2];
        assert_eq!(
            s.propGetDoubleN.unwrap()(ph, n(b"size\0"), 2, both.as_mut_ptr()),
            0
        );
        assert_eq!(both, [5.0, 2.0]);
        assert_eq!(
            s.propSetDouble.unwrap()(ph, n(b"size\0"), 2, 0.0),
            ofx::kOfxStatErrBadIndex,
            "fixed dimension"
        );
        assert_eq!(
            s.propSetInt.unwrap()(ph, n(b"locked\0"), 0, 1),
            ofx::kOfxStatErrValue,
            "host-only"
        );
        assert_eq!(set.int("locked", 0), Some(7));

        // Variable dimension: grows on set, set-N gives the exact length.
        assert_eq!(s.propSetInt.unwrap()(ph, n(b"list\0"), 2, 9), 0);
        assert_eq!(set.get("list"), Some(Values::Int(vec![0, 0, 9])));
        let vals = [4, 5];
        assert_eq!(
            s.propSetIntN.unwrap()(ph, n(b"list\0"), 2, vals.as_ptr()),
            0
        );
        let mut dim: c_int = 0;
        assert_eq!(
            s.propGetDimension.unwrap()(ph, n(b"list\0"), &raw mut dim),
            0
        );
        assert_eq!(dim, 2);

        // Strings: the returned pointer is the stored value.
        let mut out: *mut c_char = std::ptr::null_mut();
        assert_eq!(
            s.propGetString.unwrap()(ph, n(b"label\0"), 0, &raw mut out),
            0
        );
        assert_eq!(CStr::from_ptr(out).to_str().unwrap(), "hello");
        assert_eq!(
            s.propSetString.unwrap()(ph, n(b"label\0"), 0, n(b"bye\0")),
            0
        );
        assert_eq!(set.string("label", 0).as_deref(), Some("bye"));
        assert_eq!(
            s.propSetString.unwrap()(ph, n(b"label\0"), 0, std::ptr::null()),
            ofx::kOfxStatErrValue
        );

        // Reset goes back to the default.
        assert_eq!(s.propReset.unwrap()(ph, n(b"size\0")), 0);
        assert_eq!(set.get("size"), Some(Values::Double(vec![1.0, 2.0])));

        // Pointers round-trip untouched.
        set.define_rw("data", Values::Pointer(vec![0]));
        let marker = 0x1234usize as *mut c_void;
        assert_eq!(s.propSetPointer.unwrap()(ph, n(b"data\0"), 0, marker), 0);
        let mut back: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            s.propGetPointer.unwrap()(ph, n(b"data\0"), 0, &raw mut back),
            0
        );
        assert_eq!(back, marker);
    }
    release_prop_handle(&set);
    // SAFETY: a stale handle must be refused, not dereferenced.
    unsafe {
        let mut d = 0.0;
        assert_eq!(
            s.propGetDouble.unwrap()(ph, n(b"size\0"), 0, &raw mut d),
            ofx::kOfxStatErrBadHandle
        );
        assert_eq!(
            s.propGetDouble.unwrap()(std::ptr::null_mut(), n(b"size\0"), 0, &raw mut d),
            ofx::kOfxStatErrBadHandle
        );
    }
}

#[test]
fn the_host_set_is_readable_and_read_only() {
    let mut h = host();
    let s = prop_suite(&h);
    // SAFETY: the host struct is alive and in place.
    unsafe {
        let raw = &*h.as_ptr();
        let mut out: *mut c_char = std::ptr::null_mut();
        assert_eq!(
            s.propGetString.unwrap()(raw.host, ofx::kOfxPropName.as_ptr().cast(), 0, &raw mut out),
            0
        );
        assert_eq!(CStr::from_ptr(out).to_str().unwrap(), "org.voxy.test");
        assert_eq!(
            s.propSetString.unwrap()(raw.host, ofx::kOfxPropName.as_ptr().cast(), 0, n(b"x\0")),
            ofx::kOfxStatErrValue
        );
    }
}

#[test]
fn memory_blocks_are_aligned_and_only_ours_can_be_freed() {
    let h = host();
    let p = h.fetch_suite(ofx::kOfxMemorySuite, 1);
    // SAFETY: the host returns its static suite; the block is written within
    // its size and freed once.
    unsafe {
        let m = &*p.cast::<ofx::OfxMemorySuiteV1>();
        let before = live_blocks();
        let mut block: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            m.memoryAlloc.unwrap()(std::ptr::null_mut(), 100, &raw mut block),
            0
        );
        assert_eq!(block as usize % 64, 0);
        block.cast::<u8>().write_bytes(0xAB, 100);
        assert_eq!(live_blocks(), before + 1);
        assert_eq!(m.memoryFree.unwrap()(block), 0);
        assert_eq!(m.memoryFree.unwrap()(block), ofx::kOfxStatErrBadHandle);
        let mut local = 0u8;
        assert_eq!(
            m.memoryFree.unwrap()((&raw mut local).cast()),
            ofx::kOfxStatErrBadHandle
        );
    }
}
