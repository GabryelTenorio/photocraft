//! JPEG XL decoding, against files this suite synthesises.
//!
//! Fixtures come from `zune-jpegxl` (a pure-Rust lossless JPEG XL encoder) plus containers this
//! file builds by hand around its codestream, which is how the ISOBMFF EXIF and XMP boxes are
//! covered. `zune-jpegxl` writes bare codestreams only, so what a pure-Rust encoder cannot produce
//! — the image header's orientation, animations, float and CMYK samples — is left to the real-file
//! corpus (`corpus/jxl`, `cargo xtask corpus --jxl`, not wired up yet) and is called out below.

#![cfg(not(target_arch = "wasm32"))]

mod common;
use common::*;
use photocraft_codecs::*;
use zune_core::bit_depth::BitDepth;
use zune_core::colorspace::ColorSpace;
use zune_core::options::EncoderOptions;
use zune_jpegxl::JxlSimpleEncoder;

/// A lossless JPEG XL file for `img` (8- or 16-bit, Luma, Luma+A, RGB or RGBA), as a bare
/// codestream. Those four colour spaces are what `zune-jpegxl` can encode.
fn encode_jxl(img: &Image) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let space = match img.layout() {
        ChannelLayout::Gray => ColorSpace::Luma,
        ChannelLayout::GrayA => ColorSpace::LumaA,
        ChannelLayout::Rgb => ColorSpace::RGB,
        ChannelLayout::Rgba => ColorSpace::RGBA,
        other => panic!("zune-jpegxl cannot encode {other:?}"),
    };
    let depth = match img.sample_type() {
        SampleType::U16 => BitDepth::Sixteen,
        _ => BitDepth::Eight,
    };
    // 16-bit samples go in as native-endian bytes, which is how Image stores them.
    let bytes = img.data().to_vec();
    let mut out = Vec::new();
    JxlSimpleEncoder::new(&bytes, EncoderOptions::new(w as usize, h as usize, space, depth)).encode(&mut out).expect("encode a lossless JPEG XL fixture");
    out
}

/// An ISOBMFF container around `codestream`, with optional EXIF and XMP boxes.
///
/// Box layout, per ISO/IEC 18181-2: the 12-byte signature box, an `ftyp` box, the metadata boxes,
/// then the `jxlc` codestream box last (a box with a size, so the parser knows more may follow).
fn container(codestream: &[u8], exif: Option<&[u8]>, xmp: Option<&str>) -> Vec<u8> {
    fn box_out(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
        out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"\0\0\0\x0CJXL \x0D\x0A\x87\x0A");
    // ftyp: major brand "JXL ", minor version 0, one compatible brand "jxl ".
    box_out(&mut out, b"ftyp", b"JXL \0\0\0\0jxl ");
    if let Some(exif) = exif {
        // The Exif box prefixes the TIFF data with the offset of its header (always 0 here).
        let mut payload = 0u32.to_be_bytes().to_vec();
        payload.extend_from_slice(exif);
        box_out(&mut out, b"Exif", &payload);
    }
    if let Some(xmp) = xmp {
        box_out(&mut out, b"xml ", xmp.as_bytes());
    }
    box_out(&mut out, b"jxlc", codestream);
    out
}

#[test]
fn a_lossless_rgb_file_decodes_back_exactly() {
    let img = synth(23, 17, ChannelLayout::Rgb, SampleType::U8, 3, 0.0);
    let bytes = encode_jxl(&img);
    assert_eq!(detect(&bytes), Some(Format::Jxl), "a bare codestream is recognised");
    let back = decode(&bytes).unwrap();
    assert_eq!((back.dimensions(), back.layout(), back.sample_type()), ((23, 17), ChannelLayout::Rgb, SampleType::U8));
    assert_eq!(back.data(), img.data(), "lossless: every sample comes back");
}

