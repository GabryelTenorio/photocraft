//! JPEG XL (ISO/IEC 18181), read-only: see [`crate::ASYMMETRIC_EXCEPTIONS`].
//!
//! Decoding is done by `jxl-oxide`, a pure-Rust decoder, already used for the JPEG XL-compressed
//! tiles of a DNG in `photocraft-raw` and therefore already linked into every native build. Like
//! heic-rs in `photocraft-heif`, every call into it runs under `catch_unwind`, so a decoder panic
//! on a malformed file becomes [`CodecError::Malformed`] instead of taking the app down.
//!
//! Both file shapes decode: the bare codestream (signature `FF 0A`) and the ISOBMFF container
//! (`JXL ` signature box), which is where EXIF and XMP live.
//!
//! The browser build (wasm32) leaves the decoder out — it added 0.88 MB to a wasm that must stay
//! under the per-file cap of static hosts (`packaging/web/package.sh`) — and reports these files as
//! [`CodecError::Unsupported`], like `photocraft-raw` does for JPEG XL DNG.
//!
//! Orientation lives in the JPEG XL image header, not in EXIF, and `jxl-oxide` applies it while
//! writing the stream. The EXIF and XMP Orientation tags are therefore rewritten to 1 on the way
//! out, so an export that copies them cannot turn the picture a second time.

use crate::Format;
use crate::error::CodecError;
use crate::image::Image;
use crate::options::Limits;

#[cfg(not(target_arch = "wasm32"))]
use crate::image::{ChannelLayout, DecodeWarning, Metadata, SampleType};
#[cfg(not(target_arch = "wasm32"))]
use jxl_oxide::image::BitDepth;
#[cfg(not(target_arch = "wasm32"))]
use jxl_oxide::{AllocTracker, AuxBoxData, JxlImage, PixelFormat};

const F: Format = Format::Jxl;

/// The reason given when this build has no JPEG XL decoder.
#[cfg(target_arch = "wasm32")]
pub(crate) const NOT_IN_BUILD: &str = "JPEG XL support isn't included in the browser version of PhotoCraft; open the file in the desktop app";

/// The browser build has no JPEG XL decoder (see the module docs).
#[cfg(target_arch = "wasm32")]
pub(crate) fn decode(_bytes: &[u8], _limits: &Limits, _keep_orientation: bool) -> Result<Image, CodecError> {
    Err(CodecError::unsupported(F, NOT_IN_BUILD))
}

/// Decodes a JPEG XL file (bare codestream or container) into its first frame.
///
/// `keep_orientation` hands the pixels back as coded: `jxl-oxide` always applies the header's
/// orientation while writing the stream, so the decoder turns the result back by the inverse
/// transform, which is the only way to get the stored geometry out of it.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn decode(bytes: &[u8], limits: &Limits, keep_orientation: bool) -> Result<Image, CodecError> {
    guarded(|| decode_inner(bytes, limits, keep_orientation))
}

#[cfg(not(target_arch = "wasm32"))]
fn decode_inner(bytes: &[u8], limits: &Limits, keep_orientation: bool) -> Result<Image, CodecError> {
    let mut image = JxlImage::builder().alloc_tracker(AllocTracker::with_limit(alloc_budget(limits))).read(bytes).map_err(|e| CodecError::malformed(F, e))?;

    // The header states the size, and nothing is decoded before the limits have seen it. The
    // decoder applies the orientation itself, so both the stored and the upright size must fit:
    // orientations 5-8 swap the sides.
    let (orientation, coded_w, coded_h) = {
        let header = image.image_header();
        (header.metadata.orientation.clamp(1, 8) as u16, header.size.width, header.size.height)
    };
    let pixel_format = image.pixel_format();
    let layout = layout_for(pixel_format);
    let sample = sample_for(&image);
    limits.check(coded_w, coded_h, layout, sample)?;
    if orientation >= 5 {
        limits.check(coded_h, coded_w, layout, sample)?;
    }

    // A file whose colour space is an ICC profile that jxl-oxide cannot map onto a built-in
    // encoding would be converted to sRGB, which needs an external CMS (none is linked in) and so
    // fails outright. Asking for the profile itself keeps the pixels in the file's own space,
    // needs no conversion, and leaves the embedded profile valid for them.
    let embedded = image.original_icc().map(<[u8]>::to_vec);
    if let Some(profile) = embedded.as_deref()
        && matches!(image.image_header().metadata.colour_encoding, jxl_oxide::color::ColourEncoding::IccProfile(_))
    {
        image.request_icc(profile).map_err(|e| CodecError::malformed(F, e))?;
    }

    let render = image.render_frame(0).map_err(|e| CodecError::malformed(F, e))?;
    let mut stream = render.stream();
    let (w, h) = (stream.width(), stream.height());
    if stream.channels() as usize != pixel_format.channels() {
        return Err(CodecError::malformed(F, "the decoded channels do not match the image header"));
    }
    let count =
        (w as usize).checked_mul(h as usize).and_then(|p| p.checked_mul(layout.channels())).ok_or(CodecError::LimitExceeded("image size overflows".into()))?;

    let mut img = match sample {
        SampleType::U8 => {
            let mut data = vec![0u8; count];
            if stream.write_to_buffer(&mut data) != count {
                return Err(CodecError::malformed(F, "the codestream ends early"));
            }
            Image::from_u8(w, h, layout, data)?
        }
        SampleType::U16 => {
            let mut data = vec![0u16; count];
            if stream.write_to_buffer(&mut data) != count {
                return Err(CodecError::malformed(F, "the codestream ends early"));
            }
            Image::from_u16(w, h, layout, &data)?
        }
        _ => {
            let mut data = vec![0.0f32; count];
            if stream.write_to_buffer(&mut data) != count {
                return Err(CodecError::malformed(F, "the codestream ends early"));
            }
            Image::from_f32(w, h, layout, &data)?
        }
    };

    img.icc = embedded;
    img.meta = metadata(&image, !keep_orientation)?;
    if image.num_loaded_keyframes() > 1 {
        img.warnings.push(DecodeWarning::MoreFrames { total: Some(image.num_loaded_keyframes() as u32) });
    }
    if keep_orientation {
        // Undo what the stream did, so the pixels come back as they are stored.
        img = un_orient(img, orientation)?;
    }
    Ok(img)
}

