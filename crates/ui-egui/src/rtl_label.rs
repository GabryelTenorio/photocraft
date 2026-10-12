//! Right-to-left text in single-line UI labels (layer names), laid out by the text engine.
//!
//! egui 0.36 shapes each font run, so Arabic letters join, but it has no bidi: it lays a line's
//! runs out left to right, so an Arabic or Hebrew name shows its words in reverse order, and its
//! "…" cuts the start of the name instead of the end (#1408). A label with right-to-left text is
//! laid out by PhotoCraft's text engine instead (parley: Unicode bidi, HarfRust shaping and the
//! installed fallback fonts, the same path as document text), in the UI's own Inter face and
//! size. It is rasterized once as a coverage mask and tinted when painted, so theme, selection
//! and visibility colours need no new image. Images are cached per text, size and display
//! scale, and only labels that are painted are rendered. Every other label stays on egui's path.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, PoisonError};

use egui::{Color32, ColorImage, FontFamily, FontId, Galley, Painter, Pos2, Rect, TextureHandle, TextureOptions, Vec2, pos2, vec2};
use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::TextLayer;
use photocraft_doc::text::{AntiAlias, CharStyle, TextRun};
use photocraft_geom::Affine;

const ELLIPSIS: char = '…';
/// Room above and below the row for marks and tall fallback glyphs (points).
const PAD: f32 = 4.0;
/// Room left and right of the advance for overhanging glyphs (pixels).
const MARGIN_PX: f32 = 1.0;
/// Labels wider than this are not drawn by the engine (pixels).
const MAX_WIDTH_PX: f32 = 4096.0;
const CACHE_LIMIT: usize = 256;
const CACHE_ID: &str = "rtl-label-images";
const TEXTURE_NAME: &str = "rtl-label";

/// A single-line label ready to paint.
pub enum Line {
    /// egui's own layout (text without right-to-left characters).
    Galley(Arc<Galley>),
    /// A text-engine rendering; `size` is the label's logical size in points.
    Image(Image),
}

#[derive(Clone)]
pub struct Image {
    texture: TextureHandle,
    size: Vec2,
    /// The texture's size in points, including [`PAD`] and [`MARGIN_PX`].
    painted: Vec2,
}

impl Line {
    /// The label's size in points: its advance width by the font's row height.
    pub fn size(&self) -> Vec2 {
        match self {
            Line::Galley(g) => g.size(),
            Line::Image(i) => i.size,
        }
    }

    /// Paint with its top-left corner at `pos`, in `color`.
    pub fn paint(self, painter: &Painter, pos: Pos2, color: Color32) {
        match self {
            Line::Galley(g) => painter.galley(pos, g, color),
            Line::Image(i) => {
                let ppp = painter.ctx().pixels_per_point();
                let min = pos2(pos.x - MARGIN_PX / ppp, pos.y - pad_px(ppp) / ppp);
                // Pixel-aligned, so the mask is drawn 1:1.
                let min = pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                painter.image(i.texture.id(), Rect::from_min_size(min, i.painted), uv, color);
            }
        }
    }
}

/// [`PAD`] in whole device pixels, so the baseline stays on the pixel grid.
fn pad_px(ppp: f32) -> f32 {
    (PAD * ppp).round()
}

/// Does `text` need bidi reordering, which egui doesn't do? True for right-to-left letters
/// (Hebrew, Arabic, Syriac, Thaana, N'Ko and the other right-to-left blocks) and explicit
/// direction marks and controls.
pub fn needs_engine(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c as u32,
            0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF
            | 0x200F | 0x202B | 0x202E | 0x2067)
    })
}

/// The bundled face egui draws `font` with: family and CSS weight.
fn face(font: &FontId) -> Option<(&'static str, u16)> {
    match &font.family {
        FontFamily::Proportional => Some(("Inter", 400)),
        FontFamily::Monospace => Some(("JetBrains Mono", 400)),
        FontFamily::Name(name) => match name.as_ref() {
            "medium" => Some(("Inter", 500)),
            "semibold" => Some(("Inter", 600)),
            _ => None,
        },
    }
}

