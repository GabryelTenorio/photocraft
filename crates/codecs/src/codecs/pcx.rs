//! PCX (ZSoft Paintbrush) decode support, read-only.
//!
//! A 128-byte header, per-scanline RLE image data, and a 256-entry VGA palette in the trailer:
//! a `0x0C` marker followed by 768 bytes of RGB. Only the common 8-bit single-plane form is read;
//! the 1-, 2- and 4-bit and the multi-plane forms report a clear "unsupported" rather than a
//! picture that is quietly wrong.
//!
//! The format is public domain and its spec is published by ZSoft, so this is implemented from
//! that spec alone.
//!
//! Feature: `pcx` (optional; enabled in official builds). Read-only: PCX has no widely agreed
//! encoder and nothing in the app writes it.

use crate::Format;
use crate::error::CodecError;
use crate::image::Image;
#[cfg(feature = "pcx")]
use crate::image::{ChannelLayout, SampleType};
use crate::options::Limits;

const F: Format = Format::Pcx;
/// The fixed header every PCX file starts with.
#[cfg(feature = "pcx")]
const HEADER: usize = 128;
/// The manufacturer byte at offset 0.
#[cfg(feature = "pcx")]
const MANUFACTURER: u8 = 0x0A;
/// Offset of the color-planes byte in the header.
#[cfg(feature = "pcx")]
const PLANES_AT: usize = 65;
/// Offset of the bytes-per-scanline field in the header.
#[cfg(feature = "pcx")]
const BYTES_PER_LINE_AT: usize = 66;
/// The byte that introduces the 256-colour VGA palette in the trailer.
#[cfg(feature = "pcx")]
const PALETTE_MARKER: u8 = 0x0C;
/// 256 palette entries of R, G, B.
#[cfg(feature = "pcx")]
const PALETTE_BYTES: usize = 256 * 3;

/// Error message when the `pcx` feature is not enabled.
#[cfg(not(feature = "pcx"))]
pub(crate) const NOT_IN_BUILD: &str = "PCX support isn't included in this build of PhotoCraft";

#[cfg(not(feature = "pcx"))]
pub(crate) fn decode(_bytes: &[u8], _limits: &Limits, _keep_orientation: bool) -> Result<Image, CodecError> {
    Err(CodecError::unsupported(F, NOT_IN_BUILD))
}

/// Read a little-endian `u16` out of the header. Every offset is within the 128 bytes already
/// checked, but the reads stay bounds-checked rather than indexed.
#[cfg(feature = "pcx")]
fn le16(bytes: &[u8], at: usize) -> Result<u16, CodecError> {
    let pair: [u8; 2] = bytes.get(at..at + 2).and_then(|slice| slice.try_into().ok()).ok_or_else(|| CodecError::malformed(F, "PCX: header is truncated"))?;
    Ok(u16::from_le_bytes(pair))
}

/// The 256-entry VGA palette from the trailer, as RGB triples.
#[cfg(feature = "pcx")]
fn palette(bytes: &[u8]) -> Result<[(u8, u8, u8); 256], CodecError> {
    // marker + 768 bytes, so the palette needs at least that many bytes behind it.
    let start = bytes.len().checked_sub(PALETTE_BYTES + 1).ok_or_else(|| CodecError::malformed(F, "PCX: no room for a VGA palette"))?;
    if bytes.get(start) != Some(&PALETTE_MARKER) {
        return Err(CodecError::malformed(F, "PCX: missing the 0x0C byte before the VGA palette"));
    }
    let raw = &bytes[start + 1..];
    let mut out = [(0u8, 0u8, 0u8); 256];
    for (entry, slot) in out.iter_mut().enumerate() {
        let at = entry * 3;
        // The slice is 768 bytes long, so every read inside it lands.
        if let (Some(r), Some(g), Some(b)) = (raw.get(at), raw.get(at + 1), raw.get(at + 2)) {
            *slot = (*r, *g, *b);
        }
    }
    Ok(out)
}

/// Decode one scanline of RLE bytes into exactly `width` palette indices. A run is a byte whose
/// top two bits are set, giving a count in its low six bits followed by the value; anything else
/// is a literal pixel.
#[cfg(feature = "pcx")]
fn scanline(mut src: &[u8], out: &mut [u8]) -> Result<(), CodecError> {
    let mut written = 0usize;
    while written < out.len() {
        let control = *src.first().ok_or_else(|| CodecError::malformed(F, "PCX: scanline data is truncated"))?;
        src = &src[1..];
        if control & 0xC0 == 0xC0 {
            let count = (control & 0x3F) as usize;
            if count == 0 {
                return Err(CodecError::malformed(F, "PCX: zero-length RLE run"));
            }
            let value = *src.first().ok_or_else(|| CodecError::malformed(F, "PCX: scanline data is truncated"))?;
            src = &src[1..];
            let count = count.min(out.len() - written);
            for slot in &mut out[written..written + count] {
                *slot = value;
            }
            written += count;
        } else {
            out[written] = control;
            written += 1;
        }
    }
    Ok(())
}

