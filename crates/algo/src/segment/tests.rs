use super::gmm::Gmm;
use super::maxflow::Graph;
use super::*;
use photocraft_geom::Rect;

// ---------- max-flow ----------

#[test]
fn maxflow_tiny_graph() {
    // s→a 3, s→b 2, a→b 1, a→t 2, b→t 3: max flow 5.
    let mut g = Graph::with_capacity(2, 1);
    g.add_tweights(0, 3.0, 2.0);
    g.add_tweights(1, 2.0, 3.0);
    g.add_edge(0, 1, 1.0, 0.0);
    assert_eq!(g.maxflow(), 5.0);
}

#[test]
fn maxflow_clrs_network() {
    // The classic textbook network (max flow 23): v1..v4 = 0..3.
    let mut g = Graph::with_capacity(4, 5);
    g.add_tweights(0, 16.0, 0.0);
    g.add_tweights(1, 13.0, 0.0);
    g.add_tweights(2, 0.0, 20.0);
    g.add_tweights(3, 0.0, 4.0);
    g.add_edge(0, 1, 10.0, 4.0);
    g.add_edge(0, 2, 12.0, 0.0);
    g.add_edge(2, 1, 9.0, 0.0);
    g.add_edge(1, 3, 14.0, 0.0);
    g.add_edge(3, 2, 7.0, 0.0);
    assert_eq!(g.maxflow(), 23.0);
    // Min cut {s, v1, v2, v4} | {v3, t}.
    assert!(g.in_source(0) && g.in_source(1) && g.in_source(3));
    assert!(!g.in_source(2));
    assert_eq!(g.residual_out_of_source_side(), 0.0);
}

/// Reference max flow (Edmonds–Karp on a dense matrix; node 0 = s, n−1 = t).
fn edmonds_karp(cap: &mut [Vec<f64>]) -> f64 {
    let n = cap.len();
    let mut flow = 0.0;
    loop {
        let mut prev = vec![usize::MAX; n];
        prev[0] = 0;
        let mut q = std::collections::VecDeque::from([0usize]);
        while let Some(u) = q.pop_front() {
            for v in 0..n {
                if prev[v] == usize::MAX && cap[u][v] > 1e-9 {
                    prev[v] = u;
                    q.push_back(v);
                }
            }
        }
        if prev[n - 1] == usize::MAX {
            return flow;
        }
        let mut b = f64::MAX;
        let mut v = n - 1;
        while v != 0 {
            b = b.min(cap[prev[v]][v]);
            v = prev[v];
        }
        let mut v = n - 1;
        while v != 0 {
            cap[prev[v]][v] -= b;
            cap[v][prev[v]] += b;
            v = prev[v];
        }
        flow += b;
    }
}

#[test]
fn maxflow_matches_reference_on_random_graphs() {
    let mut rng = Rng::new(7);
    for trial in 0..60 {
        let n = 3 + (trial % 12);
        let mut g = Graph::with_capacity(n, n * 3);
        let mut cap = vec![vec![0.0f64; n + 2]; n + 2];
        let mut edges = Vec::new();
        for i in 0..n {
            let (s, t) = ((rng.f32() * 10.0).round(), (rng.f32() * 10.0).round());
            let (s, t) = (if rng.f32() < 0.4 { 0.0 } else { s }, if rng.f32() < 0.4 { 0.0 } else { t });
            g.add_tweights(i, s, t);
            cap[0][i + 1] += s as f64;
            cap[i + 1][n + 1] += t as f64;
        }
        for i in 0..n {
            for j in i + 1..n {
                if rng.f32() < 0.35 {
                    let (a, b) = ((rng.f32() * 8.0).round(), (rng.f32() * 8.0).round());
                    g.add_edge(i, j, a, b);
                    cap[i + 1][j + 1] += a as f64;
                    cap[j + 1][i + 1] += b as f64;
                    edges.push((i, j, a, b));
                }
            }
        }
        let orig = cap.clone();
        let reference = edmonds_karp(&mut cap);
        let f = g.maxflow();
        assert!((f - reference).abs() < 1e-3, "trial {trial}: {f} vs {reference}");
        // The reported cut has exactly that capacity.
        let side = |i: usize| g.in_source(i);
        let mut cut = 0.0;
        for i in 0..n {
            if side(i) {
                cut += orig[i + 1][n + 1];
            } else {
                cut += orig[0][i + 1];
            }
        }
        for &(i, j, a, b) in &edges {
            if side(i) && !side(j) {
                cut += a as f64;
            }
            if side(j) && !side(i) {
                cut += b as f64;
            }
        }
        assert!((cut - reference).abs() < 1e-3, "trial {trial}: cut {cut} vs flow {reference}");
    }
}

