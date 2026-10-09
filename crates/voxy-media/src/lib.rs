//! Media I/O for voXY.
//!
//! - Still images (PNG, JPEG) with the pure-Rust `image` crate.
//! - [`video`] (feature `ffmpeg`, on by default): FFmpeg demuxing and
//!   decoding with optional hardware acceleration, frame-accurate seeking
//!   and flick timestamps. FFmpeg is linked dynamically; voXY ships an LGPL
//!   build (see `LICENSING.md`).
//! - [`timebase`]: exact conversion between stream time bases and flicks.
//!
//! - [`playback`]: audio output (cpal, or a timer-driven null output), the
//!   master clock, and the feeder thread that renders timeline audio.
//! - [`sync`]: choosing the video frame for the master clock (A/V sync).
//! - [`audio`] (feature `ffmpeg`): sample-accurate audio decoding and
//!   resampling.

use std::path::Path;

#[cfg(feature = "ffmpeg")]
pub mod audio;
pub mod playback;
pub mod sync;
pub mod timebase;
#[cfg(feature = "ffmpeg")]
pub mod video;

/// A decoded still image: 8-bit RGBA, sRGB-encoded, straight alpha,
/// rows top to bottom with no padding.
#[derive(Clone, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for DecodedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not decode {path}: {source}")]
    Decode {
        path: String,
        #[source]
        source: image::ImageError,
    },
    /// A video or audio file could not be opened, probed or decoded.
    #[error("{path}: {message}")]
    Media { path: String, message: String },
}

fn decode_err(path: &Path, source: image::ImageError) -> MediaError {
    match source {
        image::ImageError::IoError(source) => MediaError::Io {
            path: path.display().to_string(),
            source,
        },
        source => MediaError::Decode {
            path: path.display().to_string(),
            source,
        },
    }
}

/// Decodes an image file to RGBA8.
pub fn load_image(path: &Path) -> Result<DecodedImage, MediaError> {
    let img = image::ImageReader::open(path)
        .map_err(|source| MediaError::Io {
            path: path.display().to_string(),
            source,
        })?
        .with_guessed_format()
        .map_err(|source| MediaError::Io {
            path: path.display().to_string(),
            source,
        })?
        .decode()
        .map_err(|e| decode_err(path, e))?
        .into_rgba8();
    Ok(DecodedImage {
        width: img.width(),
        height: img.height(),
        pixels: img.into_raw(),
    })
}

/// Reads an image's pixel size without decoding it.
pub fn image_dimensions(path: &Path) -> Result<(u32, u32), MediaError> {
    image::image_dimensions(path).map_err(|e| decode_err(path, e))
}
