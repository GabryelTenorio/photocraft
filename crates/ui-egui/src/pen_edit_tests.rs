use super::*;
use crate::canvas::tool_event;
use crate::direct_select::Drag;
use serde_json::json;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 200, "height": 200})).unwrap();
    app.ui.tool = Tool::Pen;
    app.ui.extras.snap = false;
    app.sync_views();
    app
}

fn square(app: &mut PhotocraftApp) {
    app.run(
        "path.set",
        json!({"name":"work", "path":{"subpaths":[{"closed":true,"knots":[
            [40,40], {"anchor":[120,40],"in":[100,20],"out":[140,60],"smooth":true}, [120,120], [40,120]
        ]}]}}),
    )
    .unwrap();
    app.ui.selected_path = Some("work".into());
}

fn work(app: &PhotocraftApp) -> Path {
    app.session.active().unwrap().doc.work_path.clone().unwrap()
}

fn steps(app: &PhotocraftApp) -> usize {
    app.session.active().unwrap().history.past_len()
}

fn event(app: &mut PhotocraftApp, p: [f64; 2], kind: &str, mods: Modifiers) {
    let [x, y] = p;
    let ev = match kind {
        "down" => ToolEvent::Down { x, y, pressure: 1.0 },
        "move" => ToolEvent::Move { x, y, pressure: 1.0 },
        "up" => ToolEvent::Up { x, y },
        _ => panic!("unknown test event"),
    };
    tool_event(app, ev, mods);
}

fn click(app: &mut PhotocraftApp, p: [f64; 2], mods: Modifiers) {
    event(app, p, "down", mods);
    event(app, p, "up", mods);
}

fn draft(app: &mut PhotocraftApp) {
    for p in [[40.0, 40.0], [120.0, 40.0], [120.0, 120.0]] {
        click(app, p, Modifiers::NONE);
    }
}

#[test]
fn standard_pen_adds_a_curve_anchor_without_changing_the_curve_and_undoes_once() {
    let mut app = app();
    square(&mut app);
    let before = work(&app);
    let seg = before.subpaths[0].segments()[0];
    let at = edit::eval(&seg, 0.5);
    let n = steps(&app);
    assert_eq!(cursor(&app, [at.x, at.y], Modifiers::NONE), PenCursor::Add);
    assert_eq!(steps(&app), n, "hover changes neither history nor path");
    click(&mut app, [at.x, at.y], Modifiers::NONE);
    let after = work(&app);
    assert_eq!(after.subpaths[0].knots.len(), 5);
    let halves = after.subpaths[0].segments();
    for i in 0..=100 {
        let t = f64::from(i) / 100.0;
        let expected = edit::eval(&seg, t);
        let actual = if t <= 0.5 { edit::eval(&halves[0], t * 2.0) } else { edit::eval(&halves[1], (t - 0.5) * 2.0) };
        assert!((expected.x - actual.x).hypot(expected.y - actual.y) < 1e-7);
    }
    assert!(app.ui.pen.is_none());
    assert_eq!(steps(&app), n + 1);
    assert!(app.session.undo());
    assert_eq!(work(&app), before);
    assert!(app.session.redo());
    assert_eq!(work(&app), after);
}

#[test]
fn standard_pen_deletes_existing_anchor_and_disabled_auto_draws_instead() {
    let mut app = app();
    square(&mut app);
    let n = steps(&app);
    assert_eq!(cursor(&app, [120.0, 120.0], Modifiers::NONE), PenCursor::Delete);
    click(&mut app, [120.0, 120.0], Modifiers::NONE);
    assert_eq!(work(&app).subpaths[0].knots.len(), 3);
    assert_eq!(steps(&app), n + 1);
    let before = work(&app);
    app.ui.tool_options.pen_auto_add_delete = false;
    assert_eq!(cursor(&app, [40.0, 40.0], Modifiers::NONE), PenCursor::New);
    click(&mut app, [40.0, 40.0], Modifiers::NONE);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 1);
    assert_eq!(work(&app), before);
    assert_eq!(steps(&app), n + 1);
}