#[test]
fn grid_cut_separates_two_halves() {
    let img = RgbImage::from_fn(20, 10, |x, _| if x < 10 { [0.9, 0.1, 0.1] } else { [0.1, 0.1, 0.9] });
    let n = img.w * img.h;
    let mut fixed = vec![FREE; n];
    fixed[5 * 20 + 1] = HARD_FG;
    fixed[5 * 20 + 18] = HARD_BG;
    let zeros = vec![0.0; n];
    let beta = contrast_beta(&img);
    let cut = grid_cut(&img, &zeros, &zeros, &fixed, 50.0, beta);
    for y in 0..10 {
        for x in 0..20 {
            assert_eq!(cut[y * 20 + x], x < 10, "({x},{y})");
        }
    }
}

// ---------- GMM ----------

#[test]
fn gmm_recovers_synthetic_clusters() {
    let mut rng = Rng::new(3);
    let truth = [([0.2f32, 0.3, 0.8], 0.03f32, 0.5f32), ([0.8, 0.2, 0.1], 0.05, 0.3), ([0.5, 0.9, 0.4], 0.02, 0.2)];
    let mut samples = Vec::new();
    for (mean, sd, w) in truth {
        for _ in 0..(w * 6000.0) as usize {
            samples.push(mean.map(|m| m + sd * rng.normal()));
        }
    }
    let g = Gmm::fit(&samples, 3, gmm::DEFAULT_REG).unwrap();
    assert_eq!(g.comps.len(), 3);
    for (mean, sd, w) in truth {
        let c = g.comps.iter().min_by(|a, b| d2(a.mean, mean).total_cmp(&d2(b.mean, mean))).unwrap();
        assert!(d2(c.mean, mean).sqrt() < 0.01, "{:?} vs {mean:?}", c.mean);
        assert!((c.weight - w).abs() < 0.03, "{} vs {w}", c.weight);
        let var = c.cov[0][0];
        assert!((var.sqrt() - sd).abs() < 0.01, "sd {} vs {sd}", var.sqrt());
    }
    assert!(g.neg_log([0.2, 0.3, 0.8]) < g.neg_log([0.5, 0.5, 0.5]));
    // Refit keeps the model stable.
    let r = g.refit(&samples).unwrap();
    assert_eq!(r.comps.len(), 3);
}

// ---------- SLIC ----------

#[test]
fn slic_counts_compactness_and_edges() {
    let mut rng = Rng::new(11);
    let (w, h) = (120usize, 120usize);
    let img = RgbImage::from_fn(w, h, |x, _| if x < 60 { [0.8, 0.3, 0.2] } else { [0.2, 0.5, 0.8] });
    let img = RgbImage { px: img.px.iter().map(|p| p.map(|v| (v + 0.02 * rng.normal()).clamp(0.0, 1.0))).collect(), ..img };
    let sp = slic::slic(&img, 64, 10.0, 10);
    assert!((40..=90).contains(&sp.count), "count {}", sp.count);
    let mut bbox = vec![(usize::MAX, usize::MAX, 0usize, 0usize); sp.count];
    let mut left = vec![0usize; sp.count];
    let mut total = vec![0usize; sp.count];
    for y in 0..h {
        for x in 0..w {
            let l = sp.labels[y * w + x] as usize;
            let b = &mut bbox[l];
            *b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
            total[l] += 1;
            if x < 60 {
                left[l] += 1;
            }
        }
    }
    let s = sp.step;
    for (l, b) in bbox.iter().enumerate() {
        assert!(total[l] > 0);
        assert!(((b.2 - b.0 + 1) as f32) <= 3.0 * s && ((b.3 - b.1 + 1) as f32) <= 3.0 * s, "superpixel {l} too elongated: {b:?} (S={s})");
        // Superpixels respect the strong colour edge.
        let purity = left[l].max(total[l] - left[l]) as f32 / total[l] as f32;
        assert!(purity > 0.95, "superpixel {l} straddles the edge ({purity})");
    }
}

// ---------- GrabCut / object selection ----------

