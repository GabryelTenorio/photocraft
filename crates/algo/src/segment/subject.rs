//! Select Subject (classical, heuristic).
//!
//! Photoshop uses a learned model; we approximate it with salient-object detection feeding a
//! GrabCut:
//!
//! 1. SLIC superpixels on a working-resolution copy.
//! 2. Per-superpixel saliency from "Saliency Optimization from Robust Background Detection"
//!    (W. Zhu, S. Liang, Y. Wei and J. Sun, CVPR 2014): boundary connectivity
//!    `BndCon(p) = Len_bnd(p) / sqrt(Area(p))` over geodesic colour distances gives a background
//!    probability; background-weighted contrast gives foreground cues; a least-squares
//!    optimisation with smoothness between neighbours combines them.
//! 3. Blended with frequency-tuned saliency (R. Achanta, S. Hemami, F. Estrada and
//!    S. Süsstrunk, "Frequency-tuned Salient Region Detection", CVPR 2009): distance of the
//!    lightly blurred CIELAB image from its mean colour.
//!    The boundary prior only takes border regions whose colour recurs on another side of the
//!    frame, so a subject cut off by the frame edge is not mistaken for background.
//! 4. Otsu's threshold on the saliency map seeds a GrabCut trimap (low-saliency border pixels
//!    are definite background; salient ones stay free, so the subject can reach the edge); the
//!    result is cleaned (small islands dropped, small holes filled) and re-cut at full
//!    resolution.
//!
//! Works well for a distinct object on a comparatively plain background; it is not a semantic
//! segmenter.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use photocraft_geom::Rect;

use super::grabcut::{self, BG, PR_BG, PR_FG};
use super::slic::{rgb_to_lab, slic_lab};
use super::{Region, Sampler};

/// Working-resolution budget (pixels).
pub const WORK_PX: usize = 110_000;
const SIGMA_CLR: f32 = 10.0;
const SIGMA_SPA: f32 = 0.25;

/// Selects the most salient object on the canvas.
///
/// Flat margins round a placed picture (a photo on a larger white or transparent canvas) are
/// left out first: otherwise the margin is the frame edge the boundary prior calls background,
/// and the whole photo is "salient" against it.
pub fn select_subject(sampler: &dyn Sampler, canvas: Rect) -> Option<Region> {
    if canvas.width() < 4 || canvas.height() < 4 {
        return None;
    }
    let step = super::scale_for(canvas, WORK_PX);
    let img = sampler.rgb_scaled(canvas, step);
    match content_window(sampler, canvas, &img, step) {
        Some(window) if window != canvas => {
            let step = super::scale_for(window, WORK_PX);
            subject_scaled(sampler, window, step, sampler.rgb_scaled(window, step))
        }
        _ => subject_scaled(sampler, canvas, step, img),
    }
}

/// The part of `canvas` a subject is looked for in: inside any flat margins round a placed
/// picture (see [`content_window`]), else the whole canvas. Shared with the learned model.
pub fn subject_window(sampler: &dyn Sampler, canvas: Rect) -> Rect {
    if canvas.width() < 4 || canvas.height() < 4 {
        return canvas;
    }
    let step = super::scale_for(canvas, WORK_PX);
    content_window(sampler, canvas, &sampler.rgb_scaled(canvas, step), step).unwrap_or(canvas)
}

/// Share of the largest detected object another one must reach to be kept as a subject too.
pub const MAIN_SUBJECT_FRAC: f32 = 0.35;

