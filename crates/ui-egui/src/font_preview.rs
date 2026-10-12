//! Small font-menu samples, rendered by the same font database as document text.

use std::collections::VecDeque;
use std::sync::PoisonError;

use egui::{ColorImage, Rect, TextureHandle, TextureOptions, pos2, vec2};
use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::TextLayer;
use photocraft_engine::prefs::FontPreview;
use photocraft_geom::Affine;

const SAMPLE: &str = "AaBbCc";
/// The Latin and second-script sample widths and the sample height at the menu's medium (18 pt).
const WIDTH: f32 = 100.0;
const SECOND_WIDTH: f32 = 72.0;
const GAP: f32 = 6.0;
const HEIGHT: f32 = 24.0;
/// The medium point size the widths above are measured at.
const MEDIUM: f32 = 18.0;
const CACHE_LIMIT: usize = 128;
const CACHE_ID: &str = "font-menu-previews";

/// A width or height measured at 18 pt, scaled to the sample's point size.
fn scaled(v: f32, size: f32) -> f32 {
    (v * size / MEDIUM).round()
}

/// The Latin sample's `[width, height]` box for a point size: [`WIDTH`]×[`HEIGHT`] at 18 pt (the
/// menu's medium), proportional otherwise.
fn box_for(size: f32) -> [f32; 2] {
    [scaled(WIDTH, size), scaled(HEIGHT, size)]
}

/// The font menu's sample geometry for Preferences ▸ Type ▸ Font Preview Size: the sample's point
/// size and its Latin box; `None` when the preview is off.
pub(crate) fn sample(preview: FontPreview) -> Option<(f32, [f32; 2])> {
    let size = match preview {
        FontPreview::Off => return None,
        FontPreview::Small => 14.0,
        FontPreview::Medium => MEDIUM,
        FontPreview::Large => 24.0,
        FontPreview::ExtraLarge => 32.0,
        FontPreview::Huge => 40.0,
    };
    Some((size, box_for(size)))
}

/// Family, display scale, the font database generations (so a family installed or fetched
/// (served fonts) after its first miss gets a sample once it is available) and the point size.
type Key = (String, u32, (u64, u64), u32);
#[derive(Clone)]
struct Samples {
    latin: TextureHandle,
    second: Option<TextureHandle>,
}
#[derive(Clone, Default)]
struct Cache(VecDeque<(Key, Option<Samples>)>);

struct SampleImages {
    latin: ColorImage,
    second: Option<ColorImage>,
}

/// The first sample's text, and the second script's sample for a face with Latin and another
/// script ([`crate::font_samples`]). `chars` is the face's ascending charmap and `code_pages`
/// its OS/2 code page bits. A face without Latin shows its own script's sample alone, and a
/// symbol face its first six glyphs.
fn sample_texts(chars: &[char], code_pages: u64) -> (String, Option<&'static str>) {
    let latin = SAMPLE.chars().all(|c| chars.binary_search(&c).is_ok());
    let script = crate::font_samples::primary(chars, latin, code_pages).map(|s| s.sample);
    match (latin, script) {
        (true, second) => (SAMPLE.into(), second),
        (false, Some(own)) => (own.into(), None),
        (false, None) => (chars.iter().copied().filter(|c| !c.is_whitespace()).take(6).collect(), None),
    }
}

fn render(engine: &mut photocraft_text::TextEngine, family: &str, text: &str, slot: f32, scale: u32, size: f32) -> Option<ColorImage> {
    if text.is_empty() {
        return None;
    }
    let layer = TextLayer { text: text.into(), font_family: family.into(), size_pt: size * scale as f32, ..Default::default() };
    let layout = engine.layout(&layer, 72.0);
    let bounds = layout.bounds()?;
    let width = (slot as u32 * scale) as usize;
    let height = (scaled(HEIGHT, size) as u32 * scale) as usize;
    let transform = Affine::translate(f64::from(-bounds[0]), f64::from((height as f32 - (bounds[3] - bounds[1])) / 2.0 - bounds[1]));
    // Reject oversized ink before the rasterizer allocates a surface (malformed installed fonts).
    let ink = photocraft_text::render::ink_rect(&layout, &transform);
    if ink.is_empty() || ink.width() > width as u32 * 4 || ink.height() > height as u32 * 4 {
        return None;
    }
    let rendered = photocraft_text::render::rasterize(&layout, &transform, PixelFormat::new(ColorMode::Rgb, SampleType::U8, true), layer.antialias);
    let mut rgba = vec![[0u8; 4]; width * height];
    rendered.surface.read_rgba8_into(photocraft_geom::Rect::new(0, 0, width as i32, height as i32), &mut rgba);
    Some(ColorImage::from_rgba_unmultiplied([width, height], &rgba.into_iter().flat_map(|p| [255, 255, 255, p[3]]).collect::<Vec<_>>()))
}