/// A textured disc on a noisy background: (image, ground-truth mask).
pub(crate) fn disc_scene(w: usize, h: usize, cx: f32, cy: f32, r: f32, seed: u64) -> (RgbImage, Vec<bool>) {
    let mut rng = Rng::new(seed);
    let mut img = RgbImage::new(w, h);
    let mut truth = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let inside = dx * dx + dy * dy <= r * r;
            truth[y * w + x] = inside;
            let n = 0.05 * rng.normal();
            img.px[y * w + x] = if inside {
                // Stripes between two warm colours plus noise.
                let t = (((x + y) as f32 * 0.35).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
                [0.85 + 0.05 * t + n, 0.2 + 0.3 * t + n, 0.15 + n]
            } else {
                [0.25 + n, 0.45 + n, 0.7 + n]
            }
            .map(|v| v.clamp(0.0, 1.0));
        }
    }
    (img, truth)
}

pub(crate) fn iou(region: &Region, truth: &[bool], w: usize, h: usize) -> f32 {
    let (mut inter, mut uni) = (0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            let a = region.at(x as i32, y as i32) >= 0.5;
            let b = truth[y * w + x];
            inter += (a && b) as usize;
            uni += (a || b) as usize;
        }
    }
    inter as f32 / uni.max(1) as f32
}

#[test]
fn grabcut_trimap_on_disc() {
    let (img, truth) = disc_scene(120, 100, 60.0, 50.0, 30.0, 1);
    let mut tri = vec![grabcut::BG; img.w * img.h];
    for y in 12..88 {
        for x in 22..98 {
            tri[y * img.w + x] = grabcut::PR_FG;
        }
    }
    grabcut::grabcut(&img, &mut tri, 8).unwrap();
    let (mut inter, mut uni) = (0, 0);
    for (t, g) in tri.iter().zip(&truth) {
        let a = *t == grabcut::PR_FG || *t == grabcut::FG;
        inter += (a && *g) as usize;
        uni += (a || *g) as usize;
    }
    let iou = inter as f32 / uni as f32;
    assert!(iou > 0.95, "IoU {iou}");
}

#[test]
fn object_select_disc_full_resolution() {
    let (img, truth) = disc_scene(200, 160, 100.0, 80.0, 50.0, 2);
    let s = ImageSampler { img: &img, origin: (0, 0) };
    let canvas = Rect::new(0, 0, 200, 160);
    let reg = grabcut::object_select(&s, canvas, Rect::new(40, 20, 160, 140), 1_000_000).unwrap();
    let v = iou(&reg, &truth, 200, 160);
    assert!(v > 0.95, "IoU {v}");
}

#[test]
fn object_select_downsampled_with_band_refinement() {
    let (img, truth) = disc_scene(640, 480, 330.0, 250.0, 150.0, 3);
    let s = ImageSampler { img: &img, origin: (0, 0) };
    let canvas = Rect::new(0, 0, 640, 480);
    // A small working budget forces step > 1 and the full-resolution band re-cut.
    let reg = grabcut::object_select(&s, canvas, Rect::new(160, 80, 500, 420), 20_000).unwrap();
    let v = iou(&reg, &truth, 640, 480);
    assert!(v > 0.95, "IoU {v}");
    // Boundary accuracy: few pixels wrong far (> 2 px) from the true edge.
    let mut far_wrong = 0;
    for y in 0..480 {
        for x in 0..640 {
            let (dx, dy) = (x as f32 + 0.5 - 330.0, y as f32 + 0.5 - 250.0);
            let d = (dx * dx + dy * dy).sqrt() - 150.0;
            if d.abs() > 2.0 && (reg.at(x, y) >= 0.5) != truth[y as usize * 640 + x as usize] {
                far_wrong += 1;
            }
        }
    }
    assert!(far_wrong < 300, "{far_wrong} pixels wrong away from the edge");
}

// ---------- Quick selection ----------

/// Left: textured warm region; right: cool region; strong vertical edge at x = 60.
fn two_regions(w: usize, h: usize) -> RgbImage {
    let mut rng = Rng::new(5);
    RgbImage::from_fn(w, h, |x, y| {
        let n = 0.04 * rng.normal();
        let t = ((x as f32 * 0.5).sin() * (y as f32 * 0.3).cos()) * 0.05;
        if x < 60 { [0.8 + t + n, 0.5 + n, 0.3 + t] } else { [0.2 + n, 0.35 + t, 0.6 + n] }.map(|v| v.clamp(0.0, 1.0))
    })
}

#[test]
fn quick_select_with_points_far_off_canvas_selects_nothing() {
    // A cast saturates to i32::MAX; adding one to it once overflowed.
    let img = two_regions(140, 100);
    let s = ImageSampler { img: &img, origin: (0, 0) };
    let canvas = Rect::new(0, 0, 140, 100);
    for p in [(3e9, 0.0), (0.0, 3e9), (f32::MAX, f32::MAX)] {
        assert!(quick::quick_select(&s, canvas, &[p], 8.0).is_none(), "{p:?}");
    }
}

