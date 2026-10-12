//! Pencil stroke smoothing: the browser reports the Pencil far less often than it samples (and
//! Safari has no `getCoalescedEvents`), so a quick circle arrives as a few dozen points and would
//! paint as a polygon. Each segment between two reported points is filled in along a centripetal
//! Catmull-Rom spline through its neighbours, which passes through every real sample, follows the
//! curve without overshooting at sharp turns, and needs only one sample of look-ahead.
//!
//! Platform-free so it tests natively. Positions are window pixels.

/// One Pencil sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub x: f64,
    pub y: f64,
    pub pressure: f64,
    pub tilt_x: f64,
    pub tilt_y: f64,
    pub twist: f64,
}

/// Fill-in spacing along a segment, in pixels.
const STEP: f64 = 1.5;
/// Most points one segment is filled with.
const MAX_STEPS: usize = 64;
/// Samples closer than this to the last one are only kept for their pressure.
const MIN_MOVE: f64 = 0.2;

#[derive(Debug, Default)]
pub struct Smoother {
    /// The last up-to-four samples of the stroke (oldest first).
    pts: Vec<Sample>,
    /// The first sample's segment hasn't been emitted (it waits for the look-ahead).
    started: bool,
}

impl Smoother {
    /// A new stroke: the first point goes out at once.
    pub fn down(&mut self, s: Sample) -> Vec<Sample> {
        if !finite(&s) {
            self.pts.clear();
            return Vec::new();
        }
        self.pts = vec![s];
        self.started = true;
        vec![s]
    }

    /// The next sample: emits the filled-in segment *before* it (one sample of look-ahead).
    pub fn push(&mut self, s: Sample) -> Vec<Sample> {
        if !self.started || !finite(&s) {
            return Vec::new();
        }
        if let Some(last) = self.pts.last_mut()
            && (s.x - last.x).hypot(s.y - last.y) < MIN_MOVE
        {
            // Pressing harder in place: keep the newest pressure and pose for that point.
            *last = Sample { x: last.x, y: last.y, ..s };
            return Vec::new();
        }
        self.pts.push(s);
        if self.pts.len() > 4 {
            self.pts.remove(0);
        }
        match self.pts.as_slice() {
            // Not enough look-ahead for the first segment yet.
            [_, _] => Vec::new(),
            // First segment: the stroke's start doubles as its own outer neighbour.
            [a, b, c] => segment(*a, *a, *b, *c),
            [a, b, c, d] => segment(*a, *b, *c, *d),
            _ => Vec::new(),
        }
    }

    /// The Pencil lifted: emit the last segment (its end doubles as its own outer neighbour).
    pub fn up(&mut self) -> Vec<Sample> {
        let out = match self.pts.as_slice() {
            [a, b] => segment(*a, *a, *b, *b),
            [.., a, b, c] => segment(*a, *b, *c, *c),
            _ => Vec::new(),
        };
        self.pts.clear();
        self.started = false;
        out
    }
}

fn finite(s: &Sample) -> bool {
    [s.x, s.y, s.pressure, s.tilt_x, s.tilt_y, s.twist].iter().all(|v| v.is_finite())
}

