//! The brush cursor for any tip, as Photoshop draws it: the outline of the tip itself, so an
//! elliptical (Roundness), rotated (Angle), flipped or sampled tip shows its own shape rather than a
//! circle. The outline is the tip's contour at a coverage threshold, traced from
//! [`crate::brush_preview::tip_alpha`] with marching squares and cached per tip setting; the canvas
//! maps it through the view each frame, so the view's rotation and flips apply too.

use std::sync::Arc;

use photocraft_paint::BrushSettings;

/// One outline segment, in tip units: the cell centre is the origin and 1 is the brush radius
/// (half its Size), x right and y down, before the view transform.
pub(crate) type Segment = [[f32; 2]; 2];

/// Cell size the contour is traced at: fine enough for a smooth outline at large sizes, small
/// enough to trace in a few milliseconds once per tip change.
const CELL: u32 = 160;

/// Does `brush` need its tip's outline rather than the plain circle? A round tip at full roundness
/// is a circle whatever its angle or flips.
pub(crate) fn needs_outline(brush: &BrushSettings) -> bool {
    brush.tip.bitmap().is_some() || brush.roundness < 0.995
}

/// The tip's outline in tip units, at the Normal Brush Tip threshold (half coverage) or, with
/// `full`, at the very edge (Full Size Brush Tip). Cached on `ctx` per tip setting.
pub(crate) fn unit_outline(ctx: &egui::Context, brush: &BrushSettings, full: bool) -> Arc<Vec<Segment>> {
    let sig = crate::brush_preview::tip_sig(&brush.tip, brush.hardness, brush.angle, brush.roundness, (brush.flip_x, brush.flip_y)) ^ u64::from(full);
    let id = egui::Id::new("photocraft-tip-outline");
    if let Some((cached, segs)) = ctx.data(|d| d.get_temp::<(u64, Arc<Vec<Segment>>)>(id))
        && cached == sig
    {
        return segs;
    }
    let alpha = crate::brush_preview::tip_alpha(&brush.tip, brush.hardness, brush.angle, brush.roundness, (brush.flip_x, brush.flip_y), CELL);
    let segs = Arc::new(contour(&alpha, CELL, if full { 0.02 } else { 0.5 }));
    ctx.data_mut(|d| d.insert_temp(id, (sig, segs.clone())));
    segs
}