/// Keeps the main subject(s) of a model's foreground probabilities `prob` (`w × h`, 0–1): the
/// objects (8-connected areas at least half likely) whose area is at least [`MAIN_SUBJECT_FRAC`]
/// of the largest one. A salient-object model also finds smaller things (a bag at someone's
/// feet); Select Subject keeps the main subject, as Photoshop does, and two people of similar
/// size both stay. The soft fringe within a few pixels of a kept object stays; everything else
/// becomes 0.
pub fn keep_main_subjects(prob: &mut [f32], w: usize, h: usize) {
    if w == 0 || h == 0 || prob.len() != w.saturating_mul(h) {
        return;
    }
    let fg: Vec<bool> = prob.iter().map(|p| *p >= 0.5).collect();
    let (labels, sizes) = super::components(&fg, w, h);
    let largest = sizes.iter().skip(1).copied().max().unwrap_or(0);
    if largest == 0 {
        return;
    }
    let min = (largest as f32 * MAIN_SUBJECT_FRAC).ceil() as usize;
    let kept: Vec<bool> = labels.iter().map(|l| *l != 0 && sizes.get(*l as usize).is_some_and(|s| *s >= min)).collect();
    // Grow the kept objects by a few pixels (separable square dilation) so their soft edges stay.
    const FRINGE: usize = 4;
    let mut rows = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(FRINGE), (x + FRINGE + 1).min(w));
            rows[y * w + x] = kept[y * w + x0..y * w + x1].iter().any(|v| *v);
        }
    }
    for x in 0..w {
        for y in 0..h {
            let (y0, y1) = (y.saturating_sub(FRINGE), (y + FRINGE + 1).min(h));
            if !(y0..y1).any(|yy| rows[yy * w + x])
                && let Some(p) = prob.get_mut(y * w + x)
            {
                *p = 0.0;
            }
        }
    }
}

/// A selection over `window` from a model's foreground probabilities `prob` (`pw × ph`,
/// row-major, 0–1, stretched over the whole window): bilinearly resampled to every pixel, so
/// the model's soft edges stay soft. `None` when nothing is at least half selected.
pub fn region_from_probability(prob: &[f32], pw: usize, ph: usize, window: Rect) -> Option<Region> {
    let (w, h) = (usize::try_from(window.width()).ok()?, usize::try_from(window.height()).ok()?);
    if pw == 0 || ph == 0 || prob.len() != pw.checked_mul(ph)? || w == 0 || h == 0 {
        return None;
    }
    let at = |x: usize, y: usize| prob.get(y.min(ph - 1) * pw + x.min(pw - 1)).copied().unwrap_or(0.0);
    let (sx, sy) = (pw as f32 / w as f32, ph as f32 / h as f32);
    let mut mask = vec![0u8; w.checked_mul(h)?];
    let row = |y: usize, out: &mut [u8]| {
        let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        let (y0, ty) = (fy as usize, fy.fract());
        for (x, o) in out.iter_mut().enumerate() {
            let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
            let (x0, tx) = (fx as usize, fx.fract());
            let top = at(x0, y0) + (at(x0 + 1, y0) - at(x0, y0)) * tx;
            let bot = at(x0, y0 + 1) + (at(x0 + 1, y0 + 1) - at(x0, y0 + 1)) * tx;
            let v = top + (bot - top) * ty;
            *o = if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 };
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        mask.par_chunks_mut(w).enumerate().for_each(|(y, r)| row(y, r));
    }
    #[cfg(target_arch = "wasm32")]
    mask.chunks_mut(w).enumerate().for_each(|(y, r)| row(y, r));
    if !mask.iter().any(|v| *v >= 128) {
        return None;
    }
    super::trim_region(Region { bbox: window, mask })
}

/// Largest per-channel difference (0–1) a flat margin may have from its outermost colour.
const FLAT: f32 = 3.0 / 255.0;
/// Share of a picture's edge line that must differ from the margin: a placed photo's edge is a
/// straight line across the whole side, an object on a plain backdrop only touches it here and
/// there (and its backdrop is part of the picture, not a margin).
const STRAIGHT_EDGE: f32 = 0.9;

fn near(p: [f32; 3], q: [f32; 3]) -> bool {
    (0..3).all(|c| (p[c] - q[c]).abs() <= FLAT)
}

