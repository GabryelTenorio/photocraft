//! DPX (SMPTE 268M) reader: the digital-intermediate frame format written by film scanners
//! and grading systems. Read-only in this crate (a DPX export path is not provided).
//!
//! Supported: a single image element with descriptor 50 (RGB), 51 (RGBA), 52 (ABGR),
//! 1 (single channel) or 6 (luminance), 8/10/12/16-bit integer samples, packing 0 (packed
//! bit fields) and 1 (filled to 16-bit), both byte orders, and the per-element reference
//! low/high levels. Everything else is a clear `Unsupported`/`Malformed` error.

use crate::error::CodecError;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::Limits;

const F: Format = Format::Dpx;
/// The file information header is 768 bytes; the image information header follows it.
const IMAGE_HEADER: usize = 768;

fn bad(msg: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, msg)
}

/// Byte order from the 4-byte magic (`SDPX` big-endian, `XPDS` little-endian).
pub(crate) fn is_dpx(b: &[u8]) -> bool {
    matches!(b.get(0..4), Some([0x53, 0x44, 0x50, 0x58]) | Some([0x58, 0x50, 0x44, 0x53]))
}

/// Big-endian file (`SDPX`)? `None` when the magic is not DPX.
fn big_endian(b: &[u8]) -> Option<bool> {
    match b.get(0..4)? {
        [0x53, 0x44, 0x50, 0x58] => Some(true),
        [0x58, 0x50, 0x44, 0x53] => Some(false),
        _ => None,
    }
}

struct Fields {
    big: bool,
}

impl Fields {
    fn u16(&self, b: &[u8], o: usize) -> Option<u16> {
        let c = b.get(o..o + 2)?;
        Some(if self.big { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) })
    }
    fn u32(&self, b: &[u8], o: usize) -> Option<u32> {
        let c = b.get(o..o + 4)?;
        Some(if self.big { u32::from_be_bytes([c[0], c[1], c[2], c[3]]) } else { u32::from_le_bytes([c[0], c[1], c[2], c[3]]) })
    }
    fn f32(&self, b: &[u8], o: usize) -> Option<f32> {
        Some(f32::from_bits(self.u32(b, o)?))
    }
}

/// Location of the image data and its format, read from the two headers.
struct Element {
    layout: ChannelLayout,
    bits: u32,
    packing: u16,
    data: usize,
    eol_padding: usize,
    /// How many components per pixel are stored (3/4 for colour, 1 for single/luma).
    comps: usize,
    /// True for descriptor 52 (`ABGR`): stored A,B,G,R and re-ordered on read.
    abgr: bool,
    ref_low: f64,
    ref_high: f64,
}

fn parse(b: &[u8]) -> Result<(Fields, u32, u32, Element), CodecError> {
    let big = big_endian(b).ok_or_else(|| bad("not a DPX file"))?;
    let f = Fields { big };
    let image_offset = f.u32(b, 4).ok_or_else(|| bad("truncated file header"))? as usize;
    let elements = f.u16(b, 770).ok_or_else(|| bad("truncated image header"))?;
    let width = f.u32(b, 772).ok_or_else(|| bad("truncated image header"))?;
    let height = f.u32(b, 776).ok_or_else(|| bad("truncated image header"))?;
    if elements != 1 {
        return Err(CodecError::unsupported(F, format!("{elements} image elements (only one is supported)")));
    }
    // The first image element record begins right after pixels/lines (offset 780).
    let e = 780;
    let descriptor = *b.get(e + 20).ok_or_else(|| bad("truncated image element"))?;
    let bits = u32::from(*b.get(e + 23).ok_or_else(|| bad("truncated image element"))?);
    let packing = f.u16(b, e + 24).ok_or_else(|| bad("truncated image element"))?;
    let el_off = f.u32(b, e + 28).ok_or_else(|| bad("truncated image element"))? as usize;
    let eol_padding = f.u32(b, e + 32).ok_or_else(|| bad("truncated image element"))? as usize;
    let ref_low = f.f32(b, e + 8).map_or(0.0, f64::from);
    let ref_high = f.f32(b, e + 16).map_or(0.0, f64::from);
    let (layout, comps, abgr) = match descriptor {
        50 => (ChannelLayout::Rgb, 3, false),
        51 => (ChannelLayout::Rgba, 4, false),
        52 => (ChannelLayout::Rgba, 4, true),
        1 | 6 => (ChannelLayout::Gray, 1, false),
        d => return Err(CodecError::unsupported(F, format!("image element descriptor {d} (only 1/6/50/51/52 are supported)"))),
    };
    if !matches!(bits, 8 | 10 | 12 | 16) {
        return Err(CodecError::unsupported(F, format!("{bits}-bit samples")));
    }
    if packing > 1 {
        return Err(CodecError::unsupported(F, format!("packing {packing} (only packed and filled-to-16 are supported)")));
    }
    // A written element offset is absolute; a zero means the file image offset.
    let data = if el_off >= IMAGE_HEADER { el_off } else { image_offset };
    Ok((f, width, height, Element { layout, bits, packing, data, eol_padding, comps, abgr, ref_low, ref_high }))
}