/// Marching squares over the pixel centres of an `n × n` coverage cell: the segments where the
/// coverage crosses `threshold`, linearly interpolated, in tip units (see [`Segment`]). The cell
/// border counts as empty, so a tip that fills its cell still closes.
pub(crate) fn contour(alpha: &[f32], n: u32, threshold: f32) -> Vec<Segment> {
    let n = n as i64;
    let at = |x: i64, y: i64| -> f32 {
        if x < 0 || y < 0 || x >= n || y >= n {
            return 0.0;
        }
        alpha.get((y * n + x) as usize).copied().unwrap_or(0.0)
    };
    // `tip_alpha` puts the tip's radius at n / 2 - 1 pixels from the cell centre.
    let (c, r) = (n as f32 / 2.0, (n as f32 / 2.0 - 1.0).max(1.0));
    let unit = |x: f32, y: f32| [(x + 0.5 - c) / r, (y + 0.5 - c) / r];
    // The point on the edge from grid point a (value va) to b (value vb) where the threshold falls.
    let cross = |a: (f32, f32), va: f32, b: (f32, f32), vb: f32| {
        let t = if (vb - va).abs() > f32::EPSILON { ((threshold - va) / (vb - va)).clamp(0.0, 1.0) } else { 0.5 };
        unit(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    };
    let mut out = Vec::new();
    for y in -1..n {
        for x in -1..n {
            let p = [(x as f32, y as f32), (x as f32 + 1.0, y as f32), (x as f32 + 1.0, y as f32 + 1.0), (x as f32, y as f32 + 1.0)];
            let v = [at(x, y), at(x + 1, y), at(x + 1, y + 1), at(x, y + 1)];
            let case = v.iter().enumerate().fold(0u8, |m, (i, val)| if *val >= threshold { m | 1 << i } else { m });
            if case == 0 || case == 15 {
                continue;
            }
            // The crossing on each edge of the square: top, right, bottom, left.
            let e = |k: usize| {
                let (i, j) = (k, (k + 1) % 4);
                cross(p[i], v[i], p[j], v[j])
            };
            let pairs: &[(usize, usize)] = match case {
                1 | 14 => &[(3, 0)],
                2 | 13 => &[(0, 1)],
                3 | 12 => &[(3, 1)],
                4 | 11 => &[(1, 2)],
                6 | 9 => &[(0, 2)],
                7 | 8 => &[(3, 2)],
                // Saddles: split by the centre's value.
                5 => {
                    if v.iter().sum::<f32>() / 4.0 >= threshold {
                        &[(3, 2), (0, 1)]
                    } else {
                        &[(3, 0), (1, 2)]
                    }
                }
                10 => {
                    if v.iter().sum::<f32>() / 4.0 >= threshold {
                        &[(3, 0), (1, 2)]
                    } else {
                        &[(3, 2), (0, 1)]
                    }
                }
                _ => &[],
            };
            for &(a, b) in pairs {
                out.push([e(a), e(b)]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_paint::TipShape;

    fn extent(segs: &[Segment], axis: usize) -> f32 {
        segs.iter().flat_map(|s| s.iter()).map(|p| p[axis].abs()).fold(0.0, f32::max)
    }

    fn outline(tip: &TipShape, angle: f32, roundness: f32) -> Vec<Segment> {
        let alpha = crate::brush_preview::tip_alpha(tip, 1.0, angle, roundness, (false, false), CELL);
        contour(&alpha, CELL, 0.5)
    }

    #[test]
    fn a_round_tip_traces_a_unit_circle() {
        let segs = outline(&TipShape::Round, 0.0, 1.0);
        assert!(segs.len() > 100);
        for p in segs.iter().flat_map(|s| s.iter()) {
            let r = p[0].hypot(p[1]);
            assert!((r - 1.0).abs() < 0.03, "{p:?} at {r}");
        }
    }

    #[test]
    fn roundness_squashes_and_angle_turns_the_outline() {
        // 50 % roundness at 0°: as wide as the brush, half as tall.
        let flat = outline(&TipShape::Round, 0.0, 0.5);
        assert!((extent(&flat, 0) - 1.0).abs() < 0.03 && (extent(&flat, 1) - 0.5).abs() < 0.03, "{} × {}", extent(&flat, 0), extent(&flat, 1));
        // Turned 90°: tall and narrow.
        let tall = outline(&TipShape::Round, 90.0, 0.5);
        assert!((extent(&tall, 0) - 0.5).abs() < 0.03 && (extent(&tall, 1) - 1.0).abs() < 0.03);
        // 45° anticlockwise: the long axis runs from bottom-left to top-right on screen (y down).
        let diagonal = outline(&TipShape::Round, 45.0, 0.3);
        let far = diagonal.iter().flat_map(|s| s.iter()).max_by(|a, b| a[0].hypot(a[1]).total_cmp(&b[0].hypot(b[1]))).unwrap();
        assert!(far[0] * far[1] < 0.0, "{far:?}");
    }

    #[test]
    fn a_sampled_tip_traces_its_own_shape() {
        // A bar: wide and low, filling the middle third vertically.
        let tip = TipShape::Sampled(photocraft_paint::GrayTile::from_fn(30, 30, |_, y| if (10..20).contains(&y) { 1.0 } else { 0.0 }));
        let segs = outline(&tip, 0.0, 1.0);
        assert!(extent(&segs, 0) > 0.9, "full width");
        assert!(extent(&segs, 1) < 0.4, "a third of the height");
    }

    #[test]
    fn only_tips_that_are_not_circles_need_the_outline() {
        let mut b = BrushSettings::default();
        assert!(!needs_outline(&b), "the plain round tip keeps the circle");
        b.angle = 30.0;
        assert!(!needs_outline(&b), "a circle turned is still a circle");
        b.roundness = 0.5;
        assert!(needs_outline(&b));
        let s = BrushSettings { tip: TipShape::Sampled(photocraft_paint::GrayTile::from_fn(8, 8, |_, _| 1.0)), ..Default::default() };
        assert!(needs_outline(&s));
    }

    /// The cursor bitmap of a 50 % round tip, 40 px across its long axis: the outline crosses the
    /// long axis 40 px out and the short one 20 px out, and the middle stays clear.
    #[test]
    fn the_bitmap_draws_the_ellipse() {
        let segs = outline(&TipShape::Round, 0.0, 0.5);
        let screen: Vec<[egui::Vec2; 2]> = segs.iter().map(|s| s.map(|q| egui::vec2(q[0], q[1]) * 40.0)).collect();
        let image = crate::tool_cursor::rasterize_outline(&screen, false, 1.0).unwrap();
        let [hx, hy] = image.hotspot;
        let alpha = |dx: i32, dy: i32| {
            let (x, y) = ((i32::from(hx) + dx) as usize, (i32::from(hy) + dy) as usize);
            image.rgba[(y * usize::from(image.size[0]) + x) * 4 + 3]
        };
        assert!(alpha(40, 0) > 128 && alpha(-40, 0) > 128, "the long axis ends 40 px out");
        assert!(alpha(0, 20) > 128 && alpha(0, -20) > 128, "the short axis ends 20 px out");
        assert_eq!(alpha(0, 40), 0, "nothing at 40 px on the short axis");
        assert_eq!(alpha(0, 0), 0, "the middle stays clear");
        assert!(crate::tool_cursor::rasterize_outline(&screen, false, f32::NAN).is_none());
    }

    /// The outline follows the view: zoom scales it and a rotated view turns it.
    #[test]
    fn the_outline_turns_with_the_view() {
        let segs = outline(&TipShape::Round, 0.0, 0.5);
        let xf = |rotation: f32| crate::canvas::ViewXform {
            rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0)),
            zoom: 2.0,
            center: [100.0, 100.0],
            flip: false,
            rotation,
            aspect: 1.0,
        };
        let reach =
            |v: &[[egui::Vec2; 2]], axis: usize| v.iter().flat_map(|s| s.iter()).map(|p| if axis == 0 { p.x.abs() } else { p.y.abs() }).fold(0.0, f32::max);
        let at = egui::pos2(400.0, 300.0);
        let flat = crate::canvas::brush_outline_on_screen(&xf(0.0), at, 20.0, &segs);
        assert!((reach(&flat, 0) - 20.0).abs() < 1.0 && (reach(&flat, 1) - 10.0).abs() < 1.0, "10 px radius at 200 %");
        let turned = crate::canvas::brush_outline_on_screen(&xf(90.0), at, 20.0, &segs);
        assert!((reach(&turned, 0) - 10.0).abs() < 1.0 && (reach(&turned, 1) - 20.0).abs() < 1.0, "a quarter turn of the view");
    }

    #[test]
    fn hostile_inputs_trace_nothing_or_stay_bounded() {
        assert!(contour(&[], CELL, 0.5).is_empty());
        let full = vec![1.0; (8 * 8) as usize];
        let segs = contour(&full, 8, 0.5);
        assert!(!segs.is_empty() && extent(&segs, 0) <= 1.5, "a full cell closes at its border");
        let nan = vec![f32::NAN; 64];
        assert!(contour(&nan, 8, 0.5).is_empty());
    }
}