#[test]
fn quick_select_grows_without_leaking() {
    let img = two_regions(140, 100);
    let s = ImageSampler { img: &img, origin: (0, 0) };
    let canvas = Rect::new(0, 0, 140, 100);
    let reg = quick::quick_select(&s, canvas, &[(20.0, 50.0), (35.0, 50.0)], 8.0).unwrap();
    let mut inside = 0;
    for y in 0..100 {
        for x in 0..140 {
            let v = reg.at(x, y);
            if x >= 62 {
                assert!(v < 0.5, "leaked across the edge at ({x},{y}): {v}");
            }
            if x < 58 && v >= 0.5 {
                inside += 1;
            }
        }
    }
    // Grew over (nearly) the whole left region, far beyond the stroke.
    assert!(inside as f32 > 0.97 * (58.0 * 100.0), "only {inside} pixels selected");
}

#[test]
fn quick_select_stops_at_thin_line() {
    // Same colour on both sides of a dark 2-pixel line.
    let mut rng = Rng::new(9);
    let img = RgbImage::from_fn(120, 80, |x, _| {
        let n = 0.03 * rng.normal();
        if x == 60 || x == 61 { [0.05, 0.05, 0.05] } else { [0.6 + n, 0.6 + n, 0.55 + n] }
    });
    let s = ImageSampler { img: &img, origin: (0, 0) };
    let reg = quick::quick_select(&s, Rect::new(0, 0, 120, 80), &[(30.0, 40.0)], 6.0).unwrap();
    assert!(reg.at(10, 10) >= 0.5 && reg.at(50, 70) >= 0.5);
    for y in 0..80 {
        for x in 63..120 {
            assert!(reg.at(x, y) < 0.5, "crossed the line at ({x},{y})");
        }
    }
}

#[test]
fn quick_select_a_drag_selects_around_itself_not_a_big_object() {
    // Photoshop 25.4 on this very scene: an 80 px drag inside a 180 px radius disc selects about
    // the stroke (5285 px), not the disc (~100,000 px): the disc is far too big for the stroke.
    let (img, truth) = disc_scene(600, 500, 300.0, 250.0, 180.0, 4);
    let s = ImageSampler { img: &img, origin: (0, 0) };
    let reg = quick::quick_select(&s, Rect::new(0, 0, 600, 500), &[(260.0, 250.0), (340.0, 250.0)], 30.0).unwrap();
    let n = count(&reg);
    assert!((3000..9000).contains(&n), "{n} px");
    assert!(reg.at(300, 250) >= 0.5 && reg.at(262, 250) >= 0.5);
    assert!(iou(&reg, &truth, 600, 500) < 0.2);
}

// ---------- Quick selection: Photoshop's measured behaviour ----------
//
// Each case below was run in Photoshop 25.4 (the same image and stroke, the mouse held 1 s so
// its solver settles); the numbers in the comments are Photoshop's.

const QS_RED: [f32; 3] = [200.0 / 255.0, 60.0 / 255.0, 60.0 / 255.0];
const QS_BLUE: [f32; 3] = [60.0 / 255.0, 60.0 / 255.0, 200.0 / 255.0];

/// A `w` x `h` image of `bg` with `fg` over `[x0, x1) x [y0, y1)`.
fn qs_scene(w: usize, h: usize, bg: [f32; 3], fg: [f32; 3], rect: (usize, usize, usize, usize)) -> RgbImage {
    let (x0, y0, x1, y1) = rect;
    RgbImage::from_fn(w, h, |x, y| if x >= x0 && x < x1 && y >= y0 && y < y1 { fg } else { bg })
}

/// A centred `side` square of `fg` on `bg` in a `w` x `w` image.
fn qs_square(w: usize, side: usize, fg: [f32; 3], bg: [f32; 3]) -> RgbImage {
    let x0 = (w - side) / 2;
    qs_scene(w, w, bg, fg, (x0, x0, x0 + side, x0 + side))
}

fn qs(img: &RgbImage, pts: &[(f32, f32)], size: f32) -> Region {
    let s = ImageSampler { img, origin: (0, 0) };
    quick::quick_select(&s, Rect::new(0, 0, img.w as i32, img.h as i32), pts, size).expect("a selection")
}

fn count(r: &Region) -> usize {
    r.mask.iter().filter(|v| **v >= 128).count()
}