#[test]
fn draft_edits_never_commit_and_first_anchor_closes_unless_alt_converts() {
    let mut app = app();
    draft(&mut app);
    let original = app.ui.pen.as_ref().unwrap().knots.clone();
    let n = steps(&app);
    assert_eq!(cursor(&app, [40.0, 40.0], Modifiers::NONE), PenCursor::Close);
    assert_eq!(cursor(&app, [40.0, 40.0], Modifiers::ALT), PenCursor::Convert);
    assert_eq!(cursor(&app, [80.0, 40.0], Modifiers::NONE), PenCursor::Add);
    click(&mut app, [80.0, 40.0], Modifiers::NONE);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 4);
    assert_eq!(steps(&app), n);
    assert!(app.session.active().unwrap().doc.work_path.is_none());
    assert!(super::super::pen_undo_last_point(&mut app));
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, original);
    assert_eq!(cursor(&app, [120.0, 40.0], Modifiers::NONE), PenCursor::Delete);
    click(&mut app, [120.0, 40.0], Modifiers::NONE);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 2);
    assert_eq!(steps(&app), n);
    assert!(super::super::pen_undo_last_point(&mut app));
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, original);
    click(&mut app, [40.0, 40.0], Modifiers::NONE);
    assert!(app.ui.pen.is_none());
    assert_eq!(steps(&app), n + 1);
    assert_eq!(cursor(&app, [40.0, 40.0], Modifiers::NONE), PenCursor::Delete);
}

#[test]
fn draft_alt_conversion_is_sticky_and_edits_shown_handles_independently() {
    let mut app = app();
    draft(&mut app);
    let original = app.ui.pen.as_ref().unwrap().knots.clone();
    let n = steps(&app);
    event(&mut app, [40.0, 40.0], "down", Modifiers::ALT);
    event(&mut app, [40.0, 60.0], "move", Modifiers::NONE);
    assert_eq!(cursor(&app, [150.0, 150.0], Modifiers::NONE), PenCursor::Convert);
    let preview = app.ui.pen.as_ref().unwrap().knots.clone();
    assert_eq!(preview[0], [[40.0, 40.0], [40.0, 20.0], [40.0, 60.0]]);
    event(&mut app, [40.0, 60.0], "up", Modifiers::NONE);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, preview);
    assert_eq!(steps(&app), n);
    assert_eq!(cursor(&app, [40.0, 60.0], Modifiers::ALT), PenCursor::Convert);
    event(&mut app, [40.0, 60.0], "down", Modifiers::ALT);
    event(&mut app, [60.0, 70.0], "move", Modifiers::NONE);
    event(&mut app, [60.0, 70.0], "up", Modifiers::NONE);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots[0], [[40.0, 40.0], [40.0, 20.0], [60.0, 70.0]]);
    assert!(app.ui.pen.as_ref().unwrap().unlinked.contains(&0));
    assert!(super::super::pen_undo_last_point(&mut app));
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, preview);
    assert!(super::super::pen_undo_last_point(&mut app));
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, original);
    click(&mut app, [120.0, 40.0], Modifiers::ALT);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots[1], [[120.0, 40.0]; 3]);
    assert!(app.session.active().unwrap().doc.work_path.is_none());
}

#[test]
fn last_draft_anchor_retains_outgoing_handle_adjustment() {
    let mut app = app();
    click(&mut app, [40.0, 40.0], Modifiers::NONE);
    event(&mut app, [120.0, 40.0], "down", Modifiers::NONE);
    event(&mut app, [140.0, 60.0], "move", Modifiers::NONE);
    event(&mut app, [140.0, 60.0], "up", Modifiers::NONE);
    assert_eq!(cursor(&app, [120.0, 40.0], Modifiers::NONE), PenCursor::Draw);
    event(&mut app, [120.0, 40.0], "down", Modifiers::NONE);
    event(&mut app, [150.0, 40.0], "move", Modifiers::NONE);
    event(&mut app, [150.0, 40.0], "up", Modifiers::NONE);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots[1], [[120.0, 40.0], [100.0, 20.0], [150.0, 40.0]]);
    assert_eq!(cursor(&app, [120.0, 40.0], Modifiers::ALT), PenCursor::Convert);
    click(&mut app, [120.0, 40.0], Modifiers::ALT);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots[1], [[120.0, 40.0]; 3]);
}

#[test]
fn alt_conversion_of_a_finished_path_previews_and_commits_once_after_modifier_release() {
    let mut app = app();
    square(&mut app);
    let before = work(&app);
    let n = steps(&app);
    assert_eq!(cursor(&app, [40.0, 120.0], Modifiers::ALT), PenCursor::Convert);
    event(&mut app, [40.0, 120.0], "down", Modifiers::ALT);
    event(&mut app, [40.0, 140.0], "move", Modifiers::NONE);
    assert_eq!(work(&app), before);
    assert_eq!(cursor(&app, [170.0, 170.0], Modifiers::NONE), PenCursor::Convert);
    let Some(Drag::Edit { preview: Some(preview), .. }) = app.ui.direct_selection.drag.clone() else { panic!("conversion preview") };
    event(&mut app, [40.0, 140.0], "up", Modifiers::NONE);
    assert_eq!(work(&app), preview);
    assert_eq!(app.ui.direct_selection.anchors, vec![[0, 3]]);
    assert_eq!(steps(&app), n + 1);
    assert!(app.ui.pen.is_none());
}