#[cfg(feature = "pcx")]
pub(crate) fn decode(bytes: &[u8], limits: &Limits, _keep_orientation: bool) -> Result<Image, CodecError> {
    if bytes.len() < HEADER {
        return Err(CodecError::malformed(F, "PCX: shorter than its 128-byte header"));
    }
    if bytes.first() != Some(&MANUFACTURER) {
        return Err(CodecError::malformed(F, "PCX: not a PCX file (bad manufacturer byte)"));
    }
    let encoding = bytes.get(2).copied().unwrap_or(0);
    if encoding != 0 && encoding != 1 {
        return Err(CodecError::unsupported(F, format!("PCX encoding {encoding}")));
    }
    let bits = bytes.get(3).copied().unwrap_or(0);
    let planes = bytes.get(PLANES_AT).copied().unwrap_or(0);
    if bits != 8 || planes != 1 {
        return Err(CodecError::unsupported(F, format!("PCX with {bits} bits over {planes} planes; only 8-bit single-plane images are read")));
    }

    // The image box is inclusive, so its size is one more than the span. A box that runs
    // backwards is refused outright rather than saturating to a one-pixel sliver.
    let (xmin, ymin) = (le16(bytes, 4)?, le16(bytes, 6)?);
    let (xmax, ymax) = (le16(bytes, 8)?, le16(bytes, 10)?);
    if xmax < xmin || ymax < ymin {
        return Err(CodecError::malformed(F, "PCX: the image box runs backwards"));
    }
    let width = u32::from(xmax - xmin) + 1;
    let height = u32::from(ymax - ymin) + 1;
    let bytes_per_line = le16(bytes, BYTES_PER_LINE_AT)? as usize;
    if bytes_per_line < width as usize {
        return Err(CodecError::malformed(F, "PCX: bytes per scanline is shorter than the image is wide"));
    }

    // Refuse the dimensions before a single byte of image data is allocated.
    limits.check(width, height, ChannelLayout::Rgb, SampleType::U8)?;

    let palette = palette(bytes)?;
    let data_start = HEADER;
    let data_end = bytes.len().checked_sub(PALETTE_BYTES + 1).ok_or_else(|| CodecError::malformed(F, "PCX: no room for image data"))?;
    let body = bytes.get(data_start..data_end).ok_or_else(|| CodecError::malformed(F, "PCX: image data is out of bounds"))?;

    let mut rgb = vec![0u8; width as usize * height as usize * 3];
    let mut line = vec![0u8; bytes_per_line];
    for row in 0..height as usize {
        let from = row * bytes_per_line;
        let chunk = body.get(from..from + bytes_per_line).ok_or_else(|| CodecError::malformed(F, "PCX: image data ends before the last scanline"))?;
        if encoding == 1 {
            scanline(chunk, &mut line)?;
        } else {
            line.copy_from_slice(chunk);
        }
        for (column, &index) in line.iter().take(width as usize).enumerate() {
            let (r, g, b) = palette[index as usize];
            let at = (row * width as usize + column) * 3;
            rgb[at] = r;
            rgb[at + 1] = g;
            rgb[at + 2] = b;
        }
    }
    Image::from_u8(width, height, ChannelLayout::Rgb, rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build without the feature reports a clear "unsupported" instead of a confusing error.
    #[cfg(not(feature = "pcx"))]
    #[test]
    fn without_the_feature_pcx_is_a_clear_unsupported() {
        let err = decode(&[0xFF; 200], &Limits::default(), false).expect_err("no pcx in this build");
        assert!(err.to_string().contains("isn't included"), "{err}");
    }

    /// A file shorter than the fixed header is refused rather than read past its end.
    #[cfg(feature = "pcx")]
    #[test]
    fn a_short_file_is_malformed() {
        let err = decode(&[0u8; 64], &Limits::default(), false).expect_err("no header");
        assert!(err.to_string().contains("128-byte header"), "{err}");
    }

    /// Anything that is not a PCX file is refused on the manufacturer byte.
    #[cfg(feature = "pcx")]
    #[test]
    fn a_wrong_manufacturer_byte_is_malformed() {
        let mut bytes = vec![0u8; HEADER + 8];
        bytes[0] = 0x0B;
        let err = decode(&bytes, &Limits::default(), false).expect_err("not pcx");
        assert!(err.to_string().contains("manufacturer"), "{err}");
    }

    /// A header claiming absurd dimensions is refused by `Limits` before any image data is
    /// allocated, which is what keeps a crafted 65535x65535 header from exhausting memory.
    #[cfg(feature = "pcx")]
    #[test]
    fn limits_refuse_an_absurd_image_before_allocating() {
        let mut bytes = vec![0u8; HEADER + 8];
        bytes[0] = MANUFACTURER;
        bytes[2] = 1;
        bytes[3] = 8;
        bytes[PLANES_AT] = 1;
        // Xmin..Xmax and Ymin..Ymax = 0..65535, and a plausible scanline stride.
        bytes[9] = 0xFF;
        bytes[11] = 0xFF;
        bytes[BYTES_PER_LINE_AT] = 0xFF;
        bytes[BYTES_PER_LINE_AT + 1] = 0xFF;
        let limits = Limits { max_width: 64, max_height: 64, ..Limits::default() };
        let err = decode(&bytes, &limits, false).expect_err("far past the limit");
        assert!(err.to_string().contains("exceed"), "{err}");
    }

    /// A swapped Xmin/Xmax is refused outright rather than wrapping to a huge image or silently
    /// becoming a one-pixel sliver.
    #[cfg(feature = "pcx")]
    #[test]
    fn a_swapped_box_is_refused_rather_than_wrapping() {
        let mut bytes = vec![0u8; HEADER + 8];
        bytes[0] = MANUFACTURER;
        bytes[2] = 1;
        bytes[3] = 8;
        bytes[PLANES_AT] = 1;
        bytes[4] = 0x10; // Xmin = 16
        bytes[8] = 0x02; // Xmax = 2
        let err = decode(&bytes, &Limits::default(), false).expect_err("backwards box");
        assert!(err.to_string().contains("backwards"), "{err}");
    }

    /// The VGA palette needs its `0x0C` marker; without it the trailer is not a palette.
    #[cfg(feature = "pcx")]
    #[test]
    fn a_missing_palette_marker_is_malformed() {
        let mut bytes = vec![0u8; HEADER + PALETTE_BYTES + 4];
        bytes[0] = MANUFACTURER;
        bytes[2] = 1;
        bytes[3] = 8;
        bytes[PLANES_AT] = 1;
        bytes[BYTES_PER_LINE_AT] = 1; // a stride wide enough for the one-pixel box
        // The byte where the marker belongs is left at 0.
        let err = decode(&bytes, &Limits::default(), false).expect_err("no palette");
        assert!(err.to_string().contains("0x0C"), "{err}");
    }

    /// An RLE run counts its own byte, so `0xC1` repeats the next byte once — not twice.
    #[cfg(feature = "pcx")]
    #[test]
    fn an_rle_run_counts_once_per_low_six_bits() {
        let mut out = [0u8; 3];
        scanline(&[0xC2, 0x7F, 0x00], &mut out).expect("a short run");
        assert_eq!(out, [0x7F, 0x7F, 0x00]);
    }

    /// The whole path together: a 2x1 image whose two indices look up their own palette
    /// entries. This is the test that would have caught the header offsets, the RLE off-by-one
    /// and the 48-byte trailer read the original decoder had.
    #[cfg(feature = "pcx")]
    #[test]
    fn a_minimal_file_decodes_to_its_palette_pixels() {
        let mut bytes = vec![0u8; HEADER + 2 + 1 + PALETTE_BYTES];
        bytes[0] = MANUFACTURER;
        bytes[1] = 5; // version
        bytes[2] = 1; // RLE
        bytes[3] = 8; // bits per pixel
        // Xmax = 1, so the inclusive box is two pixels wide; one row.
        bytes[8] = 0x01;
        bytes[PLANES_AT] = 1;
        bytes[BYTES_PER_LINE_AT] = 2;
        // Two literal pixels, indices 1 and 2.
        bytes[HEADER] = 1;
        bytes[HEADER + 1] = 2;
        let palette_at = HEADER + 2;
        bytes[palette_at] = PALETTE_MARKER;
        // The first entry follows the marker; each entry is three bytes of R, G, B.
        let rgb = palette_at + 1;
        bytes[rgb + 3] = 10;
        bytes[rgb + 4] = 20;
        bytes[rgb + 5] = 30;
        bytes[rgb + 6] = 40;
        bytes[rgb + 7] = 50;
        bytes[rgb + 8] = 60;

        let image = decode(&bytes, &Limits::default(), false).expect("a valid PCX");
        assert_eq!((image.width(), image.height()), (2, 1));
        assert_eq!(image.data(), &[10, 20, 30, 40, 50, 60]);
    }
}
