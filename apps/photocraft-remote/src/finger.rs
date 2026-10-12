//! Drawing with a finger: the toolbar's finger toggle, for phones and tablets without a pen. One
//! finger draws; two still pan, pinch and turn, and a held finger still picks a colour.
//!
//! A finger that lands can't yet be told from the first of a pinch, or from a hold, so its samples
//! wait until it moves past [`START_SLOP`] (or lifts: a dot). A second finger before that makes it
//! a gesture with nothing drawn; one held still for [`crate::gesture::HOLD_MS`] picks a colour.
//!
//! Platform-free so it tests natively.

use crate::smooth::Sample;

/// A finger moving this far (page pixels) from where it landed starts drawing.
pub const START_SLOP: f64 = 10.0;
/// Most samples held back before the stroke starts.
const MAX_PENDING: usize = 1024;

/// What the page does with a finger's input.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Not drawing yet.
    Wait,
    /// Start the stroke with these samples (the first is where it began).
    Begin(Vec<Sample>),
    /// Continue the stroke.
    Move(Vec<Sample>),
}

/// The drawing finger.
#[derive(Debug)]
pub struct FingerStroke {
    pub id: i32,
    /// When it landed (ms).
    pub down_ms: f64,
    /// Where it landed, and where it is now (page pixels).
    pub start: (f64, f64),
    pub at: (f64, f64),
    /// Drawing has started.
    pub started: bool,
    pending: Vec<Sample>,
}

impl FingerStroke {
    pub fn new(id: i32, now_ms: f64, at: (f64, f64), first: Sample) -> Self {
        Self { id, down_ms: now_ms, start: at, at, started: false, pending: vec![first] }
    }

    /// The finger moved to `at` (page pixels), with `samples` (window pixels).
    pub fn moved(&mut self, at: (f64, f64), samples: Vec<Sample>) -> Step {
        if !(at.0.is_finite() && at.1.is_finite()) {
            return Step::Wait;
        }
        self.at = at;
        if self.started {
            return Step::Move(samples);
        }
        let room = MAX_PENDING.saturating_sub(self.pending.len());
        self.pending.extend(samples.into_iter().take(room));
        if (at.0 - self.start.0).hypot(at.1 - self.start.1) > START_SLOP {
            self.started = true;
            return Step::Begin(std::mem::take(&mut self.pending));
        }
        Step::Wait
    }

    /// The finger lifted: what is left to draw. A tap that never moved draws a dot.
    pub fn lift(&mut self) -> Step {
        if self.started {
            return Step::Move(Vec::new());
        }
        self.started = true;
        Step::Begin(std::mem::take(&mut self.pending))
    }

    /// Held still for `hold_ms` without drawing: the finger picks a colour instead.
    pub fn held(&self, now_ms: f64, hold_ms: f64) -> bool {
        !self.started && now_ms - self.down_ms >= hold_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: f64, y: f64) -> Sample {
        Sample { x, y, pressure: 1.0, tilt_x: 0.0, tilt_y: 0.0, twist: 0.0 }
    }

    #[test]
    fn a_finger_draws_once_it_moves_with_nothing_lost() {
        let mut f = FingerStroke::new(1, 0.0, (100.0, 100.0), s(50.0, 50.0));
        // Inside the slop it waits (it may yet be a pinch or a hold).
        assert_eq!(f.moved((104.0, 100.0), vec![s(52.0, 50.0)]), Step::Wait);
        assert!(!f.started);
        // Past it, the stroke starts from where the finger landed.
        assert_eq!(f.moved((115.0, 100.0), vec![s(57.5, 50.0)]), Step::Begin(vec![s(50.0, 50.0), s(52.0, 50.0), s(57.5, 50.0)]));
        assert_eq!(f.moved((120.0, 100.0), vec![s(60.0, 50.0)]), Step::Move(vec![s(60.0, 50.0)]));
        assert_eq!(f.lift(), Step::Move(Vec::new()));
    }

    #[test]
    fn a_tap_draws_a_dot_and_a_still_finger_holds() {
        let mut f = FingerStroke::new(1, 1000.0, (10.0, 10.0), s(5.0, 5.0));
        assert!(!f.held(1200.0, 450.0));
        assert!(f.held(1450.0, 450.0));
        assert_eq!(f.lift(), Step::Begin(vec![s(5.0, 5.0)]));
        // Drawing, it no longer holds.
        assert!(!f.held(5000.0, 450.0));
    }

    #[test]
    fn hostile_input_is_bounded() {
        let mut f = FingerStroke::new(1, 0.0, (0.0, 0.0), s(0.0, 0.0));
        assert_eq!(f.moved((f64::NAN, 0.0), vec![s(1.0, 1.0)]), Step::Wait);
        for _ in 0..10 {
            assert_eq!(f.moved((1.0, 1.0), vec![s(1.0, 1.0); 500]), Step::Wait);
        }
        let Step::Begin(all) = f.lift() else { panic!("a lift draws") };
        assert_eq!(all.len(), MAX_PENDING);
    }
}