/// Undoes the orientation `jxl-oxide`'s stream applied, leaving the metadata exactly as the caller
/// asked for it: `Image::oriented` stands the EXIF and XMP Orientation tags upright too, which is
/// what must *not* happen here — `keep_orientation` means the file as it is, tags included.
#[cfg(not(target_arch = "wasm32"))]
fn un_orient(mut img: Image, o: u16) -> Result<Image, CodecError> {
    if o == 1 {
        return Ok(img);
    }
    let meta = std::mem::take(&mut img.meta);
    Ok(img.oriented(inverse(o))?.with_meta(meta))
}

/// The budget handed to `jxl-oxide`: it decodes into f32/i32 planes, so allow a few times the
/// output, and never more than the caller's own per-buffer limit.
#[cfg(not(target_arch = "wasm32"))]
fn alloc_budget(limits: &Limits) -> usize {
    let b = limits.max_pixels.saturating_mul(16).min(limits.max_alloc);
    usize::try_from(b).unwrap_or(usize::MAX)
}

/// The sample type a file decodes to: its own bit depth. Anything wider than 8 bits is written
/// into `u16`, everything else into `u8`, both exactly for a lossless file.
#[cfg(not(target_arch = "wasm32"))]
fn sample_for(image: &JxlImage) -> SampleType {
    match image.image_header().metadata.bit_depth {
        BitDepth::FloatSample { .. } => SampleType::F32,
        BitDepth::IntegerSample { bits_per_sample } if bits_per_sample > 8 => SampleType::U16,
        _ => SampleType::U8,
    }
}

/// Every [`PixelFormat`] maps to a layout, so this cannot fail.
#[cfg(not(target_arch = "wasm32"))]
fn layout_for(pixel_format: PixelFormat) -> ChannelLayout {
    match pixel_format {
        PixelFormat::Gray => ChannelLayout::Gray,
        PixelFormat::Graya => ChannelLayout::GrayA,
        PixelFormat::Rgb => ChannelLayout::Rgb,
        PixelFormat::Rgba => ChannelLayout::Rgba,
        PixelFormat::Cmyk => ChannelLayout::Cmyk,
        PixelFormat::Cmyka => ChannelLayout::CmykA,
    }
}

/// EXIF and XMP, which only the container carries. The EXIF box holds a 4-byte offset to the TIFF
/// header followed by the TIFF data itself.
///
/// `upright` rewrites the Orientation tags to 1: the orientation the pixels already carry is the
/// image header's, which is what `jxl-oxide` applied, so an export that copies the tags must not
/// apply it a second time.
#[cfg(not(target_arch = "wasm32"))]
fn metadata(image: &JxlImage, upright: bool) -> Result<Metadata, CodecError> {
    let boxes = image.aux_boxes();
    let exif = match boxes.first_exif().map_err(|e| CodecError::malformed(F, e))? {
        AuxBoxData::Data(raw) => {
            let payload = raw.payload();
            usize::try_from(raw.tiff_header_offset()).ok().and_then(|at| payload.get(at..)).map(<[u8]>::to_vec)
        }
        // Not present, or the box is still being decompressed: either way there is nothing to keep.
        AuxBoxData::NotFound | AuxBoxData::Decoding => None,
    };
    let xmp = match boxes.first_xml() {
        AuxBoxData::Data(xml) => String::from_utf8(xml.to_vec()).ok(),
        AuxBoxData::NotFound | AuxBoxData::Decoding => None,
    };
    Ok(Metadata {
        exif: exif.map(|e| if upright { crate::orientation::upright_exif(&e).into_owned() } else { e }),
        xmp: xmp.map(|x| if upright { crate::orientation::upright_xmp(&x).into_owned() } else { x }),
        ..Default::default()
    })
}

