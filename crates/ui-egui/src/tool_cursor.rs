//! Canvas cursors must move with the OS pointer, independently of the canvas frame rate: the brush
//! outline and crosshairs are OS cursor bitmaps on every desktop platform (Windows, macOS, X11 and
//! Wayland), so they track the hardware pointer instead of trailing it by the app's render,
//! present and compositor latency. A painted outline lagged the pointer by several refreshes on
//! Linux and macOS and "jumped to catch up" on slow frames (#1343). Windows' stock crosshair
//! inverts grey into grey, so the crosshairs are our black/white bitmap there too.

use std::sync::Arc;

use egui::{CursorIcon, CustomCursorImage, Painter, Pos2, Stroke, vec2};

mod hand;
pub(crate) mod tip;

/// The largest cursor bitmap, in physical pixels; a larger tip paints its outline instead (with a
/// small OS crosshair at the hotspot). Windows takes winit's limit. Elsewhere, X servers and
/// hardware cursor planes commonly cap cursors at 256 px and fall back to slow software cursors
/// (or reject them) past that, so stay within it there (#968 used 128). Either way it keeps
/// allocations bounded for hostile brush sizes or zooms.
const MAX_SIDE: u16 = if cfg!(target_os = "windows") { 2048 } else { 255 };

/// What a bitmap cursor shows. `Hand` is the Hand tool's open hand, or its fist while it drags.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    Circle { radius: f32, centre: bool },
    Crosshair { length: f32, gap: f32 },
    Hand { closed: bool },
}

#[derive(Clone)]
struct Cached {
    shape: Shape,
    scale: f32,
    image: CustomCursorImage,
}

/// The image is sticky in egui's platform output. Clear it every pass, including when a
/// document closes or the pointer leaves the canvas; a later widget's cursor takes precedence.
pub(crate) struct CursorLifecycle;

impl egui::plugin::Plugin for CursorLifecycle {
    fn debug_name(&self) -> &'static str {
        "PhotoCraft tool cursor"
    }

    fn on_begin_pass(&mut self, ui: &mut egui::Ui) {
        ui.ctx().set_cursor_image(None);
    }

    fn on_end_pass(&mut self, ui: &mut egui::Ui) {
        ui.ctx().output_mut(|o| {
            if o.cursor_image.is_some() {
                if o.cursor_icon == CursorIcon::None {
                    // None marks our request until all widgets have had their say. A backend
                    // that cannot upload the bitmap must still leave a visible pointer.
                    o.cursor_icon = CursorIcon::Crosshair;
                } else {
                    o.cursor_image = None;
                }
            }
        });
    }
}

pub(crate) fn circle(painter: &Painter, at: Pos2, radius: f32, centre: bool) -> CursorIcon {
    show(painter, at, Shape::Circle { radius, centre })
}

/// The brush cursor as the tip's own outline (`tip`): `segments` are offsets from the pointer in
/// points, already through the view. A bitmap cursor where the OS takes one, else painted (with
/// the small crosshair for an outline too big for a cursor), as [`circle`] does.
pub(crate) fn outline(painter: &Painter, at: Pos2, segments: &[[egui::Vec2; 2]], centre: bool) -> CursorIcon {
    let ctx = painter.ctx();
    let reach = segments.iter().flat_map(|s| s.iter()).map(|v| v.length()).fold(0.0f32, f32::max);
    if !reach.is_finite() || reach <= 0.0 {
        return crosshair(painter, at, 4.0, 0.0);
    }
    if native(ctx) {
        let scale = native_scale(ctx);
        if let Some(image) = cached_outline(ctx, segments, centre, scale) {
            ctx.set_cursor_image(Some(image));
            return CursorIcon::None;
        }
        paint_outline(painter, at, segments, centre);
        if let Some(image) = cached_image(ctx, Shape::Crosshair { length: 6.0, gap: 2.0 }, scale) {
            ctx.set_cursor_image(Some(image));
        }
        return CursorIcon::None;
    }
    paint_outline(painter, at, segments, centre);
    CursorIcon::None
}

fn paint_outline(painter: &Painter, at: Pos2, segments: &[[egui::Vec2; 2]], centre: bool) {
    let [dark, light] = crate::theme::Tokens::cursor_outline();
    for (width, color) in [(3.0, dark), (1.0, light)] {
        let stroke = Stroke::new(width, color);
        for s in segments {
            painter.line_segment([at + s[0], at + s[1]], stroke);
        }
        if centre {
            paint_crosshair(painter, at, 3.0, 0.0, stroke);
        }
    }
}

