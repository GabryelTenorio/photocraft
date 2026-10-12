//! Warp Text: bends the glyph outlines of a type layer with one of Photoshop's fifteen warp
//! styles (the PSD `warp` descriptor's `warpStyle`).
//!
//! The style math lives in `photocraft_geom::warp` (shared with Edit › Transform › Warp): our
//! own closed-form approximations of each style's look (observed behaviour, not Adobe's
//! formulas), the identity at `bend = 0` and continuous in `bend`. The mapping is evaluated per
//! point; the renderer subdivides long segments so straight edges bend.

use photocraft_doc::text::{TextWarp, TypeDistortion, TypeDistortionKind};
use photocraft_geom::warp::{StyleWarp, WarpStyle};
use photocraft_geom::{Affine, Point};

/// Warp styles in Photoshop's menu order: (PSD `warpStyle` value, short id used by commands).
pub const STYLES: [(&str, &str); 15] = [
    ("warpArc", "arc"),
    ("warpArcLower", "arcLower"),
    ("warpArcUpper", "arcUpper"),
    ("warpArch", "arch"),
    ("warpBulge", "bulge"),
    ("warpShellLower", "shellLower"),
    ("warpShellUpper", "shellUpper"),
    ("warpFlag", "flag"),
    ("warpWave", "wave"),
    ("warpFish", "fish"),
    ("warpRise", "rise"),
    ("warpFisheye", "fisheye"),
    ("warpInflate", "inflate"),
    ("warpSqueeze", "squeeze"),
    ("warpTwist", "twist"),
];

/// PSD style name (`warpArc`) for a short id (`arc`, case-insensitive) or a PSD name.
pub fn psd_style(id: &str) -> Option<&'static str> {
    let l = id.to_ascii_lowercase();
    if l == "none" || l == "warpnone" {
        return Some("warpNone");
    }
    STYLES.iter().find(|(psd, short)| psd.to_ascii_lowercase() == l || short.to_ascii_lowercase() == l).map(|(psd, _)| *psd)
}

/// Short id (`flag`, `none`) for a PSD style name (`warpFlag`, `warpNone`): the reverse of [`psd_style`].
pub fn short_style(psd: &str) -> Option<&'static str> {
    if psd == "warpNone" {
        return Some("none");
    }
    STYLES.iter().find(|(p, _)| *p == psd).map(|(_, short)| *short)
}

/// A warp prepared for one layout: the shared style math of
/// [`photocraft_geom::warp::StyleWarp`] (also used by Edit › Transform › Warp) over the text box,
/// then the layer's free transforms ([`TypeDistortion`]: custom mesh, Distort, Perspective).
#[derive(Clone, Debug)]
pub struct Warp {
    style: Option<StyleWarp>,
    ops: Vec<Op>,
}

/// One free transform, ready to map text-space points.
#[derive(Clone, Debug)]
struct Op {
    frame: Affine,
    back: Affine,
    kind: TypeDistortionKind,
    /// Text-space segment length that stays smooth under the op.
    segment: f64,
}

impl Op {
    fn new(d: &TypeDistortion) -> Option<Op> {
        if !d.is_valid() {
            return None;
        }
        let back = d.frame.inverse()?;
        // About 2 document pixels at the frame's scale.
        let scale = d.frame.determinant().abs().sqrt().max(1e-6);
        Some(Op { frame: d.frame, back, kind: d.kind.clone(), segment: (2.0 / scale).clamp(0.05, 64.0) })
    }

    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let p = self.frame.apply(Point::new(x, y));
        let (qx, qy) = self.kind.apply(p.x, p.y);
        let q = self.back.apply(Point::new(qx, qy));
        (q.x, q.y)
    }
}

impl Warp {
    /// Prepares `w` for text whose bounds are `[x0, y0, x1, y1]` (text space). `None` for
    /// `warpNone`, unknown styles, an all-zero warp or empty bounds.
    pub fn new(w: &TextWarp, bounds: [f32; 4]) -> Option<Warp> {
        Some(Warp { style: Some(style_warp(w, bounds)?), ops: Vec::new() })
    }

    /// The Warp Text style `w` (if any) followed by the free transforms `distortions` (invalid
    /// ones are skipped). `None` when nothing bends the outlines.
    pub fn with_distortions(w: Option<&TextWarp>, bounds: Option<[f32; 4]>, distortions: &[TypeDistortion]) -> Option<Warp> {
        let style = w.zip(bounds).and_then(|(w, b)| style_warp(w, b));
        let ops: Vec<Op> = distortions.iter().take(photocraft_doc::text::MAX_TYPE_DISTORTIONS).filter_map(Op::new).collect();
        (style.is_some() || !ops.is_empty()).then_some(Warp { style, ops })
    }

