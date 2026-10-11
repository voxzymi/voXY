//! Clips (`kOfxTypeClip`): the effect's named image inputs and its output.

use std::sync::Arc;

use voxy_ofx_sys as ofx;

use crate::image::{Components, Depth, Premult};
use crate::props::{PropSet, Values};

/// One clip: its name and property set.
#[derive(Debug)]
pub struct Clip {
    pub name: String,
    pub props: Arc<PropSet>,
}

/// What the host tells an instance's clip about its images.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipInfo {
    pub depth: Depth,
    pub components: Components,
    pub premult: Premult,
    pub pixel_aspect: f64,
    /// Frames per second.
    pub frame_rate: f64,
    /// First and last frame.
    pub frame_range: [f64; 2],
    pub connected: bool,
}

impl Default for ClipInfo {
    fn default() -> Self {
        ClipInfo {
            depth: Depth::Float,
            components: Components::Rgba,
            premult: Premult::Premultiplied,
            pixel_aspect: 1.0,
            frame_rate: 30.0,
            frame_range: [0.0, 0.0],
            connected: true,
        }
    }
}

impl Clip {
    /// A clip descriptor with the spec's defaults.
    pub fn descriptor(name: &str) -> Clip {
        let p = PropSet::new("clip descriptor");
        p.ro(ofx::kOfxPropType, PropSet::consts(&[ofx::kOfxTypeClip]));
        p.ro(ofx::kOfxPropName, Values::strings(&[name]));
        p.rw(ofx::kOfxPropLabel, Values::strings(&[name]));
        p.rw(ofx::kOfxPropShortLabel, Values::strings(&[name]));
        p.rw(ofx::kOfxPropLongLabel, Values::strings(&[name]));
        p.var(
            ofx::kOfxImageEffectPropSupportedComponents,
            Values::String(Vec::new()),
        );
        p.rw(
            ofx::kOfxImageEffectPropTemporalClipAccess,
            Values::Int(vec![0]),
        );
        p.rw(ofx::kOfxImageClipPropOptional, Values::Int(vec![0]));
        p.rw(
            ofx::kOfxImageClipPropFieldExtraction,
            PropSet::consts(&[ofx::kOfxImageFieldDoubled]),
        );
        p.rw(ofx::kOfxImageClipPropIsMask, Values::Int(vec![0]));
        p.rw(ofx::kOfxImageEffectPropSupportsTiles, Values::Int(vec![1]));
        Clip {
            name: name.to_owned(),
            props: Arc::new(p),
        }
    }

    /// An instance of `descriptor`: its properties, read-only, plus what
    /// the host knows about the images.
    pub fn instance(descriptor: &Clip, info: ClipInfo) -> Clip {
        let p = descriptor.props.duplicate("clip instance");
        for name in p.names() {
            p.set_plugin_writable(&name, false);
        }
        let s = |v: &[u8]| PropSet::consts(&[v]);
        p.ro(ofx::kOfxImageEffectPropPixelDepth, s(info.depth.ofx()));
        p.ro(ofx::kOfxImageEffectPropComponents, s(info.components.ofx()));
        p.ro(
            ofx::kOfxImageClipPropUnmappedPixelDepth,
            s(info.depth.ofx()),
        );
        p.ro(
            ofx::kOfxImageClipPropUnmappedComponents,
            s(info.components.ofx()),
        );
        p.ro(
            ofx::kOfxImageEffectPropPreMultiplication,
            s(info.premult.ofx()),
        );
        p.ro(
            ofx::kOfxImagePropPixelAspectRatio,
            Values::Double(vec![info.pixel_aspect]),
        );
        p.ro(
            ofx::kOfxImageEffectPropFrameRate,
            Values::Double(vec![info.frame_rate]),
        );
        p.ro(
            ofx::kOfxImageEffectPropUnmappedFrameRate,
            Values::Double(vec![info.frame_rate]),
        );
        p.ro(
            ofx::kOfxImageEffectPropFrameRange,
            Values::Double(info.frame_range.to_vec()),
        );
        p.ro(
            ofx::kOfxImageEffectPropUnmappedFrameRange,
            Values::Double(info.frame_range.to_vec()),
        );
        p.ro(ofx::kOfxImageClipPropFieldOrder, s(ofx::kOfxImageFieldNone));
        p.ro(
            ofx::kOfxImageClipPropConnected,
            Values::Int(vec![i32::from(info.connected)]),
        );
        p.ro(
            ofx::kOfxImageClipPropContinuousSamples,
            Values::Int(vec![0]),
        );
        Clip {
            name: descriptor.name.clone(),
            props: Arc::new(p),
        }
    }
}