/// Selected pixels inside and outside `[x0, x1) x [y0, y1)`.
fn inside_outside(r: &Region, rect: (i32, i32, i32, i32)) -> (usize, usize) {
    let (mut i, mut o) = (0, 0);
    for y in r.bbox.y0..r.bbox.y1 {
        for x in r.bbox.x0..r.bbox.x1 {
            if r.at(x, y) >= 0.5 {
                if x >= rect.0 && x < rect.2 && y >= rect.1 && y < rect.3 { i += 1 } else { o += 1 }
            }
        }
    }
    (i, o)
}

#[test]
fn quick_select_works_at_full_or_exactly_half_resolution() {
    // Up to 512 x 512 pixels Photoshop works on the whole document, above at half resolution,
    // whatever the document's shape or the brush.
    assert_eq!(quick::working_step(512, 512), 1);
    assert_eq!(quick::working_step(513, 512), 2);
    assert_eq!(quick::working_step(520, 300), 1);
    assert_eq!(quick::working_step(900, 300), 2);
    assert_eq!(quick::working_step(5000, 3500), 2);
    // Half resolution: the mask is scaled up bilinearly, so a selected square's edge steps
    // 64, 191, 255 (Photoshop's own values on a 640 px document).
    let img = qs_square(640, 200, QS_RED, QS_BLUE);
    let r = qs(&img, &[(320.0, 320.0)], 30.0);
    let row: Vec<u8> = (216..223).map(|x| (r.at(x, 320) * 255.0).round() as u8).collect();
    assert_eq!(row, [0, 0, 0, 64, 191, 255, 255]);
}

#[test]
fn quick_select_click_in_a_flat_area_selects_about_the_brush() {
    // Photoshop: 1025-1120 px for a 30 px brush (the disc grows a little while the button is held).
    let img = qs_scene(400, 400, QS_RED, QS_RED, (0, 0, 0, 0));
    let n = count(&qs(&img, &[(200.0, 200.0)], 30.0));
    assert!((700..1600).contains(&n), "{n} px");
}

#[test]
fn quick_select_click_takes_an_object_only_while_it_is_small_enough() {
    // 30 px brush, red on blue: Photoshop selects a 220 px square whole, not a 240 px one.
    let img = qs_square(500, 220, QS_RED, QS_BLUE);
    let (i, o) = inside_outside(&qs(&img, &[(250.0, 250.0)], 30.0), (140, 140, 360, 360));
    assert!(i >= 220 * 220 - 10 && o == 0, "220: {i} in, {o} out");
    let img = qs_square(500, 240, QS_RED, QS_BLUE);
    let n = count(&qs(&img, &[(250.0, 250.0)], 30.0));
    assert!(n < 2000, "240: {n} px");
    // A weak edge (15 levels) holds only a small square: 94 px yes, 120 px no.
    let (lo, hi) = ([125.0 / 255.0, 110.0 / 255.0, 110.0 / 255.0], [110.0 / 255.0, 110.0 / 255.0, 125.0 / 255.0]);
    let img = qs_square(500, 94, lo, hi);
    let (i, o) = inside_outside(&qs(&img, &[(250.0, 250.0)], 30.0), (203, 203, 297, 297));
    assert!(i >= 94 * 94 - 10 && o == 0, "94: {i} in, {o} out");
    let img = qs_square(500, 120, lo, hi);
    let n = count(&qs(&img, &[(250.0, 250.0)], 30.0));
    assert!(n < 2000, "120: {n} px");
}

#[test]
fn quick_select_takes_a_narrow_bar_end_to_end_but_only_the_end_near_it() {
    // An 80 x 400 bar: a click in the middle selects all of it; a click 30 px from its end only
    // the end (Photoshop: 3762 px, cut just above the brush).
    let img = qs_scene(500, 500, QS_BLUE, QS_RED, (100, 50, 180, 450));
    let (i, o) = inside_outside(&qs(&img, &[(140.0, 250.0)], 30.0), (100, 50, 180, 450));
    assert!(i >= 80 * 400 - 10 && o == 0, "middle: {i} in, {o} out");
    let r = qs(&img, &[(140.0, 420.0)], 30.0);
    let (i, o) = inside_outside(&r, (100, 380, 180, 450));
    assert!(i > 2500 && o == 0, "near the end: {i} in the end, {o} elsewhere");
}

