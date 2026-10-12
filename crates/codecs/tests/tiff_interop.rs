//! Interoperability of the TIFF writer (#4484). Third-party software that only understands
//! baseline TIFF 6.0 refused files from PhotoCraft; two things in the directory kept it from
//! opening them:
//!
//! * the default compression was ZIP/Deflate, an Adobe extension (compression 8) rather than a
//!   scheme of the format itself, which older readers (notably the Windows imaging stacks) do
//!   not decode;
//! * `RowsPerStrip` was the `tiff` encoder's ~1 MB strip size with no upper bound, so a short
//!   image declared far more rows per strip than it has rows (9010 for a 23-row image), which a
//!   reader that trusts the tag over `StripByteCounts` cannot reconcile with `ImageLength`.
//!
//! These tests pin the fixed behaviour.

mod common;
use common::*;
use photocraft_codecs::*;

const TAG_ROWS_PER_STRIP: u16 = 278;

#[test]
fn the_default_compression_is_lzw() {
    assert_eq!(EncodeOptions::default().tiff_compression, TiffCompression::Lzw);
    // LZW is TIFF compression 5; ZIP/Deflate (the Adobe extension) would be 8.
    let img = test_image(ChannelLayout::Rgb, SampleType::U8);
    let bytes = encode(&img, Format::Tiff, &EncodeOptions::default()).unwrap();
    assert_eq!(tiff_info(&bytes).unwrap().pages[0].compression, 5);
}

#[test]
fn every_compression_still_round_trips() {
    for layout in [ChannelLayout::Gray, ChannelLayout::Rgb, ChannelLayout::Rgba, ChannelLayout::Cmyk] {
        for compression in [TiffCompression::None, TiffCompression::Lzw, TiffCompression::Deflate, TiffCompression::PackBits] {
            let img = test_image(layout, SampleType::U8);
            let bytes = encode(&img, Format::Tiff, &EncodeOptions { tiff_compression: compression, ..Default::default() }).unwrap();
            let back = decode(&bytes).unwrap();
            assert_eq!(back.data(), img.data(), "{layout:?} {compression:?}");
        }
    }
}

#[test]
fn rows_per_strip_is_never_larger_than_the_image() {
    // A short image (what the bug hit), a one-pixel image, and images tall enough for the
    // encoder's own ~1 MB strip sizing.
    for (w, h) in [(37u32, 23u32), (1, 1), (600, 40), (1000, 500), (5, 9000)] {
        for layout in [ChannelLayout::Gray, ChannelLayout::Rgb, ChannelLayout::Rgba] {
            let img = synth(w, h, layout, SampleType::U8, 3, 0.0);
            for compression in [TiffCompression::None, TiffCompression::Lzw, TiffCompression::Deflate, TiffCompression::PackBits] {
                let bytes = encode(&img, Format::Tiff, &EncodeOptions { tiff_compression: compression, ..Default::default() }).unwrap();
                let rows = tiff_tag_u64(&bytes, TAG_ROWS_PER_STRIP).unwrap_or_else(|| panic!("{w}x{h} {layout:?} {compression:?}: no RowsPerStrip"));
                assert!(rows >= 1, "{w}x{h} {layout:?} {compression:?}: RowsPerStrip {rows}");
                assert!(rows <= u64::from(h), "{w}x{h} {layout:?} {compression:?}: RowsPerStrip {rows} > ImageLength {h}");
            }
        }
    }
}

#[test]
fn a_short_image_is_one_strip_of_its_own_height() {
    // The concrete regression: 37x23 used to be written with RowsPerStrip 9010 (RGB8).
    for layout in [ChannelLayout::Gray, ChannelLayout::Rgb, ChannelLayout::Rgba] {
        for compression in [TiffCompression::None, TiffCompression::Lzw, TiffCompression::Deflate] {
            let img = test_image(layout, SampleType::U8);
            let bytes = encode(&img, Format::Tiff, &EncodeOptions { tiff_compression: compression, ..Default::default() }).unwrap();
            assert_eq!(tiff_tag_u64(&bytes, TAG_ROWS_PER_STRIP), Some(23), "{layout:?} {compression:?}");
        }
    }
}
