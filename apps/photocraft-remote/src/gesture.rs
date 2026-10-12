//! Finger gestures on the Remote Canvas page, platform-free so they test natively.
//!
//! The Pencil draws and works the app's controls; fingers navigate, as in Procreate:
//! - one-finger drag scrolls (pans the canvas, scrolls panels); one-finger tap clicks;
//! - two fingers pan, pinch-zoom and rotate the canvas (after a small dead zone, so a pinch
//!   doesn't wobble it); on lifting, the desktop snaps a rotation near a right angle to it;
//! - a two-finger tap undoes, a three-finger tap redoes;
//! - touch and hold one finger to pick a colour (the Eyedropper), drag to keep picking, lift to
//!   keep the colour.
//!
//! Palm rejection: touches that start while the Pencil is down, or shortly after it lifted, do
//! nothing for the rest of their gesture.

/// Positions and distances are in page (CSS) pixels; times in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    /// Move the content by `dx, dy` (positive: right/down), with the pointer at `x, y`.
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    /// Zoom by `factor` (> 1: in) about `x, y`.
    Zoom {
        x: f64,
        y: f64,
        factor: f64,
    },
    /// Rotate the canvas by `degrees` (clockwise positive) about `x, y`.
    Rotate {
        x: f64,
        y: f64,
        degrees: f64,
    },
    /// The fingers that rotated the canvas lifted at `x, y` (where a snap pivots).
    RotateEnd {
        x: f64,
        y: f64,
    },
    /// A click at `x, y`.
    Tap {
        x: f64,
        y: f64,
    },
    Undo,
    Redo,
    /// The held finger picks the colour at `x, y` (first when the hold is recognised, then as it
    /// moves).
    Pick {
        x: f64,
        y: f64,
    },
    /// The picking finger lifted at `x, y`.
    PickEnd {
        x: f64,
        y: f64,
    },
}

/// Movement under this (from where a finger went down) still counts as a tap.
const TAP_SLOP: f64 = 12.0;
/// A tap lasts at most this long.
const TAP_MS: f64 = 350.0;
/// Touches starting this soon after the Pencil lifted are taken for the palm.
const PALM_MS: f64 = 400.0;
/// Most fingers tracked at once.
const MAX_TOUCHES: usize = 10;
/// Two fingers must turn this far (degrees) before the canvas starts rotating.
const ROTATE_DEAD_ZONE: f64 = 8.0;
/// A single finger held still (within the tap slop) this long picks a colour.
pub const HOLD_MS: f64 = 450.0;

#[derive(Clone, Copy, Debug)]
struct Touch {
    id: i32,
    start: (f64, f64),
    last: (f64, f64),
}

#[derive(Debug, Default)]
pub struct Gestures {
    touches: Vec<Touch>,
    /// Most fingers down at once in the current gesture.
    most: usize,
    started_ms: f64,
    /// A finger moved past the tap slop: no tap at the end.
    moved: bool,
    /// The gesture began as a palm: ignore it until every finger lifts.
    palm: bool,
    /// Turn of the first two fingers since the gesture began, in degrees (before the dead zone
    /// is passed), and whether the canvas is rotating.
    turned: f64,
    rotating: bool,
    /// The fingers' centre at the last move (where a rotation snaps about).
    centre: (f64, f64),
    /// A held finger is picking a colour: the gesture is only that until every finger lifts.
    picking: bool,
    pen_down: bool,
    /// When the Pencil last lifted (`None`: not yet).
    pen_up_ms: Option<f64>,
}

impl Gestures {
    pub fn pen(&mut self, down: bool, now_ms: f64) {
        if self.pen_down && !down {
            self.pen_up_ms = Some(now_ms);
        }
        self.pen_down = down;
        // The Pencil touching mid-gesture: the fingers were a resting hand.
        if down && !self.touches.is_empty() {
            self.palm = true;
        }
    }