/// The points from `p1` (exclusive) to `p2` (inclusive) on the centripetal Catmull-Rom spline
/// through `p0, p1, p2, p3`; pressure and pose interpolate linearly along the segment.
fn segment(p0: Sample, p1: Sample, p2: Sample, p3: Sample) -> Vec<Sample> {
    let len = (p2.x - p1.x).hypot(p2.y - p1.y);
    let n = ((len / STEP).ceil() as usize).clamp(1, MAX_STEPS);
    // Knot spacing: sqrt of the distance (centripetal), never zero.
    let knot = |a: &Sample, b: &Sample| (a.x - b.x).hypot(a.y - b.y).sqrt().max(1e-4);
    let (t0, t1) = (0.0, knot(&p0, &p1));
    let t2 = t1 + knot(&p1, &p2);
    let t3 = t2 + knot(&p2, &p3);
    let point = |t: f64| -> (f64, f64) {
        // Barry–Goldman pyramid.
        let lerp = |a: (f64, f64), b: (f64, f64), ta: f64, tb: f64| {
            let w = if tb - ta == 0.0 { 0.0 } else { (t - ta) / (tb - ta) };
            (a.0 + (b.0 - a.0) * w, a.1 + (b.1 - a.1) * w)
        };
        let (q0, q1, q2, q3) = ((p0.x, p0.y), (p1.x, p1.y), (p2.x, p2.y), (p3.x, p3.y));
        let a1 = lerp(q0, q1, t0, t1);
        let a2 = lerp(q1, q2, t1, t2);
        let a3 = lerp(q2, q3, t2, t3);
        let b1 = lerp(a1, a2, t0, t2);
        let b2 = lerp(a2, a3, t1, t3);
        lerp(b1, b2, t1, t2)
    };
    (1..=n)
        .map(|i| {
            let u = i as f64 / n as f64;
            let (x, y) = if i == n { (p2.x, p2.y) } else { point(t1 + (t2 - t1) * u) };
            let mix = |a: f64, b: f64| a + (b - a) * u;
            // Twist takes the shorter way round.
            let dt = (p2.twist - p1.twist + 540.0).rem_euclid(360.0) - 180.0;
            Sample {
                x,
                y,
                pressure: mix(p1.pressure, p2.pressure),
                tilt_x: mix(p1.tilt_x, p2.tilt_x),
                tilt_y: mix(p1.tilt_y, p2.tilt_y),
                twist: (p1.twist + dt * u).rem_euclid(360.0),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: f64, y: f64, pressure: f64) -> Sample {
        Sample { x, y, pressure, tilt_x: 0.0, tilt_y: 0.0, twist: 0.0 }
    }

    /// Feed a stroke, return everything emitted.
    fn run(samples: &[Sample]) -> Vec<Sample> {
        let mut sm = Smoother::default();
        let mut out = sm.down(samples[0]);
        for p in &samples[1..] {
            out.extend(sm.push(*p));
        }
        out.extend(sm.up());
        out
    }

    #[test]
    fn a_sparse_circle_comes_out_round() {
        // 24 samples round a circle of radius 100: as a polygon its edges sag up to
        // 100·(1 − cos 7.5°) ≈ 0.86 px inside the circle, and its corners show.
        let n = 24;
        let pts: Vec<Sample> = (0..=n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                s(100.0 * a.cos(), 100.0 * a.sin(), 0.5)
            })
            .collect();
        let out = run(&pts);
        assert!(out.len() > 300, "filled in densely: {}", out.len());
        // Every point lies on the circle (measured: 0.011 px inside, 0.38 px on the two end
        // segments, which have no neighbour beyond the stroke).
        let err = |p: &Sample| (p.x.hypot(p.y) - 100.0).abs();
        let per_seg = out.len() / n;
        let worst_inner = out[per_seg..out.len() - per_seg].iter().map(err).fold(0.0, f64::max);
        let worst = out.iter().map(err).fold(0.0, f64::max);
        assert!(worst_inner < 0.15, "off the circle by {worst_inner}");
        // The end segments have no neighbour beyond the stroke's end; still well under a polygon's sag.
        assert!(worst < 0.5, "off the circle by {worst}");
        // Consecutive points are close: no straight jumps. (The spline's parameter isn't arc
        // length, so spacing varies a little around STEP; a 2 px chord here sags 0.005 px.)
        let gap = out.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).fold(0.0, f64::max);
        assert!(gap <= STEP * 1.5, "largest gap {gap}");
        // The real samples are all on the path, start and end exact.
        assert_eq!((out[0].x, out[0].y), (100.0, 0.0));
        let last = out[out.len() - 1];
        assert!((last.x - 100.0).abs() < 1e-9 && last.y.abs() < 1e-9);
    }

    #[test]
    fn pressure_ramps_smoothly_along_segments() {
        let out = run(&[s(0.0, 0.0, 0.1), s(30.0, 0.0, 0.4), s(60.0, 0.0, 0.7), s(90.0, 0.0, 1.0)]);
        assert!(out.windows(2).all(|w| w[1].pressure >= w[0].pressure - 1e-12));
        assert!(out.windows(2).all(|w| (w[1].pressure - w[0].pressure) < 0.06), "no pressure steps");
        assert!((out[out.len() - 1].pressure - 1.0).abs() < 1e-12);
    }

    #[test]
    fn short_strokes_and_taps() {
        // A tap: just the point.
        let mut sm = Smoother::default();
        assert_eq!(sm.down(s(5.0, 5.0, 0.3)).len(), 1);
        assert!(sm.up().is_empty());
        // Two points: a straight segment, emitted on lift.
        let out = run(&[s(0.0, 0.0, 0.5), s(10.0, 0.0, 0.5)]);
        assert_eq!(out.first().map(|p| p.x), Some(0.0));
        assert_eq!(out.last().map(|p| p.x), Some(10.0));
        assert!(out.iter().all(|p| p.y.abs() < 1e-9));
    }

    #[test]
    fn pressing_in_place_updates_pressure_without_new_points() {
        let mut sm = Smoother::default();
        sm.down(s(0.0, 0.0, 0.1));
        assert!(sm.push(s(0.05, 0.0, 0.6)).is_empty());
        sm.push(s(20.0, 0.0, 0.6));
        let out = sm.up();
        // The segment starts from the stronger pressure, not the touch-down's 0.1.
        assert!(out.iter().all(|p| p.pressure > 0.5), "{out:?}");
    }

    #[test]
    fn hostile_samples_are_dropped() {
        let mut sm = Smoother::default();
        assert!(sm.down(s(f64::NAN, 0.0, 0.5)).is_empty());
        assert!(sm.push(s(1.0, 1.0, 0.5)).is_empty(), "no stroke started");
        sm.down(s(0.0, 0.0, 0.5));
        assert!(sm.push(s(f64::INFINITY, 0.0, 0.5)).is_empty());
        // A huge jump is capped in points, never an allocation storm.
        sm.push(s(1e12, 0.0, 0.5));
        assert!(sm.up().len() <= MAX_STEPS);
    }
}