#[test]
fn gray_gray_alpha_and_rgba_all_decode() {
    for layout in [ChannelLayout::Gray, ChannelLayout::GrayA, ChannelLayout::Rgb, ChannelLayout::Rgba] {
        let img = synth(19, 13, layout, SampleType::U8, 5, 0.0);
        let back = decode(&encode_jxl(&img)).unwrap_or_else(|e| panic!("{layout:?}: {e}"));
        assert_eq!(back.layout(), layout, "{layout:?}");
        assert_eq!(back.sample_type(), SampleType::U8, "{layout:?}");
        assert_eq!(back.dimensions(), img.dimensions(), "{layout:?}");
        assert_eq!(back.data(), img.data(), "{layout:?}: lossless");
    }
}

#[test]
fn sixteen_bit_samples_survive_without_being_stretched_to_eight() {
    let img = synth(21, 15, ChannelLayout::Rgb, SampleType::U16, 7, 0.0);
    let back = decode(&encode_jxl(&img)).unwrap();
    assert_eq!(back.sample_type(), SampleType::U16, "16-bit is kept, not reduced");
    assert_eq!(back.data(), img.data(), "16-bit lossless is exact");
    // A proof it is really 16-bit and not 8-bit values in a wider buffer.
    let samples = back.to_u16_samples().unwrap();
    assert!(samples.iter().filter(|v| *v % 257 != 0).count() > samples.len() / 2);
}

#[test]
fn the_container_form_decodes_too() {
    let img = synth(31, 19, ChannelLayout::Rgb, SampleType::U8, 11, 0.0);
    let file = container(&encode_jxl(&img), None, None);
    assert_eq!(detect(&file), Some(Format::Jxl), "the container signature box is recognised");
    assert_eq!(decode(&file).unwrap().data(), img.data());
}

#[test]
fn exif_and_xmp_come_out_of_the_container() {
    let img = synth(17, 11, ChannelLayout::Rgb, SampleType::U8, 13, 0.0);
    let exif = sample_exif();
    let file = container(&encode_jxl(&img), Some(&exif), Some(SAMPLE_XMP));
    let back = decode(&file).unwrap();
    assert_eq!(back.meta.exif.as_deref(), Some(&exif[..]), "the TIFF payload, prefix offset removed");
    assert_eq!(back.meta.xmp.as_deref(), Some(SAMPLE_XMP));
    // A bare codestream carries neither.
    let bare = decode(&encode_jxl(&img)).unwrap();
    assert!(bare.meta.exif.is_none() && bare.meta.xmp.is_none());
}

#[test]
fn the_exif_orientation_is_rewritten_to_one() {
    // The pixels are turned upright by the image header, which is what JPEG XL uses; an EXIF tag
    // left at 6 would turn the picture a second time on the next export.
    let img = synth(13, 9, ChannelLayout::Rgb, SampleType::U8, 17, 0.0);
    let mut exif = sample_exif();
    exif[18] = 6; // the Orientation SHORT's value (header 8, count 2, tag 2, type 2, count 4)
    let file = container(&encode_jxl(&img), Some(&exif), None);

    let back = decode(&file).unwrap();
    assert_eq!(exif_orientation(back.meta.exif.as_deref().unwrap()), 1, "rewritten for the export");
    let kept = decode_with(&file, &DecodeOptions { keep_orientation: true, ..Default::default() }).unwrap();
    assert_eq!(exif_orientation(kept.meta.exif.as_deref().unwrap()), 6, "kept verbatim when asked");
}

