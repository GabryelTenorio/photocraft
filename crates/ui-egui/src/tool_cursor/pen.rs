//! The licensed Pen tool SVG and original point-editing marks, with a stable hotspot.

use super::{CustomCursorImage, MAX_SIDE, Painter, PenCursor, Pos2, Shape, cached_image, stroke_coverage};
use std::sync::Arc;

pub(super) const SIZE: f32 = 20.0;
const NIB: f32 = 2.3 / 24.0;

#[derive(Clone)]
struct Texture {
    rgba: Arc<[u8]>,
    handle: egui::TextureHandle,
}

pub(super) fn rasterize(scale: f32, state: PenCursor) -> Option<CustomCursorImage> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let size = (SIZE * scale).ceil();
    let canvas_size = (30.0 * scale).ceil();
    // Leave a transparent physical pixel around the SVG; validate before decoding or allocating.
    if !canvas_size.is_finite() || size < 1.0 || canvas_size > f32::from(MAX_SIDE - 2) {
        return None;
    }
    let source_side = size as u16;
    // Reserve the same badge space in every state, so changing state cannot shift the nib.
    let side = (canvas_size as u16).checked_add(2)?;
    let bytes = usize::from(side).checked_mul(usize::from(side))?.checked_mul(4)?;
    let hot = 1 + (NIB * size).round() as u16;
    let [dark, light] = crate::theme::Tokens::cursor_outline();
    let mut rgba = vec![0; bytes];
    if state != PenCursor::Convert {
        let source = std::str::from_utf8(include_bytes!("../../../../assets/icons/pen-tool.svg")).ok()?;
        let load = |width: &str| {
            let svg = source.replace("currentColor", "#ffffff").replace("stroke-width=\"2\"", width);
            egui_extras::image::load_svg_bytes_with_size(
                svg.as_bytes(),
                egui::load::SizeHint::Size { width: u32::from(source_side), height: u32::from(source_side), maintain_aspect_ratio: true },
                &Default::default(),
            )
            .ok()
        };
        let dark_mask = load("stroke-width=\"3.75\"")?;
        let light_mask = load("stroke-width=\"1.75\"")?;
        let expected_size = [usize::from(source_side); 2];
        let pixels = expected_size[0].checked_mul(expected_size[1])?;
        if dark_mask.size != expected_size || light_mask.size != expected_size || dark_mask.pixels.len() != pixels || light_mask.pixels.len() != pixels {
            return None;
        }
        for (i, (dark_pixel, light_pixel)) in dark_mask.pixels.iter().zip(&light_mask.pixels).enumerate() {
            let a_dark = f32::from(dark_pixel.a()) * f32::from(dark.a()) / (255.0 * 255.0);
            let a_light = f32::from(light_pixel.a()) * f32::from(light.a()) / (255.0 * 255.0);
            let alpha = a_light + a_dark * (1.0 - a_light);
            let white = if alpha > 0.0 { (255.0 * a_light / alpha).round() as u8 } else { 0 };
            let row = i / expected_size[0] + 1;
            let col = i % expected_size[0] + 1;
            let offset = row.checked_mul(usize::from(side))?.checked_add(col)?.checked_mul(4)?;
            rgba.get_mut(offset..offset.checked_add(4)?)?.copy_from_slice(&[white, white, white, (255.0 * alpha).round() as u8]);
        }
    }
    if state != PenCursor::Draw {
        for row in 0..side {
            for col in 0..side {
                let x = (f32::from(col) - 1.0) / scale - 24.0;
                let y = (f32::from(row) - 1.0) / scale - 5.0;
                let distance = match state {
                    PenCursor::New => star_distance(x, y),
                    PenCursor::Close => (x.hypot(y) - 2.3).abs(),
                    PenCursor::Add => super::cross_distance(x.abs(), y.abs(), 2.8, 0.0),
                    PenCursor::Delete => (x - x.clamp(-2.8, 2.8)).hypot(y),
                    PenCursor::Convert => corner_distance((f32::from(col) - f32::from(hot)) / scale, (f32::from(row) - f32::from(hot)) / scale),
                    PenCursor::Draw => continue,
                };
                let (fill, outline) = stroke_coverage(distance * scale, scale);
                let a_dark = outline * f32::from(dark.a()) / 255.0;
                let a_light = fill * f32::from(light.a()) / 255.0;
                let badge_alpha = a_light + a_dark * (1.0 - a_light);
                let offset = usize::from(row).checked_mul(usize::from(side))?.checked_add(usize::from(col))?.checked_mul(4)?;
                let pixel = rgba.get_mut(offset..offset.checked_add(4)?)?;
                let base_alpha = f32::from(*pixel.get(3)?) / 255.0;
                let base_white = f32::from(*pixel.first()?) / 255.0 * base_alpha;
                let alpha = badge_alpha + base_alpha * (1.0 - badge_alpha);
                let white = if alpha > 0.0 { (255.0 * (a_light + base_white * (1.0 - badge_alpha)) / alpha).round() as u8 } else { 0 };
                pixel.copy_from_slice(&[white, white, white, (255.0 * alpha).round() as u8]);
            }
        }
    }
    Some(CustomCursorImage { rgba: Arc::from(rgba), size: [side, side], hotspot: [hot, hot] })
}

/// Two slanted strokes meet at the hotspot: an original Convert Point corner, without a nib.
fn corner_distance(x: f32, y: f32) -> f32 {
    let mut distance = f32::MAX;
    for [dx, dy] in [[6.0, 15.0], [15.0, 6.0]] {
        let along = ((x * dx + y * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        distance = distance.min((x - along * dx).hypot(y - along * dy));
    }
    distance
}

/// Three crossed strokes form the new-path asterisk, drawn from original primitives.
fn star_distance(x: f32, y: f32) -> f32 {
    let mut distance = f32::MAX;
    for [dx, dy] in [[0.0, 1.0], [0.866_025_4, 0.5], [0.866_025_4, -0.5]] {
        let along = (x * dx + y * dy).clamp(-2.8, 2.8);
        distance = distance.min((x - along * dx).hypot(y - along * dy));
    }
    distance
}

/// Immediate viewports and web paint the same cached bitmap when they cannot upload an OS cursor.
pub(super) fn paint(painter: &Painter, at: Pos2, state: PenCursor) -> bool {
    if !at.x.is_finite() || !at.y.is_finite() {
        return false;
    }
    let ctx = painter.ctx();
    let scale = ctx.pixels_per_point();
    let Some(image) = cached_image(ctx, &Shape::Pen(state), scale) else { return false };
    let min = at - egui::vec2(f32::from(image.hotspot[0]), f32::from(image.hotspot[1])) / scale;
    let rect = egui::Rect::from_min_size(min, egui::vec2(f32::from(image.size[0]), f32::from(image.size[1])) / scale);
    if !rect.is_finite() {
        return false;
    }
    let id = egui::Id::new("photocraft-pen-cursor-texture");
    let texture = ctx.data(|d| d.get_temp::<Texture>(id)).filter(|t| Arc::ptr_eq(&t.rgba, &image.rgba)).unwrap_or_else(|| {
        let texture = Texture {
            rgba: image.rgba.clone(),
            handle: ctx.load_texture(
                "PhotoCraft Pen cursor",
                egui::ColorImage::from_rgba_unmultiplied(image.size.map(usize::from), &image.rgba),
                egui::TextureOptions::LINEAR,
            ),
        };
        ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
        texture
    });
    painter.image(texture.handle.id(), rect, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    true
}