#[test]
fn quick_select_a_short_drag_takes_what_a_click_does_not() {
    // A 300 px square: a click selects about the brush; a 60 px drag the whole square.
    let img = qs_square(500, 300, QS_RED, QS_BLUE);
    assert!(count(&qs(&img, &[(250.0, 250.0)], 30.0)) < 2000);
    let (i, o) = inside_outside(&qs(&img, &[(220.0, 250.0), (280.0, 250.0)], 30.0), (100, 100, 400, 400));
    assert!(i >= 300 * 300 - 10 && o == 0, "{i} in, {o} out");
}

#[test]
fn quick_select_on_a_large_document_grows_its_window_to_the_object() {
    // A 1000 x 600 document (half resolution) with an 80 x 960 bar: Photoshop selects all of it
    // from a click in the middle (76800 px). The working window starts around the stroke and grows
    // until the selection no longer runs into it.
    let img = qs_scene(1000, 600, QS_BLUE, QS_RED, (20, 260, 980, 340));
    // The mouse on pixel (500, 300), as in Photoshop: its centre.
    let (i, o) = inside_outside(&qs(&img, &[(500.5, 300.5)], 30.0), (20, 260, 980, 340));
    assert!(i >= 960 * 80 * 98 / 100 && o < 2000, "{i} in, {o} out");
}

// ---------- Select subject ----------

#[test]
fn select_subject_finds_centred_object() {
    let (img, truth) = disc_scene(200, 150, 100.0, 75.0, 40.0, 6);
    let s = ImageSampler { img: &img, origin: (0, 0) };
    let reg = subject::select_subject(&s, Rect::new(0, 0, 200, 150)).unwrap();
    let v = iou(&reg, &truth, 200, 150);
    assert!(v > 0.9, "IoU {v}");
}

#[test]
fn boundary_prior_skips_a_subject_cut_off_by_the_frame() {
    // Sides: 1 left, 2 top, 4 right, 8 bottom; colours on a line.
    let sides = [1u8, 2, 8, 0, 8, 8, 9];
    let colour = [0.0f32, 3.0, 80.0, 0.0, 5.0, 82.0, 1.0];
    let d = |a: usize, b: usize| (colour[a] - colour[b]).abs();
    // Background recurs round the frame; the subject's colour shows on the bottom side only.
    assert_eq!(subject::background_border(&sides, &d), vec![true, true, false, false, true, false, true]);
    // A frame whose sides all differ keeps the whole border, as the paper's prior does.
    let colour = [0.0f32, 50.0, 100.0, 150.0];
    let d = |a: usize, b: usize| (colour[a] - colour[b]).abs();
    assert_eq!(subject::background_border(&[1, 2, 4, 8], &d), vec![true; 4]);
}

#[test]
fn select_subject_reaches_the_frame_edge_it_is_cut_off_by() {
    // A subject running out of the bottom of the frame (a portrait's shoulders, say).
    let (img, truth) = disc_scene(200, 150, 100.0, 135.0, 60.0, 11);
    let s = ImageSampler { img: &img, origin: (0, 0) };
    let reg = subject::select_subject(&s, Rect::new(0, 0, 200, 150)).unwrap();
    let v = iou(&reg, &truth, 200, 150);
    assert!(v > 0.85, "IoU {v}");
    // The bottom row under the subject is selected, the background beside it is not.
    assert!(reg.at(100, 149) >= 0.5 && reg.at(5, 149) < 0.5, "{} {}", reg.at(100, 149), reg.at(5, 149));
}

/// `img` placed at `(ox, oy)` on a white `w × h` canvas (a photo layer on a larger document).
fn on_white(img: &RgbImage, w: usize, h: usize, ox: usize, oy: usize) -> RgbImage {
    RgbImage::from_fn(w, h, |x, y| if x >= ox && y >= oy && x - ox < img.w && y - oy < img.h { img.at(x - ox, y - oy) } else { [1.0; 3] })
}

#[test]
fn content_window_trims_flat_margins_only() {
    let (photo, _) = disc_scene(60, 40, 30.0, 20.0, 10.0, 3);
    let img = on_white(&photo, 100, 50, 25, 4);
    let s = ImageSampler { img: &img, origin: (10, 20) };
    let canvas = Rect::new(10, 20, 110, 70);
    assert_eq!(subject::flat_margins(&img).0, [25, 4, 15, 6]);
    // Found on full-resolution pixels, or on 3×3 blocks and then snapped to the exact edge.
    for step in [1, 3] {
        let coarse = img.downsample(step);
        assert_eq!(subject::content_window(&s, canvas, &coarse, step), Some(Rect::new(35, 24, 95, 64)), "step {step}");
    }
    // A photo with no margins, a blank canvas, and an object on a plain white backdrop (the
    // backdrop belongs to the scene) keep the whole frame.
    assert_eq!(subject::flat_margins(&photo).0, [0; 4]);
    assert_eq!(subject::flat_margins(&RgbImage::from_fn(30, 30, |_, _| [1.0; 3])).0, [0; 4]);
    let product = RgbImage::from_fn(80, 60, |x, y| {
        let (dx, dy) = (x as f32 - 40.0, y as f32 - 30.0);
        if dx * dx + dy * dy < 400.0 { [0.8, 0.2, 0.1] } else { [1.0; 3] }
    });
    assert_eq!(subject::flat_margins(&product).0, [0; 4]);
}

