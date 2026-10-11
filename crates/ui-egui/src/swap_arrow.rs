//! The Switch Colors arrow in the toolbar's colour-chip block: a bold diagonal curve from the
//! icon's top-right to its bottom-left with a filled head at each end, drawn in the 13x13 icon
//! box. The geometry is measured from the reference glyph, and it lives here so its proportions
//! are unit-testable.

use egui::{pos2, vec2, Color32, Painter, Pos2, Shape, Stroke, Vec2};

/// The curve's endpoints on the 13x13 icon grid: it sweeps the top-right to bottom-left
/// diagonal (the opposite corner pair from the old quarter arc), like the reference glyph.
pub const START: Pos2 = pos2(10.2, 1.8);
pub const END: Pos2 = pos2(2.2, 11.0);
/// The quadratic Bezier control point: the curve leaves the start shallower than the diagonal
/// and arrives at the end steeper, so the heads tilt apart like the reference glyph's.
pub const CONTROL: Pos2 = pos2(5.5, 4.7);
/// Stroke width, from the reference glyph's bold band at this scale.
pub const WIDTH: f32 = 2.5;
/// Filled arrowhead size: length along the curve, half-width across it.
pub const HEAD_LEN: f32 = 2.2;
pub const HEAD_HALF: f32 = 1.9;

/// The curve's tangent at the start (false) or the end (true), pointing into the curve.
fn tangent(at_end: bool) -> Vec2 {
    if at_end {
        vec2(END.x - CONTROL.x, END.y - CONTROL.y)
    } else {
        vec2(CONTROL.x - START.x, CONTROL.y - START.y)
    }
}

/// Points sampled along the curve, `steps` segments.
pub fn curve_points(steps: usize) -> Vec<Pos2> {
    let n = steps.max(1);
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let (a, b) = (1.0 - t, t);
            pos2(
                a * a * START.x + 2.0 * a * b * CONTROL.x + b * b * END.x,
                a * a * START.y + 2.0 * a * b * CONTROL.y + b * b * END.y,
            )
        })
        .collect()
}

/// The filled head at `pt` pointing outward along the unit vector `out`.
fn head_triangle(pt: Pos2, out: Vec2) -> [Pos2; 3] {
    let tip = pt + out * HEAD_LEN;
    let side = vec2(-out.y, out.x);
    [tip, pt + side * HEAD_HALF, pt - side * HEAD_HALF]
}

/// Paint the glyph with its top-left at `o`, in ink colour `ink`.
pub fn paint(p: &Painter, o: Pos2, ink: Color32) {
    let stroke = Stroke::new(WIDTH, ink);
    let curve = curve_points(10).iter().map(|q| o + q.to_vec2()).collect::<Vec<_>>();
    p.add(Shape::line(curve, stroke));
    for (pt, out) in [(START, -tangent(false).normalized()), (END, tangent(true).normalized())] {
        let head = head_triangle(pt, out).iter().map(|q| o + q.to_vec2()).collect::<Vec<_>>();
        p.add(Shape::convex_polygon(head, ink, Stroke::NONE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every part of the glyph stays inside the 13x13 icon box: the curve and both heads.
    #[test]
    fn the_glyph_stays_inside_the_13px_icon_box() {
        let heads = [head_triangle(START, -tangent(false).normalized()), head_triangle(END, tangent(true).normalized())];
        for q in curve_points(200).into_iter().chain(heads.iter().flatten().copied()) {
            assert!((0.0..=13.0).contains(&q.x), "x out of the icon box at {q:?}");
            assert!((0.0..=13.0).contains(&q.y), "y out of the icon box at {q:?}");
        }
    }

    /// The curve sweeps top-right to bottom-left and both heads point outward along it:
    /// the start's head up-right, the end's head down-left, as in the reference glyph.
    #[test]
    fn heads_point_outward_along_the_curve() {
        let (t0, t1) = (tangent(false).normalized(), tangent(true).normalized());
        assert!(t0.x < 0.0 && t0.y > 0.0, "the curve leaves the start down-left, got {t0:?}");
        assert!(t1.x < 0.0 && t1.y > 0.0, "the curve arrives at the end moving down-left, got {t1:?}");
        let (tip0, tip1) = (START - t0 * HEAD_LEN, END + t1 * HEAD_LEN);
        assert!(tip0.x > START.x && tip0.y < START.y, "the start head points up-right, got {tip0:?}");
        assert!(tip1.x < END.x && tip1.y > END.y, "the end head points down-left, got {tip1:?}");
    }
}