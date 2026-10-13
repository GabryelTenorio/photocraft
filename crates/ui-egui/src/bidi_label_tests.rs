//! The bidi pre-pass: string rules everywhere, and what egui actually draws when an Arabic or
//! Hebrew system font is installed.

use super::*;
use egui::epaint::text::{FontData, FontDefinitions, FontFamily, Fonts, LayoutJob, TextFormat, TextOptions};
use std::sync::Arc;

#[test]
fn text_without_right_to_left_characters_is_untouched() {
    for s in ["Layer 1", "", "Background copy 2", "图层 1", "Ебать"] {
        assert!(matches!(for_egui(s), Cow::Borrowed(t) if t == s), "{s}");
    }
}

#[test]
fn right_to_left_words_swap_places_but_keep_their_letters() {
    // egui reverses each word's letters itself; only the word order changes here.
    assert_eq!(for_egui("سلام عليكم"), "عليكم سلام");
    assert_eq!(for_egui("שלום עולם"), "עולם שלום");
    assert_eq!(for_egui("ئەمە تێستە بۆ ئیش"), "ئیش بۆ تێستە ئەمە");
    // One word: nothing to reorder.
    assert_eq!(for_egui("طبقة"), "طبقة");
}

#[test]
fn numbers_and_latin_keep_reading_left_to_right() {
    // European digits and Latin are runs of their own, in the right visual place.
    assert_eq!(for_egui("عدد 123 مرحبا"), "مرحبا 123 عدد");
    assert_eq!(for_egui("طبقة 1 Layer"), "Layer 1 طبقة");
    // Arabic-Indic digits are Arabic-script, which egui reverses: pre-reversed so they read ١٢٣.
    assert_eq!(for_egui("شلام ١٢٣ x"), "x ٣٢١ شلام");
    // A left-to-right label keeps its order; a number right after an Arabic word belongs to it
    // (UAX #9 W2), as in browsers.
    assert_eq!(for_egui("Layer طبقة"), "Layer طبقة");
    assert_eq!(for_egui("Layer طبقة 2"), "Layer 2 طبقة");
}

#[test]
fn persian_joiners_stay_inside_their_word() {
    // "می‌خواهم" has a ZWNJ: still one word, letters untouched.
    assert_eq!(for_egui("می\u{200C}خواهم سلام"), "سلام می\u{200C}خواهم");
}

/// An installed font covering `sample`, if any of the usual ones is present.
fn system_font(candidates: &[&str]) -> Option<Vec<u8>> {
    candidates.iter().find_map(|p| std::fs::read(p).ok())
}

const ARABIC_FONTS: [&str; 6] = [
    "/System/Library/Fonts/GeezaPro.ttc",
    "/System/Library/Fonts/SFArabic.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
    "/usr/share/fonts/noto/NotoSansArabic-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
];

fn fonts_with(bytes: Vec<u8>) -> Fonts {
    let mut defs = FontDefinitions::default();
    defs.font_data.insert("rtl-test".into(), Arc::new(FontData::from_owned(bytes)));
    if let Some(family) = defs.families.get_mut(&FontFamily::Proportional) {
        family.push("rtl-test".into());
    }
    Fonts::new(TextOptions::default(), defs)
}

/// x of each drawn (non-continuation) glyph, with the character it was attributed to.
fn drawn(fonts: &mut Fonts, text: &str) -> Vec<(char, f32)> {
    let mut job = LayoutJob::default();
    job.append(text, 0.0, TextFormat { font_id: egui::FontId::proportional(20.0), ..Default::default() });
    let galley = fonts.with_pixels_per_point(1.0).layout_job(job);
    galley.rows.iter().flat_map(|r| r.glyphs.iter()).filter(|g| !g.uv_rect.is_nothing()).map(|g| (g.chr, g.pos.x)).collect()
}

fn x_of(glyphs: &[(char, f32)], c: char) -> f32 {
    glyphs.iter().find(|(g, _)| *g == c).map(|(_, x)| *x).unwrap_or_else(|| panic!("{c} drawn: {glyphs:?}"))
}

#[test]
fn arabic_draws_first_word_rightmost_with_letters_joined() {
    let Some(bytes) = system_font(&ARABIC_FONTS) else {
        eprintln!("skipped: no Arabic system font");
        return;
    };
    let mut fonts = fonts_with(bytes);
    let shown = drawn(&mut fonts, &for_egui("سلام عليكم"));
    // The first word (سلام) is on the right: its first letter right of the second word's.
    assert!(x_of(&shown, 'س') > x_of(&shown, 'ع'), "{shown:?}");
    // سلام draws as three glyphs (س, the لا ligature, م): the letters kept their logical order.
    // Reversed letters (مالس) can't form the ligature and draw four.
    let word = drawn(&mut fonts, &for_egui("سلام"));
    let mirrored = drawn(&mut fonts, "مالس");
    assert!(word.len() < mirrored.len(), "ligature: {word:?} vs {mirrored:?}");
}

#[test]
fn hebrew_draws_first_word_rightmost() {
    let Some(bytes) = system_font(&[
        "/System/Library/Fonts/SFHebrew.ttf",
        "/System/Library/Fonts/ArialHB.ttc",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
    ]) else {
        eprintln!("skipped: no Hebrew system font");
        return;
    };
    let mut fonts = fonts_with(bytes);
    let shown = drawn(&mut fonts, &for_egui("שלום עולם"));
    // ש starts the first word, ע the second: ש is further right.
    assert!(x_of(&shown, 'ש') > x_of(&shown, 'ע'), "{shown:?}");
}

#[test]
fn a_long_right_to_left_layer_name_keeps_its_beginning() {
    let Some(bytes) = system_font(&ARABIC_FONTS) else {
        eprintln!("skipped: no Arabic system font");
        return;
    };
    let name = "سلام عليكم يا أصدقاء الطبقات";
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(400.0, 100.0)).build_ui_state(
        |ui, out: &mut Option<Arc<egui::Galley>>| {
            *out = crate::layer_row_ui::truncated(ui.painter(), name, egui::FontId::proportional(20.0), egui::Color32::WHITE, false, 90.0);
        },
        None,
    );
    let mut defs = FontDefinitions::default();
    defs.font_data.insert("rtl-test".into(), Arc::new(FontData::from_owned(bytes)));
    if let Some(family) = defs.families.get_mut(&FontFamily::Proportional) {
        family.push("rtl-test".into());
    }
    h.ctx.set_fonts(defs);
    h.run_steps(3);
    let g = h.state().clone().expect("room for a cut name");
    let shown = g.job.text.clone();
    assert!(g.size().x <= 90.0, "{} wide", g.size().x);
    // Cut in logical order: the first word survives, and "…" sits at the reading end (left).
    assert!(shown.starts_with('…') && shown.contains("سلام") && !shown.contains("الطبقات"), "{shown}");
    // Short enough to fit: drawn whole.
    let mut short = egui_kittest::Harness::builder().with_size(egui::vec2(400.0, 100.0)).build_ui_state(
        |ui, out: &mut Option<Arc<egui::Galley>>| {
            *out = crate::layer_row_ui::truncated(ui.painter(), "سلام عليكم", egui::FontId::proportional(20.0), egui::Color32::WHITE, false, 300.0);
        },
        None,
    );
    short.run_steps(1);
    assert_eq!(short.state().as_ref().map(|g| g.job.text.clone()).as_deref(), Some("عليكم سلام"));
}
