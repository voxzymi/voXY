//! Smoke tests: the bindings expose the API voXY hosts, with the 1.5.1
//! names and the C layouts (the layout assertions themselves are in the
//! generated file and run at compile time).

use voxy_ofx_sys as ofx;

fn name(c: &[u8]) -> &str {
    std::str::from_utf8(c.strip_suffix(b"\0").expect("nul-terminated")).expect("ASCII")
}

#[test]
fn string_constants_are_nul_terminated_c_strings() {
    assert_eq!(
        name(ofx::kOfxImageEffectPluginApi),
        "OfxImageEffectPluginAPI"
    );
    assert_eq!(name(ofx::kOfxPropertySuite), "OfxPropertySuite");
    assert_eq!(name(ofx::kOfxParameterSuite), "OfxParameterSuite");
    assert_eq!(name(ofx::kOfxImageEffectSuite), "OfxImageEffectSuite");
    assert_eq!(
        name(ofx::kOfxImageEffectActionRender),
        "OfxImageEffectActionRender"
    );
    assert_eq!(
        name(ofx::kOfxImageEffectContextFilter),
        "OfxImageEffectContextFilter"
    );
}

#[test]
fn status_codes_and_api_version() {
    assert_eq!(ofx::kOfxStatOK, 0);
    assert_eq!(ofx::kOfxStatFailed, 1);
    assert_eq!(ofx::kOfxStatReplyDefault, 14);
    assert_eq!(ofx::kOfxImageEffectPluginApiVersion, 1);
}

#[test]
fn host_and_plugin_structs_have_the_c_shape() {
    let ptr = size_of::<usize>();
    // OfxHost: the host property set and fetchSuite.
    assert_eq!(size_of::<ofx::OfxHost>(), 2 * ptr);
    // OfxPlugin: API name, API version, identifier, two version ints,
    // setHost and mainEntry.
    assert_eq!(size_of::<ofx::OfxPlugin>(), 6 * ptr);
    assert_eq!(size_of::<ofx::OfxRectD>(), 32);
    assert_eq!(size_of::<ofx::OfxRectI>(), 16);
    assert_eq!(size_of::<ofx::OfxRGBAColourF>(), 16);
}