/// The orientation that undoes `o` (EXIF's codes, which JPEG XL reuses): 2, 3, 4, 5 and 7 are
/// their own inverses; 6 and 8 are each other's.
#[cfg(not(target_arch = "wasm32"))]
const fn inverse(o: u16) -> u16 {
    match o {
        6 => 8,
        8 => 6,
        _ => o,
    }
}

/// Runs `f`, turning a `jxl-oxide` panic into an error. It parses untrusted files and still
/// panics on some of them (`FrameBuffer::from_grids` expects its size arithmetic to fit, and
/// `ImageStream::from_render` asserts the orientation is 1-8).
#[cfg(not(target_arch = "wasm32"))]
fn guarded<T>(f: impl FnOnce() -> Result<T, CodecError>) -> Result<T, CodecError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| Err(CodecError::malformed(F, "the JPEG XL decoder failed on this file")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn orientation_inverses() {
        // 6 and 8 turn by opposite quarters; the rest undo themselves.
        assert_eq!(inverse(6), 8);
        assert_eq!(inverse(8), 6);
        for o in 1..=8u16 {
            assert_eq!(inverse(inverse(o)), o, "{o}");
            assert!((1..=8).contains(&inverse(o)));
        }
    }

    /// `inverse` must really undo an orientation: `keep_orientation` turns the decoder's oriented
    /// pixels back, so a wrong table here would hand out a transposed image.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn inverse_undoes_the_crate_s_own_orientation() {
        // An asymmetric picture, so a wrong flip or quarter turn cannot pass.
        let (w, h) = (5u32, 3u32);
        let values: Vec<f32> = (0..(w * h * 3) as usize).map(|i| i as f32 / 44.0).collect();
        let img = Image::from_normalized(w, h, ChannelLayout::Rgb, SampleType::U8, &values).unwrap();
        for o in 2..=8u16 {
            let turned = img.clone().oriented(o).unwrap();
            let back = turned.oriented(inverse(o)).unwrap();
            assert_eq!(back.dimensions(), img.dimensions(), "{o}");
            assert_eq!(back.data(), img.data(), "orientation {o} did not round-trip");
        }
    }

    /// `keep_orientation` promises the file as stored: the pixels *and* the Orientation tags, so
    /// `un_orient` must not let `Image::oriented` rewrite the metadata on its way back.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn un_orient_keeps_the_metadata_verbatim() {
        use crate::exif_orientation;
        let (w, h) = (5u32, 3u32);
        let values: Vec<f32> = (0..(w * h * 3) as usize).map(|i| i as f32 / 44.0).collect();
        let img = Image::from_normalized(w, h, ChannelLayout::Rgb, SampleType::U8, &values).unwrap();
        // Orientation 6, as a JPEG XL header states it and its EXIF box repeats it.
        let exif = {
            let mut e = b"II*\0\x08\0\0\0".to_vec();
            e.extend_from_slice(&1u16.to_le_bytes());
            e.extend_from_slice(&0x0112u16.to_le_bytes());
            e.extend_from_slice(&3u16.to_le_bytes());
            e.extend_from_slice(&1u32.to_le_bytes());
            e.extend_from_slice(&6u32.to_le_bytes());
            e.extend_from_slice(&0u32.to_le_bytes());
            e
        };
        let verbatim = Metadata { exif: Some(exif.clone()), ..Default::default() };

        // What the decoder does: the stream has already turned the pixels, and the metadata is
        // attached afterwards, exactly as it came out of the container.
        let mut turned = img.clone().oriented(6).unwrap();
        turned.meta = verbatim.clone();
        let kept = un_orient(turned, 6).unwrap();
        assert_eq!(kept.data(), img.data(), "pixels back as stored");
        assert_eq!(kept.meta.exif, Some(exif.clone()), "the Orientation tag is left alone");

        // Without the helper the second turn stands the tag upright as well, which is exactly the
        // double rotation `keep_orientation` has to avoid.
        let mut leaky = img.clone();
        leaky.meta = verbatim.clone();
        let leaky = leaky.oriented(6).unwrap().oriented(8).unwrap();
        assert_eq!(exif_orientation(leaky.meta.exif.as_deref().unwrap_or_default()), 1, "Image::oriented rewrites the tag");

        // Orientation 1 is a no-op, not a turn by 180.
        let mut plain = img.clone();
        plain.meta = verbatim;
        let same = un_orient(plain.clone(), 1).unwrap();
        assert_eq!(same.data(), img.data());
        assert_eq!(same.meta.exif, Some(exif));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn pixel_formats_map_to_layouts() {
        assert_eq!(layout_for(PixelFormat::Gray), ChannelLayout::Gray);
        assert_eq!(layout_for(PixelFormat::Graya), ChannelLayout::GrayA);
        assert_eq!(layout_for(PixelFormat::Rgb), ChannelLayout::Rgb);
        assert_eq!(layout_for(PixelFormat::Rgba), ChannelLayout::Rgba);
        assert_eq!(layout_for(PixelFormat::Cmyk), ChannelLayout::Cmyk);
        assert_eq!(layout_for(PixelFormat::Cmyka), ChannelLayout::CmykA);
    }
}