#[test]
fn auto_add_and_delete_target_selected_saved_paths_and_layers() {
    for target in ["Saved", "shape", "mask"] {
        let mut app = app();
        square(&mut app);
        let before = work(&app);
        let layer = app.session.active().unwrap().active_layer.unwrap();
        match target {
            "Saved" => {
                app.run("path.set", json!({"name":"Saved","path":photocraft_engine::vector_cmds::path_json(&before)})).unwrap();
                app.ui.selected_path = Some("Saved".into());
            }
            "shape" => {
                app.run("shape.create", json!({"kind":"rect","rect":[40,40,80,80],"fill":"#3070c0"})).unwrap();
                app.ui.selected_path = Some("layer".into());
            }
            "mask" => {
                app.run("layer.vectorMask.add", json!({"layer":layer.0,"path":photocraft_engine::vector_cmds::path_json(&before)})).unwrap();
                app.ui.selected_path = Some("layer".into());
                app.ui.vector_mask_target = true;
            }
            _ => panic!("target fixture"),
        }
        let selected = crate::direct_select::candidates(&app)[0].0.clone();
        let n = steps(&app);
        assert_eq!(cursor(&app, [80.0, 120.0], Modifiers::NONE), PenCursor::Add, "{target}");
        click(&mut app, [80.0, 120.0], Modifiers::NONE);
        let (_, added) = crate::direct_select::candidates(&app).into_iter().find(|(r, _)| *r == selected).unwrap();
        assert_eq!(added.subpaths[0].knots.len(), 5, "{target}");
        assert_eq!(work(&app), before);
        assert_eq!(steps(&app), n + 1);
        let inserted = added.subpaths[0].knots.iter().find(|k| (k.anchor.x - 80.0).abs() < 1.0 && k.anchor.y == 120.0).unwrap().anchor;
        assert_eq!(cursor(&app, [inserted.x, inserted.y], Modifiers::NONE), PenCursor::Delete);
        click(&mut app, [inserted.x, inserted.y], Modifiers::NONE);
        assert_eq!(steps(&app), n + 2);
        let (_, deleted) = crate::direct_select::candidates(&app).into_iter().find(|(r, _)| *r == selected).unwrap();
        assert_eq!(deleted.subpaths[0].knots.len(), 4);
        assert_eq!(work(&app), before);
    }
}

#[test]
fn raw_pointer_action_survives_pixel_and_grid_snapping_at_multiple_zooms() {
    for zoom in [0.5, 1.0, 2.0] {
        let mut app = app();
        square(&mut app);
        app.ui.views[0].zoom = zoom;
        app.ui.extras.snap = true;
        let at = [80.0, 120.0 + 5.6 / f64::from(zoom)];
        assert_eq!(cursor(&app, at, Modifiers::NONE), PenCursor::Add);
        click(&mut app, at, Modifiers::NONE);
        assert_eq!(work(&app).subpaths[0].knots.len(), 5);
    }
    let mut app = app();
    square(&mut app);
    app.run(
        "prefs.set",
        json!({"values":{"guidesGridAndSlices.gridlineEvery":100,"guidesGridAndSlices.gridUnit":"pixels","guidesGridAndSlices.subdivisions":4}}),
    )
    .unwrap();
    app.ui.extras.snap = true;
    app.ui.extras.grid = true;
    app.ui.view.snap_to.grid = true;
    assert_eq!(cursor(&app, [40.0, 120.0], Modifiers::NONE), PenCursor::Delete);
    click(&mut app, [40.0, 120.0], Modifiers::NONE);
    assert_eq!(work(&app).subpaths[0].knots.len(), 3);
    assert!(app.prefs_rt.snap.is_none());
}

