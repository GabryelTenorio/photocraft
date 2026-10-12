//! The painting cursor for brushes with a tip shape: sampled tip outlines and rotated or
//! flattened round tips. Everything else falls back to the plain circle (`tool_cursor`).

use std::sync::Arc;

use egui::{CursorIcon, Painter, Pos2, Vec2};
use photocraft_paint::brush::{BrushSettings, TipShape};

use crate::canvas::ViewXform;

/// Below this on-screen radius an outline is unreadable: show the crosshair instead.
const MIN_RADIUS: f32 = 3.0;

#[derive(Clone)]
struct Cached {
    key: Key,
    rings: Option<Arc<Vec<Vec<[f32; 2]>>>>,
}

/// The brush fields an outline depends on.
#[derive(Clone)]
struct Key {
    tip: TipShape,
    angle: f32,
    roundness: f32,
    hardness: f32,
    flip_x: bool,
    flip_y: bool,
    full: bool,
}

impl Key {
    fn of(b: &BrushSettings, full: bool) -> Self {
        Self { tip: b.tip.clone(), angle: b.angle, roundness: b.roundness, hardness: b.hardness, flip_x: b.flip_x, flip_y: b.flip_y, full }
    }
    fn matches(&self, b: &BrushSettings, full: bool) -> bool {
        self.tip == b.tip
            && self.angle == b.angle
            && self.roundness == b.roundness
            && self.hardness == b.hardness
            && self.flip_x == b.flip_x
            && self.flip_y == b.flip_y
            && self.full == full
    }
}

/// The brush cursor at `at`. `radius` is size / 2 at the view's logical zoom, before aspect;
/// `normal` is Preferences › Cursors "Normal Brush Tip" (the 50 % contour, not the full tip).
pub(crate) fn cursor(painter: &Painter, at: Pos2, brush: &BrushSettings, radius: f32, xf: &ViewXform, normal: bool, centre: bool) -> CursorIcon {
    if let Some(rings) = rings(painter.ctx(), brush, !normal) {
        let pts = mapped_rings(&rings, radius, xf);
        let extent = pts.iter().flatten().fold(0.0f32, |m, p| m.max(p.length()));
        if extent < MIN_RADIUS {
            return crate::tool_cursor::crosshair(painter, at, 4.0, 0.0);
        }
        return crate::tool_cursor::outline(painter, at, pts, centre);
    }
    let r = if normal { photocraft_paint::half_coverage_radius(radius, brush.hardness).max(1.0) } else { radius };
    if (xf.aspect - 1.0).abs() < 1e-6 {
        crate::tool_cursor::circle(painter, at, r, centre)
    } else {
        crate::tool_cursor::ellipse(painter, at, Vec2::new(r * xf.aspect, r), xf.rotation.to_radians() * if xf.flip { -1.0 } else { 1.0 }, centre)
    }
}

/// The cached contour stays in native radius units; only its presentation follows the view.
fn mapped_rings(rings: &[Vec<[f32; 2]>], radius: f32, xf: &ViewXform) -> Vec<Vec<Vec2>> {
    let x = xf.map_vec(Vec2::new(radius, 0.0));
    let y = xf.map_vec(Vec2::new(0.0, radius));
    rings.iter().map(|r| r.iter().map(|p| x * p[0] + y * p[1]).collect()).collect()
}

