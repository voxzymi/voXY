//! The image effect suite, called through `fetchSuite` the way a plugin
//! calls it: descriptors and clips, then an instance reading its source
//! image and writing its output.

#![allow(unsafe_code, clippy::unwrap_used)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::Arc;

use voxy_ofx_host::clip::ClipInfo;
use voxy_ofx_host::effect::{Effect, Project};
use voxy_ofx_host::host::host_props;
use voxy_ofx_host::image::{Components, Depth, Image, ImageSource, Premult, RectI};
use voxy_ofx_host::params::LocalValues;
use voxy_ofx_host::suites::{HostStruct, effect_handle, live_images, release_effect_handle};
use voxy_ofx_sys as ofx;

struct Suites {
    _host: HostStruct,
    fx: &'static ofx::OfxImageEffectSuiteV1,
    props: &'static ofx::OfxPropertySuiteV1,
}

fn suites() -> Suites {
    let host = HostStruct::new(host_props());
    let get = |name: &[u8]| {
        let p = host.fetch_suite(name, 1);
        assert!(!p.is_null());
        p
    };
    // SAFETY: the host returns its static suites of these types.
    let (fx, props) = unsafe {
        (
            &*get(ofx::kOfxImageEffectSuite).cast::<ofx::OfxImageEffectSuiteV1>(),
            &*get(ofx::kOfxPropertySuite).cast::<ofx::OfxPropertySuiteV1>(),
        )
    };
    Suites {
        _host: host,
        fx,
        props,
    }
}

fn c(s: &[u8]) -> *const c_char {
    s.as_ptr().cast()
}

/// A 2×2 source and a 2×2 output, both float RGBA.
#[derive(Debug)]
struct TwoByTwo {
    source: Arc<Image>,
    output: Arc<Image>,
}

impl TwoByTwo {
    fn new() -> TwoByTwo {
        let b = RectI::new(0, 0, 2, 2);
        #[rustfmt::skip]
        let px = [
            0.1, 0.2, 0.3, 1.0,   0.4, 0.5, 0.6, 1.0,
            0.7, 0.8, 0.9, 1.0,   0.0, 0.0, 0.0, 0.0,
        ];
        TwoByTwo {
            source: Arc::new(
                Image::from_rgba(
                    b,
                    &px,
                    Depth::Float,
                    Components::Rgba,
                    Premult::Premultiplied,
                )
                .unwrap(),
            ),
            output: Arc::new(
                Image::new(
                    b,
                    Depth::Float,
                    Components::Rgba,
                    Premult::Premultiplied,
                    [1.0, 1.0],
                    1.0,
                )
                .unwrap(),
            ),
        }
    }
}

impl ImageSource for TwoByTwo {
    fn image(&self, clip: &str, time: f64) -> Option<Arc<Image>> {
        (time == 0.0).then(|| match clip {
            "Output" => Arc::clone(&self.output),
            _ => Arc::clone(&self.source),
        })
    }

    fn region_of_definition(&self, _clip: &str, _time: f64) -> Option<[f64; 4]> {
        Some([0.0, 0.0, 2.0, 2.0])
    }
}