#[test]
fn stale_draft_conversion_cannot_change_another_document_or_a_restored_draft() {
    let mut app = app();
    draft(&mut app);
    event(&mut app, [40.0, 40.0], "down", Modifiers::ALT);
    app.run("file.new", json!({"width":200,"height":200})).unwrap();
    let n = steps(&app);
    event(&mut app, [40.0, 70.0], "move", Modifiers::NONE);
    event(&mut app, [40.0, 70.0], "up", Modifiers::NONE);
    assert!(app.ui.pen.is_none());
    assert_eq!(steps(&app), n);
    assert!(app.session.active().unwrap().doc.work_path.is_none());
    draft(&mut app);
    let restored = serde_json::from_value(serde_json::to_value(&app.ui.pen).unwrap()).unwrap();
    app.ui.pen = restored;
    assert_eq!(cursor(&app, [40.0, 40.0], Modifiers::ALT), PenCursor::New);
    event(&mut app, [40.0, 70.0], "move", Modifiers::NONE);
    assert!(app.ui.pen.is_none());
}

#[test]
fn settings_keep_auto_enabled_for_old_files_and_preserve_explicit_disable() {
    let default: crate::state::ToolOptions = serde_json::from_value(json!({"vector_mode":"path"})).unwrap();
    assert!(default.pen_auto_add_delete);
    let disabled: crate::state::ToolOptions = serde_json::from_value(json!({"pen_auto_add_delete":false})).unwrap();
    assert!(!disabled.pen_auto_add_delete);
    assert_eq!(serde_json::to_value(disabled).unwrap()["pen_auto_add_delete"], false);
}

#[test]
fn an_unselected_work_path_does_not_steal_plain_pen_clicks() {
    let mut app = app();
    square(&mut app);
    app.ui.selected_path = None;
    let before = work(&app);
    let n = steps(&app);
    assert_eq!(cursor(&app, [80.0, 120.0], Modifiers::NONE), PenCursor::New);
    click(&mut app, [80.0, 120.0], Modifiers::NONE);
    assert_eq!(work(&app), before);
    assert_eq!(steps(&app), n);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 1);
}

#[test]
fn layer_locks_remove_auto_edit_hits_for_shapes_and_vector_masks() {
    for target in ["shape", "mask"] {
        let mut app = app();
        if target == "shape" {
            app.run("shape.create", json!({"kind":"rect","rect":[40,40,80,80],"fill":"#3070c0"})).unwrap();
        } else {
            app.run("layer.vectorMask.add", json!({"path":{"subpaths":[{"closed":true,"knots":[[40,40],[120,40],[120,120],[40,120]]}]}})).unwrap();
            app.ui.vector_mask_target = true;
        }
        app.ui.selected_path = Some("layer".into());
        let n = steps(&app);
        assert_eq!(cursor(&app, [80.0, 120.0], Modifiers::NONE), PenCursor::Add);
        app.run("layer.lockLayers", json!({"pixels":true})).unwrap();
        let (_, before) = crate::direct_select::candidates(&app)[0].clone();
        assert_eq!(cursor(&app, [80.0, 120.0], Modifiers::NONE), PenCursor::New);
        app.ui.tool = Tool::AddAnchorPoint;
        click(&mut app, [80.0, 120.0], Modifiers::NONE);
        assert_eq!(crate::direct_select::candidates(&app)[0].1, before);
        assert_eq!(steps(&app), n + 1, "only the lock operation changes history");
        app.ui.tool = Tool::ConvertPoint;
        event(&mut app, [40.0, 120.0], "down", Modifiers::NONE);
        event(&mut app, [40.0, 140.0], "move", Modifiers::NONE);
        event(&mut app, [40.0, 140.0], "up", Modifiers::NONE);
        assert_eq!(crate::direct_select::candidates(&app)[0].1, before);
        assert_eq!(steps(&app), n + 1);
    }
}

#[test]
fn invalid_conversion_preserves_draft_and_committed_geometry_without_history() {
    for committed in [false, true] {
        let mut app = app();
        if committed {
            square(&mut app);
        } else {
            draft(&mut app);
        }
        let before_work = app.session.active().unwrap().doc.work_path.clone();
        let before_draft = app.ui.pen.as_ref().map(|pen| pen.knots.clone());
        let n = steps(&app);
        event(&mut app, [40.0, 40.0], "down", Modifiers::ALT);
        event(&mut app, [2_000_000.0, 40.0], "up", Modifiers::ALT);
        assert_eq!(app.session.active().unwrap().doc.work_path, before_work);
        assert_eq!(app.ui.pen.as_ref().map(|pen| pen.knots.clone()), before_draft);
        assert_eq!(steps(&app), n);
        assert!(app.ui.status_error);
    }
}