#[test]
fn select_subject_ignores_a_white_canvas_round_the_photo() {
    // A busy photo (the disc on noise) in the middle of a wider white document: the subject is
    // the disc, not the whole photo rectangle.
    let (photo, truth) = disc_scene(160, 150, 80.0, 75.0, 40.0, 12);
    let (w, h, ox) = (320usize, 150usize, 80usize);
    let img = on_white(&photo, w, h, ox, 0);
    let s = ImageSampler { img: &img, origin: (0, 0) };
    let reg = subject::select_subject(&s, Rect::new(0, 0, w as i32, h as i32)).unwrap();
    let truth: Vec<bool> = (0..w * h).map(|i| (i % w).checked_sub(ox).is_some_and(|x| x < 160 && truth[(i / w) * 160 + x])).collect();
    let v = iou(&reg, &truth, w, h);
    assert!(v > 0.85, "IoU {v}");
    assert!(reg.at(ox as i32 + 3, 3) < 0.5, "photo corner selected");
}

#[test]
fn probability_maps_upsample_to_soft_regions() {
    // A 4 × 2 map: right half certain, left half not; over a 40 × 20 window at (5, 7).
    let prob = [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0];
    let window = Rect::new(5, 7, 45, 27);
    let r = subject::region_from_probability(&prob, 4, 2, window).unwrap();
    assert!(r.at(40, 15) > 0.99 && r.at(8, 15) < 0.01);
    // Bilinear between the halves: the edge is soft, and the box is trimmed to what's selected.
    let mid = r.at(25, 15);
    assert!(mid > 0.0 && mid < 1.0, "{mid}");
    assert!(r.bbox.x0 > window.x0 && r.bbox.x1 == window.x1);
    // Nothing likely, mismatched sizes, empty windows and NaNs give nothing, never a panic.
    assert!(subject::region_from_probability(&[0.2; 8], 4, 2, window).is_none());
    assert!(subject::region_from_probability(&prob, 3, 2, window).is_none());
    assert!(subject::region_from_probability(&prob, 4, 2, Rect::new(0, 0, 0, 5)).is_none());
    assert!(subject::region_from_probability(&[f32::NAN; 8], 4, 2, window).is_none());
}

#[test]
fn main_subjects_drop_small_objects_but_keep_peers() {
    // A 60 × 20 map: a big object (400 px), a small one (16 px) and a soft fringe round each.
    let (w, h) = (60, 20);
    let blob = |x0: usize, y0: usize, s: usize, p: &mut Vec<f32>| {
        for y in y0..y0 + s {
            for x in x0..x0 + s {
                p[y * w + x] = 0.95;
            }
        }
        for x in x0..x0 + s {
            p[(y0 + s) * w + x] = 0.3; // fringe just below
        }
    };
    let mut p = vec![0.0f32; w * h];
    blob(2, 0, 18, &mut p);
    blob(40, 2, 4, &mut p);
    subject::keep_main_subjects(&mut p, w, h);
    assert_eq!(p[5 * w + 5], 0.95, "the main object stays");
    assert_eq!(p[18 * w + 5], 0.3, "with its soft fringe");
    assert_eq!((p[3 * w + 41], p[6 * w + 41]), (0.0, 0.0), "the small object and its fringe go");
    // Two objects of similar size both stay.
    let mut p = vec![0.0f32; w * h];
    blob(2, 0, 15, &mut p);
    blob(35, 0, 12, &mut p);
    subject::keep_main_subjects(&mut p, w, h);
    assert!(p[5 * w + 5] > 0.9 && p[5 * w + 40] > 0.9);
    // Nothing likely, or a wrong size: unchanged, never a panic.
    let mut q = vec![0.2f32; 12];
    subject::keep_main_subjects(&mut q, 4, 3);
    assert_eq!(q, vec![0.2; 12]);
    subject::keep_main_subjects(&mut q, 5, 3);
}