/// Flat margin widths of `img` in its own pixels, `[left, top, right, bottom]`, and their colour.
///
/// The margin colour is a colour at least two corners share; the picture is the bounding box of
/// the pixels in any other colour. It counts as a placed picture, with margins round it, only
/// when that box is solid: each of its edge lines is at least [`STRAIGHT_EDGE`] picture. An
/// object on a plain backdrop fails that (its box edges are mostly backdrop), so the backdrop
/// stays part of the scene.
pub(super) fn flat_margins(img: &super::RgbImage) -> ([usize; 4], [f32; 3]) {
    let (w, h) = (img.w, img.h);
    if w < 2 || h < 2 {
        return ([0; 4], [0.0; 3]);
    }
    let at = |x: usize, y: usize| img.px.get(y * w + x).copied().unwrap_or_default();
    let corners = [at(0, 0), at(w - 1, 0), at(0, h - 1), at(w - 1, h - 1)];
    let Some(bg) = corners.iter().copied().find(|c| corners.iter().filter(|d| near(**d, *c)).count() >= 2) else { return ([0; 4], [0.0; 3]) };
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if !near(at(x, y), bg) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    if x0 == usize::MAX {
        return ([0; 4], bg);
    }
    let solid = |n: usize, px: &dyn Fn(usize) -> [f32; 3]| (0..n).filter(|i| !near(px(*i), bg)).count() as f32 >= STRAIGHT_EDGE * n as f32;
    let (bw, bh) = (x1 - x0, y1 - y0);
    let box_solid = solid(bh, &|i| at(x0, y0 + i))
        && solid(bh, &|i| at(x1 - 1, y0 + i))
        && solid(bw, &|i| at(x0 + i, y0))
        && solid(bw, &|i| at(x0 + i, y1 - 1));
    if !box_solid {
        return ([0; 4], bg);
    }
    ([x0, y0, w - x1, h - y1], bg)
}

/// The part of `canvas` inside its flat margins (see [`flat_margins`]), found on `img` (`canvas`
/// sampled with `step`) and then placed exactly by re-reading the edge blocks at full
/// resolution. `None` when there are no margins or they leave too little.
pub(super) fn content_window(sampler: &dyn Sampler, canvas: Rect, img: &super::RgbImage, step: usize) -> Option<Rect> {
    let ([l, t, r, b], bg) = flat_margins(img);
    if l + t + r + b == 0 || img.w.saturating_sub(l + r) < 8 || img.h.saturating_sub(t + b) < 8 {
        return None;
    }
    let s = i32::try_from(step).ok()?;
    let off = |i: usize| i32::try_from(i).ok().and_then(|i| i.checked_mul(s));
    // Coarse window: whole blocks. The block holding the true edge is not flat (it mixes margin and
    // picture), so it stays in; the snap below moves into it.
    let mut win = Rect::new(
        canvas.x0.saturating_add(off(l)?),
        canvas.y0.saturating_add(off(t)?),
        canvas.x1.min(canvas.x0.saturating_add(off(img.w - r)?)),
        canvas.y1.min(canvas.y0.saturating_add(off(img.h - b)?)),
    );
    // Full-resolution lines in the margin colour from one side of `strip`, read as columns or rows.
    let snap = |strip: Rect, columns: bool, from_end: bool| -> i32 {
        let strip = strip.intersect(&canvas);
        let im = sampler.rgb(strip);
        let (sw, sh) = (im.w, im.h);
        let (lines, len) = if columns { (sw, sh) } else { (sh, sw) };
        let px = |k: usize, i: usize| {
            let k = if from_end { lines - 1 - k } else { k };
            let (x, y) = if columns { (k, i) } else { (i, k) };
            im.px.get(y * sw + x).copied().unwrap_or_default()
        };
        let mut k = 0;
        while k < lines && (0..len).all(|i| near(px(k, i), bg)) {
            k += 1;
        }
        i32::try_from(k).unwrap_or(0)
    };
    if l > 0 {
        win.x0 += snap(Rect::new(win.x0, win.y0, win.x0.saturating_add(s), win.y1), true, false);
    }
    if r > 0 {
        win.x1 -= snap(Rect::new(win.x1.saturating_sub(s), win.y0, win.x1, win.y1), true, true);
    }
    if t > 0 {
        win.y0 += snap(Rect::new(win.x0, win.y0, win.x1, win.y0.saturating_add(s)), false, false);
    }
    if b > 0 {
        win.y1 -= snap(Rect::new(win.x0, win.y1.saturating_sub(s), win.x1, win.y1), false, true);
    }
    (win.width() >= 8 && win.height() >= 8).then_some(win)
}