#[test]
fn leaving_pen_during_conversion_does_not_finish_the_preview() {
    let mut app = app();
    draft(&mut app);
    let base = app.ui.pen.as_ref().unwrap().knots.clone();
    event(&mut app, [40.0, 40.0], "down", Modifiers::ALT);
    event(&mut app, [40.0, 70.0], "move", Modifiers::ALT);
    assert_ne!(app.ui.pen.as_ref().unwrap().knots, base);
    app.ui.tool = Tool::Brush;
    super::super::finish_pen_if_left(&mut app);
    assert!(app.ui.pen.is_none());
    assert_eq!(work(&app).subpaths[0].knots[0], Knot::corner(40.0, 40.0), "existing tool-switch finish uses the unedited draft");
}

fn grid(app: &mut PhotocraftApp) {
    app.run(
        "prefs.set",
        json!({"values": {
            "guidesGridAndSlices.gridlineEvery":100,
            "guidesGridAndSlices.gridUnit":"pixels",
            "guidesGridAndSlices.subdivisions":4
        }}),
    )
    .unwrap();
    app.ui.extras.snap = true;
    app.ui.extras.grid = true;
    app.ui.view.snap_to.grid = true;
}

#[test]
fn stationary_grid_conversion_removes_handles_instead_of_creating_them() {
    for target in ["pen", "convert", "draft"] {
        let mut app = app();
        if target == "draft" {
            click(&mut app, [43.0, 43.0], Modifiers::NONE);
            event(&mut app, [120.0, 43.0], "down", Modifiers::NONE);
            event(&mut app, [140.0, 63.0], "move", Modifiers::NONE);
            event(&mut app, [140.0, 63.0], "up", Modifiers::NONE);
            event(&mut app, [120.0, 43.0], "down", Modifiers::ALT);
            event(&mut app, [140.0, 63.0], "move", Modifiers::ALT);
            event(&mut app, [140.0, 63.0], "up", Modifiers::ALT);
        } else {
            app.run(
                "path.set",
                json!({"name":"work","path":{"subpaths":[{"closed":false,"knots":[
                    {"anchor":[43,43],"in":[23,23],"out":[63,63],"smooth":true},[120,43],[120,120]
                ]}]}}),
            )
            .unwrap();
            app.ui.selected_path = Some("work".into());
        }
        grid(&mut app);
        let n = steps(&app);
        let at = if target == "draft" { [120.0, 43.0] } else { [43.0, 43.0] };
        let mods = if target == "convert" {
            app.ui.tool = Tool::ConvertPoint;
            Modifiers::NONE
        } else {
            Modifiers::ALT
        };
        assert_eq!(cursor(&app, at, mods), PenCursor::Convert);
        event(&mut app, at, "down", mods);
        event(&mut app, at, "move", Modifiers::NONE);
        event(&mut app, at, "up", Modifiers::NONE);
        if target == "draft" {
            assert_eq!(app.ui.pen.as_ref().unwrap().knots[1], [at; 3]);
            assert_eq!(steps(&app), n);
        } else {
            assert_eq!(work(&app).subpaths[0].knots[0], Knot::corner(at[0], at[1]));
            assert_eq!(steps(&app), n + 1);
        }
        assert!(app.prefs_rt.snap.is_none());
        assert!(app.prefs_rt.snap_lines.is_empty());
    }
}

#[test]
fn grid_conversion_keeps_raw_geometry_after_alt_release() {
    for committed in [false, true] {
        let mut app = app();
        if committed {
            app.run("path.set", json!({"name":"work","path":{"subpaths":[{"closed":false,"knots":[[43,43],[120,43]]}]}})).unwrap();
            app.ui.selected_path = Some("work".into());
        } else {
            click(&mut app, [43.0, 43.0], Modifiers::NONE);
            click(&mut app, [120.0, 43.0], Modifiers::NONE);
        }
        grid(&mut app);
        event(&mut app, [43.0, 43.0], "down", Modifiers::ALT);
        event(&mut app, [52.0, 62.0], "move", Modifiers::NONE);
        event(&mut app, [52.0, 62.0], "up", Modifiers::NONE);
        if committed {
            let knot = work(&app).subpaths[0].knots[0];
            assert_eq!((knot.anchor, knot.in_ctrl, knot.out_ctrl), (Point::new(43.0, 43.0), Point::new(34.0, 24.0), Point::new(52.0, 62.0)));
        } else {
            assert_eq!(app.ui.pen.as_ref().unwrap().knots[0], [[43.0, 43.0], [34.0, 24.0], [52.0, 62.0]]);
        }
    }
}