/// The tip outline in radius units, cached across frames; `None` when the plain circle says it all
/// (a plain round tip) or the tip has no traceable shape.
fn rings(ctx: &egui::Context, brush: &BrushSettings, full: bool) -> Option<Arc<Vec<Vec<[f32; 2]>>>> {
    if matches!(brush.tip, TipShape::Round) && brush.angle == 0.0 && brush.roundness >= 1.0 {
        return None;
    }
    let id = egui::Id::new("photocraft-tip-outline");
    if let Some(c) = ctx.data(|d| d.get_temp::<Cached>(id))
        && c.key.matches(brush, full)
    {
        return c.rings;
    }
    let traced = photocraft_paint::outline::tip_outline(brush, full).map(Arc::new);
    ctx.data_mut(|d| d.insert_temp(id, Cached { key: Key::of(brush, full), rings: traced.clone() }));
    traced
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, pos2, vec2};
    use photocraft_paint::tile::GrayTile;

    fn tips() -> [BrushSettings; 2] {
        let sampled = GrayTile::from_fn(11, 7, |x, y| if (x < 4 && y < 6) || (y > 3 && x < 10) { 1.0 } else { 0.0 });
        [
            BrushSettings { size: 80.0, angle: 39.0, roundness: 0.35, hardness: 0.8, ..Default::default() },
            BrushSettings { size: 80.0, tip: TipShape::Sampled(sampled), angle: 31.0, roundness: 0.65, flip_x: true, ..Default::default() },
        ]
    }

    fn view(aspect: f32, ppp: f32, zoom: f32, rotation: f32, flip: bool) -> ViewXform {
        ViewXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), zoom: zoom / ppp, center: [20.0, 30.0], aspect, rotation, flip }
    }

    #[test]
    fn shaped_tip_contours_follow_native_view_projection_and_reuse_cache() {
        let ctx = egui::Context::default();
        for brush in tips() {
            for full in [false, true] {
                let native = rings(&ctx, &brush, full).unwrap();
                for aspect in [0.1, 1.0, 10.0] {
                    for ppp in [1.0, 2.0] {
                        for zoom in [0.5, 3.0] {
                            for rotation in [0.0, 37.0] {
                                for flip in [false, true] {
                                    let xf = view(aspect, ppp, zoom, rotation, flip);
                                    let mapped = mapped_rings(&native, 40.0 * xf.zoom, &xf);
                                    let hotspot = xf.to_screen(20.0, 30.0);
                                    assert_eq!(mapped.len(), native.len());
                                    for (native, mapped) in native.iter().zip(mapped) {
                                        assert_eq!(mapped.len(), native.len());
                                        for (p, got) in native.iter().zip(mapped) {
                                            let expected = xf.to_screen(20.0 + p[0] * 40.0, 30.0 + p[1] * 40.0) - hotspot;
                                            assert!((got - expected).length() < 0.002, "{xf:?}: {got:?} vs {expected:?}");
                                        }
                                    }
                                    assert!(Arc::ptr_eq(&native, &rings(&ctx, &brush, full).unwrap()), "view changes keep the native contour cache");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn square_unrotated_shaped_contours_keep_upstream_offsets() {
        let ctx = egui::Context::default();
        for brush in tips() {
            let native = rings(&ctx, &brush, true).unwrap();
            let mapped = mapped_rings(&native, 40.0, &view(1.0, 1.0, 1.0, 0.0, false));
            for (native, mapped) in native.iter().zip(mapped) {
                for (p, got) in native.iter().zip(mapped) {
                    assert_eq!(got, Vec2::new(p[0], p[1]) * 40.0);
                }
            }
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_cursor_bitmaps_contain_projected_tip_nodes() {
        for (tip_index, brush) in tips().iter().enumerate() {
            for aspect in [0.1, 1.0, 10.0] {
                for ppp in [1.0, 2.0] {
                    for rotation in [0.0, 37.0] {
                        for flip in [false, true] {
                            let ctx = egui::Context::default();
                            let xf = view(aspect, ppp, 1.0, rotation, flip);
                            let mut input = egui::RawInput::default();
                            input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().native_pixels_per_point = Some(ppp);
                            let mut output = ctx.run_ui(input, |ui| {
                                let icon = cursor(ui.painter(), pos2(200.0, 200.0), brush, 8.0 * xf.zoom, &xf, false, false);
                                ui.ctx().set_cursor_icon(icon);
                            });
                            output.textures_delta.clear();
                            let bitmap = output.platform_output.cursor_image.unwrap();
                            let native = rings(&ctx, brush, true).unwrap();
                            let mapped = mapped_rings(&native, 8.0 * xf.zoom, &xf);
                            // The existing small-tip crosshair fallback is intentional.
                            if mapped.iter().flatten().any(|p| p.length() >= MIN_RADIUS) {
                                let scale = crate::tool_cursor::native_scale(&ctx);
                                let [w, h] = bitmap.size.map(i32::from);
                                for p in mapped.iter().flatten() {
                                    let x = (f32::from(bitmap.hotspot[0]) + p.x * scale).round() as i32;
                                    let y = (f32::from(bitmap.hotspot[1]) + p.y * scale).round() as i32;
                                    let painted = (y - 2..=y + 2).any(|dy| {
                                        (x - 2..=x + 2).any(|dx| dx >= 0 && dx < w && dy >= 0 && dy < h && bitmap.rgba[((dy * w + dx) * 4 + 3) as usize] > 0)
                                    });
                                    assert!(painted, "tip={tip_index}, {xf:?}, node={p:?}, bitmap={bitmap:?}");
                                }
                            }
                            if let Ok(dir) = std::env::var("PHOTOCRAFT_VIEW_CURSOR_CAPTURE_DIR") {
                                std::fs::create_dir_all(&dir).unwrap();
                                let image = photocraft_codecs::Image::from_u8(
                                    u32::from(bitmap.size[0]),
                                    u32::from(bitmap.size[1]),
                                    photocraft_codecs::ChannelLayout::Rgba,
                                    bitmap.rgba.to_vec(),
                                )
                                .unwrap();
                                let png = photocraft_codecs::encode(&image, photocraft_codecs::Format::Png, &Default::default()).unwrap();
                                std::fs::write(
                                    std::path::Path::new(&dir).join(format!("tip{tip_index}-par{aspect}-dpi{ppp}-rot{rotation}-flip{flip}.png")),
                                    png,
                                )
                                .unwrap();
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn oversized_shaped_tips_paint_the_projected_outline_and_keep_native_hotspot() {
        for brush in tips() {
            for ppp in [1.0, 2.0] {
                let ctx = egui::Context::default();
                let xf = view(10.0, ppp, 16.0, 37.0, true);
                let at = pos2(200.0, 200.0);
                let radius = 40.0 * xf.zoom;
                let mut input = egui::RawInput::default();
                input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().native_pixels_per_point = Some(ppp);
                let mut output = ctx.run_ui(input.clone(), |ui| {
                    let icon = cursor(ui.painter(), at, &brush, radius, &xf, false, false);
                    ui.ctx().set_cursor_icon(icon);
                });
                output.textures_delta.clear();
                let hotspot = output.platform_output.cursor_image.unwrap();
                assert!(hotspot.size[0] <= 64, "oversized tips keep a small native hotspot");
                let native = rings(&ctx, &brush, true).unwrap();
                let mapped = mapped_rings(&native, radius, &xf);
                for expected in mapped.iter().flatten().map(|p| at + *p) {
                    assert!(output.shapes.iter().any(|s| {
                        matches!(&s.shape, egui::Shape::Path(path) if path.stroke.width > 0.0 && path.points.iter().any(|p| (*p - expected).length() < 0.001))
                    }), "the full projected ring remains in the painted draw list: {expected:?}");
                }
                let mut cross = ctx.run_ui(input, |ui| {
                    let icon = crate::tool_cursor::crosshair(ui.painter(), at, 6.0, 2.0);
                    ui.ctx().set_cursor_icon(icon);
                });
                cross.textures_delta.clear();
                assert_eq!(hotspot.rgba, cross.platform_output.cursor_image.unwrap().rgba, "the native hotspot is the existing fallback crosshair");
            }
        }
    }
}