fn subject_scaled(sampler: &dyn Sampler, canvas: Rect, step: usize, img: super::RgbImage) -> Option<Region> {
    let (w, h) = (img.w, img.h);
    let sal = saliency(&img);
    let t = otsu(&sal).clamp(0.15, 0.85);
    let mut trimap = vec![PR_BG; w * h];
    let mut any_fg = false;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            // The frame edge is definite background only where it is not salient: a subject
            // cut off by the frame must be able to reach it.
            let border = x == 0 || y == 0 || x + 1 == w || y + 1 == h;
            trimap[i] = if sal[i] >= t {
                any_fg = true;
                PR_FG
            } else if border || sal[i] < 0.4 * t {
                BG
            } else {
                PR_BG
            };
        }
    }
    if !any_fg {
        return None;
    }
    let (fg, bg) = grabcut::grabcut(&img, &mut trimap, 6)?;
    let low: Vec<bool> = trimap.iter().map(|v| *v == PR_FG || *v == grabcut::FG).collect();
    let low = super::clean_mask(&low, w, h, 0.2, 0.05);
    if !low.iter().any(|v| *v) {
        return None;
    }
    super::finish_region(sampler, canvas, step, &low, w, h, Some((&fg, &bg)))
}

/// Per-pixel saliency in `0..=1` (see the module docs).
pub fn saliency(img: &super::RgbImage) -> Vec<f32> {
    let (w, h) = (img.w, img.h);
    let lab: Vec<[f32; 3]> = img.px.iter().map(|p| rgb_to_lab(*p)).collect();
    let target = ((w * h) / 150).clamp(16, 400);
    let sp = slic_lab(&lab, w, h, target, 10.0, 8);
    let n = sp.count;
    // Region statistics.
    let mut mean = vec![[0.0f64; 3]; n];
    let mut pos = vec![[0.0f64; 2]; n];
    let mut cnt = vec![0.0f64; n];
    // Which image sides (bits: left, top, right, bottom) each region touches.
    let mut sides = vec![0u8; n];
    for y in 0..h {
        for x in 0..w {
            let l = sp.labels[y * w + x] as usize;
            let c = lab[y * w + x];
            for k in 0..3 {
                mean[l][k] += c[k] as f64;
            }
            pos[l][0] += x as f64 / w as f64;
            pos[l][1] += y as f64 / h as f64;
            cnt[l] += 1.0;
            sides[l] |= u8::from(x == 0) | (u8::from(y == 0) << 1) | (u8::from(x + 1 == w) << 2) | (u8::from(y + 1 == h) << 3);
        }
    }
    let mean: Vec<[f32; 3]> = mean.iter().zip(&cnt).map(|(m, c)| m.map(|v| (v / c.max(1.0)) as f32)).collect();
    let pos: Vec<[f32; 2]> = pos.iter().zip(&cnt).map(|(p, c)| p.map(|v| (v / c.max(1.0)) as f32)).collect();
    let dapp = |a: usize, b: usize| super::d2(mean[a], mean[b]).sqrt();
    let on_border = background_border(&sides, &dapp);
    // Adjacency.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut link = |a: usize, b: usize| {
        if a != b && !adj[a].contains(&b) {
            adj[a].push(b);
            adj[b].push(a);
        }
    };
    for y in 0..h {
        for x in 0..w {
            let l = sp.labels[y * w + x] as usize;
            if x + 1 < w {
                link(l, sp.labels[y * w + x + 1] as usize);
            }
            if y + 1 < h {
                link(l, sp.labels[(y + 1) * w + x] as usize);
            }
        }
    }
    // Boundary connectivity from all-pairs geodesic distances (Dijkstra per region).
    let mut wbg = vec![0.0f32; n];
    let mut geo = vec![f32::INFINITY; n];
    for (p, wb) in wbg.iter_mut().enumerate() {
        dijkstra(p, &adj, &dapp, &mut geo);
        let (mut area, mut len) = (0.0f32, 0.0f32);
        for q in 0..n {
            let s = (-geo[q] * geo[q] / (2.0 * SIGMA_CLR * SIGMA_CLR)).exp();
            area += s;
            if on_border[q] {
                len += s;
            }
        }
        let bnd = len / area.max(1e-6).sqrt();
        *wb = 1.0 - (-bnd * bnd / 2.0).exp();
    }
    // Background-weighted contrast.
    let mut wctr = vec![0.0f32; n];
    for p in 0..n {
        let mut s = 0.0;
        for q in 0..n {
            let (dx, dy) = (pos[p][0] - pos[q][0], pos[p][1] - pos[q][1]);
            let wspa = (-(dx * dx + dy * dy) / (2.0 * SIGMA_SPA * SIGMA_SPA)).exp();
            s += dapp(p, q) * wspa * wbg[q];
        }
        wctr[p] = s;
    }
    let mx = wctr.iter().copied().fold(0.0f32, f32::max).max(1e-6);
    let wfg: Vec<f32> = wctr.iter().map(|v| v / mx).collect();
    // Saliency optimisation: min Σ wbg s² + Σ wfg (s−1)² + Σ_{i~j} w_ij (s_i − s_j)² (Gauss–Seidel).
    let wij: Vec<Vec<f32>> = (0..n).map(|p| adj[p].iter().map(|&q| (-dapp(p, q).powi(2) / (2.0 * SIGMA_CLR * SIGMA_CLR)).exp() + 0.1).collect()).collect();
    let mut s = wfg.clone();
    for _ in 0..200 {
        for p in 0..n {
            let (mut num, mut den) = (wfg[p], wbg[p] + wfg[p]);
            for (q, wq) in adj[p].iter().zip(&wij[p]) {
                num += wq * s[*q];
                den += wq;
            }
            s[p] = num / den.max(1e-6);
        }
    }
    // Frequency-tuned saliency.
    let mut mean_all = [0.0f64; 3];
    for c in &lab {
        for k in 0..3 {
            mean_all[k] += c[k] as f64;
        }
    }
    let mean_all = mean_all.map(|v| (v / lab.len() as f64) as f32);
    let blurred = super::RgbImage { w, h, px: lab.clone() };
    let blurred = blur_lab(&blurred);
    let ft: Vec<f32> = blurred.px.iter().map(|c| super::d2(*c, mean_all).sqrt()).collect();
    let ftmax = ft.iter().copied().fold(0.0f32, f32::max).max(1e-6);
    let smin = s.iter().copied().fold(f32::MAX, f32::min);
    let smax = s.iter().copied().fold(f32::MIN, f32::max);
    let srange = (smax - smin).max(1e-6);
    let mut out: Vec<f32> = sp.labels.iter().zip(&ft).map(|(l, f)| 0.75 * (s[*l as usize] - smin) / srange + 0.25 * f / ftmax).collect();
    let (lo, hi) = out.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let r = (hi - lo).max(1e-6);
    for v in &mut out {
        *v = (*v - lo) / r;
    }
    out
}