#[test]
fn subject_window_is_inside_flat_margins() {
    let (photo, _) = disc_scene(60, 40, 30.0, 20.0, 10.0, 3);
    let img = on_white(&photo, 100, 50, 25, 4);
    let s = ImageSampler { img: &img, origin: (0, 0) };
    assert_eq!(subject::subject_window(&s, Rect::new(0, 0, 100, 50)), Rect::new(25, 4, 85, 44));
    let p = ImageSampler { img: &photo, origin: (0, 0) };
    assert_eq!(subject::subject_window(&p, Rect::new(0, 0, 60, 40)), Rect::new(0, 0, 60, 40));
}

#[test]
fn saliency_highlights_the_object() {
    let (img, truth) = disc_scene(120, 90, 60.0, 45.0, 22.0, 8);
    let sal = subject::saliency(&img);
    let (mut fi, mut fn_, mut bi, mut bn) = (0.0, 0.0, 0.0, 0.0);
    for (s, t) in sal.iter().zip(&truth) {
        if *t {
            fi += s;
            fn_ += 1.0;
        } else {
            bi += s;
            bn += 1.0;
        }
    }
    assert!(fi / fn_ > 0.6 && bi / bn < 0.25, "fg {} bg {}", fi / fn_, bi / bn);
}

// ---------- Focus area ----------

#[test]
fn focus_area_selects_sharp_half() {
    let mut rng = Rng::new(12);
    let (w, h) = (160usize, 100usize);
    let noise: Vec<f32> = (0..w * h).map(|_| rng.f32()).collect();
    // Right half: heavily blurred copy of the texture.
    let blurred = crate::matting::box_mean(&noise, w, h, 4);
    let img = RgbImage::from_fn(w, h, |x, y| {
        let v = if x < 80 { noise[y * w + x] } else { blurred[y * w + x] };
        [v, v * 0.8, 0.3]
    });
    let s = ImageSampler { img: &img, origin: (0, 0) };
    let reg = focus::focus_area(&s, Rect::new(0, 0, w as i32, h as i32), 0.5, 0.0).unwrap();
    let (mut l, mut r) = (0usize, 0usize);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if reg.at(x, y) >= 0.5 {
                if x < 80 { l += 1 } else { r += 1 }
            }
        }
    }
    assert!(l as f32 > 0.85 * 80.0 * h as f32, "sharp half {l}");
    assert!((r as f32) < 0.1 * 80.0 * h as f32, "blurred half {r}");
}

// ---------- helpers ----------

#[test]
fn sampler_downsampling_matches_image_downsample() {
    let img = RgbImage::from_fn(37, 29, |x, y| [x as f32 / 37.0, y as f32 / 29.0, ((x * y) % 7) as f32 / 7.0]);
    let s = ImageSampler { img: &img, origin: (5, 3) };
    let a = s.rgb_scaled(Rect::new(5, 3, 42, 32), 4);
    let b = img.downsample(4);
    assert_eq!((a.w, a.h), (b.w, b.h));
    for (p, q) in a.px.iter().zip(&b.px) {
        for c in 0..3 {
            assert!((p[c] - q[c]).abs() < 1e-5);
        }
    }
}

#[test]
fn clean_mask_drops_islands_and_fills_holes() {
    let (w, h) = (40, 40);
    let mut m = vec![false; w * h];
    for y in 5..30 {
        for x in 5..30 {
            m[y * w + x] = true;
        }
    }
    m[15 * w + 15] = false; // hole
    m[36 * w + 36] = true; // island
    let c = clean_mask(&m, w, h, 0.1, 0.05);
    assert!(c[15 * w + 15]);
    assert!(!c[36 * w + 36]);
}

/// Writes the quick-selection test scenes as raw RGB8 (for measuring them in Photoshop).
#[test]
#[ignore]
fn dump_quick_scenes() {
    let Ok(dir) = std::env::var("QS_DUMP") else { return };
    let save = |name: &str, img: &RgbImage| {
        let bytes: Vec<u8> = img.px.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
        std::fs::write(format!("{dir}/{name}_{}x{}.rgb", img.w, img.h), bytes).unwrap();
    };
    save("two_regions", &two_regions(140, 100));
    let mut rng = Rng::new(9);
    let line = RgbImage::from_fn(120, 80, |x, _| {
        let n = 0.03 * rng.normal();
        if x == 60 || x == 61 { [0.05, 0.05, 0.05] } else { [0.6 + n, 0.6 + n, 0.55 + n] }
    });
    save("thin_line", &line);
    save("disc_scene", &disc_scene(600, 500, 300.0, 250.0, 180.0, 4).0);
}
