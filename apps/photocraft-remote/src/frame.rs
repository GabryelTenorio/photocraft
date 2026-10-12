//! Decoding the window frames the desktop app sends (format: `apps/photocraft/src/remote_server.rs`).

/// One changed rectangle of the window, RGBA.
#[derive(Debug, PartialEq)]
pub struct Tile<'a> {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub rgba: &'a [u8],
}

#[derive(Debug, PartialEq)]
pub struct Frame {
    /// Every tile is present (the first frame, or after a resize).
    pub full: bool,
    pub width: u32,
    pub height: u32,
    /// The inflated tile stream.
    body: Vec<u8>,
}

/// Largest window accepted (pixels per side).
const MAX_SIDE: u32 = 16_384;

impl Frame {
    pub fn decode(msg: &[u8]) -> Result<Self, String> {
        let word = |i: usize| msg.get(i..i + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
        if msg.get(..4) != Some(b"PCRF".as_slice()) {
            return Err("not a frame".into());
        }
        let (Some(flags), Some(width), Some(height)) = (word(4), word(8), word(12)) else { return Err("short frame".into()) };
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(format!("bad frame size {width}×{height}"));
        }
        // Tiles never cover more than the frame, plus their 8-byte headers.
        let pixels = width as usize * height as usize;
        let limit = pixels * 4 + pixels.div_ceil(64 * 64 / 4) * 8 + 1024;
        let body = miniz_oxide::inflate::decompress_to_vec_with_limit(msg.get(16..).unwrap_or_default(), limit).map_err(|e| format!("inflate: {e:?}"))?;
        Ok(Self { full: flags & 1 != 0, width, height, body })
    }

    /// The tiles, clipped to the frame; a truncated or out-of-bounds tile ends the list.
    pub fn tiles(&self) -> impl Iterator<Item = Tile<'_>> {
        let mut at = 0usize;
        std::iter::from_fn(move || {
            let b = &self.body;
            let half = |i: usize| b.get(at + i * 2..at + i * 2 + 2).and_then(|s| s.try_into().ok()).map(|s| u32::from(u16::from_le_bytes(s)));
            let (x, y, w, h) = (half(0)?, half(1)?, half(2)?, half(3)?);
            if w == 0 || h == 0 || x + w > self.width || y + h > self.height {
                return None;
            }
            let len = w as usize * h as usize * 4;
            let rgba = b.get(at + 8..at + 8 + len)?;
            at += 8 + len;
            Some(Tile { x, y, w, h, rgba })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(full: bool, w: u32, h: u32, tiles: &[(u16, u16, u16, u16, u8)]) -> Vec<u8> {
        let mut body = Vec::new();
        for &(x, y, tw, th, v) in tiles {
            for n in [x, y, tw, th] {
                body.extend_from_slice(&n.to_le_bytes());
            }
            body.extend(std::iter::repeat_n(v, tw as usize * th as usize * 4));
        }
        let mut msg = b"PCRF".to_vec();
        for n in [u32::from(full), w, h] {
            msg.extend_from_slice(&n.to_le_bytes());
        }
        msg.extend(miniz_oxide::deflate::compress_to_vec(&body, 1));
        msg
    }

    #[test]
    fn decodes_tiles() {
        let f = Frame::decode(&message(true, 100, 70, &[(0, 0, 64, 64, 1), (64, 64, 36, 6, 2)])).unwrap();
        assert!(f.full);
        assert_eq!((f.width, f.height), (100, 70));
        let t: Vec<_> = f.tiles().map(|t| (t.x, t.y, t.w, t.h, t.rgba.len(), t.rgba[0])).collect();
        assert_eq!(t, vec![(0, 0, 64, 64, 64 * 64 * 4, 1), (64, 64, 36, 6, 36 * 6 * 4, 2)]);
    }

    #[test]
    fn rejects_bad_messages() {
        assert!(Frame::decode(b"nope").is_err());
        assert!(Frame::decode(b"PCRF\0\0").is_err());
        assert!(Frame::decode(&message(false, 0, 10, &[])).is_err());
        assert!(Frame::decode(&message(false, 100_000, 10, &[])).is_err());
        // A tile outside the frame ends the list instead of drawing out of bounds.
        let f = Frame::decode(&message(false, 10, 10, &[(5, 5, 6, 1, 0)])).unwrap();
        assert_eq!(f.tiles().count(), 0);
        // A truncated stream ends the list.
        let mut m = message(false, 10, 10, &[(0, 0, 2, 2, 0)]);
        m.truncate(m.len() - 3);
        if let Ok(f) = Frame::decode(&m) {
            assert_eq!(f.tiles().count(), 0);
        }
        // A deflate bomb is refused.
        let mut bomb = b"PCRF".to_vec();
        for n in [0u32, 1, 1] {
            bomb.extend_from_slice(&n.to_le_bytes());
        }
        bomb.extend(miniz_oxide::deflate::compress_to_vec(&vec![0u8; 1 << 20], 1));
        assert!(Frame::decode(&bomb).is_err());
    }
}
