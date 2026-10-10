//! Content-addressed artifacts (research §8): every piece of evidence Forge records carries its
//! media type, byte size and a content hash, so another model (or a human) can identify exactly
//! which bytes a result refers to.

use std::path::Path;

use serde::Serialize;

/// The content-hash algorithm used for artifacts. Recorded in every digest so the value is
/// self-describing.
pub const HASH_ALGORITHM: &str = "blake3";

/// A digest rendered as `blake3:<hex>`.
pub fn hash_bytes(bytes: &[u8]) -> String {
    format!("{HASH_ALGORITHM}:{}", blake3::hash(bytes).to_hex())
}

/// An artifact: a name or path, a media type, its size and its content hash.
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    /// Path relative to the workspace when known, else a synthetic name.
    pub name: String,
    pub media_type: String,
    pub bytes: usize,
    /// Self-describing digest, e.g. `blake3:ab12…`.
    pub hash: String,
}

impl Artifact {
    /// An artifact from bytes already in memory.
    pub fn from_bytes(name: impl Into<String>, media_type: impl Into<String>, bytes: &[u8]) -> Self {
        Self { name: name.into(), media_type: media_type.into(), bytes: bytes.len(), hash: hash_bytes(bytes) }
    }

    /// An artifact from a file; `None` when it cannot be read.
    pub fn from_file(name: impl Into<String>, media_type: impl Into<String>, path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        Some(Self::from_bytes(name, media_type, &bytes))
    }
}

/// A media type from a file extension (best effort; `application/octet-stream` when unknown).
pub fn media_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("tif" | "tiff") => "image/tiff",
        Some("psd") => "image/vnd.adobe.photoshop",
        Some("pcraft") => "application/x-photocraft",
        Some("json") => "application/json",
        Some("txt" | "log") => "text/plain",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable_and_self_describing() {
        assert_eq!(hash_bytes(b""), "blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
        assert_eq!(hash_bytes(b"forge"), hash_bytes(b"forge"));
        assert_ne!(hash_bytes(b"forge"), hash_bytes(b"forge "));
    }

    #[test]
    fn media_types_follow_extensions() {
        assert_eq!(media_type(Path::new("a/b/ui.png")), "image/png");
        assert_eq!(media_type(Path::new("out.pcraft")), "application/x-photocraft");
        assert_eq!(media_type(Path::new("7.log")), "text/plain");
        assert_eq!(media_type(Path::new("mystery")), "application/octet-stream");
    }

    #[test]
    fn from_bytes_records_size_and_hash() {
        let a = Artifact::from_bytes("x", "text/plain", b"hello");
        assert_eq!(a.bytes, 5);
        assert_eq!(a.hash, hash_bytes(b"hello"));
        assert_eq!(a.media_type, "text/plain");
    }
}
