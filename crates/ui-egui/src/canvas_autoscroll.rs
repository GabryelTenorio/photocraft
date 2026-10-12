//! Auto-scroll while a tool drag runs past the canvas edge (#4183). As in Photoshop, dragging a
//! marquee, lasso, brush stroke or crop off the visible part of a zoomed image scrolls the view
//! toward the pointer, so a selection can be larger than what is on screen. The view stops at the
//! scroll limits (it never auto-scrolls an image that already fits into empty pasteboard).

use crate::state::View;
use egui::{Pos2, Rect, Vec2, vec2};

/// Slowest and fastest auto-scroll, in egui points per second.
const MIN_SPEED: f32 = 120.0;
const MAX_SPEED: f32 = 2400.0;
/// Extra speed per point the pointer is past the edge.
const GAIN: f32 = 12.0;
/// A long frame moves at most this many seconds' worth, so a stall doesn't jump the view.
const MAX_DT: f32 = 0.05;

/// How far, in screen points, to scroll this frame for a drag whose pointer is at `pointer` over a
/// canvas of `rect`. Zero while the pointer is inside the canvas. Each axis points toward the
/// pointer and speeds up the further past the edge it is.
pub fn edge_step(pointer: Pos2, rect: Rect, dt: f32) -> Vec2 {
    if !pointer.x.is_finite() || !pointer.y.is_finite() || !dt.is_finite() || dt <= 0.0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return Vec2::ZERO;
    }
    let past = |p: f32, lo: f32, hi: f32| {
        if p < lo {
            p - lo
        } else if p > hi {
            p - hi
        } else {
            0.0
        }
    };
    let dt = dt.min(MAX_DT);
    let axis = |d: f32| if d == 0.0 { 0.0 } else { d.signum() * (MIN_SPEED + GAIN * d.abs()).min(MAX_SPEED) * dt };
    vec2(axis(past(pointer.x, rect.left(), rect.right())), axis(past(pointer.y, rect.top(), rect.bottom())))
}