fn images(engine: &mut photocraft_text::TextEngine, family: &str, scale: u32, size: f32) -> Option<SampleImages> {
    // Unknown/unreadable families keep their plain menu label, rather than a misleading fallback.
    if !engine.fonts.has_family(family) {
        return None;
    }
    let (first, second) = sample_texts(&engine.fonts.charmap(family, 400, false), engine.fonts.code_pages(family, 400, false));
    let latin = render(engine, family, &first, scaled(WIDTH, size), scale, size)?;
    let second = second.and_then(|text| render(engine, family, text, scaled(SECOND_WIDTH, size), scale, size));
    Some(SampleImages { latin, second })
}

fn key(ctx: &egui::Context, family: &str, size: f32) -> Key {
    let scale = ctx.pixels_per_point().ceil().clamp(1.0, 4.0) as u32;
    (family.to_owned(), scale, (photocraft_text::fonts::generation(), photocraft_text::served::generation()), size as u32)
}

/// Width a row reserves for its samples: wider once a painted row showed the family has a
/// second script, so the label truncates before the samples. Never renders, so rows scrolled out of
/// view cost nothing.
pub(crate) fn slot_width(ctx: &egui::Context, family: &str, size: f32) -> f32 {
    let key = key(ctx, family, size);
    let second = ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(egui::Id::new(CACHE_ID));
        cache.0.iter().any(|(k, samples)| k == &key && samples.as_ref().is_some_and(|s| s.second.is_some()))
    });
    if second { scaled(WIDTH, size) + GAP + scaled(SECOND_WIDTH, size) } else { scaled(WIDTH, size) }
}