/// The stored sample values of one row, `width * comps` of them.
fn row_samples(row: &[u8], width: usize, el: &Element, big: bool, out: &mut Vec<u32>) -> Result<(), CodecError> {
    let bits = el.bits as usize;
    out.clear();
    out.try_reserve(width.saturating_mul(el.comps)).map_err(|_| CodecError::LimitExceeded("cannot allocate row samples".into()))?;
    if el.packing == 1 {
        // Filled to 16-bit boundaries: one left-justified sample per 16-bit word.
        let shift = 16 - bits;
        for c in row.as_chunks::<2>().0.iter().take(width * el.comps) {
            let w = if big { u16::from_be_bytes(*c) } else { u16::from_le_bytes(*c) };
            out.push(u32::from(w) >> shift);
        }
        if out.len() < width * el.comps {
            return Err(bad("truncated scanline"));
        }
    } else if bits == 8 {
        for i in 0..width * el.comps {
            out.push(u32::from(*row.get(i).ok_or_else(|| bad("truncated scanline"))?));
        }
    } else if bits == 16 {
        for c in row.as_chunks::<2>().0.iter().take(width * el.comps) {
            out.push(u32::from(if big { u16::from_be_bytes(*c) } else { u16::from_le_bytes(*c) }));
        }
        if out.len() < width * el.comps {
            return Err(bad("truncated scanline"));
        }
    } else {
        // Packed 10- or 12-bit: contiguous MSB-first bit fields.
        let mut bit = 0usize;
        for _ in 0..width * el.comps {
            let mut v = 0u32;
            for _ in 0..bits {
                let byte = *row.get(bit / 8).ok_or_else(|| bad("truncated scanline"))?;
                v = (v << 1) | u32::from((byte >> (7 - bit % 8)) & 1);
                bit += 1;
            }
            out.push(v);
        }
    }
    Ok(())
}

