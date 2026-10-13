//! A solid fill blurred by the engine's Gaussian Blur stays uniform at the canvas border: the
//! filter repeats the canvas-edge pixel for out-of-bounds taps, which is an identity on a
//! constant. Both blur arms are covered (radius <= 4 the exact separable kernel, larger radii
//! the three box passes), at every bit depth, and undo still restores the fill.

use photocraft_engine::Session;
use photocraft_geom::Rect;
use serde_json::json;

const W: usize = 100;

/// The active layer's pixels over the whole canvas, interleaved RGBA.
fn active_pixels(s: &Session) -> Vec<f32> {
    let st = s.active().unwrap();
    st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap().read_region(Rect::new(0, 0, W as i32, W as i32))
}

#[test]
fn gaussian_blur_preserves_a_solid_fill_at_the_canvas_border() {
    const RADII: [f64; 5] = [2.0, 4.0, 40.0, 50.0, 70.0];
    for depth in [8, 16, 32] {
        let mut s = Session::new();
        s.execute("file.new", json!({ "width": W, "height": W, "depth": depth })).unwrap();
        s.execute("layer.new.layer", json!({})).unwrap();
        s.execute("edit.fill", json!({ "color": "#3366cc" })).unwrap();
        let fill = active_pixels(&s);
        let tol = match depth {
            8 => 1.0 / 255.0,
            16 => 1.0 / 65535.0,
            _ => 1e-5,
        };
        // Applied in place, so any per-pass border step compounds over the radii.
        for radius in RADII {
            s.execute("filter.blur.gaussianBlur", json!({ "radius": radius })).unwrap();
            let px = active_pixels(&s);
            let at = |x: usize, y: usize| &px[(y * W + x) * 4..(y * W + x) * 4 + 4];
            let interior = at(W / 2, W / 2);
            for i in 0..W {
                for (x, y) in [(i, 0), (i, W - 1), (0, i), (W - 1, i)] {
                    for c in 0..4 {
                        assert!(
                            (at(x, y)[c] - interior[c]).abs() <= tol,
                            "depth {depth} radius {radius}: edge ({x},{y}) ch {c}: {:?} vs {:?}",
                            at(x, y),
                            interior
                        );
                    }
                }
            }
        }
        for _ in RADII {
            s.execute("edit.undo", json!({})).unwrap();
        }
        assert_eq!(active_pixels(&s), fill, "depth {depth}: undo must restore the fill");
    }
}