/// Move `view.center` by `doc_step` (document pixels), never past the scroll limits in the
/// direction of travel. A view the user already panned past a limit (Overscroll on) is not pulled
/// back: it just doesn't scroll further out. `size` is the canvas in points, already divided by
/// the pixel aspect. True when the view moved.
pub fn scroll(view: &mut View, doc_step: Vec2, size: Vec2, ppp: f32) -> bool {
    if !doc_step.x.is_finite() || !doc_step.y.is_finite() || doc_step == Vec2::ZERO {
        return false;
    }
    // Only an axis where the image is larger than the view scrolls: one that fits stays put.
    let zoom = view.zoom / if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    let larger = |len: u32, size: f32| zoom.is_finite() && zoom > 0.0 && len as f32 * zoom > size;
    let doc_step = vec2(if larger(view.doc_size[0], size.x) { doc_step.x } else { 0.0 }, if larger(view.doc_size[1], size.y) { doc_step.y } else { 0.0 });
    if doc_step == Vec2::ZERO {
        return false;
    }
    let current = view.center;
    let mut limited = view.clone();
    limited.center = [current[0] + doc_step.x, current[1] + doc_step.y];
    let wanted = limited.center;
    crate::scrollbars::clamp_view(&mut limited, size, ppp);
    let axis = |cur: f32, want: f32, lim: f32| {
        if want > cur { want.min(cur.max(lim)) } else { want.max(cur.min(lim)) }
    };
    let next = [axis(current[0], wanted[0], limited.center[0]), axis(current[1], wanted[1], limited.center[1])];
    let moved = (next[0] - current[0]).abs() > 1e-3 || (next[1] - current[1]).abs() > 1e-3;
    if moved {
        view.center = next;
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn rect() -> Rect {
        Rect::from_min_max(pos2(100.0, 50.0), pos2(500.0, 350.0))
    }

    #[test]
    fn inside_the_canvas_does_not_scroll() {
        assert_eq!(edge_step(pos2(300.0, 200.0), rect(), 1.0 / 60.0), Vec2::ZERO);
        assert_eq!(edge_step(pos2(100.0, 350.0), rect(), 1.0 / 60.0), Vec2::ZERO, "the edge itself is inside");
    }

    #[test]
    fn past_an_edge_scrolls_toward_the_pointer() {
        let dt = 1.0 / 60.0;
        let right = edge_step(pos2(520.0, 200.0), rect(), dt);
        assert!(right.x > 0.0 && right.y == 0.0, "{right:?}");
        let left = edge_step(pos2(80.0, 200.0), rect(), dt);
        assert!(left.x < 0.0 && left.y == 0.0, "{left:?}");
        let corner = edge_step(pos2(90.0, 360.0), rect(), dt);
        assert!(corner.x < 0.0 && corner.y > 0.0, "{corner:?}");
    }

    #[test]
    fn further_out_is_faster_up_to_a_cap() {
        let dt = 1.0 / 60.0;
        let near = edge_step(pos2(505.0, 200.0), rect(), dt).x;
        let far = edge_step(pos2(600.0, 200.0), rect(), dt).x;
        let very_far = edge_step(pos2(50_000.0, 200.0), rect(), dt).x;
        assert!(near < far, "{near} < {far}");
        assert!((very_far - MAX_SPEED * dt).abs() < 1e-3, "capped: {very_far}");
    }

    #[test]
    fn hostile_input_does_not_scroll() {
        let dt = 1.0 / 60.0;
        assert_eq!(edge_step(pos2(f32::NAN, 200.0), rect(), dt), Vec2::ZERO);
        assert_eq!(edge_step(pos2(f32::INFINITY, 200.0), rect(), dt), Vec2::ZERO);
        assert_eq!(edge_step(pos2(600.0, 200.0), rect(), f32::NAN), Vec2::ZERO);
        assert_eq!(edge_step(pos2(600.0, 200.0), rect(), -1.0), Vec2::ZERO);
        assert_eq!(edge_step(pos2(600.0, 200.0), Rect::NOTHING, dt), Vec2::ZERO);
        // A multi-second stall moves no more than one capped frame.
        assert!(edge_step(pos2(600.0, 200.0), rect(), 10.0).x <= MAX_SPEED * MAX_DT + 1e-3);
    }

    fn view(zoom: f32, center: [f32; 2]) -> View {
        View { zoom, center, doc_size: [2000, 1000], ..Default::default() }
    }

    #[test]
    fn a_zoomed_view_scrolls_and_stops_at_the_limit() {
        // 2000 × 1000 at 100 % in a 400 × 300 canvas: the centre can range over 200..1800 in x.
        let size = vec2(400.0, 300.0);
        let mut v = view(1.0, [1000.0, 500.0]);
        assert!(scroll(&mut v, vec2(50.0, 0.0), size, 1.0));
        assert_eq!(v.center, [1050.0, 500.0]);
        let mut v = view(1.0, [1790.0, 500.0]);
        assert!(scroll(&mut v, vec2(50.0, 0.0), size, 1.0));
        assert_eq!(v.center, [1800.0, 500.0], "stops at the right limit");
        assert!(!scroll(&mut v, vec2(50.0, 0.0), size, 1.0), "and stays there");
    }

    #[test]
    fn an_image_that_fits_does_not_scroll() {
        let mut v = view(0.1, [1000.0, 500.0]);
        assert!(!scroll(&mut v, vec2(50.0, 50.0), vec2(400.0, 300.0), 1.0));
        assert_eq!(v.center, [1000.0, 500.0]);
        // Wider than the view but not taller: only x scrolls.
        let mut v = view(0.25, [1000.0, 500.0]);
        assert!(scroll(&mut v, vec2(50.0, 50.0), vec2(400.0, 300.0), 1.0));
        assert_eq!(v.center, [1050.0, 500.0]);
    }

    #[test]
    fn an_overscrolled_view_is_not_pulled_back() {
        // Already panned past the right limit (Overscroll on): scrolling further right does
        // nothing, scrolling left moves normally.
        let size = vec2(400.0, 300.0);
        let mut v = view(1.0, [1900.0, 500.0]);
        assert!(!scroll(&mut v, vec2(50.0, 0.0), size, 1.0));
        assert_eq!(v.center, [1900.0, 500.0]);
        assert!(scroll(&mut v, vec2(-50.0, 0.0), size, 1.0));
        assert_eq!(v.center, [1850.0, 500.0]);
    }

    #[test]
    fn non_finite_steps_are_ignored() {
        let mut v = view(1.0, [1000.0, 500.0]);
        assert!(!scroll(&mut v, vec2(f32::NAN, 0.0), vec2(400.0, 300.0), 1.0));
        assert_eq!(v.center, [1000.0, 500.0]);
    }
}