/// The outline's bitmap, cached by its quantised geometry so an unchanged tip and view reuse it.
fn cached_outline(ctx: &egui::Context, segments: &[[egui::Vec2; 2]], centre: bool, scale: f32) -> Option<CustomCursorImage> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (centre, scale.to_bits(), segments.len()).hash(&mut h);
    for p in segments.iter().flat_map(|s| s.iter()) {
        ((p.x * 8.0).round() as i32, (p.y * 8.0).round() as i32).hash(&mut h);
    }
    let key = h.finish();
    let id = egui::Id::new("photocraft-tool-cursor-outline");
    if let Some((cached, image)) = ctx.data(|d| d.get_temp::<(u64, CustomCursorImage)>(id))
        && cached == key
    {
        return Some(image);
    }
    let image = rasterize_outline(segments, centre, scale)?;
    ctx.data_mut(|d| d.insert_temp(id, (key, image.clone())));
    Some(image)
}

/// Distance from `p` to the segment `a`–`b`.
fn segment_distance(p: egui::Vec2, a: egui::Vec2, b: egui::Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).length()
}

/// The outline drawn into a cursor bitmap with the circle's 1 px light line between dark ones.
/// Each segment only touches the pixels within its stroke, so the cost follows the outline's
/// length, not the bitmap's area.
fn rasterize_outline(segments: &[[egui::Vec2; 2]], centre: bool, scale: f32) -> Option<CustomCursorImage> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let reach = segments.iter().flat_map(|s| s.iter()).map(|v| v.length()).fold(if centre { 3.0f32 } else { 0.0 }, f32::max);
    let half = ((reach + 1.5) * scale + 1.0).ceil();
    if !half.is_finite() || half > f32::from((MAX_SIDE - 1) / 2) {
        return None;
    }
    let hot = half as u16;
    let side = usize::from(hot * 2 + 1);
    let n = side.checked_mul(side)?;
    let (mut fill, mut outline) = (vec![0.0f32; n], vec![0.0f32; n]);
    let mut stroke = |a: egui::Vec2, b: egui::Vec2| {
        let (a, b) = (a * scale, b * scale);
        let pad = 1.5 * scale + 1.0;
        let lo = |v: f32| ((v - pad + f32::from(hot)).floor().max(0.0)) as usize;
        let hi = |v: f32| ((v + pad + f32::from(hot)).ceil().max(0.0) as usize).min(side - 1);
        for row in lo(a.y.min(b.y))..=hi(a.y.max(b.y)) {
            for col in lo(a.x.min(b.x))..=hi(a.x.max(b.x)) {
                let p = egui::vec2(col as f32 - f32::from(hot), row as f32 - f32::from(hot));
                let (f, o) = stroke_coverage(segment_distance(p, a, b), scale);
                if let (Some(fv), Some(ov)) = (fill.get_mut(row * side + col), outline.get_mut(row * side + col)) {
                    *fv = fv.max(f);
                    *ov = ov.max(o);
                }
            }
        }
    };
    for s in segments {
        stroke(s[0], s[1]);
    }
    if centre {
        for d in [egui::vec2(3.0, 0.0), egui::vec2(-3.0, 0.0), egui::vec2(0.0, 3.0), egui::vec2(0.0, -3.0)] {
            stroke(egui::Vec2::ZERO, d);
        }
    }
    let [dark, light] = crate::theme::Tokens::cursor_outline();
    let mut rgba = Vec::with_capacity(n.checked_mul(4)?);
    for (f, o) in fill.iter().zip(&outline) {
        let a_dark = o * f32::from(dark.a()) / 255.0;
        let a_light = f * f32::from(light.a()) / 255.0;
        let alpha = a_light + a_dark * (1.0 - a_light);
        let white = if alpha > 0.0 { (255.0 * a_light / alpha).round() as u8 } else { 0 };
        rgba.extend_from_slice(&[white, white, white, (255.0 * alpha).round() as u8]);
    }
    let size = hot * 2 + 1;
    Some(CustomCursorImage { rgba: Arc::from(rgba), size: [size, size], hotspot: [hot, hot] })
}