/// What one rendering depends on. Colour is not part of it: the image is tinted when painted.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    text: String,
    family: &'static str,
    weight: u16,
    italic: bool,
    /// Font size in pixels, the row height and baseline in pixels, and the display scale.
    size_px: u32,
    row_px: u32,
    baseline_px: u32,
    ppp: u32,
    /// The font database generations: a fallback font that arrives later gets a new image.
    fonts: (u64, u64),
}

/// Laid-out and rendered labels, most recently used last; `None` for text the engine couldn't
/// lay out. `fits` remembers the text shown for a label at a width in pixels ([`fitted`]).
#[derive(Clone, Default)]
struct Cache {
    images: VecDeque<(Key, Option<Image>)>,
    fits: HashMap<(Key, u32), Option<String>>,
}

/// The engine's style for `key`.
fn layer(text: &str, key: &Key) -> TextLayer {
    let style = CharStyle {
        font_family: key.family.into(),
        weight: key.weight,
        faux_italic: key.italic,
        size_pt: key.size_px as f32 / 64.0,
        color: photocraft_color::Color::BLACK,
        ..Default::default()
    };
    TextLayer {
        text: text.into(),
        font_family: key.family.into(),
        size_pt: style.size_pt,
        runs: vec![TextRun { len: text.len(), style }],
        ..Default::default()
    }
}

/// The advance width of `text` in pixels (72 dpi: a point is a pixel), or `None` when it can't
/// be laid out.
fn measure(engine: &mut photocraft_text::TextEngine, text: &str, key: &Key) -> Option<f32> {
    let layout = engine.layout(&layer(text, key), 72.0);
    let line = layout.lines.first()?;
    let width = line.x1 - line.x0;
    (width.is_finite() && width <= MAX_WIDTH_PX).then_some(width)
}

/// Rasterize `key.text` as a white coverage mask, its baseline at `key.baseline_px` below the
/// top [`PAD`], and its logical width.
fn render(engine: &mut photocraft_text::TextEngine, key: &Key) -> Option<(ColorImage, f32)> {
    let layout = engine.layout(&layer(&key.text, key), 72.0);
    let line = layout.lines.first()?;
    let advance = line.x1 - line.x0;
    if !advance.is_finite() || advance <= 0.0 || advance > MAX_WIDTH_PX {
        return None;
    }
    let ppp = f32::from_bits(key.ppp);
    let pad = pad_px(ppp);
    let width = (advance + 2.0 * MARGIN_PX).ceil() as usize;
    let height = (key.row_px as f32 / 64.0 + 2.0 * pad).ceil() as usize;
    let baseline = pad + key.baseline_px as f32 / 64.0;
    let transform = Affine::translate(f64::from(MARGIN_PX - line.x0), f64::from(baseline - line.baseline));
    // Reject absurd ink (malformed fallback fonts) before the rasterizer allocates for it.
    let ink = photocraft_text::render::ink_rect(&layout, &transform);
    if ink.width() > width as u32 * 4 || ink.height() > height as u32 * 4 {
        return None;
    }
    let rendered = photocraft_text::render::rasterize(&layout, &transform, PixelFormat::new(ColorMode::Rgb, SampleType::U8, true), AntiAlias::Smooth);
    let mut rgba = vec![[0u8; 4]; width * height];
    rendered.surface.read_rgba8_into(photocraft_geom::Rect::new(0, 0, width as i32, height as i32), &mut rgba);
    let pixels: Vec<u8> = rgba.into_iter().flat_map(|p| [255, 255, 255, p[3]]).collect();
    Some((ColorImage::from_rgba_unmultiplied([width, height], &pixels), advance))
}

/// The largest `k` in `0..n` for which `fits(k)` holds, `fits` being true up to some `k` and
/// false after it; `None` when `fits(0)` is false.
fn longest_fitting(n: usize, mut fits: impl FnMut(usize) -> bool) -> Option<usize> {
    if n == 0 || !fits(0) {
        return None;
    }
    let (mut lo, mut hi) = (0, n - 1);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Some(lo)
}

/// The first `chars` characters of `text` (trailing spaces dropped) and "…": the logical end is
/// cut, which a right-to-left paragraph shows on its left.
fn cut(text: &str, chars: usize) -> String {
    let end = text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i);
    let mut s = text.get(..end).unwrap_or(text).trim_end().to_string();
    s.push(ELLIPSIS);
    s
}

