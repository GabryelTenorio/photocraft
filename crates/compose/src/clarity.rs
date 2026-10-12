//! Clarity and Dehaze: the two neighbourhood (spatial) stages of Photoshop 2026's combined
//! "Clarity and Dehaze" adjustment layer.
//!
//! The formulas mirror the Camera Raw filter's (`photocraft-algo`'s `camera_raw`), which is the
//! same pair of controls pulled into a layer: Dehaze is the dark-channel-prior transmission of
//! He, Sun & Tang (2010) estimated on a small proxy and guided-filtered, Clarity a large-radius
//! guided-filter local-contrast boost of the midtones.
//!
//! Both stages read the whole buffer, so the compositor evaluates them over the full canvas
//! (see `crate::spatial`), never per tile: a per-tile evaluation would seam at tile and band
//! boundaries. `apply` is also what the destructive Image ▸ Adjustments path and the adjustment
//! dialogs' preview call, on whatever buffer they hold.

use crate::Buffer;

/// Applies the combined adjustment to `buf`'s RGB (alpha untouched). `clarity` and `dehaze` are
/// the -100..=100 sliders; 0 leaves that stage out.
pub fn apply(buf: &mut Buffer, clarity: f32, dehaze: f32) {
    let (w, h) = (buf.rect.width() as usize, buf.rect.height() as usize);
    if w == 0 || h == 0 || (clarity == 0.0 && dehaze == 0.0) {
        return;
    }
    if dehaze != 0.0 {
        dehaze_stage(&mut buf.px, w, h, dehaze / 100.0);
    }
    if clarity != 0.0 {
        clarity_stage(&mut buf.px, w, h, clarity / 100.0);
    }
}

/// BT.709 luma of a display-RGB colour.
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Bilinear sample of channel `c` of a `w × h × ch` buffer, clamped to the edges.
fn bilinear(buf: &[f32], w: usize, h: usize, ch: usize, c: usize, x: f32, y: f32) -> f32 {
    if w == 0 || h == 0 {
        return 0.0;
    }
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let p = |xx: usize, yy: usize| buf[(yy * w + xx) * ch + c];
    let top = p(x0, y0) + (p(x1, y0) - p(x0, y0)) * tx;
    let bot = p(x0, y1) + (p(x1, y1) - p(x0, y1)) * tx;
    top + (bot - top) * ty
}

/// Box mean of a single-channel buffer (radius `r`, edges clamped). Separable, running sums;
/// `f64` accumulators keep the long column passes exact.
fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 || w == 0 || h == 0 {
        return v.to_vec();
    }
    let n = (2 * r + 1) as f64;
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        let src = &v[y * w..(y + 1) * w];
        let at = |k: i64| src[k.clamp(0, w as i64 - 1) as usize] as f64;
        let mut acc: f64 = (-(r as i64)..=r as i64).map(at).sum();
        for (x, o) in tmp[y * w..(y + 1) * w].iter_mut().enumerate() {
            *o = (acc / n) as f32;
            acc += at(x as i64 + r as i64 + 1) - at(x as i64 - r as i64);
        }
    }
    let mut out = vec![0.0f32; w * h];
    for x in 0..w {
        let at = |y: i64| tmp[y.clamp(0, h as i64 - 1) as usize * w + x] as f64;
        let mut acc: f64 = (-(r as i64)..=r as i64).map(at).sum();
        for y in 0..h {
            out[y * w + x] = (acc / n) as f32;
            acc += at(y as i64 + r as i64 + 1) - at(y as i64 - r as i64);
        }
    }
    out
}

/// Guided filter of `p` guided by `i` (He, Sun & Tang 2010).
fn guided(i: &[f32], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let mi = box_mean(i, w, h, r);
    let mp = box_mean(p, w, h, r);
    let ip: Vec<f32> = i.iter().zip(p).map(|(a, b)| a * b).collect();
    let ii: Vec<f32> = i.iter().map(|a| a * a).collect();
    let mip = box_mean(&ip, w, h, r);
    let mii = box_mean(&ii, w, h, r);
    let a: Vec<f32> = (0..w * h).map(|k| (mip[k] - mi[k] * mp[k]) / (mii[k] - mi[k] * mi[k] + eps)).collect();
    let b: Vec<f32> = (0..w * h).map(|k| mp[k] - a[k] * mi[k]).collect();
    let ma = box_mean(&a, w, h, r);
    let mb = box_mean(&b, w, h, r);
    (0..w * h).map(|k| ma[k] * i[k] + mb[k]).collect()
}