#[test]
fn grid_snapping_onto_the_first_anchor_does_not_start_a_close_drag() {
    let mut app = app();
    app.ui.tool_options.pen_auto_add_delete = false;
    for p in [[50.0, 50.0], [125.0, 50.0], [125.0, 125.0]] {
        click(&mut app, p, Modifiers::NONE);
    }
    grid(&mut app);
    let n = steps(&app);
    let outside = [56.0, 50.0];
    assert_eq!(cursor(&app, outside, Modifiers::NONE), PenCursor::Draw);
    click(&mut app, outside, Modifiers::NONE);
    let pen = app.ui.pen.as_ref().expect("a raw miss must draw even when snapped onto the first anchor");
    assert_eq!(pen.knots.len(), 4);
    assert_eq!(pen.knots[3][0], pen.knots[0][0]);
    assert!(!super::super::closing(pen));
    assert!(app.session.active().unwrap().doc.work_path.is_none());
    assert_eq!(steps(&app), n);
}

#[test]
fn undo_during_a_close_drag_keeps_prior_draft_edit_history_and_ignores_late_events() {
    let mut app = app();
    draft(&mut app);
    let original = app.ui.pen.as_ref().unwrap().knots.clone();
    click(&mut app, [80.0, 40.0], Modifiers::NONE);
    let added = app.ui.pen.as_ref().unwrap().knots.clone();
    let n = steps(&app);
    event(&mut app, [40.0, 40.0], "down", Modifiers::NONE);
    event(&mut app, [60.0, 20.0], "move", Modifiers::NONE);
    assert!(super::super::closing(app.ui.pen.as_ref().unwrap()));
    assert!(super::super::pen_undo_last_point(&mut app));
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, added);
    event(&mut app, [70.0, 10.0], "move", Modifiers::ALT);
    event(&mut app, [70.0, 10.0], "up", Modifiers::NONE);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, added);
    assert!(super::super::pen_undo_last_point(&mut app));
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, original);
    assert!(app.session.active().unwrap().doc.work_path.is_none());
    assert_eq!(steps(&app), n);
}

#[test]
fn finishing_a_close_drag_as_a_shape_keeps_its_fill_and_commits_once() {
    let mut app = app();
    app.ui.tool_options.vector_mode = "shape".into();
    app.ui.tool_options.shape_fill = true;
    draft(&mut app);
    let n = steps(&app);
    event(&mut app, [40.0, 40.0], "down", Modifiers::NONE);
    event(&mut app, [60.0, 20.0], "move", Modifiers::NONE);
    super::super::pen_commit(&mut app, false);
    event(&mut app, [60.0, 20.0], "up", Modifiers::NONE);
    assert!(app.ui.pen.is_none());
    let st = app.session.active().unwrap();
    let layer = st.doc.layer(st.active_layer.unwrap()).unwrap();
    let photocraft_doc::LayerContent::Shape(shape) = &layer.content else { panic!("expected a shape") };
    assert!(shape.fill.is_some());
    assert!(shape.path.subpaths[0].closed);
    assert_eq!(shape.path.subpaths[0].knots.len(), 3);
    assert_eq!(shape.path.subpaths[0].knots[0].out_ctrl, Point::new(60.0, 20.0));
    assert_eq!(steps(&app), n + 1);
}

#[test]
fn failed_commit_during_a_close_drag_preserves_the_folded_draft_after_late_events() {
    let mut app = app();
    draft(&mut app);
    click(&mut app, [80.0, 40.0], Modifiers::NONE);
    app.ui.pen.as_mut().unwrap().knots[2][0] = [1e40, 40.0];
    let n = steps(&app);
    let undo = app.ui.pen.as_ref().unwrap().edit_undo.clone();
    let owner = app.ui.pen.as_ref().unwrap().owner.clone();
    event(&mut app, [40.0, 40.0], "down", Modifiers::NONE);
    event(&mut app, [60.0, 20.0], "move", Modifiers::NONE);
    let expected = super::super::fold_close(app.ui.pen.as_ref().unwrap()).unwrap().knots;
    super::super::pen_commit(&mut app, false);
    let pen = app.ui.pen.as_ref().expect("failed close must preserve its draft");
    assert!(pen.commit_failed);
    assert!(!pen.dragging && !pen.adjusting_last && !super::super::closing(pen));
    assert_eq!(pen.knots, expected);
    event(&mut app, [90.0, 10.0], "move", Modifiers::ALT);
    event(&mut app, [90.0, 10.0], "up", Modifiers::NONE);
    let pen = app.ui.pen.as_ref().unwrap();
    assert_eq!(pen.knots, expected);
    assert_eq!(pen.knots.len(), 4);
    assert_eq!(pen.owner, owner);
    assert_eq!(pen.edit_undo, undo);
    assert!(pen.owner.as_ref().unwrap().valid(&app));
    assert!(pen.pending_last_handle.is_none() && pen.edit.is_none());
    assert!(app.session.active().unwrap().doc.work_path.is_none());
    assert_eq!(steps(&app), n);
}

