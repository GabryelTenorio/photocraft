//! Malformed action files must return an error without trapping or unbounded recursion.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = photocraft_psd::atn::parse(data);
});
