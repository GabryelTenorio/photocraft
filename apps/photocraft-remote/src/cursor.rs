//! The brush outline under a hovering Pencil. PhotoCraft's canvas cursor is an OS cursor, which
//! never appears in the window capture, so the desktop sends its shape (`{"t": "cursor", …}`,
//! `apps/photocraft/src/remote_server.rs`) and the page draws it where the Pencil hovers, with no
//! round trip.
//!
//! Platform-free so it tests natively.

use serde_json::Value;

/// The outline: a circle or a brush tip's rings in window pixels (they match the brush on the
/// canvas), or a crosshair in UI points (a marker that keeps its size on any screen).
#[derive(Clone, Debug, PartialEq)]
pub enum Outline {
    Circle {
        radius: f64,
        centre: bool,
    },
    Cross {
        length: f64,
        gap: f64,
    },
    /// Closed rings of offsets from the hotspot.
    Rings {
        rings: Vec<Vec<(f64, f64)>>,
        centre: bool,
    },
}

/// Largest outline drawn, in window pixels (a hostile message can't make a huge path).
const MAX_RADIUS: f64 = 50_000.0;
/// Most rings, and points per ring, read from a message.
const MAX_RINGS: usize = 16;
const MAX_RING_POINTS: usize = 256;
/// Largest crosshair arm, in points.
const MAX_ARM: f64 = 64.0;
/// Arm length of the crosshair marking a circle's centre, in CSS pixels.
const CENTRE_ARM: f64 = 3.0;

/// A cursor message's outline: `Some(None)` when there is none, `None` when `v` isn't a valid
/// cursor message.
pub fn parse(v: &Value) -> Option<Option<Outline>> {
    if v.get("t")?.as_str()? != "cursor" {
        return None;
    }
    let num = |k: &str| v.get(k).and_then(Value::as_f64).filter(|n| n.is_finite());
    match v.get("shape")?.as_str()? {
        "none" => Some(None),
        "circle" => {
            let radius = num("r").filter(|r| *r > 0.0)?.min(MAX_RADIUS);
            Some(Some(Outline::Circle { radius, centre: v.get("c").and_then(Value::as_bool).unwrap_or(false) }))
        }
        "cross" => {
            let length = num("l").filter(|l| *l > 0.0)?.min(MAX_ARM);
            let gap = num("g").unwrap_or(0.0).clamp(0.0, length);
            Some(Some(Outline::Cross { length, gap }))
        }
        "rings" => {
            let point = |p: &Value| {
                let at = |i: usize| p.get(i).and_then(Value::as_f64).filter(|n| n.is_finite()).map(|n| n.clamp(-MAX_RADIUS, MAX_RADIUS));
                Some((at(0)?, at(1)?))
            };
            let rings: Vec<Vec<(f64, f64)>> = v
                .get("rings")?
                .as_array()?
                .iter()
                .take(MAX_RINGS)
                .map(|r| r.as_array().map(|r| r.iter().take(MAX_RING_POINTS).map(point).collect::<Option<Vec<_>>>()))
                .collect::<Option<Option<Vec<_>>>>()??
                .into_iter()
                .filter(|r| r.len() >= 2)
                .collect();
            if rings.is_empty() {
                return None;
            }
            Some(Some(Outline::Rings { rings, centre: v.get("c").and_then(Value::as_bool).unwrap_or(false) }))
        }
        _ => None,
    }
}

/// One thing to stroke, in CSS pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Ring {
        x: f64,
        y: f64,
        r: f64,
    },
    Line {
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
    },
    /// A closed polygon.
    Poly {
        points: Vec<(f64, f64)>,
    },
}