pub(crate) fn crosshair(painter: &Painter, at: Pos2, length: f32, gap: f32) -> CursorIcon {
    show(painter, at, Shape::Crosshair { length, gap })
}

/// Can this pass show an OS cursor bitmap? eframe's immediate viewports and its web integration
/// don't upload cursor images; every desktop root viewport does (winit custom cursors).
fn native(ctx: &egui::Context) -> bool {
    cfg!(not(target_arch = "wasm32")) && ctx.viewport_id() == egui::ViewportId::ROOT
}

/// Bitmap pixels per egui point. winit takes an `NSCursor` image's dimensions as logical points on
/// macOS, so there the bitmap scales only with the UI zoom (the monitor's density would enlarge it
/// a second time on Retina displays); Windows, X11 and Wayland take physical pixels. (The macOS
/// sizing is from @jerrykimo's #968.)
pub(crate) fn native_scale(ctx: &egui::Context) -> f32 {
    if cfg!(target_os = "macos") { ctx.zoom_factor() } else { ctx.pixels_per_point() }
}

/// The Hand tool's pointer: an open hand, a fist while it drags (`closed`). winit maps `Grab` and
/// `Grabbing` to the four-arrow move cursor on Windows, so there it is a bitmap; macOS and the
/// Linux themes already draw hands, and the OS moves those without waiting for a frame.
pub(crate) fn hand(ctx: &egui::Context, closed: bool) -> CursorIcon {
    hand_with(ctx, closed, cfg!(target_os = "windows") && ctx.viewport_id() == egui::ViewportId::ROOT)
}

fn hand_with(ctx: &egui::Context, closed: bool, bitmap: bool) -> CursorIcon {
    if bitmap && let Some(image) = cached_image(ctx, Shape::Hand { closed }, native_scale(ctx)) {
        ctx.set_cursor_image(Some(image));
        return CursorIcon::None;
    }
    if closed { CursorIcon::Grabbing } else { CursorIcon::Grab }
}

fn show(painter: &Painter, at: Pos2, shape: Shape) -> CursorIcon {
    let ctx = painter.ctx();
    // Where cursor images can't be uploaded (immediate viewports, the web), paint the shape.
    if native(ctx) {
        let scale = native_scale(ctx);
        if let Some(image) = cached_image(ctx, shape, scale) {
            ctx.set_cursor_image(Some(image));
            return CursorIcon::None;
        }
        // An enormous tip cannot fit an OS cursor. Preserve its full-size outline, with an OS
        // crosshair at the hotspot so the user still gets immediate position feedback.
        paint(painter, at, shape);
        if let Some(image) = cached_image(ctx, Shape::Crosshair { length: 6.0, gap: 2.0 }, scale) {
            ctx.set_cursor_image(Some(image));
        }
        return CursorIcon::None;
    }
    paint(painter, at, shape);
    CursorIcon::None
}

fn cached_image(ctx: &egui::Context, shape: Shape, scale: f32) -> Option<CustomCursorImage> {
    let id = egui::Id::new("photocraft-tool-cursor");
    if let Some(cached) = ctx.data(|d| d.get_temp::<Cached>(id))
        && cached.shape == shape
        && cached.scale == scale
    {
        return Some(cached.image);
    }
    let image = rasterize(shape, scale)?;
    ctx.data_mut(|d| d.insert_temp(id, Cached { shape, scale, image: image.clone() }));
    Some(image)
}

fn paint(painter: &Painter, at: Pos2, shape: Shape) {
    if extent(shape).is_none() {
        return;
    }
    let [dark, light] = crate::theme::Tokens::cursor_outline();
    for (width, color) in [(3.0, dark), (1.0, light)] {
        let stroke = Stroke::new(width, color);
        match shape {
            Shape::Circle { radius, centre } => {
                painter.circle_stroke(at, radius, stroke);
                if centre {
                    paint_crosshair(painter, at, 3.0, 0.0, stroke);
                }
            }
            Shape::Crosshair { length, gap } => paint_crosshair(painter, at, length, gap, stroke),
            // Only ever a bitmap: without one the OS draws its own grab cursors.
            Shape::Hand { .. } => {}
        }
    }
}

