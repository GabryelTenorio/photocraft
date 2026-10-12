//! JPEG XL (.jxl) decode support, read-only.
//!
//! Uses `jxl-oxide` (pure-Rust decoder, dual-licensed MIT/Apache-2.0) behind the `jxl` feature
//! flag, through the same `JxlImage` entry points [`crates/raw`](crate) already uses for JPEG XL
//! DNG segments. Every call into the decoder runs under `catch_unwind`, so a decoder panic on a
//! malformed file becomes a [`CodecError`] instead of taking the app down.
//!
//! `jxl-oxide` renders to 16-bit samples, so a document is opened as `U8`-capable `U16` data at
//! the file's own channel count; the first frame of an animation is the image. ICC and EXIF/XMP
//! are not surfaced by the renderer and are left empty rather than guessed at.
//!
//! Feature: `jxl` (optional; enabled in official builds). Write stays unsupported until a
//! pure-Rust encoder exists.

use crate::Format;
use crate::error::CodecError;
use crate::image::Image;
#[cfg(feature = "jxl")]
use crate::image::{ChannelLayout, SampleType};
use crate::options::Limits;

const F: Format = Format::Jxl;

/// Error message when the `jxl` feature is not enabled.
#[cfg(not(feature = "jxl"))]
pub(crate) const NOT_IN_BUILD: &str = "JPEG XL support isn't included in this build of PhotoCraft";

#[cfg(not(feature = "jxl"))]
pub(crate) fn decode(_bytes: &[u8], _limits: &Limits, _keep_orientation: bool) -> Result<Image, CodecError> {
    Err(CodecError::unsupported(F, NOT_IN_BUILD))
}

#[cfg(feature = "jxl")]
pub(crate) fn decode(bytes: &[u8], limits: &Limits, _keep_orientation: bool) -> Result<Image, CodecError> {
    guarded(|| decode_inner(bytes, limits))
}

/// Decode one JPEG XL image. Split from [`decode`] so the panic guard wraps the whole decoder.
#[cfg(feature = "jxl")]
fn decode_inner(bytes: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    use jxl_oxide::{AllocTracker, JxlImage};

    // The decoder works in f32/i32 planes, so its working set is a few times the output; cap it
    // at the caller's per-buffer limit rather than trusting the file's own size claim.
    let budget = usize::try_from(limits.max_alloc).unwrap_or(usize::MAX);
    let image =
        JxlImage::builder().alloc_tracker(AllocTracker::with_limit(budget)).read(bytes).map_err(|e| CodecError::malformed(F, format!("JPEG XL: {e}")))?;

    let render = image.render_frame(0).map_err(|e| CodecError::malformed(F, format!("JPEG XL: {e}")))?;
    let mut stream = render.stream_no_alpha();
    let layout = match stream.channels() {
        1 => ChannelLayout::Gray,
        3 => ChannelLayout::Rgb,
        n => return Err(CodecError::unsupported(F, format!("JPEG XL with {n} channels per pixel"))),
    };

    let (width, height) = (stream.width(), stream.height());
    limits.check(width, height, layout, SampleType::U16)?;

    let count = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(layout.channels()))
        .ok_or_else(|| CodecError::malformed(F, "JPEG XL: image dimensions overflow"))?;
    let mut samples = vec![0u16; count];
    if stream.write_to_buffer(&mut samples) != count {
        return Err(CodecError::malformed(F, "JPEG XL data is truncated"));
    }
    Image::from_u16(width, height, layout, &samples)
}

/// A decoder panic on a malformed file becomes an error, never a crash — the same guard
/// [`crates/raw`](crate) puts around its own jxl-oxide calls.
#[cfg(feature = "jxl")]
fn guarded(f: impl FnOnce() -> Result<Image, CodecError>) -> Result<Image, CodecError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| Err(CodecError::malformed(F, "the JPEG XL decoder failed on this file")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build without the feature reports a clear "unsupported" instead of a confusing error.
    #[cfg(not(feature = "jxl"))]
    #[test]
    fn without_the_feature_jxl_is_a_clear_unsupported() {
        let err = decode(&[0xFF; 32], &Limits::default(), false).expect_err("no jxl in this build");
        assert!(err.to_string().contains("isn't included"), "{err}");
    }

    /// Garbage is a malformed error, never a panic — the guard is what makes that true.
    #[cfg(feature = "jxl")]
    #[test]
    fn garbage_is_an_error_and_not_a_panic() {
        let err = decode(&[0xFF; 64], &Limits::default(), false).expect_err("not a jxl file");
        assert!(!err.to_string().is_empty(), "{err}");
    }

    /// A header claiming absurd dimensions is refused by `Limits` before anything is allocated.
    #[cfg(feature = "jxl")]
    #[test]
    fn limits_refuse_an_absurd_image() {
        let limits = Limits { max_width: 4, max_height: 4, ..Limits::default() };
        let _ = decode(&[0xFF; 64], &limits, false).expect_err("far past the limit");
    }
}