/// Fast guided filter (He & Sun 2015): the linear coefficients are computed on an `s×` subsampled
/// copy and upsampled bilinearly, visually equivalent at the large radii Clarity uses.
fn guided_fast(i: &[f32], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let s = (r / 8).clamp(1, 8);
    if s == 1 {
        return guided(i, p, w, h, r, eps);
    }
    let (sw, sh) = (w.div_ceil(s), h.div_ceil(s));
    let down = |v: &[f32]| -> Vec<f32> {
        let mut o = vec![0.0f32; sw * sh];
        let mut c = vec![0.0f32; sw * sh];
        for y in 0..h {
            for x in 0..w {
                let k = (y / s) * sw + x / s;
                o[k] += v[y * w + x];
                c[k] += 1.0;
            }
        }
        o.iter().zip(&c).map(|(a, b)| a / b.max(1.0)).collect()
    };
    let (si, sp) = (down(i), down(p));
    let rs = (r / s).max(1);
    let mi = box_mean(&si, sw, sh, rs);
    let mp = box_mean(&sp, sw, sh, rs);
    let ip: Vec<f32> = si.iter().zip(&sp).map(|(a, b)| a * b).collect();
    let ii: Vec<f32> = si.iter().map(|a| a * a).collect();
    let mip = box_mean(&ip, sw, sh, rs);
    let mii = box_mean(&ii, sw, sh, rs);
    let a: Vec<f32> = (0..sw * sh).map(|k| (mip[k] - mi[k] * mp[k]) / (mii[k] - mi[k] * mi[k] + eps)).collect();
    let b: Vec<f32> = (0..sw * sh).map(|k| mp[k] - a[k] * mi[k]).collect();
    let (ma, mb) = (box_mean(&a, sw, sh, rs), box_mean(&b, sw, sh, rs));
    let sf = s as f32;
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let fy = (y as f32 + 0.5) / sf - 0.5;
        for x in 0..w {
            let fx = (x as f32 + 0.5) / sf - 0.5;
            let av = bilinear(&ma, sw, sh, 1, 0, fx, fy);
            let bv = bilinear(&mb, sw, sh, 1, 0, fx, fy);
            out[y * w + x] = av * i[y * w + x] + bv;
        }
    }
    out
}

/// Dehaze: dark-channel-prior transmission on a ≤768 px proxy, upsampled (K. He et al. 2010).
fn dehaze_stage(px: &mut [[f32; 4]], w: usize, h: usize, amount: f32) {
    if w == 0 || h == 0 {
        return;
    }
    let k = w.max(h).div_ceil(768).max(1);
    let (sw, sh) = (w.div_ceil(k), h.div_ceil(k));
    let mut small = vec![[0.0f32; 3]; sw * sh];
    let mut cnt = vec![0f32; sw * sh];
    for y in 0..h {
        for x in 0..w {
            let i = (y / k) * sw + x / k;
            for c in 0..3 {
                small[i][c] += px[y * w + x][c].clamp(0.0, 1.0);
            }
            cnt[i] += 1.0;
        }
    }
    for (s, c) in small.iter_mut().zip(&cnt) {
        *s = s.map(|v| v / c.max(1.0));
    }
    let r = (sw.max(sh) / 60).max(2);
    let min_filter = |v: &[f32]| -> Vec<f32> {
        let mut a = vec![0.0f32; sw * sh];
        for y in 0..sh {
            for x in 0..sw {
                let (x0, x1) = (x.saturating_sub(r), (x + r).min(sw - 1));
                a[y * sw + x] = (x0..=x1).map(|xx| v[y * sw + xx]).fold(f32::MAX, f32::min);
            }
        }
        let mut b = vec![0.0f32; sw * sh];
        for y in 0..sh {
            for x in 0..sw {
                let (y0, y1) = (y.saturating_sub(r), (y + r).min(sh - 1));
                b[y * sw + x] = (y0..=y1).map(|yy| a[yy * sw + x]).fold(f32::MAX, f32::min);
            }
        }
        b
    };
    let dark = min_filter(&small.iter().map(|c| c[0].min(c[1]).min(c[2])).collect::<Vec<_>>());
    // Atmospheric light: mean colour of the brightest 0.1 % of the dark channel.
    let mut idx: Vec<usize> = (0..sw * sh).collect();
    idx.sort_by(|&a, &b| dark[b].total_cmp(&dark[a]));
    let top = (sw * sh / 1000).max(1);
    let mut air = [0.0f32; 3];
    for &i in &idx[..top] {
        for c in 0..3 {
            air[c] += small[i][c] / top as f32;
        }
    }
    let air = air.map(|v| v.max(0.2));
    let norm_dark = min_filter(&small.iter().map(|c| (c[0] / air[0]).min(c[1] / air[1]).min(c[2] / air[2])).collect::<Vec<_>>());
    let t_raw: Vec<f32> = norm_dark.iter().map(|d| 1.0 - 0.95 * d).collect();
    let guide: Vec<f32> = small.iter().map(|c| luma(*c)).collect();
    let t = guided(&guide, &t_raw, sw, sh, r * 2, 1e-3);
    let ks = k as f32;
    for y in 0..h {
        for x in 0..w {
            let tv = bilinear(&t, sw, sh, 1, 0, (x as f32 + 0.5) / ks - 0.5, (y as f32 + 0.5) / ks - 0.5).clamp(0.1, 1.0);
            let q = &mut px[y * w + x];
            for c in 0..3 {
                let i = q[c];
                let j = (i - air[c]) / tv + air[c];
                q[c] = if amount >= 0.0 { i + (j - i) * amount } else { i + (air[c] - i) * (-amount) * 0.5 * (1.0 - tv * 0.5) };
            }
        }
    }
}

