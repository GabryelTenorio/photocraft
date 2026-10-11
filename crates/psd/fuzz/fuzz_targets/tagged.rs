//! Fuzz the per-block views of a tagged block: `parsed` (typed, well-known keys) and
//! `check_structure` (strict re-parse of blocks that carry their own inner sizes: placed layers,
//! linked files, object effects, filter caches). Both must return `Ok` / `Err` / `None`, never
//! panic, for an arbitrary key and arbitrary data. The first four bytes pick the key so every typed
//! and strict branch is reachable; the rest is the block data.
#![no_main]
use libfuzzer_sys::fuzz_target;
use photocraft_psd::tagged::TaggedBlock;
fuzz_target!(|data: &[u8]| {
    let (key, body) = match data.split_first_chunk::<4>() {
        Some((k, rest)) => (*k, rest.to_vec()),
        None => (*b"luni", data.to_vec()),
    };
    let b = TaggedBlock::new(key, body);
    let _ = b.parsed();
    let _ = b.check_structure();
});