    pub fn down(&mut self, id: i32, x: f64, y: f64, now_ms: f64) -> Vec<Gesture> {
        if !x.is_finite() || !y.is_finite() || self.touches.len() >= MAX_TOUCHES {
            return Vec::new();
        }
        if self.touches.is_empty() {
            self.most = 0;
            self.moved = false;
            self.turned = 0.0;
            self.rotating = false;
            self.picking = false;
            self.started_ms = now_ms;
            self.palm = self.pen_down || self.pen_up_ms.is_some_and(|up| now_ms - up < PALM_MS);
        }
        self.touches.retain(|t| t.id != id);
        self.touches.push(Touch { id, start: (x, y), last: (x, y) });
        self.most = self.most.max(self.touches.len());
        Vec::new()
    }

    pub fn moved(&mut self, id: i32, x: f64, y: f64) -> Vec<Gesture> {
        if !x.is_finite() || !y.is_finite() {
            return Vec::new();
        }
        let before = self.centroid_and_spread();
        let angle_before = self.angle();
        let first = self.touches.first().map(|t| t.id);
        let Some(t) = self.touches.iter_mut().find(|t| t.id == id) else { return Vec::new() };
        t.last = (x, y);
        if self.picking {
            // Only the finger that held picks; others are ignored until all lift.
            return if first == Some(id) { vec![Gesture::Pick { x, y }] } else { Vec::new() };
        }
        if (x - t.start.0).hypot(y - t.start.1) > TAP_SLOP {
            self.moved = true;
        }
        // The fingers' turn counts from the first move, inside the tap slop too, so the canvas
        // ends up exactly as far round as the fingers.
        let turn = match (angle_before, self.angle()) {
            // Wrapped to (−180, 180]: crossing the ±180° line is a small turn, not a full one.
            (Some(a0), Some(a1)) => (a1 - a0 + 540.0).rem_euclid(360.0) - 180.0,
            _ => 0.0,
        };
        if !self.rotating {
            self.turned += turn;
        }
        if self.palm || !self.moved {
            return Vec::new();
        }
        let (Some((c0, s0)), Some((c1, s1))) = (before, self.centroid_and_spread()) else { return Vec::new() };
        let mut out = Vec::new();
        let (dx, dy) = (c1.0 - c0.0, c1.1 - c0.1);
        if dx != 0.0 || dy != 0.0 {
            out.push(Gesture::Scroll { x: c1.0, y: c1.1, dx, dy });
        }
        if self.touches.len() >= 2 && s0 > 1.0 && s1 > 1.0 && s1 != s0 {
            out.push(Gesture::Zoom { x: c1.0, y: c1.1, factor: s1 / s0 });
        }
        self.centre = c1;
        if self.rotating {
            if turn != 0.0 {
                out.push(Gesture::Rotate { x: c1.0, y: c1.1, degrees: turn });
            }
        } else if self.touches.len() >= 2 && self.turned.abs() >= ROTATE_DEAD_ZONE {
            // Catch up with the whole turn so far: the canvas lines up under the fingers.
            self.rotating = true;
            out.push(Gesture::Rotate { x: c1.0, y: c1.1, degrees: self.turned });
        }
        out
    }

    /// Time passing with fingers down (`now_ms`, called by a timer): one finger held still for
    /// [`HOLD_MS`] starts picking a colour.
    pub fn tick(&mut self, now_ms: f64) -> Vec<Gesture> {
        let [t] = self.touches.as_slice() else { return Vec::new() };
        if self.picking || self.palm || self.moved || self.most != 1 || now_ms - self.started_ms < HOLD_MS {
            return Vec::new();
        }
        self.picking = true;
        vec![Gesture::Pick { x: t.last.0, y: t.last.1 }]
    }