pub(crate) fn paint(ui: &egui::Ui, family: &str, row: Rect, size: f32) {
    let ctx = ui.ctx();
    let scale = ctx.pixels_per_point().ceil().clamp(1.0, 4.0) as u32;
    let key = key(ctx, family, size);
    let id = egui::Id::new(CACHE_ID);
    let cached = ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(id);
        let index = cache.0.iter().position(|(k, _)| k == &key)?;
        let entry = cache.0.remove(index)?;
        let samples = entry.1.clone();
        cache.0.push_back(entry);
        Some(samples)
    });
    let samples = cached.unwrap_or_else(|| {
        let samples = images(&mut photocraft_text::shared().lock().unwrap_or_else(PoisonError::into_inner), family, scale, size).map(|img| {
            let load = |image| ctx.load_texture(format!("font-sample-{family}"), image, TextureOptions::LINEAR);
            Samples { latin: load(img.latin), second: img.second.map(load) }
        });
        if samples.as_ref().is_some_and(|s| s.second.is_some()) {
            // The row reserved only the Latin width until now.
            ctx.request_repaint();
        }
        ctx.data_mut(|d| {
            let cache = d.get_temp_mut_or_default::<Cache>(id);
            if cache.0.len() >= CACHE_LIMIT {
                cache.0.pop_front();
            }
            cache.0.push_back((key, samples.clone()));
        });
        samples
    });
    if let Some(samples) = samples {
        let tint = crate::theme::Tokens::get(ctx).text;
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let [w, h] = box_for(size);
        let latin = Rect::from_min_size(pos2(row.right() - w - 8.0, row.center().y - h / 2.0), vec2(w, h));
        ui.painter().image(samples.latin.id(), latin, uv, tint);
        if let Some(second) = samples.second {
            let sw = scaled(SECOND_WIDTH, size);
            let rect = Rect::from_min_size(pos2(latin.left() - GAP - sw, latin.top()), vec2(sw, h));
            ui.painter().image(second.id(), rect, uv, tint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(engine: &mut photocraft_text::TextEngine, family: &str, scale: u32) -> Option<ColorImage> {
        images(engine, family, scale, MEDIUM).map(|i| i.latin)
    }

    fn charmap(text: &str) -> Vec<char> {
        let mut chars: Vec<char> = text.chars().collect();
        chars.sort_unstable();
        chars
    }

    #[test]
    fn previews_use_the_requested_face_and_scale() {
        let mut engine = photocraft_text::TextEngine::new();
        let sans = image(&mut engine, "Inter", 1).expect("bundled sans");
        let mono = image(&mut engine, "JetBrains Mono", 1).expect("bundled mono");
        assert_eq!(sans.size, [100, 24]);
        assert!(sans.pixels.iter().any(|p| p.a() > 0));
        assert_ne!(sans.pixels, mono.pixels, "samples use different font outlines");
        assert_eq!(image(&mut engine, "Inter", 2).unwrap().size, [200, 48]);
        assert!(image(&mut engine, "Missing font", 1).is_none());
    }

    /// Preferences ▸ Type ▸ Font Preview Size: Small…Huge scale the sample, Off removes it.
    #[test]
    fn the_preview_size_preference_scales_the_sample() {
        assert_eq!(sample(FontPreview::Off), None);
        assert_eq!(sample(FontPreview::Medium), Some((18.0, [100.0, 24.0])));
        let Some((_, [w, h])) = sample(FontPreview::Huge) else { panic!("Huge previews") };
        assert!(w > 100.0 && h > 24.0, "Huge is bigger than Medium: {w}×{h}");
        assert!(sample(FontPreview::Small).unwrap().1[0] < sample(FontPreview::ExtraLarge).unwrap().1[0], "sizes are ordered");
        let mut engine = photocraft_text::TextEngine::new();
        let big = images(&mut engine, "Inter", 1, 32.0).unwrap().latin;
        assert_eq!(big.size, [178, 43], "the box follows the size");
    }

    #[test]
    fn latin_only_families_get_no_second_sample() {
        let mut engine = photocraft_text::TextEngine::new();
        for family in ["Inter", "JetBrains Mono"] {
            assert!(images(&mut engine, family, 1, MEDIUM).expect("bundled").second.is_none(), "{family}");
        }
        assert!(images(&mut engine, "", 1, MEDIUM).is_none());
        assert!(images(&mut engine, "Missing font", 1, MEDIUM).is_none());
    }

    /// The Arabic sample and core letters (alef, lam, meem, ain).
    const ARABIC: &str = "أبجد هوز\u{627}\u{644}\u{645}\u{639}";

    #[test]
    fn faces_with_latin_and_another_script_get_its_sample() {
        let latin = format!("{SAMPLE} ");
        let (text, second) = sample_texts(&charmap(&format!("{latin}{ARABIC}")), 0);
        assert_eq!((text.as_str(), second), (SAMPLE, Some("أبجد هوز")));
        assert_eq!(sample_texts(&charmap(&latin), 0), (SAMPLE.into(), None));
        let (text, second) = sample_texts(&charmap(&format!("{latin}हिन्दी\u{915}\u{930}\u{93E}")), 0);
        assert_eq!((text.as_str(), second), (SAMPLE, Some("हिन्दी")), "Hindi");
        let (text, second) = sample_texts(&charmap(&format!("{latin}あア亜\u{306E}\u{30F3}\u{65E5}\u{FF71}")), 0);
        assert_eq!((text.as_str(), second), (SAMPLE, Some("あア亜")), "Japanese");
    }

    #[test]
    fn faces_without_latin_show_their_own_script_or_glyphs() {
        assert_eq!(sample_texts(&charmap(ARABIC), 0), ("أبجد هوز".into(), None), "an Arabic-only face");
        let symbols = "\u{2605}\u{2606}\u{2660}\u{2663}\u{2665}\u{2666}\u{266A}";
        let (own_glyphs, none) = sample_texts(&charmap(symbols), 0);
        assert_eq!((own_glyphs.chars().count(), none), (6, None));
        assert!(own_glyphs.chars().all(|c| symbols.contains(c)));
        assert_eq!(sample_texts(&[], 0), (String::new(), None));
    }

    /// The first installed family whose second sample is `sample` renders it in the second slot.
    fn renders_second_sample(engine: &mut photocraft_text::TextEngine, sample: &str) {
        let found = engine
            .fonts
            .families()
            .into_iter()
            .find(|f| sample_texts(&engine.fonts.charmap(f, 400, false), engine.fonts.code_pages(f, 400, false)).1 == Some(sample));
        let Some(family) = found else {
            eprintln!("skipped: no installed family has Latin and {sample}");
            return;
        };
        let images = images(engine, &family, 1, MEDIUM).expect(&family);
        let second = images.second.expect("second sample");
        assert_eq!(second.size, [SECOND_WIDTH as usize, HEIGHT as usize]);
        assert!(second.pixels.iter().any(|p| p.a() > 0), "{family}");
    }

    #[test]
    fn system_faces_render_their_second_samples() {
        let mut engine = photocraft_text::TextEngine::new();
        engine.fonts.load_system_fonts();
        for sample in ["أبجد هوز", "हिन्दी", "あア亜", "汉字", "가나다", "สวัสดี"] {
            renders_second_sample(&mut engine, sample);
        }
    }

    /// Prints what each installed family samples:
    /// `cargo test -p photocraft-ui-egui --lib list_system_samples -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints the samples of the installed fonts"]
    fn list_system_samples() {
        let mut engine = photocraft_text::TextEngine::new();
        engine.fonts.load_system_fonts();
        for family in engine.fonts.families() {
            let (first, second) = sample_texts(&engine.fonts.charmap(&family, 400, false), engine.fonts.code_pages(&family, 400, false));
            println!("{family}: {first} {}", second.unwrap_or(""));
        }
    }
}
