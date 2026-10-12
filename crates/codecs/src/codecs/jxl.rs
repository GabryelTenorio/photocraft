//! Pure-Rust lossless JPEG XL (Modular) writer.
//!
//! Currently writes a bare JPEG XL codestream. The encoder accepts 8- or 16-bit
//! gray, RGB and RGBA samples, but does not write ICC/EXIF/XMP boxes.
//! Gray+alpha sources are expanded losslessly to RGBA by the export plan.
//! The flat export pipeline converts tagged RGB documents to sRGB first.

use std::panic::{AssertUnwindSafe, catch_unwind};

use zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions as ZuneOptions};
use zune_jpegxl::JxlSimpleEncoder;

use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};

const F: Format = Format::Jxl;

pub(crate) fn encode(image: &Image, plan: Plan) -> Result<Vec<u8>, CodecError> {
    let image = image.converted(plan.layout, plan.sample);
    let space = match image.layout() {
        ChannelLayout::Gray => ColorSpace::Luma,
        ChannelLayout::GrayA => ColorSpace::LumaA,
        ChannelLayout::Rgb => ColorSpace::RGB,
        ChannelLayout::Rgba => ColorSpace::RGBA,
        _ => return Err(CodecError::unsupported(F, "JPEG XL export supports grayscale and RGB, with optional alpha")),
    };
    let depth = match image.sample_type() {
        SampleType::U8 => BitDepth::Eight,
        SampleType::U16 => BitDepth::Sixteen,
        _ => return Err(CodecError::unsupported(F, "JPEG XL lossless export supports 8 or 16-bit integer pixels")),
    };
    let (width, height) = image.dimensions();
    let samples = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(image.layout().channels()))
        .and_then(|n| n.checked_mul(image.sample_type().bytes()))
        .ok_or_else(|| CodecError::encode(F, "image dimensions overflow the buffer size"))?;
    if image.data().len() != samples {
        return Err(CodecError::InvalidImage("incorrect JPEG XL pixel buffer length".into()));
    }
    // A panic in an external image encoder must not terminate the host application.
    catch_unwind(AssertUnwindSafe(|| {
        let options = ZuneOptions::new(width as usize, height as usize, space, depth);
        let mut output = Vec::new();
        JxlSimpleEncoder::new(image.data(), options).encode(&mut output).map_err(|e| CodecError::encode(F, e.to_string()))?;
        Ok(output)
    }))
    .unwrap_or_else(|_| Err(CodecError::encode(F, "the JPEG XL encoder failed")))
}
