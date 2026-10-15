//! The host property set (`kOfxTypeImageEffectHost`): what voXY tells a
//! plugin about itself in `setHost` (design §6.4, §6.5).

use std::sync::Arc;

use voxy_ofx_sys as ofx;

use crate::props::{PropSet, Values};

/// The OpenFX API version voXY implements.
pub const API_VERSION: [i32; 2] = [1, 5];

/// The contexts voXY hosts so far. Transition and general join in O-C
/// (design §6.4).
pub const CONTEXTS: &[&[u8]] = &[
    ofx::kOfxImageEffectContextFilter,
    ofx::kOfxImageEffectContextGenerator,
    ofx::kOfxImageEffectContextTransition,
];

/// The pixel depths the host can deliver, best first (§6.5: float is
/// preferred).
pub const DEPTHS: &[&[u8]] = &[
    ofx::kOfxBitDepthFloat,
    ofx::kOfxBitDepthShort,
    ofx::kOfxBitDepthByte,
];

/// The component layouts the host can deliver (§6.5).
pub const COMPONENTS: &[&[u8]] = &[
    ofx::kOfxImageComponentRGBA,
    ofx::kOfxImageComponentRGB,
    ofx::kOfxImageComponentAlpha,
];

/// The GPU suites' "supported" property, from a header voXY does not bind
/// (owner decision D5).
pub const OPENGL_RENDER_SUPPORTED: &[u8] = b"OfxImageEffectPropOpenGLRenderSupported\0";

/// Builds the host property set. Everything in it is read-only to plugins.
pub fn host_props() -> Arc<PropSet> {
    let p = PropSet::new("host");
    let int = |n: &[u8], v: i32| p.ro(n, Values::Int(vec![v]));
    p.ro(
        ofx::kOfxPropType,
        PropSet::consts(&[ofx::kOfxTypeImageEffectHost]),
    );
    p.var_ro(ofx::kOfxPropAPIVersion, Values::Int(API_VERSION.to_vec()));
    p.ro(ofx::kOfxPropName, Values::strings(&["org.voxy.voXY"]));
    p.ro(ofx::kOfxPropLabel, Values::strings(&["voXY"]));
    p.var_ro(ofx::kOfxPropVersion, Values::Int(crate_version()));
    p.ro(
        ofx::kOfxPropVersionLabel,
        Values::strings(&[env!("CARGO_PKG_VERSION")]),
    );
    p.ro(ofx::kOfxPropHostOSHandle, Values::Pointer(vec![0]));
    int(ofx::kOfxImageEffectHostPropIsBackground, 0);
    // Interacts arrive in O-D.
    int(ofx::kOfxImageEffectPropSupportsOverlays, 0);
    int(ofx::kOfxImageEffectPropSupportsMultiResolution, 1);
    int(ofx::kOfxImageEffectPropSupportsTiles, 1);
    int(ofx::kOfxImageEffectPropTemporalClipAccess, 1);
    p.var_ro(
        ofx::kOfxImageEffectPropSupportedComponents,
        PropSet::consts(COMPONENTS),
    );
    p.var_ro(
        ofx::kOfxImageEffectPropSupportedContexts,
        PropSet::consts(CONTEXTS),
    );
    p.var_ro(
        ofx::kOfxImageEffectPropSupportedPixelDepths,
        PropSet::consts(DEPTHS),
    );
    int(ofx::kOfxImageEffectPropSupportsMultipleClipDepths, 0);
    int(ofx::kOfxImageEffectPropSupportsMultipleClipPARs, 0);
    int(ofx::kOfxImageEffectPropSetableFrameRate, 0);
    int(ofx::kOfxImageEffectPropSetableFielding, 0);
    int(ofx::kOfxImageEffectInstancePropSequentialRender, 0);
    int(ofx::kOfxImageEffectPropRenderQualityDraft, 1);
    p.ro(
        ofx::kOfxImageEffectHostPropNativeOrigin,
        PropSet::consts(&[ofx::kOfxHostNativeOriginBottomLeft]),
    );
    p.ro(OPENGL_RENDER_SUPPORTED, Values::strings(&["false"]));
    p.ro(
        ofx::kOfxImageEffectPropColourManagementStyle,
        PropSet::consts(&[ofx::kOfxImageEffectColourManagementNone]),
    );
    int(ofx::kOfxParamHostPropSupportsCustomInteract, 0);
    int(ofx::kOfxParamHostPropSupportsStringAnimation, 1);
    int(ofx::kOfxParamHostPropSupportsChoiceAnimation, 1);
    int(ofx::kOfxParamHostPropSupportsBooleanAnimation, 1);
    int(ofx::kOfxParamHostPropSupportsCustomAnimation, 0);
    int(ofx::kOfxParamHostPropSupportsStrChoice, 1);
    int(ofx::kOfxParamHostPropSupportsStrChoiceAnimation, 1);
    int(ofx::kOfxParamHostPropMaxParameters, -1);
    int(ofx::kOfxParamHostPropMaxPages, 0);
    p.ro(
        ofx::kOfxParamHostPropPageRowColumnCount,
        Values::Int(vec![0, 0]),
    );
    Arc::new(p)
}

/// The crate version as `[major, minor, patch]`.
fn crate_version() -> Vec<i32> {
    env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|n| n.parse().unwrap_or(0))
        .collect()
}