/// Describe, Describe-in-context (filter) and an instance, the way the
/// harness will drive them.
fn build(s: &Suites, images: Arc<TwoByTwo>) -> (Arc<Effect>, Arc<Effect>) {
    let general = Arc::new(Effect::descriptor());
    let gh = effect_handle(&general);
    // SAFETY: valid handles, names and out pointers.
    unsafe {
        let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        assert_eq!(s.fx.getPropertySet.unwrap()(gh, &raw mut ph), 0);
        assert_eq!(
            s.props.propSetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, c(b"Tint\0")),
            0
        );
        let contexts = [c(ofx::kOfxImageEffectContextFilter)];
        assert_eq!(
            s.props.propSetStringN.unwrap()(
                ph,
                c(ofx::kOfxImageEffectPropSupportedContexts),
                1,
                contexts.as_ptr()
            ),
            0
        );
        assert_eq!(
            s.fx.clipDefine.unwrap()(gh, c(b"Source\0"), &raw mut ph),
            0,
            "allowed on any descriptor"
        );
    }
    let ctx = Arc::new(Effect::context_descriptor(
        &general,
        ofx::kOfxImageEffectContextFilter,
    ));
    release_effect_handle(&general);
    let ch = effect_handle(&ctx);
    // SAFETY: as above.
    unsafe {
        let mut a: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        let mut b: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        assert_eq!(
            s.fx.clipDefine.unwrap()(ch, c(ofx::kOfxImageEffectSimpleSourceClipName), &raw mut a),
            0
        );
        assert_eq!(
            s.fx.clipDefine.unwrap()(ch, c(ofx::kOfxImageEffectSimpleSourceClipName), &raw mut b),
            0
        );
        assert_eq!(a, b, "the same clip");
        let comps = [c(ofx::kOfxImageComponentRGBA)];
        assert_eq!(
            s.props.propSetStringN.unwrap()(
                a,
                c(ofx::kOfxImageEffectPropSupportedComponents),
                1,
                comps.as_ptr()
            ),
            0
        );
        assert_eq!(
            s.fx.clipDefine.unwrap()(ch, c(ofx::kOfxImageEffectOutputClipName), &raw mut a),
            0
        );
        let mut ctx_name: *mut c_char = std::ptr::null_mut();
        let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        assert_eq!(s.fx.getPropertySet.unwrap()(ch, &raw mut ph), 0);
        assert_eq!(
            s.props.propGetString.unwrap()(
                ph,
                c(ofx::kOfxImageEffectPropContext),
                0,
                &raw mut ctx_name
            ),
            0
        );
        assert_eq!(
            CStr::from_ptr(ctx_name).to_bytes_with_nul(),
            ofx::kOfxImageEffectContextFilter
        );
        let mut label: *mut c_char = std::ptr::null_mut();
        assert_eq!(
            s.props.propGetString.unwrap()(ph, c(ofx::kOfxPropLabel), 0, &raw mut label),
            0
        );
        assert_eq!(
            CStr::from_ptr(label).to_str().unwrap(),
            "Tint",
            "carried from Describe"
        );
        let mut clip: ofx::OfxImageClipHandle = std::ptr::null_mut();
        assert_eq!(
            s.fx.clipGetHandle.unwrap()(ch, c(b"Source\0"), &raw mut clip, std::ptr::null_mut()),
            ofx::kOfxStatErrBadHandle,
            "descriptors have no clip instances"
        );
    }
    let inst = Arc::new(Effect::instance(
        &ctx,
        Project::default(),
        Arc::new(LocalValues::new()),
        images,
        &|_| ClipInfo::default(),
    ));
    (ctx, inst)
}

