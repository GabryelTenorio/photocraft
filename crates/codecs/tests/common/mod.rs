//! Shared helpers for integration tests: deterministic PRNG, synthetic
//! image generators, comparison metrics.
#![allow(dead_code)]

pub mod tiffgen;

use photocraft_codecs::*;

/// SplitMix64 — tiny deterministic PRNG.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next_u64() as u8).collect()
    }
}

/// Gradient + noise image with values in [0, 1]. `noise` is the amplitude
/// of uniform noise added to the gradient. Alpha is a separate gradient
/// that includes fully transparent and fully opaque regions.
pub fn synth(w: u32, h: u32, layout: ChannelLayout, sample: SampleType, seed: u64, noise: f32) -> Image {
    let mut rng = Rng::new(seed);
    let nc = layout.channels();
    let mut v = Vec::with_capacity((w * h) as usize * nc);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 / (w.max(2) - 1) as f32;
            let fy = y as f32 / (h.max(2) - 1) as f32;
            for c in 0..nc {
                let is_alpha = layout.has_alpha() && c == nc - 1;
                let base = if is_alpha {
                    ((fx + fy) * 0.75 - 0.25).clamp(0.0, 1.0)
                } else {
                    match c % 4 {
                        0 => fx,
                        1 => fy,
                        2 => 1.0 - fx * 0.5 - fy * 0.5,
                        _ => (fx * fy).sqrt(),
                    }
                };
                let n = (rng.f32() - 0.5) * noise;
                v.push((base + n).clamp(0.0, 1.0));
            }
        }
    }
    Image::from_normalized(w, h, layout, sample, &v).unwrap()
}

/// Default test image: 37x23 with moderate noise.
pub fn test_image(layout: ChannelLayout, sample: SampleType) -> Image {
    synth(37, 23, layout, sample, 0xC0DEC, 0.2)
}

/// PSNR in dB over normalized samples (clamped to [0,1]).
pub fn psnr(a: &Image, b: &Image) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions());
    assert_eq!(a.layout(), b.layout());
    let (av, bv) = (a.to_normalized(), b.to_normalized());
    let mse: f64 = av
        .iter()
        .zip(&bv)
        .map(|(x, y)| {
            let d = (x.clamp(0.0, 1.0) - y.clamp(0.0, 1.0)) as f64;
            d * d
        })
        .sum::<f64>()
        / av.len() as f64;
    if mse == 0.0 { f64::INFINITY } else { 10.0 * (1.0 / mse).log10() }
}

pub fn max_abs_diff(a: &Image, b: &Image) -> f32 {
    a.to_normalized().iter().zip(b.to_normalized()).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

/// Formats that can be written in this build.
pub fn writable_formats() -> Vec<Format> {
    Format::ALL.into_iter().filter(|f| caps(*f).write).collect()
}

/// Formats that can be written *and* read in this build.
pub fn rw_formats() -> Vec<Format> {
    Format::ALL.into_iter().filter(|f| caps(*f).write && caps(*f).read).collect()
}

/// A minimal valid little-endian TIFF-structured EXIF blob (one IFD with
/// an Orientation entry).
pub fn sample_exif() -> Vec<u8> {
    let mut v = b"II*\0\x08\0\0\0".to_vec();
    v.extend_from_slice(&1u16.to_le_bytes()); // 1 entry
    v.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
    v.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes()); // next IFD
    v
}

/// Fake ICC profile of `len` bytes (header-ish prefix + deterministic noise).
pub fn sample_icc(len: usize) -> Vec<u8> {
    let mut v = Rng::new(len as u64).bytes(len);
    v[..4].copy_from_slice(&(len as u32).to_be_bytes());
    if len >= 40 {
        v[36..40].copy_from_slice(b"acsp");
    }
    v
}

pub const SAMPLE_XMP: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/></x:xmpmeta><?xpacket end=\"w\"?>";

/// The first value of an unsigned integer tag in the first image directory of a (Big)TIFF, in
/// either byte order. `None` when the tag or the bytes behind it are not there.
pub fn tiff_tag_u64(bytes: &[u8], tag: u16) -> Option<u64> {
    let le = match bytes.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let s: [u8; 2] = bytes.get(o..o.checked_add(2)?)?.try_into().ok()?;
        Some(if le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let s: [u8; 4] = bytes.get(o..o.checked_add(4)?)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    };
    let u64_at = |o: usize| -> Option<u64> {
        let s: [u8; 8] = bytes.get(o..o.checked_add(8)?)?.try_into().ok()?;
        Some(if le { u64::from_le_bytes(s) } else { u64::from_be_bytes(s) })
    };
    let (big, ifd) = match u16_at(2)? {
        42 => (false, usize::try_from(u32_at(4)?).ok()?),
        43 => (true, usize::try_from(u64_at(8)?).ok()?),
        _ => return None,
    };
    let (entry_len, first, inline, count) = if big { (20usize, ifd + 8, 8usize, u64_at(ifd)?) } else { (12, ifd + 2, 4, u64::from(u32_at(ifd)?)) };
    for i in 0..count.min(4096) as usize {
        let e = first.checked_add(i.checked_mul(entry_len)?)?;
        if u16_at(e)? != tag {
            continue;
        }
        let ty = u16_at(e.checked_add(2)?)?;
        let n = if big { u64_at(e.checked_add(4)?)? } else { u64::from(u32_at(e.checked_add(4)?)?) };
        if n == 0 {
            return None;
        }
        let field = e.checked_add(if big { 12 } else { 8 })?;
        let size = match ty {
            1 | 2 | 6 | 7 => 1usize,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 | 16 | 17 | 18 => 8,
            _ => return None,
        };
        let at = if n as usize * size <= inline { field } else { usize::try_from(if big { u64_at(field)? } else { u64::from(u32_at(field)?) }).ok()? };
        return match ty {
            1 | 6 => bytes.get(at).map(|&b| u64::from(b)),
            3 | 8 => u16_at(at).map(u64::from),
            4 | 9 | 13 => u32_at(at).map(u64::from),
            16..=18 => u64_at(at),
            _ => None,
        };
    }
    None
}
