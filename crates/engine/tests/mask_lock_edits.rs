//! Brush (and other mask-targeted paint commands) obey layer locks: a full lock (All) refuses
//! the mask stroke and leaves the mask and history untouched, a pixels-only lock still permits
//! painting the separate mask, and an unlocked mask paints as usual (#3094).

use photocraft_engine::Session;
use serde_json::json;

/// A 16×16 white document whose selected layer has a reveal-all mask, as in the issue.
fn masked() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 16, "height": 16, "background": "white"})).unwrap();
    s.execute("layer.layerMask.revealAll", json!({})).unwrap();
    s
}

/// The mask's value at the centre: 1.0 revealed, 0.0 hidden by a black stroke.
fn mask_center(s: &Session) -> f32 {
    let st = s.active().unwrap();
    let id = st.active_layer.unwrap();
    st.doc.layer(id).unwrap().mask.as_ref().unwrap().surface.rgba(8, 8)[0]
}

fn steps(s: &Session) -> usize {
    s.active().unwrap().history.past_len()
}

/// The issue's stroke: one dab of black at the centre, on the mask.
fn stroke_mask(s: &mut Session) -> photocraft_engine::Result<serde_json::Value> {
    s.execute("paint.stroke", json!({"points": [[8, 8]], "size": 4, "hardness": 1.0, "color": "#000000", "target": "mask"}))
}

#[test]
fn a_full_lock_refuses_a_mask_stroke() {
    let mut s = masked();
    s.execute("layer.lockLayers", json!({"all": true})).unwrap();
    let before = mask_center(&s);
    let undo = steps(&s);
    let r = stroke_mask(&mut s);
    assert!(r.is_err(), "a mask stroke ran while All-locked: {r:?}");
    assert_eq!(mask_center(&s), before, "the refused stroke changed the mask");
    assert_eq!(steps(&s), undo, "the refused stroke left a history step");
}

#[test]
fn a_full_lock_still_refuses_a_pixel_stroke() {
    // The issue's control: painting the All-locked layer's own pixels stays refused.
    let mut s = masked();
    s.execute("layer.lockLayers", json!({"all": true})).unwrap();
    let r = s.execute("paint.stroke", json!({"points": [[8, 8]], "size": 4, "hardness": 1.0, "color": "#000000"}));
    assert!(r.is_err(), "a pixel stroke ran on an All-locked layer: {r:?}");
}

#[test]
fn an_all_lock_also_refuses_a_mask_fill() {
    // The shared mask-target path (`paint_surface`), not just Brush.
    let mut s = masked();
    s.execute("layer.lockLayers", json!({"all": true})).unwrap();
    let before = mask_center(&s);
    let r = s.execute("edit.fill", json!({"contents": "black", "target": "mask"}));
    assert!(r.is_err(), "a mask fill ran while All-locked: {r:?}");
    assert_eq!(mask_center(&s), before);
}

#[test]
fn a_pixels_only_lock_still_permits_the_mask() {
    let mut s = masked();
    s.execute("layer.lockLayers", json!({"pixels": true})).unwrap();
    let undo = steps(&s);
    stroke_mask(&mut s).unwrap();
    assert!(mask_center(&s) < 0.5, "the pixels-only lock also blocked the mask");
    assert_eq!(steps(&s), undo + 1, "the mask stroke is one history step");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(mask_center(&s) > 0.5, "undo restores the mask");
}

#[test]
fn an_unlocked_mask_still_paints() {
    let mut s = masked();
    let undo = steps(&s);
    stroke_mask(&mut s).unwrap();
    assert!(mask_center(&s) < 0.5, "the mask stroke did not paint");
    assert_eq!(steps(&s), undo + 1, "the mask stroke is one history step");
}
