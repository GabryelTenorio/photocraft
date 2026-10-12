//! Fuzz the `.atn` Photoshop actions parser: any input must return `Ok` or `Err`, never panic.
//! A file that parses must stay inside [`photocraft_psd::atn::MAX_COUNT`] for both actions and
//! the commands of each action.
#![no_main]
use libfuzzer_sys::fuzz_target;
use photocraft_psd::atn;

fuzz_target!(|data: &[u8]| {
    let Ok(file) = atn::parse(data) else { return };
    let cap = usize::try_from(atn::MAX_COUNT).unwrap_or(usize::MAX);
    assert!(file.actions.len() <= cap, "{} actions exceeds MAX_COUNT", file.actions.len());
    for action in &file.actions {
        assert!(action.commands.len() <= cap, "{} commands exceeds MAX_COUNT", action.commands.len());
    }
});
