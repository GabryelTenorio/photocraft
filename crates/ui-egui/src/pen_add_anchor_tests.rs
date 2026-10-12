//! #4144: the Pen tool adds an anchor point when it is clicked on a segment of an existing path
//! (the reported case: a closed path), like Photoshop and other vector editors. The idle Pen
//! inserts through `path.addAnchor`, the same edit the Add Anchor Point tool uses.

use egui::Modifiers;
use photocraft_doc::vector::Path;
use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, tool_event};
use crate::state::Tool;

fn app(tool: Tool) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 200, "height": 200})).unwrap();
    // The Pen snaps vector-tool clicks to the pixel grid by default; the tests click exact
    // document points, so turn it off rather than assert against the rounded position.
    app.session.edit_prefs(|p| p.tools.snap_vector_tools_and_transforms_to_pixel_grid = false);
    app.sync_views();
    app.ui.tool = tool;
    app
}

/// A closed square work path; knot 1 at (120, 40) is smooth, so the top edge (0 → 1) is a curve
/// and the bottom (2 → 3), right (1 → 2) and left (3 → 0) edges are straight.
fn with_path(app: &mut PhotocraftApp) {
    let path = json!({"subpaths": [{"closed": true, "knots": [
        [40, 40],
        {"anchor": [120, 40], "in": [100, 20], "out": [140, 60], "smooth": true},
        [120, 120],
        [40, 120],
    ]}]});
    app.run("path.set", json!({"name": "work", "path": path})).unwrap();
}

fn work(app: &PhotocraftApp) -> Path {
    app.session.active().unwrap().doc.work_path.clone().unwrap()
}

fn history(app: &PhotocraftApp) -> usize {
    app.session.active().unwrap().history.entries().len()
}

fn click(app: &mut PhotocraftApp, at: [f64; 2]) {
    tool_event(app, ToolEvent::Down { x: at[0], y: at[1], pressure: 1.0 }, Modifiers::NONE);
    tool_event(app, ToolEvent::Up { x: at[0], y: at[1] }, Modifiers::NONE);
}

#[test]
fn pen_click_on_a_closed_path_segment_inserts_an_anchor() {
    let mut app = app(Tool::Pen);
    with_path(&mut app);
    let (before, steps) = (work(&app), history(&app));
    click(&mut app, [80.0, 120.0]);
    let p = work(&app);
    assert_eq!(p.subpaths[0].knots.len(), 5, "the bottom edge gained an anchor");
    let k = p.subpaths[0].knots[3];
    assert!(!k.smooth && (k.anchor.y - 120.0).abs() < 1e-9 && (k.anchor.x - 80.0).abs() < 1.0, "a corner on the bottom edge: {k:?}");
    assert!(p.subpaths[0].closed, "the path stays closed");
    assert!(app.ui.pen.is_none(), "the click edited the path; it did not start a new draft");
    assert_eq!(history(&app), steps + 1, "one History step");
    assert_eq!(app.session.active().unwrap().history.undo_label(), Some("Add Anchor Point"));
    assert!(app.session.undo());
    assert_eq!(work(&app), before, "one undo restores the path exactly");
}

#[test]
fn pen_click_on_a_curve_splits_it_without_changing_its_shape() {
    let mut app = app(Tool::Pen);
    with_path(&mut app);
    let seg = work(&app).subpaths[0].segments()[0];
    let on = photocraft_vector::edit::eval(&seg, 0.5);
    click(&mut app, [on.x, on.y]);
    let p = work(&app);
    let k = p.subpaths[0].knots[1];
    assert!(k.smooth && (k.anchor.x - on.x).hypot(k.anchor.y - on.y) < 0.2, "a smooth anchor where clicked: {k:?}");
    // The two halves still pass through the old curve's quarter points.
    let halves = &p.subpaths[0].segments()[..2];
    for t in [0.25, 0.75] {
        let old = photocraft_vector::edit::eval(&seg, t);
        let near = halves
            .iter()
            .flat_map(|s| (0..=400).map(move |i| photocraft_vector::edit::eval(s, f64::from(i) / 400.0)))
            .fold(f64::MAX, |d, q| d.min((q.x - old.x).hypot(q.y - old.y)));
        assert!(near < 0.1, "t={t}: the split kept the shape ({near})");
    }
}