fn paint_crosshair(painter: &Painter, at: Pos2, length: f32, gap: f32, stroke: Stroke) {
    for d in [vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0)] {
        painter.line_segment([at + d * gap, at + d * length], stroke);
    }
}

/// Distance to the nearest arm (round caps), in physical pixels from the hotspot.
fn cross_distance(x: f32, y: f32, length: f32, gap: f32) -> f32 {
    let horizontal = (x - x.clamp(gap, length)).hypot(y);
    let vertical = x.hypot(y - y.clamp(gap, length));
    horizontal.min(vertical)
}

fn extent(shape: Shape) -> Option<f32> {
    Some(match shape {
        Shape::Circle { radius, centre } => {
            if !radius.is_finite() || radius <= 0.0 {
                return None;
            }
            if centre { radius.max(3.0) } else { radius }
        }
        Shape::Crosshair { length, gap } => {
            if !length.is_finite() || !gap.is_finite() || gap < 0.0 || length < gap {
                return None;
            }
            length
        }
        Shape::Hand { .. } => hand::HALF,
    })
}

/// How much of the pixel at `(dx, dy)` physical pixels from the hotspot the light fill covers and
/// how much the dark outline around it does, each 0 to 1. A dark crease inside the hand takes
/// from the fill, so the fill never covers it.
fn coverage(shape: Shape, dx: f32, dy: f32, scale: f32) -> (f32, f32) {
    let (x, y) = (dx.abs(), dy.abs());
    match shape {
        Shape::Circle { radius, centre } => {
            let ring = (x.hypot(y) - radius * scale).abs();
            let distance = if centre { ring.min(cross_distance(x, y, 3.0 * scale, 0.0)) } else { ring };
            stroke_coverage(distance, scale)
        }
        Shape::Crosshair { length, gap } => stroke_coverage(cross_distance(x, y, length * scale, gap * scale), scale),
        Shape::Hand { closed } => {
            let (lx, ly) = (dx / scale, dy / scale);
            let edge = hand::shape(closed, lx, ly) * scale;
            let inside = (0.5 - edge).clamp(0.0, 1.0);
            let outline = (scale + 0.5 - edge).clamp(0.0, 1.0);
            let crease = (0.45 * scale + 0.5 - hand::crease(closed, lx, ly) * scale).clamp(0.0, 1.0);
            (inside * (1.0 - crease), outline.max(inside * crease))
        }
    }
}

/// A 1 px light line with a 1 px dark one on each side, `distance` physical pixels from its centre.
fn stroke_coverage(distance: f32, scale: f32) -> (f32, f32) {
    ((0.5 * scale + 0.5 - distance).clamp(0.0, 1.0), (1.5 * scale + 0.5 - distance).clamp(0.0, 1.0))
}

fn rasterize(shape: Shape, scale: f32) -> Option<CustomCursorImage> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let extent = extent(shape)?;
    // Odd dimensions put the hotspot on the centre pixel at every DPI. Include the outline
    // and its antialiasing fringe, then check before converting or allocating.
    let half = ((extent + 1.5) * scale + 1.0).ceil();
    if !half.is_finite() || half > f32::from((MAX_SIDE - 1) / 2) {
        return None;
    }
    let hot = half as u16;
    let side = hot * 2 + 1;
    let bytes = usize::from(side).checked_mul(usize::from(side))?.checked_mul(4)?;
    let mut rgba = Vec::with_capacity(bytes);
    let [dark, light] = crate::theme::Tokens::cursor_outline();
    for row in 0..side {
        let dy = f32::from(row) - f32::from(hot);
        for col in 0..side {
            let dx = f32::from(col) - f32::from(hot);
            let (fill, outline) = coverage(shape, dx, dy, scale);
            let a_dark = outline * f32::from(dark.a()) / 255.0;
            let a_light = fill * f32::from(light.a()) / 255.0;
            let alpha = a_light + a_dark * (1.0 - a_light);
            // Straight RGBA, as required by winit, not egui's premultiplied representation.
            let white = if alpha > 0.0 { (255.0 * a_light / alpha).round() as u8 } else { 0 };
            rgba.extend_from_slice(&[white, white, white, (255.0 * alpha).round() as u8]);
        }
    }
    Some(CustomCursorImage { rgba: Arc::from(rgba), size: [side, side], hotspot: [hot, hot] })
}

#[cfg(test)]
mod tests;