#[test]
fn limits_are_checked_before_any_pixel_is_decoded() {
    let img = synth(64, 48, ChannelLayout::Rgb, SampleType::U8, 19, 0.0);
    let bytes = encode_jxl(&img);
    let tight = |limits| DecodeOptions { limits, ..Default::default() };
    assert!(matches!(decode_with(&bytes, &tight(Limits { max_width: 32, ..Default::default() })), Err(CodecError::LimitExceeded(_))));
    assert!(matches!(decode_with(&bytes, &tight(Limits { max_height: 32, ..Default::default() })), Err(CodecError::LimitExceeded(_))));
    assert!(matches!(decode_with(&bytes, &tight(Limits { max_pixels: 1000, ..Default::default() })), Err(CodecError::LimitExceeded(_))));
    assert!(matches!(decode_with(&bytes, &tight(Limits { max_alloc: 64 * 64 - 1, ..Default::default() })), Err(CodecError::LimitExceeded(_))));
}

#[test]
fn every_truncation_is_an_error_or_an_image_never_a_panic() {
    let img = synth(33, 21, ChannelLayout::Rgba, SampleType::U8, 23, 0.0);
    for bytes in [encode_jxl(&img), container(&encode_jxl(&img), Some(&sample_exif()), Some(SAMPLE_XMP))] {
        for cut in 0..bytes.len() {
            let _ = decode_as(Format::Jxl, &bytes[..cut]);
        }
    }
}

#[test]
fn bit_flips_never_panic() {
    let img = synth(29, 19, ChannelLayout::Rgb, SampleType::U8, 29, 0.0);
    let mut rng = Rng::new(42);
    for base in [encode_jxl(&img), container(&encode_jxl(&img), Some(&sample_exif()), None)] {
        for _ in 0..300 {
            let mut b = base.clone();
            for _ in 0..1 + (rng.next_u64() % 4) as usize {
                let i = (rng.next_u64() as usize) % b.len();
                b[i] ^= 1 << (rng.next_u64() % 8);
            }
            let _ = decode_as(Format::Jxl, &b);
        }
    }
}

#[test]
fn jxl_is_read_only_and_says_so() {
    let c = caps(Format::Jxl);
    assert!(c.read && !c.write);
    assert!(c.animation, "the format holds animations");
    assert!(ASYMMETRIC_EXCEPTIONS.iter().any(|(f, _)| *f == Format::Jxl), "read-only needs a documented reason");

    let img = synth(13, 9, ChannelLayout::Rgb, SampleType::U8, 31, 0.0);
    assert!(matches!(encode(&img, Format::Jxl, &EncodeOptions::default()), Err(CodecError::Unsupported { .. })));
    assert_eq!(fidelity_warnings(&img, Format::Jxl), vec![FidelityWarning::WriteUnsupported { format: Format::Jxl }]);
}

#[test]
fn broken_files_are_errors_not_crashes() {
    // Right signature, nothing behind it.
    assert!(decode_as(Format::Jxl, &[0xFF, 0x0A]).is_err());
    assert!(decode_as(Format::Jxl, b"\0\0\0\x0CJXL \x0D\x0A\x87\x0A").is_err());
    assert!(decode_as(Format::Jxl, b"").is_err());
    // An Exif box whose TIFF header offset points past its payload. The pixels are intact, but a
    // metadata box we cannot locate is a broken file, not a file to quietly open without its EXIF.
    let img = synth(9, 7, ChannelLayout::Rgb, SampleType::U8, 37, 0.0);
    let mut payload = 9999u32.to_be_bytes().to_vec();
    payload.extend_from_slice(&sample_exif());
    let mut file = b"\0\0\0\x0CJXL \x0D\x0A\x87\x0A".to_vec();
    file.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
    file.extend_from_slice(b"Exif");
    file.extend_from_slice(&payload);
    file.extend_from_slice(&((encode_jxl(&img).len() + 8) as u32).to_be_bytes());
    file.extend_from_slice(b"jxlc");
    file.extend_from_slice(&encode_jxl(&img));
    assert!(
        matches!(decode_as(Format::Jxl, &file), Err(CodecError::Malformed { format: Format::Jxl, .. })),
        "an unusable Exif box is reported, not guessed at: {:?}",
        decode_as(Format::Jxl, &file)
    );
}