#[test]
fn undo_during_a_draft_conversion_cannot_be_reversed_by_its_old_release() {
    let mut app = app();
    draft(&mut app);
    let original = app.ui.pen.as_ref().unwrap().knots.clone();
    let n = steps(&app);
    event(&mut app, [40.0, 40.0], "down", Modifiers::ALT);
    event(&mut app, [40.0, 60.0], "move", Modifiers::ALT);
    assert!(super::super::pen_undo_last_point(&mut app));
    let undone = app.ui.pen.as_ref().unwrap().knots.clone();
    assert_eq!(undone, original[..2]);
    assert!(app.ui.pen.as_ref().unwrap().edit.is_none());
    event(&mut app, [40.0, 70.0], "up", Modifiers::NONE);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, undone);
    assert_eq!(steps(&app), n);
    assert!(app.session.active().unwrap().doc.work_path.is_none());
}

#[test]
fn later_new_points_keep_prior_add_delete_and_conversion_undo_steps() {
    for action in ["add", "delete", "convert", "lastHandle"] {
        let mut app = app();
        draft(&mut app);
        let original = app.ui.pen.as_ref().unwrap().knots.clone();
        let n = steps(&app);
        match action {
            "add" => click(&mut app, [80.0, 40.0], Modifiers::NONE),
            "delete" => click(&mut app, [120.0, 40.0], Modifiers::NONE),
            "convert" => {
                event(&mut app, [40.0, 40.0], "down", Modifiers::ALT);
                event(&mut app, [40.0, 60.0], "move", Modifiers::ALT);
                event(&mut app, [40.0, 60.0], "up", Modifiers::ALT);
            }
            "lastHandle" => {
                event(&mut app, [120.0, 120.0], "down", Modifiers::NONE);
                event(&mut app, [140.0, 140.0], "move", Modifiers::NONE);
                event(&mut app, [140.0, 140.0], "up", Modifiers::NONE);
            }
            _ => panic!("undo fixture"),
        }
        let edited = app.ui.pen.as_ref().unwrap().knots.clone();
        assert_ne!(edited, original, "{action}");
        click(&mut app, [170.0, 170.0], Modifiers::NONE);
        assert!(super::super::pen_undo_last_point(&mut app));
        assert_eq!(app.ui.pen.as_ref().unwrap().knots, edited, "first undo removes only the later point: {action}");
        assert!(super::super::pen_undo_last_point(&mut app));
        assert_eq!(app.ui.pen.as_ref().unwrap().knots, original, "next undo restores the preceding edit: {action}");
        assert_eq!(steps(&app), n);
    }
}

#[test]
fn stationary_last_anchor_reclick_does_not_swallow_the_next_point_undo() {
    let mut app = app();
    draft(&mut app);
    let original = app.ui.pen.as_ref().unwrap().knots.clone();
    click(&mut app, [120.0, 120.0], Modifiers::NONE);
    assert!(app.ui.pen.as_ref().unwrap().edit_undo.is_empty());
    assert!(super::super::pen_undo_last_point(&mut app));
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, original[..2]);
}

#[test]
fn undo_during_last_handle_drag_blocks_late_moves_and_release() {
    let mut app = app();
    draft(&mut app);
    let original = app.ui.pen.as_ref().unwrap().knots.clone();
    event(&mut app, [120.0, 120.0], "down", Modifiers::NONE);
    event(&mut app, [140.0, 140.0], "move", Modifiers::NONE);
    assert_ne!(app.ui.pen.as_ref().unwrap().knots, original);
    assert!(super::super::pen_undo_last_point(&mut app));
    let pen = app.ui.pen.as_ref().unwrap();
    assert_eq!(pen.knots, original);
    assert!(!pen.dragging && !pen.adjusting_last);
    event(&mut app, [150.0, 150.0], "move", Modifiers::NONE);
    event(&mut app, [150.0, 150.0], "up", Modifiers::NONE);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots, original);
}