/// Lab distance under which two border regions count as the same background.
const SAME_BG: f32 = 2.0 * SIGMA_CLR;

/// The border regions that serve as background in the boundary prior. `sides[p]` is the bit set
/// of image sides region `p` touches (0 = not on the border).
///
/// The prior assumes the frame edge is background, but subjects are often cut off by it (a
/// portrait cut at the bottom, an object touching a side). Background usually wraps round the
/// frame, so a border region is kept only when it resembles a border region on a side it does
/// not touch itself; a region whose colour appears on its own side alone is taken for the
/// subject running out of frame. (A subject spanning a whole side, corner to corner, is still
/// ambiguous with a floor or table and stays background.)
pub(super) fn background_border(sides: &[u8], dapp: &dyn Fn(usize, usize) -> f32) -> Vec<bool> {
    let border: Vec<usize> = (0..sides.len()).filter(|p| sides[*p] != 0).collect();
    let kept: Vec<bool> = (0..sides.len())
        .map(|p| sides[p] != 0 && border.iter().any(|&q| (sides[q] & !sides[p]) != 0 && dapp(p, q) < SAME_BG))
        .collect();
    // Never strip the prior bare: without any kept region (a frame of colours that all differ),
    // fall back to the whole border, as the paper does.
    if kept.iter().any(|v| *v) { kept } else { sides.iter().map(|s| *s != 0).collect() }
}