    /// Direction from the first finger to the second, in degrees (`None` with fewer than two, or
    /// when they are on top of each other).
    fn angle(&self) -> Option<f64> {
        let (a, b) = (self.touches.first()?, self.touches.get(1)?);
        let (dx, dy) = (b.last.0 - a.last.0, b.last.1 - a.last.1);
        (dx.hypot(dy) > 1.0).then(|| dy.atan2(dx).to_degrees())
    }

    pub fn up(&mut self, id: i32, now_ms: f64) -> Vec<Gesture> {
        let Some(i) = self.touches.iter().position(|t| t.id == id) else { return Vec::new() };
        let lifted = self.touches.remove(i);
        if self.picking {
            if !self.touches.is_empty() {
                return Vec::new();
            }
            self.picking = false;
            return vec![Gesture::PickEnd { x: lifted.last.0, y: lifted.last.1 }];
        }
        // Rotation needs two fingers: once fewer remain, it's over (the desktop may snap it).
        if self.rotating && self.touches.len() < 2 {
            self.rotating = false;
            self.turned = 0.0;
            return vec![Gesture::RotateEnd { x: self.centre.0, y: self.centre.1 }];
        }
        if !self.touches.is_empty() || self.palm || self.moved || now_ms - self.started_ms > TAP_MS {
            return Vec::new();
        }
        match self.most {
            1 => vec![Gesture::Tap { x: lifted.start.0, y: lifted.start.1 }],
            2 => vec![Gesture::Undo],
            3 => vec![Gesture::Redo],
            _ => Vec::new(),
        }
    }

    /// The OS took the touch (a system gesture): end it without a tap.
    pub fn cancel(&mut self, id: i32) -> Vec<Gesture> {
        let lifted = self.touches.iter().find(|t| t.id == id).map(|t| t.last);
        self.touches.retain(|t| t.id != id);
        self.moved = true;
        if self.picking && self.touches.is_empty() {
            self.picking = false;
            // The colour picked so far stays (it was set as the finger moved).
            return lifted.map(|(x, y)| vec![Gesture::PickEnd { x, y }]).unwrap_or_default();
        }
        if self.rotating && self.touches.len() < 2 {
            self.rotating = false;
            self.turned = 0.0;
            return vec![Gesture::RotateEnd { x: self.centre.0, y: self.centre.1 }];
        }
        Vec::new()
    }