#[test]
fn command_anchor_drag_keeps_snapping_when_alt_is_pressed_mid_gesture() {
    let mut app = app();
    app.run("path.set", json!({"name":"work","path":{"subpaths":[{"closed":false,"knots":[[50,50],[120,50],[120,120]]}]}})).unwrap();
    app.ui.selected_path = Some("work".into());
    grid(&mut app);
    event(&mut app, [50.0, 50.0], "down", Modifiers::COMMAND);
    assert!(crate::direct_select::captured_drag(&app));
    assert!(!converting(&app));
    event(&mut app, [68.0, 68.0], "move", Modifiers::ALT);
    let Some(Drag::Edit { preview: Some(preview), .. }) = app.ui.direct_selection.drag.clone() else { panic!("anchor preview") };
    assert_eq!(preview.subpaths[0].knots[0].anchor, Point::new(75.0, 75.0));
    event(&mut app, [68.0, 68.0], "up", Modifiers::ALT);
    assert_eq!(work(&app).subpaths[0].knots[0].anchor, Point::new(75.0, 75.0));
}

#[test]
fn stationary_command_alt_handle_gesture_keeps_the_same_snapped_coordinate_frame() {
    let mut app = app();
    app.run(
        "path.set",
        json!({"name":"work","path":{"subpaths":[{"closed":false,"knots":[
            {"anchor":[28,28],"in":[8,8],"out":[48,48],"smooth":true},[120,28]
        ]}]}}),
    )
    .unwrap();
    app.ui.selected_path = Some("work".into());
    click(&mut app, [28.0, 28.0], Modifiers::COMMAND);
    assert_eq!(app.ui.direct_selection.anchors, vec![[0, 0]]);
    let before = work(&app);
    let n = steps(&app);
    grid(&mut app);
    let mods = Modifiers { command: true, ctrl: true, alt: true, ..Default::default() };
    event(&mut app, [46.0, 43.0], "down", mods);
    assert!(matches!(app.ui.direct_selection.drag, Some(Drag::Edit { edit: crate::direct_select::Edit::Handle(_, _, _, true), converting: false, .. })));
    assert!(!converting(&app));
    event(&mut app, [46.0, 43.0], "move", Modifiers::ALT);
    event(&mut app, [46.0, 43.0], "up", Modifiers::ALT);
    assert_eq!(work(&app), before);
    assert_eq!(steps(&app), n, "stationary snapped gesture has no history entry");
}

#[test]
#[ignore = "release-only hover measurement; no timing threshold"]
fn measure_pen_hover_on_1024_anchor_path_in_24mp_document() {
    use std::hint::black_box;
    use std::time::Instant;
    let mut app = app();
    app.run("file.new", json!({"width":6000,"height":4000})).unwrap();
    let knots: Vec<_> = (0..1024)
        .map(|i| {
            let x = f64::from(i % 64) * 80.0 + 40.0;
            let y = f64::from(i / 64) * 200.0 + 40.0;
            Knot::smooth(Point::new(x, y), Point::new(x - 20.0, y - 20.0), Point::new(x + 20.0, y + 20.0))
        })
        .collect();
    let path = Path::new(vec![Subpath { knots, closed: false, ..Default::default() }]);
    let points: Vec<_> = path.subpaths[0]
        .segments()
        .iter()
        .step_by(4)
        .map(|segment| {
            let p = edit::eval(segment, 0.5);
            [p.x, p.y]
        })
        .collect();
    app.run("path.set", json!({"name":"work","path":photocraft_engine::vector_cmds::path_json(&path)})).unwrap();
    app.ui.selected_path = Some("work".into());
    for &point in points.iter().take(16) {
        black_box(cursor(&app, point, Modifiers::NONE));
    }
    let mut samples = Vec::new();
    let mut add = 0usize;
    for &point in points.iter().cycle().take(512) {
        let start = Instant::now();
        let result = black_box(cursor(black_box(&app), black_box(point), Modifiers::NONE));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        add += usize::from(result == PenCursor::Add);
    }
    samples.sort_by(f64::total_cmp);
    assert!(add > 0);
    println!(
        "pen_hover_benchmark {{\"width\":6000,\"height\":4000,\"anchors\":1024,\"iterations\":512,\"p50_ms\":{},\"p95_ms\":{},\"add_hits\":{},\"arch\":\"{}\",\"os\":\"{}\"}}",
        samples[256],
        samples[486],
        add,
        std::env::consts::ARCH,
        std::env::consts::OS
    );
}