#[test]
fn an_instance_reads_its_source_and_writes_its_output() {
    let s = suites();
    let images = Arc::new(TwoByTwo::new());
    let (ctx, inst) = build(&s, Arc::clone(&images));
    let ih = effect_handle(&inst);
    let before = live_images();
    // SAFETY: valid handles and out pointers; image data is read and
    // written within the bounds and row bytes the image reports.
    unsafe {
        let mut src: ofx::OfxImageClipHandle = std::ptr::null_mut();
        let mut src_props: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        assert_eq!(
            s.fx.clipGetHandle.unwrap()(ih, c(b"Source\0"), &raw mut src, &raw mut src_props),
            0
        );
        let mut depth: *mut c_char = std::ptr::null_mut();
        assert_eq!(
            s.props.propGetString.unwrap()(
                src_props,
                c(ofx::kOfxImageEffectPropPixelDepth),
                0,
                &raw mut depth
            ),
            0
        );
        assert_eq!(
            CStr::from_ptr(depth).to_bytes_with_nul(),
            ofx::kOfxBitDepthFloat
        );
        assert_eq!(
            s.props.propSetInt.unwrap()(src_props, c(ofx::kOfxImageClipPropOptional), 0, 1),
            ofx::kOfxStatErrValue,
            "instance clip properties are read-only"
        );

        let mut img: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        assert_eq!(
            s.fx.clipGetImage.unwrap()(src, 0.0, std::ptr::null(), &raw mut img),
            0
        );
        assert_eq!(live_images(), before + 1);
        let mut data: *mut c_void = std::ptr::null_mut();
        let mut bounds = [0 as c_int; 4];
        let mut row_bytes: c_int = 0;
        assert_eq!(
            s.props.propGetPointer.unwrap()(img, c(ofx::kOfxImagePropData), 0, &raw mut data),
            0
        );
        assert_eq!(
            s.props.propGetIntN.unwrap()(img, c(ofx::kOfxImagePropBounds), 4, bounds.as_mut_ptr()),
            0
        );
        assert_eq!(
            s.props.propGetInt.unwrap()(img, c(ofx::kOfxImagePropRowBytes), 0, &raw mut row_bytes),
            0
        );
        assert_eq!(bounds, [0, 0, 2, 2]);
        assert_eq!(row_bytes, 32);
        // Row 1 (the top row), pixel 0: (0.7, 0.8, 0.9, 1).
        let top = data.cast::<f32>().add(row_bytes as usize / 4);
        assert_eq!(*top, 0.7);

        let mut out_clip: ofx::OfxImageClipHandle = std::ptr::null_mut();
        assert_eq!(
            s.fx.clipGetHandle.unwrap()(
                ih,
                c(b"Output\0"),
                &raw mut out_clip,
                std::ptr::null_mut()
            ),
            0
        );
        let mut out: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        assert_eq!(
            s.fx.clipGetImage.unwrap()(out_clip, 0.0, std::ptr::null(), &raw mut out),
            0
        );
        let mut out_data: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            s.props.propGetPointer.unwrap()(out, c(ofx::kOfxImagePropData), 0, &raw mut out_data),
            0
        );
        // Invert the colour of every pixel.
        let n = 2 * 2 * 4;
        let (from, to) = (data.cast::<f32>(), out_data.cast::<f32>());
        for i in 0..n {
            let v = *from.add(i);
            *to.add(i) = if i % 4 == 3 { v } else { 1.0 - v };
        }
        assert_eq!(s.fx.clipReleaseImage.unwrap()(img), 0);
        assert_eq!(s.fx.clipReleaseImage.unwrap()(out), 0);
        assert_eq!(
            s.fx.clipReleaseImage.unwrap()(out),
            ofx::kOfxStatErrBadHandle
        );
        assert_eq!(live_images(), before);

        let mut none: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        assert_eq!(
            s.fx.clipGetImage.unwrap()(src, 5.0, std::ptr::null(), &raw mut none),
            ofx::kOfxStatFailed
        );
        let mut rod = ofx::OfxRectD {
            x1: 9.0,
            y1: 9.0,
            x2: 9.0,
            y2: 9.0,
        };
        assert_eq!(
            s.fx.clipGetRegionOfDefinition.unwrap()(src, 0.0, &raw mut rod),
            0
        );
        assert_eq!([rod.x1, rod.y1, rod.x2, rod.y2], [0.0, 0.0, 2.0, 2.0]);

        assert_eq!(s.fx.abort.unwrap()(ih), 0);
        inst.set_abort(true);
        assert_eq!(s.fx.abort.unwrap()(ih), 1);

        let mut ps: ofx::OfxParamSetHandle = std::ptr::null_mut();
        assert_eq!(s.fx.getParamSet.unwrap()(ih, &raw mut ps), 0);
        assert!(!ps.is_null());
        assert_eq!(
            s.fx.clipGetHandle.unwrap()(ih, c(b"Nope\0"), &raw mut src, std::ptr::null_mut()),
            ofx::kOfxStatErrUnknown
        );
    }
    let got = images.output.to_rgba();
    let want: Vec<f32> = images
        .source
        .to_rgba()
        .iter()
        .enumerate()
        .map(|(i, v)| if i % 4 == 3 { *v } else { 1.0 - v })
        .collect();
    assert_eq!(got, want);
    release_effect_handle(&inst);
    release_effect_handle(&ctx);
    // SAFETY: a released handle must be refused.
    unsafe {
        let mut ph: ofx::OfxPropertySetHandle = std::ptr::null_mut();
        assert_eq!(
            s.fx.getPropertySet.unwrap()(ih, &raw mut ph),
            ofx::kOfxStatErrBadHandle
        );
    }
}