/// The text shown for `text` within `max_px` pixels: all of it, or the longest start that fits
/// with "…". `None` when even "…" doesn't fit.
fn fitted(engine: &mut photocraft_text::TextEngine, text: &str, key: &Key, max_px: f32) -> Option<String> {
    if measure(engine, text, key).is_some_and(|w| w <= max_px) {
        return Some(text.to_string());
    }
    let n = text.chars().count();
    let k = longest_fitting(n, |k| measure(engine, &cut(text, k), key).is_some_and(|w| w <= max_px))?;
    Some(cut(text, k))
}

/// `text` laid out and rendered by the text engine in `font`, cut with "…" to `max_width`
/// points. `None` when the engine can't draw it now (the system font scan is still running, the
/// font isn't a bundled UI face, or the text can't be laid out): the caller uses egui's layout.
pub fn line(painter: &Painter, text: &str, font: &FontId, italic: bool, max_width: f32) -> Option<Line> {
    let ctx = painter.ctx();
    let (family, weight) = face(font)?;
    let Some(engine) = photocraft_text::try_shared() else {
        // The installed fonts are still being scanned (they hold the right-to-left faces).
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return None;
    };
    let ppp = ctx.pixels_per_point();
    // egui's row metrics for the font, so the label lines up with the rest of the row.
    let reference = painter.layout_no_wrap("x".into(), font.clone(), Color32::WHITE);
    let row = reference.size().y;
    let baseline = reference.rows.first().and_then(|r| r.glyphs.first().map(|g| r.pos.y + g.pos.y)).unwrap_or(row * 0.8);
    let to_key = |v: f32| (v * ppp * 64.0).round() as u32;
    let mut key = Key {
        text: text.to_string(),
        family,
        weight,
        italic,
        size_px: to_key(font.size * crate::theme::ui_font_scale(ctx)),
        row_px: to_key(row),
        baseline_px: to_key(baseline),
        ppp: ppp.to_bits(),
        fonts: (photocraft_text::fonts::generation(), photocraft_text::served::generation()),
    };
    let id = egui::Id::new(CACHE_ID);
    let max_px = (max_width * ppp).floor();
    let fit_key = (key.clone(), max_px as u32);
    let known = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(id).fits.get(&fit_key).cloned());
    let shown = known.unwrap_or_else(|| {
        let shown = fitted(&mut engine.lock().unwrap_or_else(PoisonError::into_inner), text, &key, max_px);
        ctx.data_mut(|d| {
            let cache = d.get_temp_mut_or_default::<Cache>(id);
            if cache.fits.len() >= CACHE_LIMIT * 4 {
                cache.fits.clear();
            }
            cache.fits.insert(fit_key, shown.clone());
        });
        shown
    });
    key.text = shown?;
    image(ctx, engine, key).map(Line::Image)
}

/// The cached rendering for `key`, rendered and cached on a miss.
fn image(ctx: &egui::Context, engine: &std::sync::Mutex<photocraft_text::TextEngine>, key: Key) -> Option<Image> {
    let id = egui::Id::new(CACHE_ID);
    let cached = ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(id);
        let index = cache.images.iter().position(|(k, _)| *k == key)?;
        let entry = cache.images.remove(index)?;
        let image = entry.1.clone();
        cache.images.push_back(entry);
        Some(image)
    });
    if let Some(image) = cached {
        return image;
    }
    let ppp = f32::from_bits(key.ppp);
    let rendered = render(&mut engine.lock().unwrap_or_else(PoisonError::into_inner), &key);
    let image = rendered.map(|(pixels, advance)| {
        let painted = vec2(pixels.size[0] as f32, pixels.size[1] as f32) / ppp;
        let size = vec2(advance / ppp, key.row_px as f32 / 64.0 / ppp);
        Image { texture: ctx.load_texture(TEXTURE_NAME, pixels, TextureOptions::LINEAR), size, painted }
    });
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(id);
        if cache.images.len() >= CACHE_LIMIT {
            cache.images.pop_front();
        }
        cache.images.push_back((key, image.clone()));
    });
    image
}

#[cfg(test)]
mod tests;
