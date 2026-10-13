//! `paint.gradient` fills the whole layer (or selection), as Photoshop's Gradient tool: the ramp
//! runs between the from and to points, and the first/last stop colour continues beyond each end.
//! A shared `coalesce` key groups consecutive scripted fills into a single undo step.

use photocraft_engine::Session;
use serde_json::json;

fn px(s: &Session, x: i32, y: i32) -> Vec<f32> {
    let st = s.active().expect("an open document");
    let id = st.active_layer.expect("an active layer");
    st.doc.layer(id).expect("the active layer").surface().expect("a raster layer").pixel(x, y)
}

#[test]
fn gradient_fills_the_whole_layer_with_clamped_end_colours() {
    for depth in [8, 16, 32] {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 20, "height": 10, "depth": depth, "background": "transparent"})).unwrap();
        s.execute("paint.gradient", json!({"from": [4, 5], "to": [8, 5], "colors": ["#000000", "#ffffff"]})).unwrap();
        // Between the drag points the ramp: the pixel centres at 5.5 and 6.5 project to t = 0.375 and 0.625.
        assert!((px(&s, 5, 5)[0] - 0.375).abs() < 0.02, "depth {depth}: the ramp runs between from and to");
        assert!((px(&s, 6, 5)[0] - 0.625).abs() < 0.02, "depth {depth}: the ramp runs between from and to");
        // Beyond each end the first/last stop colour continues, before from and past to alike.
        assert!(px(&s, 0, 5)[0] < 0.01, "depth {depth}: before from keeps the first stop colour");
        assert!(px(&s, 15, 5)[0] > 0.99, "depth {depth}: beyond to keeps the last stop colour");
        // The whole layer is covered, far off the drag segment included.
        for (x, y) in [(0, 0), (0, 9), (15, 0), (19, 0), (19, 9)] {
            let p = px(&s, x, y);
            assert_eq!(p[3], 1.0, "depth {depth}: ({x},{y}) is covered");
            let clamped = if x < 4 { p[0] < 0.01 } else { p[0] > 0.99 };
            assert!(clamped, "depth {depth}: ({x},{y}) carries the clamped stop colour");
        }
    }
}

#[test]
fn gradient_coalesce_key_groups_repeated_fills_into_one_undo_step() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 20, "height": 10, "background": "transparent"})).unwrap();
    let empty = px(&s, 6, 5);
    for _ in 0..2 {
        s.execute("paint.gradient", json!({"from": [4, 5], "to": [8, 5], "colors": ["#000000", "#ffffff"], "coalesce": "shards"})).unwrap();
    }
    assert_ne!(px(&s, 6, 5), empty, "the fills landed");
    assert!(s.undo());
    assert_eq!(px(&s, 6, 5), empty, "one undo clears both coalesced fills");
    // Without the key every call records its own step.
    for _ in 0..2 {
        s.execute("paint.gradient", json!({"from": [4, 5], "to": [8, 5], "colors": ["#000000", "#ffffff"]})).unwrap();
    }
    assert!(s.undo());
    assert_ne!(px(&s, 6, 5), empty, "the first uncoalesced fill survives one undo");
    assert!(s.undo());
    assert_eq!(px(&s, 6, 5), empty, "the second undo clears the other one");
}