#[test]
fn image_memory_locks_and_frees() {
    let s = suites();
    // SAFETY: valid out pointers; the block is used within its size.
    unsafe {
        let mut m: ofx::OfxImageMemoryHandle = std::ptr::null_mut();
        assert_eq!(
            s.fx.imageMemoryAlloc.unwrap()(std::ptr::null_mut(), 256, &raw mut m),
            0
        );
        let mut p: *mut c_void = std::ptr::null_mut();
        assert_eq!(s.fx.imageMemoryLock.unwrap()(m, &raw mut p), 0);
        assert_eq!(p as usize % 64, 0);
        p.cast::<u8>().write_bytes(7, 256);
        assert_eq!(s.fx.imageMemoryUnlock.unwrap()(m), 0);
        assert_eq!(s.fx.imageMemoryFree.unwrap()(m), 0);
        assert_eq!(s.fx.imageMemoryFree.unwrap()(m), ofx::kOfxStatErrBadHandle);
        assert_eq!(
            s.fx.imageMemoryLock.unwrap()(m, &raw mut p),
            ofx::kOfxStatErrBadHandle
        );
        assert!(p.is_null());
        assert_eq!(
            s.fx.imageMemoryAlloc.unwrap()(0x40 as ofx::OfxImageEffectHandle, 8, &raw mut m),
            ofx::kOfxStatErrBadHandle
        );
    }
}

#[test]
fn host_properties_describe_vo_xy() {
    let host = host_props();
    assert_eq!(host.string("OfxPropLabel", 0).as_deref(), Some("voXY"));
    assert_eq!(
        host.strings("OfxImageEffectPropSupportedPixelDepths"),
        ["OfxBitDepthFloat", "OfxBitDepthShort", "OfxBitDepthByte"]
    );
    assert_eq!(
        host.string("OfxImageEffectPropOpenGLRenderSupported", 0)
            .as_deref(),
        Some("false")
    );
    assert_eq!(host.int("OfxImageEffectPropSupportsOverlays", 0), Some(0));
}

#[test]
fn byte_and_short_conversion_rounds_half_to_even() {
    let b = RectI::new(0, 0, 3, 1);
    // 0.5 × 255 = 127.5 → 128 (even); 0.5/255 × 255 = 0.5 → 0; 1.5/255 → 2.
    let px = [
        0.5,
        0.5 / 255.0,
        1.5 / 255.0,
        1.0,
        2.0,
        -1.0,
        0.25,
        0.5,
        0.0,
        0.0,
        0.0,
        0.0,
    ];
    let img = Image::from_rgba(
        b,
        &px,
        Depth::Byte,
        Components::Rgba,
        Premult::Premultiplied,
    )
    .unwrap();
    let back = img.to_rgba();
    let bytes: Vec<u8> = back.iter().map(|v| (v * 255.0).round() as u8).collect();
    assert_eq!(&bytes[..8], &[128, 0, 2, 255, 255, 0, 64, 128]);
    let rgb = Image::from_rgba(b, &px, Depth::Short, Components::Rgb, Premult::Opaque).unwrap();
    assert_eq!(rgb.row_bytes, 3 * 3 * 2);
    assert_eq!(rgb.to_rgba()[3], 1.0, "RGB reads back opaque");
    let alpha = Image::from_rgba(
        b,
        &px,
        Depth::Float,
        Components::Alpha,
        Premult::Premultiplied,
    )
    .unwrap();
    assert_eq!(&alpha.to_rgba()[..4], &[0.0, 0.0, 0.0, 1.0]);
}