/// Map a stored sample to a normalized value through the element's reference levels.
fn normalize(raw: u32, el: &Element) -> f64 {
    let max = ((1u64 << el.bits) - 1) as f64;
    if el.ref_high > el.ref_low {
        ((f64::from(raw) - el.ref_low) / (el.ref_high - el.ref_low)).clamp(0.0, 1.0)
    } else if max > 0.0 {
        (f64::from(raw) / max).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    let (fields, width, height, el) = parse(b)?;
    let sample = if el.bits == 8 { SampleType::U8 } else { SampleType::U16 };
    limits.check(width, height, el.layout, sample)?;
    let (w, h) = (width as usize, height as usize);
    // Filled samples occupy a full 16-bit word each; packed ones `bits` bits.
    let sample_bits = if el.packing == 1 { 16 } else { el.bits as usize };
    let row_bits = w.checked_mul(el.comps).and_then(|n| n.checked_mul(sample_bits)).ok_or_else(|| bad("row size overflow"))?;
    let row_bytes = row_bits.div_ceil(8);
    let stride = row_bytes.checked_add(el.eol_padding).ok_or_else(|| bad("row stride overflow"))?;
    let need = stride.checked_mul(h).and_then(|n| el.data.checked_add(n)).ok_or_else(|| bad("image size overflow"))?;
    if b.len() < need {
        return Err(bad("image data is truncated"));
    }
    let quant = |v: f64| -> u16 { (v * 65535.0).round() as u16 };
    let mut out_bytes = Vec::new();
    out_bytes
        .try_reserve_exact(w.saturating_mul(h).saturating_mul(el.comps).saturating_mul(sample.bytes()))
        .map_err(|_| CodecError::LimitExceeded("cannot allocate the image".into()))?;
    let mut row = Vec::new();
    for y in 0..h {
        let start = el.data + y * stride;
        let end = start + row_bytes;
        let row_slice = b.get(start..end).ok_or_else(|| bad("truncated scanline"))?;
        row_samples(row_slice, w, &el, fields.big, &mut row)?;
        for x in 0..w {
            let px = &row[x * el.comps..(x + 1) * el.comps];
            let g = |i: usize| px.get(i).copied().unwrap_or(0);
            // Stored order → canonical (already-reordered) values.
            let (r, gr, bl, al) = if el.abgr { (g(3), g(2), g(1), g(0)) } else { (g(0), g(1), g(2), g(3)) };
            let vals: [u32; 4] = match el.layout {
                ChannelLayout::Gray => [r, 0, 0, 0],
                ChannelLayout::Rgb => [r, gr, bl, 0],
                _ => [r, gr, bl, al],
            };
            let n = el.layout.channels();
            for v in vals.iter().take(n) {
                let nv = normalize(*v, &el);
                match sample {
                    SampleType::U8 => out_bytes.push((nv * 255.0).round() as u8),
                    _ => out_bytes.extend_from_slice(&quant(nv).to_ne_bytes()),
                }
            }
        }
    }
    Image::from_raw(width, height, el.layout, sample, out_bytes)
}

/// The EXIF orientation (1–8) for the DPX orientation code, so `decode` turns the frame
/// upright. 0 (left-to-right, top-to-bottom) is the normal case.
pub(crate) fn orientation(b: &[u8]) -> u16 {
    let Some(big) = big_endian(b) else { return 1 };
    let f = Fields { big };
    match f.u16(b, IMAGE_HEADER) {
        Some(1) => 2,
        Some(2) => 4,
        Some(3) => 3,
        Some(4) => 5,
        Some(5) => 8,
        Some(6) => 6,
        Some(7) => 7,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian DPX with one RGB element: 8-bit packed, else filled-to-16-bit samples.
    fn synth(width: u16, height: u16, bits: u8, pixels: &[[u16; 3]]) -> Vec<u8> {
        let mut b = vec![0u8; 780 + 72];
        b[0..4].copy_from_slice(b"XPDS");
        let off = b.len() as u32;
        b[4..8].copy_from_slice(&off.to_le_bytes()); // image data offset
        b[770..772].copy_from_slice(&1u16.to_le_bytes()); // one element
        b[772..776].copy_from_slice(&(width as u32).to_le_bytes());
        b[776..780].copy_from_slice(&(height as u32).to_le_bytes());
        let e = 780;
        b[e + 20] = 50; // descriptor RGB
        b[e + 23] = bits;
        b[e + 24..e + 26].copy_from_slice(&(u16::from(bits > 8)).to_le_bytes()); // packing: filled >8-bit
        b[e + 8..e + 12].copy_from_slice(&(0.0f32).to_le_bytes()); // ref low
        b[e + 16..e + 20].copy_from_slice(&(((1u32 << bits) - 1) as f32).to_le_bytes()); // ref high
        for px in pixels {
            for &v in px {
                if bits == 8 {
                    b.push(v as u8);
                } else {
                    b.extend_from_slice(&(v << (16 - bits)).to_le_bytes());
                }
            }
        }
        b
    }

    #[test]
    fn decodes_filled_10bit_rgb() {
        let px = [[0, 512, 1023], [1023, 0, 256]];
        let b = synth(2, 1, 10, &px);
        let img = decode(&b, &Limits::default()).unwrap();
        assert_eq!((img.width(), img.height(), img.layout(), img.sample_type()), (2, 1, ChannelLayout::Rgb, SampleType::U16));
        let v = img.to_u16_samples().unwrap();
        let q = |raw: u32| ((raw as f64 / 1023.0) * 65535.0).round() as u16;
        assert_eq!(v[0], q(0));
        assert_eq!(v[1], q(512));
        assert_eq!(v[2], q(1023));
        assert_eq!(v[3], q(1023));
        assert_eq!(v[4], q(0));
        assert_eq!(v[5], q(256));
    }

    #[test]
    fn decodes_8bit_to_u8() {
        let b = synth(1, 1, 8, &[[10, 20, 30]]);
        let img = decode(&b, &Limits::default()).unwrap();
        assert_eq!((img.layout(), img.sample_type()), (ChannelLayout::Rgb, SampleType::U8));
        assert_eq!(img.data(), &[10, 20, 30]);
    }

    #[test]
    fn truncated_is_an_error_not_a_panic() {
        let b = synth(4, 4, 10, &[[0; 3]; 16]);
        for cut in [0, 8, 100, 780, b.len() - 1] {
            assert!(decode(&b[..cut.min(b.len())], &Limits::default()).is_err(), "cut {cut}");
        }
    }

    #[test]
    fn rejects_unknown_descriptor() {
        let mut b = synth(1, 1, 8, &[[1, 2, 3]]);
        b[780 + 20] = 99;
        assert!(matches!(decode(&b, &Limits::default()), Err(CodecError::Unsupported { .. })));
    }
}