/// The outline at `(x, y)` (CSS pixels), with `scale` CSS pixels per window pixel.
pub fn marks(o: &Outline, x: f64, y: f64, scale: f64) -> Vec<Mark> {
    let cross = |length: f64, gap: f64| {
        [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)]
            .into_iter()
            .map(|(dx, dy)| Mark::Line { x0: x + dx * gap, y0: y + dy * gap, x1: x + dx * length, y1: y + dy * length })
            .collect::<Vec<_>>()
    };
    match o {
        Outline::Circle { radius, centre } => {
            let (radius, centre) = (*radius, *centre);
            let r = radius * scale;
            if !(r.is_finite() && r > 0.0) {
                return Vec::new();
            }
            let mut out = vec![Mark::Ring { x, y, r }];
            // A tip too small to see on screen still shows where it is.
            if centre || r < CENTRE_ARM {
                out.extend(cross(CENTRE_ARM, 0.0));
            }
            out
        }
        Outline::Cross { length, gap } => cross(*length, *gap),
        Outline::Rings { rings, centre } => {
            if !(scale.is_finite() && scale > 0.0) {
                return Vec::new();
            }
            let mut out: Vec<Mark> = rings.iter().map(|r| Mark::Poly { points: r.iter().map(|(dx, dy)| (x + dx * scale, y + dy * scale)).collect() }).collect();
            if *centre {
                out.extend(cross(CENTRE_ARM, 0.0));
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_cursor_messages() {
        assert_eq!(parse(&json!({"t": "cursor", "shape": "none"})), Some(None));
        assert_eq!(parse(&json!({"t": "cursor", "shape": "circle", "r": 12.5, "c": true})), Some(Some(Outline::Circle { radius: 12.5, centre: true })));
        assert_eq!(parse(&json!({"t": "cursor", "shape": "cross", "l": 6, "g": 2})), Some(Some(Outline::Cross { length: 6.0, gap: 2.0 })));
        // Not cursor messages, or broken ones.
        assert_eq!(parse(&json!({"t": "frame"})), None);
        assert_eq!(parse(&json!({"t": "cursor", "shape": "circle", "r": -1})), None);
        assert_eq!(parse(&json!({"t": "cursor", "shape": "circle"})), None);
        assert_eq!(parse(&json!({"t": "cursor", "shape": "star"})), None);
        assert_eq!(
            parse(&json!({"t": "cursor", "shape": "rings", "rings": [[[0, -4], [3, 2], [-3, 2]], [[1, 1]]], "c": true})),
            Some(Some(Outline::Rings { rings: vec![vec![(0.0, -4.0), (3.0, 2.0), (-3.0, 2.0)]], centre: true }))
        );
        assert_eq!(parse(&json!({"t": "cursor", "shape": "rings", "rings": [[[0, "x"], [1, 1]]]})), None);
        assert_eq!(parse(&json!({"t": "cursor", "shape": "rings", "rings": []})), None);
        // Hostile sizes are capped.
        assert_eq!(parse(&json!({"t": "cursor", "shape": "circle", "r": 1e300})), Some(Some(Outline::Circle { radius: MAX_RADIUS, centre: false })));
        assert_eq!(parse(&json!({"t": "cursor", "shape": "cross", "l": 1e9, "g": 1e12})), Some(Some(Outline::Cross { length: MAX_ARM, gap: MAX_ARM })));
    }

    #[test]
    fn the_ring_scales_with_the_page() {
        // A 40 px brush outline on a page showing the window at 0.5×.
        assert_eq!(marks(&Outline::Circle { radius: 40.0, centre: false }, 100.0, 50.0, 0.5), vec![Mark::Ring { x: 100.0, y: 50.0, r: 20.0 }]);
        // A centre mark, or a tip too small to see, adds a crosshair.
        assert_eq!(marks(&Outline::Circle { radius: 40.0, centre: true }, 0.0, 0.0, 0.5).len(), 5);
        assert_eq!(marks(&Outline::Circle { radius: 2.0, centre: false }, 0.0, 0.0, 0.5).len(), 5);
        // The crosshair keeps its size.
        let m = marks(&Outline::Cross { length: 6.0, gap: 2.0 }, 10.0, 10.0, 0.25);
        assert_eq!(m.first(), Some(&Mark::Line { x0: 12.0, y0: 10.0, x1: 16.0, y1: 10.0 }));
        assert!(marks(&Outline::Circle { radius: 10.0, centre: false }, 0.0, 0.0, f64::NAN).is_empty());
        // A brush tip's rings scale with the page too.
        let tip = Outline::Rings { rings: vec![vec![(0.0, -4.0), (4.0, 0.0)]], centre: false };
        assert_eq!(marks(&tip, 10.0, 10.0, 0.5), vec![Mark::Poly { points: vec![(10.0, 8.0), (12.0, 10.0)] }]);
    }
}
