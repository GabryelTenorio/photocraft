//! The licensed Pen tool SVG as an outlined cursor, with its nib as the hotspot.

use super::{CustomCursorImage, MAX_SIDE, Painter, Pos2, Shape, cached_image};
use std::sync::Arc;

pub(super) const SIZE: f32 = 20.0;
const NIB: f32 = 2.3 / 24.0;

#[derive(Clone)]
struct Texture {
    rgba: Arc<[u8]>,
    handle: egui::TextureHandle,
}

pub(super) fn rasterize(scale: f32) -> Option<CustomCursorImage> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let size = (SIZE * scale).ceil();
    // Leave a transparent physical pixel around the SVG; validate before decoding or allocating.
    if !size.is_finite() || size < 1.0 || size > f32::from(MAX_SIDE - 2) {
        return None;
    }
    let source_side = size as u16;
    let side = source_side.checked_add(2)?;
    let bytes = usize::from(side).checked_mul(usize::from(side))?.checked_mul(4)?;
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
    let [dark, light] = crate::theme::Tokens::cursor_outline();
    let mut rgba = vec![0; bytes];
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
    let hot = 1 + (NIB * size).round() as u16;
    Some(CustomCursorImage { rgba: Arc::from(rgba), size: [side, side], hotspot: [hot, hot] })
}

/// Immediate viewports and web paint the same cached bitmap when they cannot upload an OS cursor.
pub(super) fn paint(painter: &Painter, at: Pos2) -> bool {
    if !at.x.is_finite() || !at.y.is_finite() {
        return false;
    }
    let ctx = painter.ctx();
    let scale = ctx.pixels_per_point();
    let Some(image) = cached_image(ctx, Shape::Pen, scale) else { return false };
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
    let min = at - egui::vec2(f32::from(image.hotspot[0]), f32::from(image.hotspot[1])) / scale;
    let rect = egui::Rect::from_min_size(min, egui::vec2(f32::from(image.size[0]), f32::from(image.size[1])) / scale);
    painter.image(texture.handle.id(), rect, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    true
}
