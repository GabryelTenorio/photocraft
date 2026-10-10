/// Reusable Zstandard encoder at the library's default level, with frame checksums.
pub struct ZstdEncoder<'a>(zstd::zstd_safe::CCtx<'a>);

fn zstd_error(code: usize) -> std::io::Error {
    std::io::Error::other(zstd::zstd_safe::get_error_name(code))
}

/// Decode Zstandard frames into at most `max_bytes` of output.
pub fn decode_zstd(bytes: &[u8], max_bytes: usize) -> std::io::Result<Vec<u8>> {
    let mut decoded = Vec::new();
    decoded.try_reserve_exact(max_bytes).map_err(std::io::Error::other)?;
    let mut decoder = zstd::zstd_safe::DCtx::try_create().ok_or_else(|| std::io::Error::other("cannot allocate Zstandard decoder"))?;
    decoder.decompress(&mut decoded, bytes).map_err(zstd_error)?;
    Ok(decoded)
}

impl ZstdEncoder<'_> {
    pub fn new() -> std::io::Result<Self> {
        let mut encoder = zstd::zstd_safe::CCtx::try_create().ok_or_else(|| std::io::Error::other("cannot allocate Zstandard encoder"))?;
        encoder.set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true)).map_err(zstd_error)?;
        Ok(Self(encoder))
    }

    pub fn compress(&mut self, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut encoded = Vec::new();
        encoded.try_reserve_exact(zstd::zstd_safe::compress_bound(bytes.len())).map_err(std::io::Error::other)?;
        self.0.compress2(&mut encoded, bytes).map_err(zstd_error)?;
        // Avoid retaining the input-sized allocation for saved objects.
        encoded.shrink_to_fit();
        Ok(encoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_independent_and_checksummed() {
        let mut encoder = ZstdEncoder::new().unwrap();
        let mut reference = zstd::bulk::Compressor::new(0).unwrap();
        reference.include_checksum(true).unwrap();
        for bytes in [vec![42; 262_144], vec![], (0..262_147).map(|i| ((i * 37 + i / 97) % 251) as u8).collect(), vec![19; 1_048_576]] {
            let mut encoded = encoder.compress(&bytes).unwrap();
            assert_eq!(encoded, reference.compress(&bytes).unwrap());
            assert_eq!(zstd::stream::decode_all(encoded.as_slice()).unwrap(), bytes);
            assert_eq!(decode_zstd(&encoded, bytes.len()).unwrap(), bytes);
            assert!(decode_zstd(&encoded[..encoded.len() - 1], bytes.len()).is_err());
            assert!(decode_zstd(&encoded, usize::MAX).is_err());
            if !bytes.is_empty() {
                assert!(decode_zstd(&encoded, bytes.len() - 1).is_err());
            }
            assert_ne!(encoded[4] & 4, 0);
            *encoded.last_mut().unwrap() ^= 1;
            assert!(zstd::stream::decode_all(encoded.as_slice()).is_err());
            assert!(decode_zstd(&encoded, bytes.len()).is_err());
        }
    }
}
