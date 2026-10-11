//! JPEG XL files open as documents: through the same `import` File › Open uses, with their EXIF
//! and XMP, and an export never turns them a second time.
//!
//! The fixtures are synthesised by `zune-jpegxl`, a test-only pure-Rust lossless encoder, and by
//! the container this file builds around its codestream. Native only: the browser build has no
//! JPEG XL decoder.

#![cfg(not(target_arch = "wasm32"))]

use photocraft_codecs::{Format, decode, detect, exif_orientation};
use photocraft_doc::{ColorMode, SampleType};
use photocraft_io::{ExportOptions, export, import};
use zune_core::bit_depth::BitDepth;
use zune_core::colorspace::ColorSpace;
use zune_core::options::EncoderOptions;
use zune_jpegxl::JxlSimpleEncoder;

/// A 24x16 lossless JPEG XL codestream, optionally wrapped in a container carrying EXIF and XMP.
fn jxl(exif: Option<&[u8]>, xmp: Option<&str>) -> Vec<u8> {
    fn box_out(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
        out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
    }
    // A left-to-right red ramp: unmistakable if a channel or a row goes astray.
    let (w, h) = (24u32, 16u32);
    let px: Vec<u8> = (0..w * h * 3).map(|i| (i / 3 % w * 255 / w) as u8).collect();
    let mut codestream = Vec::new();
    JxlSimpleEncoder::new(&px, EncoderOptions::new(w as usize, h as usize, ColorSpace::RGB, BitDepth::Eight))
        .encode(&mut codestream)
        .expect("encode a lossless JPEG XL fixture");

    let Some(exif) = exif else { return codestream };
    let mut out = b"\0\0\0\x0CJXL \x0D\x0A\x87\x0A".to_vec();
    box_out(&mut out, b"ftyp", b"JXL \0\0\0\0jxl ");
    let mut payload = 0u32.to_be_bytes().to_vec();
    payload.extend_from_slice(exif);
    box_out(&mut out, b"Exif", &payload);
    if let Some(xmp) = xmp {
        box_out(&mut out, b"xml ", xmp.as_bytes());
    }
    box_out(&mut out, b"jxlc", &codestream);
    out
}

/// The minimal little-endian TIFF-structured EXIF this crate's own tests use, with its
/// Orientation set to `o`.
fn exif_with_orientation(o: u8) -> Vec<u8> {
    let mut exif = b"II*\0\x08\0\0\0".to_vec();
    exif.extend_from_slice(&1u16.to_le_bytes()); // 1 entry
    exif.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
    exif.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    exif.extend_from_slice(&1u32.to_le_bytes());
    exif.extend_from_slice(&(o as u32).to_le_bytes());
    exif.extend_from_slice(&0u32.to_le_bytes()); // next IFD
    exif
}

const XMP: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta><?xpacket end="w"?>"#;

#[test]
fn jxl_opens_as_an_8_bit_rgb_document() {
    let r = import("photo.jxl", &jxl(None, None)).unwrap();
    let d = &r.document;
    assert_eq!((d.size.width, d.size.height), (24, 16));
    assert_eq!((d.mode, d.depth), (ColorMode::Rgb, SampleType::U8));
    assert_eq!(d.layers.len(), 1);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn jxl_metadata_reaches_the_document_and_exports_upright() {
    let exif = exif_with_orientation(6);
    let r = import("photo.jxl", &jxl(Some(&exif), Some(XMP))).unwrap();
    let d = &r.document;
    assert!(d.metadata.exif.is_some(), "the container's EXIF reaches the document");
    assert_eq!(d.metadata.xmp.as_deref(), Some(XMP));
    // JPEG XL is read-only: save the photo as a JPEG, which keeps the EXIF.
    let out = export(d, "photo.jpg", &ExportOptions::default()).unwrap();
    assert_eq!(detect(&out.bytes), Some(Format::Jpeg));
    let back = decode(&out.bytes).unwrap();
    assert_eq!(back.dimensions(), (24, 16));
    assert_eq!(exif_orientation(back.meta.exif.as_deref().unwrap()), 1, "not turned a second time on the way out");
}

#[test]
fn jxl_cannot_be_written_and_says_so() {
    let d = import("x.jxl", &jxl(None, None)).unwrap().document;
    assert!(export(&d, "x.jxl", &ExportOptions::default()).is_err());
}

#[test]
fn broken_jxl_is_an_error_not_a_crash() {
    let bytes = jxl(Some(&exif_with_orientation(1)), Some(XMP));
    for cut in [0, 1, 2, 12, 40, bytes.len() / 2, bytes.len() - 1] {
        assert!(import("x.jxl", &bytes[..cut]).is_err(), "cut at {cut}");
    }
}
