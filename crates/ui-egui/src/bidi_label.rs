//! Visual order for single-line labels that contain right-to-left text (Arabic, Persian, Urdu,
//! Hebrew, Kurdish …), such as layer names.
//!
//! egui 0.36 shapes each font run with HarfRust, so an Arabic or Hebrew word is drawn right to
//! left with its letters joined. What it lacks is paragraph bidi: it places a line's runs left to
//! right in logical order, so `سلام عليكم` draws as `عليكم سلام`. [`for_egui`] puts the runs in
//! visual order with the Unicode bidi algorithm, but leaves the letters of every stretch egui
//! reverses by itself in logical order. Reversing those too (`reorder_line`) mirrors each word,
//! picks joining forms from the wrong neighbours and loses the لا ligature.
//!
//! Only for painted, single-line text: wrapping and eliding happen on the logical string first
//! (see [`crate::layer_row_ui::truncated`]), and editable text and accessibility labels keep the
//! logical string. Known gap: Arabic-Indic digits written against Arabic letters with no space
//! (`طبقة١`) share one egui run with them and land on the wrong side. Remove this once egui
//! orders runs by bidi level itself (emilk/egui#8577).

use std::borrow::Cow;

use unicode_bidi::BidiInfo;

/// Does `text` contain a character from a right-to-left script?
pub fn has_rtl(text: &str) -> bool {
    text.chars().any(rtl_script)
}

/// Characters egui shapes right to left: those of the right-to-left scripts (HarfRust guesses a
/// run's direction from its script), including their marks and digits. The joiners count too,
/// so a Persian word with a ZWNJ stays one stretch.
fn rtl_script(c: char) -> bool {
    matches!(
        c as u32,
        0x0590..=0x05FF // Hebrew
            | 0x0600..=0x06FF // Arabic (letters, marks, Arabic-Indic and Persian digits)
            | 0x0700..=0x074F // Syriac
            | 0x0750..=0x077F // Arabic Supplement
            | 0x0780..=0x07BF // Thaana
            | 0x07C0..=0x07FF // NKo
            | 0x0860..=0x08FF // Syriac Supplement, Arabic Extended-A/B
            | 0xFB1D..=0xFB4F // Hebrew presentation forms
            | 0xFB50..=0xFDFF // Arabic Presentation Forms-A
            | 0xFE70..=0xFEFF // Arabic Presentation Forms-B
            | 0x10800..=0x10FFF // Historic RTL scripts
            | 0x1E800..=0x1EFFF // Mende Kikakui, Adlam, Arabic mathematical symbols
    ) || matches!(c, '\u{200C}' | '\u{200D}')
}

/// `text` arranged so that egui draws it in visual order. Text without right-to-left characters
/// is returned unchanged.
pub fn for_egui(text: &str) -> Cow<'_, str> {
    if !has_rtl(text) {
        return Cow::Borrowed(text);
    }
    let info = BidiInfo::new(text, None);
    let mut out = String::with_capacity(text.len());
    for para in &info.paragraphs {
        let (levels, runs) = info.visual_runs(para, para.range.clone());
        for run in runs {
            let Some(s) = text.get(run.clone()) else { continue };
            let rtl = levels.get(run.start).is_some_and(|l| l.is_rtl());
            // A right-to-left run must read backwards on screen: reverse it, then turn back each
            // stretch egui reverses again. A left-to-right run (Latin, digits) reads as written,
            // so only egui's stretches inside it (Arabic-Indic digits) are pre-reversed.
            let shown: String = if rtl { s.chars().rev().collect() } else { s.to_owned() };
            push_unreversing_rtl_stretches(&mut out, &shown);
        }
    }
    Cow::Owned(out)
}

/// Append `s`, reversing each maximal stretch of right-to-left-script characters.
fn push_unreversing_rtl_stretches(out: &mut String, s: &str) {
    let mut stretch: Vec<char> = Vec::new();
    for c in s.chars() {
        if rtl_script(c) {
            stretch.push(c);
        } else {
            out.extend(stretch.drain(..).rev());
            out.push(c);
        }
    }
    out.extend(stretch.into_iter().rev());
}

#[cfg(test)]
#[path = "bidi_label_tests.rs"]
mod tests;
