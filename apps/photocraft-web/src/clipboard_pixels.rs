//! Bounded, alpha-preserving clipboard image decoding. Shared by event and menu paste.
pub fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    const MAX_BYTES: usize = 64 * 1024 * 1024;
    if bytes.len() > MAX_BYTES {
        return Err("Clipboard image exceeds 64 MiB".into());
    }
    let options = photocraft_codecs::DecodeOptions {
        limits: photocraft_codecs::Limits { max_width: 32768, max_height: 32768, max_pixels: (MAX_BYTES / 4) as u64, max_alloc: MAX_BYTES as u64 },
        ..Default::default()
    };
    let image = photocraft_codecs::decode_with(bytes, &options).map_err(|e| e.to_string())?;
    Ok((image.width(), image.height(), image.to_rgba8()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_round_trip_preserves_transparent_and_partial_alpha_pixels() {
        let pixels = vec![255, 0, 20, 0, 10, 30, 50, 128, 0, 0, 0, 255];
        let image = photocraft_codecs::Image::from_u8(3, 1, photocraft_codecs::ChannelLayout::Rgba, pixels.clone()).unwrap();
        let png = photocraft_codecs::encode(&image, photocraft_codecs::Format::Png, &Default::default()).unwrap();
        assert_eq!(decode(&png).unwrap(), (3, 1, pixels));
    }
    #[test]
    fn corrupt_image_is_an_error() {
        assert!(decode(b"this is not an image").is_err());
    }
}
