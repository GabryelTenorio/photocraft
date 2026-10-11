#![no_main]

use libfuzzer_sys::fuzz_target;
use photocraft_codecs::{decode_as_with, DecodeOptions, Format, Limits};

// jxl-oxide is a young decoder and parses untrusted files: run with `-timeout=10` so a hang is
// reported, not just a panic. The codec turns its panics into errors, so it is also called
// directly, where those panics still reach the fuzzer. Both shapes of file are covered (the
// magics are prepended below), and so is the keep_orientation path.
fuzz_target!(|data: &[u8]| {
    let keep_orientation = data.first().is_some_and(|b| b & 1 == 1);
    let body = data.get(1..).unwrap_or_default();
    let limits = Limits { max_width: 4096, max_height: 4096, max_pixels: 1 << 22, max_alloc: 256 << 20 };
    let opts = DecodeOptions { limits, keep_orientation };

    let budget = usize::try_from(limits.max_alloc).unwrap_or(usize::MAX);
    for bytes in [
        body.to_vec(),
        [&[0xFF, 0x0A][..], body].concat(),
        [b"\0\0\0\x0CJXL \x0D\x0A\x87\x0A"[..], body].concat(),
    ] {
        let _ = jxl_oxide::JxlImage::builder()
            .alloc_tracker(jxl_oxide::AllocTracker::with_limit(budget))
            .read(bytes.as_slice());
        let _ = decode_as_with(Format::Jxl, &bytes, &opts);
    }
});