#[test]
fn pen_click_on_a_closing_segment_wraps_to_the_end() {
    let mut app = app(Tool::Pen);
    with_path(&mut app);
    // The left edge (3 → 0) is the closing segment; the new anchor goes last.
    click(&mut app, [40.0, 80.0]);
    let p = work(&app);
    assert_eq!(p.subpaths[0].knots.len(), 5);
    let k = p.subpaths[0].knots[4];
    assert!((k.anchor.x - 40.0).abs() < 1e-9 && (k.anchor.y - 80.0).abs() < 1.0, "{k:?}");
    assert!(p.subpaths[0].closed);
}

#[test]
fn pen_click_adds_an_anchor_to_a_shape_layer_path() {
    let mut app = app(Tool::Pen);
    let id = app.run("shape.create", json!({"kind": "rect", "rect": [20, 20, 60, 60], "fill": "#3070c0"})).unwrap()["layer"].as_u64().unwrap();
    click(&mut app, [50.0, 20.0]); // the top edge, midway
    let info = app.run("shape.info", json!({"layer": id})).unwrap();
    assert_eq!(info["path"]["subpaths"][0]["knots"].as_array().unwrap().len(), 5, "the shape's path gained an anchor");
    assert!(app.ui.pen.is_none());
}

#[test]
fn pen_click_off_the_path_still_starts_a_new_draft() {
    let mut app = app(Tool::Pen);
    with_path(&mut app);
    let steps = history(&app);
    click(&mut app, [180.0, 180.0]);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 1, "a new draft path");
    assert_eq!(work(&app).subpaths[0].knots.len(), 4, "the existing path is untouched");
    assert_eq!(history(&app), steps, "a draft is not a History step");
}

#[test]
fn pen_click_on_an_anchor_starts_a_new_draft() {
    let mut app = app(Tool::Pen);
    with_path(&mut app);
    click(&mut app, [40.0, 40.0]);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 1, "an anchor hit is not a segment");
    assert_eq!(work(&app).subpaths[0].knots.len(), 4);
}

#[test]
fn pen_click_while_drawing_adds_to_the_draft_not_the_existing_path() {
    let mut app = app(Tool::Pen);
    with_path(&mut app);
    click(&mut app, [180.0, 180.0]); // start a draft
    assert!(app.ui.pen.is_some());
    click(&mut app, [80.0, 120.0]); // a segment of the existing path
    assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 2, "the draft grew");
    assert_eq!(work(&app).subpaths[0].knots.len(), 4, "the existing path is untouched while drawing");
}

#[test]
fn the_pen_shows_the_add_badge_over_a_segment() {
    use crate::tool_feedback::{Badge, badge};
    let mut app = app(Tool::Pen);
    with_path(&mut app);
    app.hover_doc = Some([80.0, 120.0]);
    assert_eq!(badge(&app, Tool::Pen, Modifiers::NONE), Some(Badge::Add));
    app.hover_doc = Some([180.0, 180.0]);
    assert_eq!(badge(&app, Tool::Pen, Modifiers::NONE), None, "empty canvas");
    app.hover_doc = Some([80.0, 120.0]);
    assert_eq!(badge(&app, Tool::Pen, Modifiers::COMMAND), None, "⌘ is Direct Selection");
    assert_eq!(badge(&app, Tool::Pen, Modifiers::ALT), None, "⌥ is Convert Point");
    // While drawing, the cursor is the Pen's own: no add badge.
    app.ui.pen = Some(crate::vector_ui::PenPath { knots: vec![[[0.0, 0.0]; 3]], ..Default::default() });
    assert_eq!(badge(&app, Tool::Pen, Modifiers::NONE), None);
}
