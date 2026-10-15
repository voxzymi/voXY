//! Images handed to plugins (`kOfxTypeImage`) and the host's image source.
//!
//! OpenFX images are stored bottom-up: row 0 is the bottom row of the
//! bounds, and `kOfxImagePropData` points at the bottom-left pixel. voXY's
//! own buffers are premultiplied (owner decision D2). Conversion from float
//! to 8 or 16 bits rounds half to even, so it is deterministic (§6.5).

use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use voxy_ofx_sys as ofx;

use crate::props::{PropSet, Values, ofx_str};
use crate::suites::PixelBuffer;

/// A pixel depth (`kOfxBitDepth*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Depth {
    Byte,
    Short,
    Float,
}

impl Depth {
    pub fn bytes(self) -> usize {
        match self {
            Depth::Byte => 1,
            Depth::Short => 2,
            Depth::Float => 4,
        }
    }

    pub fn ofx(self) -> &'static [u8] {
        match self {
            Depth::Byte => ofx::kOfxBitDepthByte,
            Depth::Short => ofx::kOfxBitDepthShort,
            Depth::Float => ofx::kOfxBitDepthFloat,
        }
    }

    pub fn from_ofx(name: &str) -> Option<Depth> {
        [Depth::Byte, Depth::Short, Depth::Float]
            .into_iter()
            .find(|d| ofx_str(d.ofx()) == name)
    }
}

/// A component layout (`kOfxImageComponent*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Components {
    Rgba,
    Rgb,
    Alpha,
}

impl Components {
    pub fn count(self) -> usize {
        match self {
            Components::Rgba => 4,
            Components::Rgb => 3,
            Components::Alpha => 1,
        }
    }

    pub fn ofx(self) -> &'static [u8] {
        match self {
            Components::Rgba => ofx::kOfxImageComponentRGBA,
            Components::Rgb => ofx::kOfxImageComponentRGB,
            Components::Alpha => ofx::kOfxImageComponentAlpha,
        }
    }

    pub fn from_ofx(name: &str) -> Option<Components> {
        [Components::Rgba, Components::Rgb, Components::Alpha]
            .into_iter()
            .find(|c| ofx_str(c.ofx()) == name)
    }
}

/// How alpha relates to colour (`kOfxImage*Multiplied`, `kOfxImageOpaque`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Premult {
    Opaque,
    Premultiplied,
    Unpremultiplied,
}

impl Premult {
    pub fn ofx(self) -> &'static [u8] {
        match self {
            Premult::Opaque => ofx::kOfxImageOpaque,
            Premult::Premultiplied => ofx::kOfxImagePreMultiplied,
            Premult::Unpremultiplied => ofx::kOfxImageUnPreMultiplied,
        }
    }

    pub fn from_ofx(name: &str) -> Option<Premult> {
        [
            Premult::Opaque,
            Premult::Premultiplied,
            Premult::Unpremultiplied,
        ]
        .into_iter()
        .find(|p| ofx_str(p.ofx()) == name)
    }
}

/// A rectangle in pixels: `x1, y1` inclusive, `x2, y2` exclusive, with y
/// up, as `OfxRectI`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct RectI {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

impl RectI {
    pub fn new(x1: i32, y1: i32, x2: i32, y2: i32) -> RectI {
        RectI { x1, y1, x2, y2 }
    }

    pub fn width(self) -> usize {
        usize::try_from(self.x2 - self.x1).unwrap_or(0)
    }

    pub fn height(self) -> usize {
        usize::try_from(self.y2 - self.y1).unwrap_or(0)
    }

