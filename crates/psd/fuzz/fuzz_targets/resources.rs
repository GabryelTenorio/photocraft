//! Fuzz the typed image-resource views: `ImageResource::parsed` for an arbitrary resource id, plus
//! the standalone `ResolutionInfo` and `VersionInfo` parsers. All must return `Ok` / `Err` / `None`
//! (and `xmp_str` an `Option`), never panic. The first two bytes pick the id so every typed branch
//! is reachable; the rest is the resource data.
#![no_main]
use libfuzzer_sys::fuzz_target;
use photocraft_psd::resources::{ImageResource, ResolutionInfo, VersionInfo};
fuzz_target!(|data: &[u8]| {
    let (id, body) = match data.split_first_chunk::<2>() {
        Some((p, rest)) => (u16::from_be_bytes(*p), rest.to_vec()),
        None => (0u16, data.to_vec()),
    };
    let r = ImageResource::new(id, body.clone());
    if let Some(res) = r.parsed() {
        let _ = res;
    }
    let _ = r.xmp_str();
    let _ = ResolutionInfo::from_bytes(&body);
    let _ = VersionInfo::from_bytes(&body);
});