fn blur_lab(img: &super::RgbImage) -> super::RgbImage {
    let (w, h) = (img.w, img.h);
    let mut cur = img.clone();
    for _ in 0..2 {
        let mut tmp = cur.clone();
        for y in 0..h {
            for x in 0..w {
                let (l, c, r) = (cur.at(x.saturating_sub(1), y), cur.at(x, y), cur.at((x + 1).min(w - 1), y));
                tmp.px[y * w + x] = [0, 1, 2].map(|k| (l[k] + 2.0 * c[k] + r[k]) / 4.0);
            }
        }
        for y in 0..h {
            for x in 0..w {
                let (u, c, d) = (tmp.at(x, y.saturating_sub(1)), tmp.at(x, y), tmp.at(x, (y + 1).min(h - 1)));
                cur.px[y * w + x] = [0, 1, 2].map(|k| (u[k] + 2.0 * c[k] + d[k]) / 4.0);
            }
        }
    }
    cur
}

#[derive(PartialEq)]
struct Item(f32, usize);
impl Eq for Item {}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0)
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

fn dijkstra(src: usize, adj: &[Vec<usize>], wt: &dyn Fn(usize, usize) -> f32, dist: &mut [f32]) {
    dist.fill(f32::INFINITY);
    dist[src] = 0.0;
    let mut heap = BinaryHeap::new();
    heap.push(Item(0.0, src));
    while let Some(Item(d, u)) = heap.pop() {
        if d > dist[u] {
            continue;
        }
        for &v in &adj[u] {
            let nd = d + wt(u, v);
            if nd < dist[v] {
                dist[v] = nd;
                heap.push(Item(nd, v));
            }
        }
    }
}

/// Otsu's threshold of values in `0..=1` (256 bins).
pub fn otsu(v: &[f32]) -> f32 {
    let mut hist = [0u64; 256];
    for x in v {
        hist[(x.clamp(0.0, 1.0) * 255.0).round() as usize] += 1;
    }
    let total = v.len() as f64;
    let sum: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * *c as f64).sum();
    let (mut wb, mut sb, mut best, mut bt) = (0.0f64, 0.0f64, -1.0f64, 0usize);
    for (t, c) in hist.iter().enumerate() {
        wb += *c as f64;
        if wb == 0.0 {
            continue;
        }
        let wf = total - wb;
        if wf == 0.0 {
            break;
        }
        sb += t as f64 * *c as f64;
        let (mb, mf) = (sb / wb, (sum - sb) / wf);
        let between = wb * wf * (mb - mf) * (mb - mf);
        if between > best {
            best = between;
            bt = t;
        }
    }
    (bt as f32 + 0.5) / 255.0
}