    /// The fingers' centre and mean distance from it.
    fn centroid_and_spread(&self) -> Option<((f64, f64), f64)> {
        let n = self.touches.len() as f64;
        if n == 0.0 {
            return None;
        }
        let cx = self.touches.iter().map(|t| t.last.0).sum::<f64>() / n;
        let cy = self.touches.iter().map(|t| t.last.1).sum::<f64>() / n;
        let spread = self.touches.iter().map(|t| (t.last.0 - cx).hypot(t.last.1 - cy)).sum::<f64>() / n;
        Some(((cx, cy), spread))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_finger_tap_clicks_and_drag_scrolls() {
        let mut g = Gestures::default();
        g.down(1, 100.0, 100.0, 0.0);
        assert!(g.moved(1, 104.0, 100.0).is_empty());
        assert_eq!(g.up(1, 100.0), vec![Gesture::Tap { x: 100.0, y: 100.0 }]);

        g.down(2, 100.0, 100.0, 1000.0);
        assert!(g.moved(2, 105.0, 100.0).is_empty());
        let out = g.moved(2, 130.0, 110.0);
        assert_eq!(out, vec![Gesture::Scroll { x: 130.0, y: 110.0, dx: 25.0, dy: 10.0 }]);
        assert!(g.up(2, 1100.0).is_empty());
    }

    #[test]
    fn two_and_three_finger_taps_undo_and_redo() {
        let mut g = Gestures::default();
        g.down(1, 100.0, 100.0, 0.0);
        g.down(2, 200.0, 100.0, 20.0);
        assert!(g.up(1, 120.0).is_empty());
        assert_eq!(g.up(2, 130.0), vec![Gesture::Undo]);
        for id in 1..=3 {
            g.down(id, f64::from(id) * 50.0, 100.0, 1000.0);
        }
        assert!(g.up(1, 1100.0).is_empty());
        assert!(g.up(2, 1100.0).is_empty());
        assert_eq!(g.up(3, 1100.0), vec![Gesture::Redo]);
    }

    #[test]
    fn pinch_zooms_about_the_centre() {
        let mut g = Gestures::default();
        g.down(1, 100.0, 100.0, 0.0);
        g.down(2, 200.0, 100.0, 0.0);
        let out = g.moved(2, 300.0, 100.0);
        // The centre moved right by 50; the spread went from 50 to 100.
        assert!(out.contains(&Gesture::Scroll { x: 200.0, y: 100.0, dx: 50.0, dy: 0.0 }));
        assert!(out.contains(&Gesture::Zoom { x: 200.0, y: 100.0, factor: 2.0 }));
        assert!(g.up(1, 50.0).is_empty() && g.up(2, 60.0).is_empty(), "a pinch is no tap");
    }

    /// Turn two fingers about their midpoint (200, 100) by `deg` from horizontal.
    fn turn(g: &mut Gestures, deg: f64) -> Vec<Gesture> {
        let (s, c) = deg.to_radians().sin_cos();
        let mut out = g.moved(1, 200.0 - 100.0 * c, 100.0 - 100.0 * s);
        out.extend(g.moved(2, 200.0 + 100.0 * c, 100.0 + 100.0 * s));
        out
    }

    fn rotations(out: &[Gesture]) -> f64 {
        out.iter().map(|g| if let Gesture::Rotate { degrees, .. } = g { *degrees } else { 0.0 }).sum()
    }

    #[test]
    fn two_fingers_rotate_after_a_dead_zone_and_end_with_a_snap_point() {
        let mut g = Gestures::default();
        g.down(1, 100.0, 100.0, 0.0);
        g.down(2, 300.0, 100.0, 0.0);
        // A few degrees of wobble while pinching: no rotation.
        assert_eq!(rotations(&turn(&mut g, 3.0)), 0.0);
        assert_eq!(rotations(&turn(&mut g, -4.0)), 0.0);
        // Past the dead zone, the whole turn so far arrives at once, then each step.
        let total: f64 = [10.0, 30.0, 85.0].iter().map(|d| rotations(&turn(&mut g, *d))).sum();
        assert!((total - 85.0).abs() < 1e-6, "rotated {total}");
        // Crossing ±180° is a small step, not a full turn.
        let mut g2 = Gestures::default();
        g2.down(1, 100.0, 100.0, 0.0);
        g2.down(2, 300.0, 100.0, 0.0);
        let total: f64 = [60.0, 120.0, 170.0, 190.0].iter().map(|d| rotations(&turn(&mut g2, *d))).sum();
        assert!((total - 190.0).abs() < 1e-6, "rotated {total}");
        // Lifting a finger ends the rotation at the fingers' centre; no tap or undo.
        assert_eq!(g.up(1, 500.0), vec![Gesture::RotateEnd { x: 200.0, y: 100.0 }]);
        assert!(g.up(2, 510.0).is_empty());
    }

    #[test]
    fn a_plain_pinch_does_not_rotate() {
        let mut g = Gestures::default();
        g.down(1, 100.0, 100.0, 0.0);
        g.down(2, 300.0, 100.0, 0.0);
        let out = g.moved(2, 400.0, 103.0);
        assert!(out.iter().all(|g| !matches!(g, Gesture::Rotate { .. })));
        assert!(g.up(1, 100.0).is_empty() && g.up(2, 100.0).is_empty());
    }

    #[test]
    fn palm_is_ignored() {
        let mut g = Gestures::default();
        g.pen(true, 0.0);
        g.down(1, 100.0, 100.0, 10.0);
        assert!(g.moved(1, 200.0, 200.0).is_empty());
        g.pen(false, 50.0);
        assert!(g.up(1, 60.0).is_empty());
        // Right after the Pencil lifted: still the palm.
        g.down(2, 100.0, 100.0, 200.0);
        assert!(g.up(2, 250.0).is_empty());
        // Later: a real tap.
        g.down(3, 100.0, 100.0, 1000.0);
        assert_eq!(g.up(3, 1050.0), vec![Gesture::Tap { x: 100.0, y: 100.0 }]);
        // The Pencil landing mid-gesture turns it into a palm.
        g.down(4, 100.0, 100.0, 2000.0);
        g.pen(true, 2010.0);
        assert!(g.moved(4, 300.0, 100.0).is_empty());
    }

    #[test]
    fn touch_and_hold_picks_a_colour() {
        let mut g = Gestures::default();
        g.down(1, 100.0, 100.0, 0.0);
        assert!(g.tick(200.0).is_empty(), "not held long enough");
        assert!(g.moved(1, 104.0, 100.0).is_empty(), "a wobble inside the slop is still a hold");
        assert_eq!(g.tick(HOLD_MS), vec![Gesture::Pick { x: 104.0, y: 100.0 }]);
        assert!(g.tick(HOLD_MS + 100.0).is_empty(), "the hold starts picking once");
        // Dragging keeps picking (no scrolling), and lifting ends it, without a tap.
        assert_eq!(g.moved(1, 180.0, 140.0), vec![Gesture::Pick { x: 180.0, y: 140.0 }]);
        // A second finger meanwhile does nothing.
        g.down(2, 300.0, 300.0, 900.0);
        assert!(g.moved(2, 400.0, 300.0).is_empty());
        assert!(g.up(2, 950.0).is_empty());
        assert_eq!(g.up(1, 1000.0), vec![Gesture::PickEnd { x: 180.0, y: 140.0 }]);
        // The next gesture is ordinary again.
        g.down(3, 10.0, 10.0, 2000.0);
        assert_eq!(g.up(3, 2050.0), vec![Gesture::Tap { x: 10.0, y: 10.0 }]);
    }

    #[test]
    fn no_hold_after_moving_with_two_fingers_or_from_the_palm() {
        let mut g = Gestures::default();
        // Scrolled first: a pause afterwards isn't a hold.
        g.down(1, 100.0, 100.0, 0.0);
        g.moved(1, 150.0, 100.0);
        assert!(g.tick(1000.0).is_empty());
        g.up(1, 1000.0);
        // Two fingers resting still: no pick (one lifted, the other stays).
        g.down(1, 100.0, 100.0, 2000.0);
        g.down(2, 200.0, 100.0, 2000.0);
        g.up(2, 2100.0);
        assert!(g.tick(3000.0).is_empty());
        g.up(1, 3000.0);
        // The resting palm while the Pencil draws.
        g.pen(true, 4000.0);
        g.down(1, 100.0, 100.0, 4000.0);
        assert!(g.tick(5000.0).is_empty());
        // The OS taking a picking finger ends the pick, keeping the colour.
        let mut g = Gestures::default();
        g.down(1, 5.0, 5.0, 0.0);
        g.tick(HOLD_MS);
        assert_eq!(g.cancel(1), vec![Gesture::PickEnd { x: 5.0, y: 5.0 }]);
    }

    #[test]
    fn hostile_input_is_harmless() {
        let mut g = Gestures::default();
        assert!(g.down(1, f64::NAN, 0.0, 0.0).is_empty());
        assert!(g.moved(9, 1.0, 1.0).is_empty());
        assert!(g.up(9, 0.0).is_empty());
        for id in 0..50 {
            g.down(id, 1.0, 1.0, 0.0);
        }
        assert_eq!(g.touches.len(), MAX_TOUCHES);
        g.cancel(0);
        assert!(g.up(1, 1.0).is_empty());
    }
}
