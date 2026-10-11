//! Fuzz the pattern parsers: the `Patt` / `Pat2` / `Pat3` tagged blocks and standalone `.pat`
//! files. Any input must return `Ok` or `Err`, never panic. Anything that parses must re-serialise
//! and re-parse to the same bytes, so the writer can never emit a pattern its own reader rejects.
//!
//! The structural equality of the *first* parse is deliberately not asserted: arbitrary input can
//! carry channel offsets and padding the canonical writer normalises. The byte-stability of the
//! second round trip is the invariant that actually holds and that catches reader/writer drift.
#![no_main]
use libfuzzer_sys::fuzz_target;
use photocraft_psd::patterns::{parse_pat_file, parse_pattern_block, write_pat_file, write_pattern_block};
fuzz_target!(|data: &[u8]| {
    if let Ok(ps) = parse_pattern_block(data) {
        let out = write_pattern_block(&ps).expect("a parsed pattern block must be writable");
        let back = parse_pattern_block(&out).expect("a written pattern block must re-parse");
        assert_eq!(write_pattern_block(&back).expect("rewritable"), out, "a pattern block is not byte-stable across a round trip");
    }
    if let Ok(ps) = parse_pat_file(data) {
        let out = write_pat_file(&ps).expect("a parsed .pat must be writable");
        let back = parse_pat_file(&out).expect("a written .pat must re-parse");
        assert_eq!(write_pat_file(&back).expect("rewritable"), out, "a .pat file is not byte-stable across a round trip");
    }
});