    /// Longest segment (text-space px) worth sending through [`Warp::apply`] unsplit.
    pub fn max_segment(&self) -> f64 {
        let s = self.style.map_or(f64::MAX, |s| s.max_segment());
        self.ops.iter().fold(s, |m, o| m.min(o.segment)).clamp(0.05, 1e6)
    }

    /// Maps a text-space point.
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let (mut x, mut y) = match &self.style {
            Some(s) => s.apply(x, y),
            None => (x, y),
        };
        for o in &self.ops {
            (x, y) = o.apply(x, y);
        }
        (x, y)
    }
}

fn style_warp(w: &TextWarp, bounds: [f32; 4]) -> Option<StyleWarp> {
    let style = WarpStyle::parse(&w.style).filter(|s| s.is_preset() && s.psd_name() == w.style)?;
    StyleWarp::new(style, f64::from(w.value), f64::from(w.horizontal_distortion), f64::from(w.vertical_distortion), !w.horizontal, bounds.map(f64::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warp(style: &str, value: f32) -> Warp {
        let w = TextWarp { style: psd_style(style).unwrap().into(), value, horizontal_distortion: 0.0, vertical_distortion: 0.0, horizontal: true };
        Warp::new(&w, [0.0, -40.0, 200.0, 10.0]).unwrap()
    }

    /// #218: the Warp Text dialog shows a layer's PSD style by its short id.
    #[test]
    fn short_ids_round_trip_with_psd_styles() {
        assert_eq!(short_style("warpFlag"), Some("flag"));
        assert_eq!(short_style("warpNone"), Some("none"));
        assert_eq!(short_style("warpCustom"), None);
        for (psd, short) in STYLES {
            assert_eq!(short_style(psd), Some(short));
            assert_eq!(psd_style(short_style(psd).unwrap()), Some(psd));
        }
    }

    #[test]
    fn styles_resolve_and_none_is_identity() {
        assert_eq!(psd_style("Arc"), Some("warpArc"));
        assert_eq!(psd_style("warpTwist"), Some("warpTwist"));
        assert_eq!(psd_style("none"), Some("warpNone"));
        assert_eq!(psd_style("spiral"), None);
        let none = TextWarp { style: "warpNone".into(), value: 50.0, ..Default::default() };
        assert!(Warp::new(&none, [0.0, 0.0, 10.0, 10.0]).is_none());
        let flat = TextWarp { style: "warpArc".into(), value: 0.0, ..Default::default() };
        assert!(Warp::new(&flat, [0.0, 0.0, 10.0, 10.0]).is_none());
    }

    #[test]
    fn every_style_is_continuous_and_fixes_the_centre_column() {
        for (_, id) in STYLES {
            let w = warp(id, 50.0);
            let (x, y) = w.apply(100.0, -15.0);
            assert!(x.is_finite() && y.is_finite(), "{id}");
            // Tiny bends barely move anything.
            let t = warp(id, 0.5);
            let (x2, y2) = t.apply(20.0, 0.0);
            assert!((x2 - 20.0).abs() < 2.0 && y2.abs() < 2.0, "{id}: {x2},{y2}");
        }
    }

    #[test]
    fn arc_raises_the_middle_relative_to_the_ends() {
        let w = warp("arc", 50.0);
        let mid = w.apply(100.0, -15.0);
        let end = w.apply(0.0, -15.0);
        // As in Photoshop: the bottom edge keeps its corners, the middle rises and the ends of
        // the upper rows swing out and down along the arc.
        let corner = w.apply(0.0, 10.0);
        assert!(corner.0.abs() < 1e-9 && (corner.1 - 10.0).abs() < 1e-9, "{corner:?}");
        assert!(mid.1 < -15.0, "the middle rises: {mid:?}");
        assert!(end.1 > mid.1 + 5.0, "ends drop below the centre: {end:?}");
        assert!(end.0 < 0.0, "ends swing out along the arc: {end:?}");
        // A negative bend smiles.
        let s = warp("arc", -50.0);
        assert!(s.apply(0.0, -15.0).1 < -20.0);
    }

    #[test]
    fn bulge_and_squeeze_change_height_in_the_middle() {
        let b = warp("bulge", 50.0);
        assert!(b.apply(100.0, -40.0).1 < -50.0);
        // Squeeze (Photoshop) pinches the sides in while the top and bottom bow out a little.
        let s = warp("squeeze", 50.0);
        assert!(s.apply(0.0, -15.0).0 > 10.0, "{:?}", s.apply(0.0, -15.0));
        assert!(s.apply(100.0, -40.0).1 < -40.0);
    }
}