/// Clarity: a large-radius guided filter's local-contrast boost, strongest in the midtones.
fn clarity_stage(px: &mut [[f32; 4]], w: usize, h: usize, amount: f32) {
    let long = w.max(h) as f32;
    let l: Vec<f32> = px.iter().map(|q| luma([q[0], q[1], q[2]]).max(0.0)).collect();
    let lc: Vec<f32> = l.iter().map(|v| v.min(1.0)).collect();
    let c = guided_fast(&lc, &lc, w, h, ((long * 0.02) as usize).max(3), 0.005);
    for (i, q) in px.iter_mut().enumerate() {
        let l0 = l[i];
        let mut lv = lc[i];
        let mw = 1.0 - (2.0 * lv.clamp(0.0, 1.0) - 1.0).powi(2);
        lv += amount * 0.8 * (lc[i] - c[i]) * mw;
        let lv = lv.max(0.0) + (l0 - lc[i]);
        let ratio = if l0 > 1e-5 { lv / l0 } else { 1.0 };
        for v in q.iter_mut().take(3) {
            *v = if l0 > 1e-5 { *v * ratio } else { *v + (lv - l0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_geom::Rect;

    fn buf(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> Buffer {
        let rect = Rect::new(0, 0, w as i32, h as i32);
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let c = f(x, y);
                px.push([c[0], c[1], c[2], 1.0]);
            }
        }
        Buffer { rect, px }
    }

    #[test]
    fn zero_sliders_are_the_identity() {
        let mut b = buf(16, 16, |x, y| [x as f32 / 15.0, y as f32 / 15.0, 0.4]);
        let want = b.px.clone();
        apply(&mut b, 0.0, 0.0);
        assert_eq!(b.px, want);
    }

    #[test]
    fn clarity_pushes_away_from_the_local_mean() {
        // A dark blob on a light field: positive clarity darkens the blob's edge (its luma is
        // below the local mean there) and brightens the field just outside it; flat regions (the
        // blob's centre, the far field) are left alone, and nothing leaves 0..=1.
        let mut b = buf(64, 64, |x, y| {
            let inside = (24..40).contains(&x) && (24..40).contains(&y);
            if inside { [0.2, 0.2, 0.2] } else { [0.8, 0.8, 0.8] }
        });
        let before = b.px.clone();
        apply(&mut b, 60.0, 0.0);
        let at = |x: usize, y: usize| y * 64 + x;
        let edge = at(24, 32);
        assert!(b.px[edge][0] < before[edge][0], "blob edge darkened: {} vs {}", b.px[edge][0], before[edge][0]);
        let field = at(22, 32);
        assert!(b.px[field][0] > before[field][0], "field brightened: {} vs {}", b.px[field][0], before[field][0]);
        let centre = at(32, 32);
        assert!((b.px[centre][0] - before[centre][0]).abs() < 1e-4, "flat centre untouched");
        assert!(b.px.iter().all(|p| (0.0..=1.0).contains(&p[0])));
    }

    #[test]
    fn dehaze_darkens_haze_and_leaves_a_clear_sky_alone() {
        // A hazy scene: a bright "sky" with a distant, washed-out darker patch. Positive dehaze
        // pulls the patch back towards its true (darker) colour; the flat sky has no dark channel
        // of its own and stays put.
        let mut b = buf(32, 32, |x, y| {
            let far = (12..20).contains(&x) && (12..20).contains(&y);
            if far { [0.35, 0.35, 0.35] } else { [0.8, 0.8, 0.8] }
        });
        let before = b.px.clone();
        apply(&mut b, 0.0, 50.0);
        let at = |x: usize, y: usize| y * 32 + x;
        let patch = at(16, 16);
        assert!(b.px[patch][0] < before[patch][0] - 0.1, "haze removed: {} vs {}", b.px[patch][0], before[patch][0]);
        let sky = at(2, 2);
        assert!((b.px[sky][0] - before[sky][0]).abs() < 0.05, "sky kept: {} vs {}", b.px[sky][0], before[sky][0]);
        // A black image has nothing to remove.
        let mut black = buf(32, 32, |_, _| [0.0, 0.0, 0.0]);
        let want = black.px.clone();
        apply(&mut black, 0.0, 50.0);
        assert!(black.px.iter().zip(&want).all(|(a, b)| (a[0] - b[0]).abs() < 1e-4));
    }

    #[test]
    fn box_mean_of_a_constant_is_the_constant() {
        let v = vec![0.25f32; 40 * 30];
        for r in [1usize, 3, 17] {
            let m = box_mean(&v, 40, 30, r);
            assert!(m.iter().all(|x| (x - 0.25).abs() < 1e-5), "r {r}");
        }
    }
}
