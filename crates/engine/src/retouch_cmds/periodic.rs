//! Bounded reads from the repeating document tile used by Pattern Preview.

use super::*;

/// At most four disjoint rectangles in the document tile. Projecting the dirty area separately
/// from the stroke bounds avoids scanning the whole document when a small dab crosses a seam.
pub(super) fn project(rect: Rect, tile: Rect) -> Vec<Rect> {
    if rect.is_empty() || tile.is_empty() {
        return Vec::new();
    }
    let axis = |a: i32, b: i32, lo: i32, hi: i32| {
        let period = i64::from(hi) - i64::from(lo);
        let length = i64::from(b) - i64::from(a);
        if length >= period {
            return vec![(lo, hi)];
        }
        let start = i64::from(lo) + (i64::from(a) - i64::from(lo)).rem_euclid(period);
        let end = start + length;
        if end <= i64::from(hi) { vec![(start as i32, end as i32)] } else { vec![(start as i32, hi), (lo, (i64::from(lo) + end - i64::from(hi)) as i32)] }
    };
    let xs = axis(rect.x0, rect.x1, tile.x0, tile.x1);
    let ys = axis(rect.y0, rect.y1, tile.y0, tile.y1);
    ys.into_iter().flat_map(|(y0, y1)| xs.iter().map(move |&(x0, x1)| Rect::new(x0, y0, x1, y1))).collect()
}

/// Composite periodic copies of one stroke, restoring each dirty patch from pre-stroke pixels.
/// Like the brush engine's union of repeated passes, maximum coverage wins: stroke opacity is
/// applied once, even when a footprint covers the same destination through multiple tiles.
pub(super) struct Composite<'a> {
    pub target: &'a mut Surface,
    pub before: &'a Surface,
    pub tile: Rect,
    pub bounds: Rect,
    pub opacity: f32,
    pub selection: Option<&'a Surface>,
    pub lock: bool,
    pub mode: BlendMode,
}

impl Composite<'_> {
    pub fn draw(&mut self, dirty: Rect, coverage: impl Fn(i32, i32) -> f32, mut paint: impl FnMut(Rect) -> Result<Region>) -> Result<Rect> {
        let mut damage = Rect::EMPTY;
        let mut work = 0_u64;
        for rect in project(dirty, self.tile) {
            // Fixed-size patches bound scratch memory independently of canvas dimensions.
            let mut y = rect.y0;
            while y < rect.y1 {
                let y1 = y.saturating_add(64).min(rect.y1);
                let mut x = rect.x0;
                while x < rect.x1 {
                    let x1 = x.saturating_add(64).min(rect.x1);
                    let dst = Rect::new(x, y, x1, y1);
                    self.patch(dst, &coverage, &mut paint, &mut work)?;
                    damage = damage.union(&dst);
                    x = x1;
                }
                y = y1;
            }
        }
        Ok(damage)
    }

    fn patch(&mut self, dst: Rect, coverage: &impl Fn(i32, i32) -> f32, paint: &mut impl FnMut(Rect) -> Result<Region>, work: &mut u64) -> Result<()> {
        let (w, h) = (i64::from(self.tile.width()), i64::from(self.tile.height()));
        let nx0 = (i64::from(self.bounds.x0) - i64::from(dst.x1)).div_euclid(w) + 1;
        let nx1 = (i64::from(self.bounds.x1) - 1 - i64::from(dst.x0)).div_euclid(w);
        let ny0 = (i64::from(self.bounds.y0) - i64::from(dst.y1)).div_euclid(h) + 1;
        let ny1 = (i64::from(self.bounds.y1) - 1 - i64::from(dst.y0)).div_euclid(h);
        let mut cov = vec![0.0_f32; dst.width() as usize * dst.height() as usize];
        let mut image = Region::new(dst, self.target.channels());
        for ny in ny0..=ny1 {
            for nx in nx0..=nx1 {
                let (dx, dy) = (nx * w, ny * h);
                let r = Rect::new(
                    (i64::from(dst.x0) + dx).max(i64::from(self.bounds.x0)) as i32,
                    (i64::from(dst.y0) + dy).max(i64::from(self.bounds.y0)) as i32,
                    (i64::from(dst.x1) + dx).min(i64::from(self.bounds.x1)) as i32,
                    (i64::from(dst.y1) + dy).min(i64::from(self.bounds.y1)) as i32,
                );
                *work = work.saturating_add(r.width() as u64 * r.height() as u64);
                if *work > 64 * 1024 * 1024 {
                    return Err(bad("paint.cloneStamp", "wrapped stroke exceeds its coverage budget"));
                }
                let mut sampled = None;
                for y in r.y0..r.y1 {
                    for x in r.x0..r.x1 {
                        let value = coverage(x, y);
                        let (tx, ty) = ((i64::from(x) - dx) as i32, (i64::from(y) - dy) as i32);
                        let i = image.index(tx, ty);
                        let Some(previous) = cov.get_mut(i) else { continue };
                        if value <= *previous {
                            continue;
                        }
                        if sampled.is_none() {
                            sampled = Some(paint(r)?);
                        }
                        if let Some(source) = &sampled {
                            image.px_mut(tx, ty).copy_from_slice(source.px(x, y));
                            *previous = value;
                        }
                    }
                }
            }
        }
        self.target.write_region(dst, &self.before.read_region(dst));
        apply_coverage(self.target, dst, &cov, self.opacity, self.selection, self.lock, &image, self.mode);
        Ok(())
    }
}

pub(super) fn sample(pre: &Document, id: Option<LayerId>, surf: &Surface, which: SampleLayers, rect: Rect, cmd: &str) -> Result<Region> {
    let tile = pre.bounds();
    if tile.is_empty() {
        return Err(bad(cmd, "cannot repeat an empty document"));
    }
    if rect.is_empty() {
        return Ok(Region::new(Rect::EMPTY, surf.channels()));
    }
    let (w, h, n) = (rect.width() as usize, rect.height() as usize, surf.channels());
    let len = w
        .checked_mul(h)
        .and_then(|v| v.checked_mul(n))
        .filter(|&v| v <= 16 * 1024 * 1024)
        .ok_or_else(|| bad(cmd, "periodic source window exceeds its 64 MiB budget"))?;
    let mut out = Region { rect, ch: n, data: vec![0.0; len] };
    // Split at tile seams. Read only the required rectangles, including the bilinear apron,
    // rather than allocating or compositing the entire document for a small clone dab.
    let mut dy = 0;
    while dy < h {
        let sy = (i64::from(rect.y0) + dy as i64 - i64::from(tile.y0)).rem_euclid(i64::from(tile.height())) as i32 + tile.y0;
        let rows = (h - dy).min((tile.y1 - sy) as usize);
        let mut dx = 0;
        while dx < w {
            let sx = (i64::from(rect.x0) + dx as i64 - i64::from(tile.x0)).rem_euclid(i64::from(tile.width())) as i32 + tile.x0;
            let cols = (w - dx).min((tile.x1 - sx) as usize);
            let part = super::sample(pre, id, surf, which, Rect::new(sx, sy, sx + cols as i32, sy + rows as i32));
            for (dst, src) in out.data.chunks_exact_mut(w * n).skip(dy).take(rows).zip(part.data.chunks_exact(cols * n)) {
                if let Some(dst) = dst.get_mut(dx * n..(dx + cols) * n) {
                    dst.copy_from_slice(src);
                }
            }
            dx += cols;
        }
        dy += rows;
    }
    Ok(out)
}
