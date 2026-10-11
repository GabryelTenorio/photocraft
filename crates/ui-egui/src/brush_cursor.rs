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

/// The brush cursor at `at`. `radius` is the tip's screen radius (size / 2 at the current zoom);
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
    crate::tool_cursor::ellipse(painter, at, Vec2::new(r * xf.aspect, r), xf.rotation.to_radians() * if xf.flip { -1.0 } else { 1.0 }, centre)
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