    fn values(self) -> Values {
        Values::Int(vec![self.x1, self.y1, self.x2, self.y2])
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// One image: its pixels and its property set.
#[derive(Debug)]
pub struct Image {
    pub bounds: RectI,
    pub depth: Depth,
    pub components: Components,
    pub premult: Premult,
    pub row_bytes: usize,
    buf: PixelBuffer,
    pub props: Arc<PropSet>,
}

/// Rounds half to even and clamps to `0..=max`.
fn quantize(v: f32, max: f32) -> f32 {
    (v.clamp(0.0, 1.0) * max).round_ties_even()
}

impl Image {
    /// A zeroed image, or `None` if the memory cannot be had.
    pub fn new(
        bounds: RectI,
        depth: Depth,
        components: Components,
        premult: Premult,
        render_scale: [f64; 2],
        pixel_aspect: f64,
    ) -> Option<Image> {
        let row_bytes = bounds.width() * components.count() * depth.bytes();
        let buf = PixelBuffer::zeroed(row_bytes.checked_mul(bounds.height())?)?;
        let props = PropSet::new("image");
        props.ro(ofx::kOfxPropType, PropSet::consts(&[ofx::kOfxTypeImage]));
        props.ro(
            ofx::kOfxImageEffectPropPixelDepth,
            PropSet::consts(&[depth.ofx()]),
        );
        props.ro(
            ofx::kOfxImageEffectPropComponents,
            PropSet::consts(&[components.ofx()]),
        );
        props.ro(
            ofx::kOfxImageEffectPropPreMultiplication,
            PropSet::consts(&[premult.ofx()]),
        );
        props.ro(
            ofx::kOfxImageEffectPropRenderScale,
            Values::Double(render_scale.to_vec()),
        );
        props.ro(
            ofx::kOfxImagePropPixelAspectRatio,
            Values::Double(vec![pixel_aspect]),
        );
        props.ro(
            ofx::kOfxImagePropData,
            Values::Pointer(vec![buf.as_ptr() as usize]),
        );
        props.ro(ofx::kOfxImagePropBounds, bounds.values());
        props.ro(ofx::kOfxImagePropRegionOfDefinition, bounds.values());
        props.ro(
            ofx::kOfxImagePropRowBytes,
            Values::Int(vec![i32::try_from(row_bytes).ok()?]),
        );
        props.ro(
            ofx::kOfxImagePropField,
            PropSet::consts(&[ofx::kOfxImageFieldNone]),
        );
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        props.ro(
            ofx::kOfxImagePropUniqueIdentifier,
            Values::strings(&[format!("voxy-{id}")]),
        );
        Some(Image {
            bounds,
            depth,
            components,
            premult,
            row_bytes,
            buf,
            props: Arc::new(props),
        })
    }

    /// An image at `depth` and `components` holding `rgba` (bottom-up
    /// rows, four floats per pixel).
    pub fn from_rgba(
        bounds: RectI,
        rgba: &[f32],
        depth: Depth,
        components: Components,
        premult: Premult,
    ) -> Option<Image> {
        let img = Image::new(bounds, depth, components, premult, [1.0, 1.0], 1.0)?;
        if rgba.len() != bounds.width() * bounds.height() * 4 {
            return None;
        }
        let picks: &[usize] = match components {
            Components::Rgba => &[0, 1, 2, 3],
            Components::Rgb => &[0, 1, 2],
            Components::Alpha => &[3],
        };
        let mut bytes = Vec::with_capacity(img.buf.len());
        for px in rgba.as_chunks::<4>().0 {
            for &c in picks {
                let v = px[c];
                match depth {
                    Depth::Float => bytes.extend_from_slice(&v.to_ne_bytes()),
                    Depth::Short => {
                        bytes.extend_from_slice(&(quantize(v, 65535.0) as u16).to_ne_bytes());
                    }
                    Depth::Byte => bytes.push(quantize(v, 255.0) as u8),
                }
            }
        }
        img.buf.write(&bytes);
        Some(img)
    }

    /// Overwrites the pixels with `rgba` (bottom-up, four floats per
    /// pixel), converted to this image's depth and components. Call only
    /// when no plugin action is writing.
    pub fn write_rgba(&self, rgba: &[f32]) {
        if let Some(tmp) =
            Image::from_rgba(self.bounds, rgba, self.depth, self.components, self.premult)
        {
            self.buf.write(&tmp.buf.to_vec());
        }
    }

    /// Multiplies colour by alpha (for a plugin that renders
    /// unpremultiplied, D2).
    pub fn premultiply(&self) {
        let mut px = self.to_rgba();
        for p in px.as_chunks_mut::<4>().0 {
            let a = p[3];
            for c in &mut p[..3] {
                *c *= a;
            }
        }
        self.write_rgba(&px);
    }

    /// Sets alpha to 1 (for a plugin that renders opaque).
    pub fn make_opaque(&self) {
        let mut px = self.to_rgba();
        for p in px.as_chunks_mut::<4>().0 {
            p[3] = 1.0;
        }
        self.write_rgba(&px);
    }

    /// An image holding `bytes` exactly as laid out in memory (bottom-up
    /// rows of `row_bytes`), or `None` if the sizes disagree.
    pub fn from_raw(
        bounds: RectI,
        depth: Depth,
        components: Components,
        premult: Premult,
        bytes: &[u8],
    ) -> Option<Image> {
        let img = Image::new(bounds, depth, components, premult, [1.0, 1.0], 1.0)?;
        if bytes.len() != img.buf.len() {
            return None;
        }
        img.buf.write(bytes);
        Some(img)
    }

    /// The bytes as laid out in memory. Call only when no plugin action is
    /// writing.
    pub fn raw(&self) -> Vec<u8> {
        self.buf.to_vec()
    }

    /// The pixels as RGBA floats, bottom-up. RGB gets alpha 1; Alpha gets
    /// colour 0. Call only when no plugin action is writing.
    pub fn to_rgba(&self) -> Vec<f32> {
        let bytes = self.buf.to_vec();
        let n = self.components.count();
        let size = self.depth.bytes();
        let channel = |i: usize| -> f32 {
            let b = &bytes[i * size..(i + 1) * size];
            match self.depth {
                Depth::Float => f32::from_ne_bytes([b[0], b[1], b[2], b[3]]),
                Depth::Short => f32::from(u16::from_ne_bytes([b[0], b[1]])) / 65535.0,
                Depth::Byte => f32::from(b[0]) / 255.0,
            }
        };
        let pixels = self.bounds.width() * self.bounds.height();
        let mut out = Vec::with_capacity(pixels * 4);
        for p in 0..pixels {
            let at = |c: usize| channel(p * n + c);
            out.extend_from_slice(&match self.components {
                Components::Rgba => [at(0), at(1), at(2), at(3)],
                Components::Rgb => [at(0), at(1), at(2), 1.0],
                Components::Alpha => [0.0, 0.0, 0.0, at(0)],
            });
        }
        out
    }
}

/// Where an instance's clip images come from, and where its output goes.
/// In the harness this is an in-memory set of images; from O-B, frames the
/// sandbox receives in shared memory.
pub trait ImageSource: Send + Sync + Debug {
    /// The image of `clip` at `time` (frames), or `None` if there is none.
    /// For the output clip this is the buffer the plugin renders into.
    fn image(&self, clip: &str, time: f64) -> Option<Arc<Image>>;

    /// The clip's region of definition at `time`, in canonical coordinates
    /// (`x1, y1, x2, y2`).
    fn region_of_definition(&self, clip: &str, time: f64) -> Option<[f64; 4]>;
}
