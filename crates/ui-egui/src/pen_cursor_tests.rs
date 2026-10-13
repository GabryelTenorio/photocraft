use super::*;
use crate::tool_cursor::PenCursor;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 200, "height": 200})).unwrap();
    app.ui.tool = Tool::Pen;
    app.ui.tool_options.vector_mode = "path".into();
    app.sync_views();
    app
}

fn anchor(app: &mut PhotocraftApp, point: [f64; 2]) {
    pen_down(app, point[0], point[1]);
    pen_up(app);
}

fn canvas_anchor(app: &mut PhotocraftApp, [x, y]: [f64; 2]) {
    crate::canvas::tool_event(app, crate::canvas::ToolEvent::Down { x, y, pressure: 1.0 }, egui::Modifiers::NONE);
    crate::canvas::tool_event(app, crate::canvas::ToolEvent::Up { x, y }, egui::Modifiers::NONE);
}

#[test]
fn canvas_close_hit_uses_the_pointer_before_pixel_and_grid_snapping() {
    let mut app = app();
    app.ui.tool_options.pen_auto_add_delete = false;
    canvas_anchor(&mut app, [40.0, 40.0]);
    canvas_anchor(&mut app, [120.0, 40.0]);
    let close = [45.6, 40.0];
    assert_eq!(pen_cursor(&app, close), PenCursor::Close);
    let history = app.session.active().unwrap().history.past_len();
    canvas_anchor(&mut app, close);
    assert!(app.ui.pen.is_none(), "rounding must not move a close hit beyond the grab radius");
    assert_eq!(app.session.active().unwrap().history.past_len(), history + 1);
    assert!(app.prefs_rt.snap.is_none());
    assert!(app.prefs_rt.snap_lines.is_empty());

    // Snapping into the grab radius must not close when the pointer advertises drawing.
    canvas_anchor(&mut app, [40.0, 40.0]);
    canvas_anchor(&mut app, [120.0, 40.0]);
    let outside = [45.49, 42.49];
    assert_eq!(pen_cursor(&app, outside), PenCursor::Draw);
    canvas_anchor(&mut app, outside);
    let draft = app.ui.pen.as_ref().unwrap();
    assert_eq!(draft.knots.len(), 3);
    assert_eq!(draft.knots[2][0], [45.0, 42.0], "new anchors keep pixel snapping");

    let mut grid = self::app();
    grid.run(
        "prefs.set",
        json!({"values": {
            "guidesGridAndSlices.gridlineEvery": 100,
            "guidesGridAndSlices.gridUnit": "pixels",
            "guidesGridAndSlices.subdivisions": 4
        }}),
    )
    .unwrap();
    grid.ui.extras.grid = false;
    canvas_anchor(&mut grid, [43.0, 43.0]);
    canvas_anchor(&mut grid, [120.0, 120.0]);
    grid.ui.extras.grid = true;
    grid.ui.extras.snap = true;
    grid.ui.view.snap_to.grid = true;
    assert_eq!(pen_cursor(&grid, [43.0, 43.0]), PenCursor::Close);
    canvas_anchor(&mut grid, [43.0, 43.0]);
    assert!(grid.ui.pen.is_none(), "an existing anchor wins over the visible grid");
    assert_eq!(grid.run("path.info", json!({"name": "work"})).unwrap()["path"]["subpaths"][0]["closed"], true);
}

#[test]
fn pen_cursor_follows_drawing_closing_and_starting_another_path() {
    let mut app = app();
    let first = [40.0, 40.0];
    assert_eq!(pen_cursor(&app, first), PenCursor::New);
    anchor(&mut app, first);
    assert_eq!(pen_cursor(&app, first), PenCursor::Draw, "one anchor cannot close a path");
    anchor(&mut app, [120.0, 40.0]);
    anchor(&mut app, [120.0, 120.0]);
    assert_eq!(pen_cursor(&app, [80.0, 80.0]), PenCursor::Draw);
    assert_eq!(pen_cursor(&app, first), PenCursor::Close);
    let history = app.session.active().unwrap().history.past_len();
    anchor(&mut app, first);
    assert!(app.ui.pen.is_none());
    assert_eq!(pen_cursor(&app, first), PenCursor::Delete, "the closed path is now editable");
    assert_eq!(pen_cursor(&app, [150.0, 150.0]), PenCursor::New);
    assert_eq!(app.session.active().unwrap().history.past_len(), history + 1);
    assert_eq!(app.run("path.info", json!({"name": "work"})).unwrap()["path"]["subpaths"][0]["closed"], true);
    anchor(&mut app, [150.0, 150.0]);
    assert_eq!(pen_cursor(&app, [150.0, 150.0]), PenCursor::Draw);
}

#[test]
fn close_cursor_and_press_share_the_zoom_scaled_hit_boundary() {
    for zoom in [0.5, 1.0, 2.0] {
        let mut app = app();
        app.ui.tool_options.pen_auto_add_delete = false;
        app.ui.views[0].zoom = zoom;
        anchor(&mut app, [40.0, 40.0]);
        anchor(&mut app, [120.0, 40.0]);
        let tolerance = 6.0 / app.point_zoom().max(0.01) as f64;
        let outside = [40.0 + tolerance * 1.01, 40.0];
        let inside = [40.0 + tolerance * 0.99, 40.0];
        assert_eq!(pen_cursor(&app, outside), PenCursor::Draw);
        assert_eq!(pen_cursor(&app, inside), PenCursor::Close);
        anchor(&mut app, inside);
        assert!(app.ui.pen.is_none(), "the displayed close action must really close at zoom {zoom}");
    }
}

#[test]
fn handle_drag_does_not_advertise_closing_until_release() {
    let mut app = app();
    anchor(&mut app, [40.0, 40.0]);
    pen_down(&mut app, 120.0, 40.0);
    pen_move(&mut app, 40.0, 40.0);
    assert_eq!(pen_cursor(&app, [40.0, 40.0]), PenCursor::Draw);
    pen_up(&mut app);
    assert_eq!(pen_cursor(&app, [40.0, 40.0]), PenCursor::Close);
}

#[test]
fn stale_and_restored_drafts_do_not_advertise_closing() {
    let mut app = app();
    anchor(&mut app, [40.0, 40.0]);
    anchor(&mut app, [120.0, 40.0]);
    assert_eq!(pen_cursor(&app, [40.0, 40.0]), PenCursor::Close);
    let draft = app.ui.pen.clone().unwrap();
    app.run("file.new", json!({"width": 200, "height": 200})).unwrap();
    app.ui.pen = Some(draft);
    assert_eq!(pen_cursor(&app, [40.0, 40.0]), PenCursor::New);
    app.ui.pen.as_mut().unwrap().owner = None;
    assert_eq!(pen_cursor(&app, [40.0, 40.0]), PenCursor::New);
}

#[test]
fn failed_close_keeps_the_close_cursor_with_the_preserved_draft() {
    let mut app = app();
    anchor(&mut app, [40.0, 40.0]);
    anchor(&mut app, [120.0, 40.0]);
    app.ui.pen.as_mut().unwrap().knots[1][0] = [1e40, 40.0];
    let history = app.session.active().unwrap().history.past_len();
    pen_down(&mut app, 40.0, 40.0);
    pen_up(&mut app);
    assert!(app.ui.pen.as_ref().unwrap().commit_failed);
    assert_eq!(app.ui.pen.as_ref().unwrap().knots.len(), 2, "a failed close keeps real anchors without its stand-in");
    assert_eq!(pen_cursor(&app, [40.0, 40.0]), PenCursor::Close);
    assert_eq!(app.session.active().unwrap().history.past_len(), history);
    app.ui.pen.as_mut().unwrap().knots[1] = [[120.0, 40.0]; 3];
    anchor(&mut app, [40.0, 40.0]);
    assert!(app.ui.pen.is_none());
    assert_eq!(app.session.active().unwrap().doc.work_path.as_ref().unwrap().subpaths[0].knots.len(), 2);
    assert_eq!(app.session.active().unwrap().history.past_len(), history + 1);
}

#[test]
#[cfg(target_os = "windows")]
fn native_pen_cursor_changes_at_close_and_clears_for_precise_navigation_and_panels() {
    use egui::{Event, Key, Modifiers, PointerButton};
    use egui_kittest::Harness;
    let mut h = Harness::builder().with_size(vec2(600.0, 500.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            if !ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                return;
            }
            egui::Panel::top("pen-cursor-test-toolbar").show(ui, |ui| {
                ui.label("Toolbar");
            });
            egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
        },
        app(),
    );
    PhotocraftApp::setup_context(&h.ctx, Default::default());
    h.run_steps(4);
    let first = h.state().last_canvas_rect.center() - vec2(40.0, 40.0);
    h.hover_at(first);
    h.run_steps(2);
    let new = h.output().platform_output.cursor_image.clone().expect("the Pen reaches the native cursor output");
    let click = |h: &mut Harness<'_, PhotocraftApp>, pos| {
        for pressed in [true, false] {
            h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
            h.step();
        }
        h.run_steps(2);
    };
    click(&mut h, first);
    let drawing = h.output().platform_output.cursor_image.clone().unwrap();
    assert_ne!(new.rgba, drawing.rgba);
    let second = first + vec2(80.0, 0.0);
    h.hover_at(second);
    h.step();
    click(&mut h, second);
    h.hover_at(first);
    h.run_steps(2);
    let closing = h.output().platform_output.cursor_image.clone().unwrap();
    assert_ne!(drawing.rgba, closing.rgba);
    assert_ne!(new.rgba, closing.rgba);
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(h.ctx.pixels_per_point());
    ctx.add_plugin(crate::tool_cursor::CursorLifecycle);
    let mut expected = ctx.run_ui(Default::default(), |ui| {
        ui.ctx().set_cursor_icon(crate::tool_cursor::close_path(ui.painter(), pos2(100.0, 100.0)));
    });
    expected.textures_delta.clear();
    let expected = expected.platform_output.cursor_image.expect("main's close-path ring");
    assert_eq!(closing.hotspot, expected.hotspot);
    assert_eq!(closing.rgba, expected.rgba, "the canvas preserves main's centred close-path pointer");
    click(&mut h, first);
    assert!(h.state().ui.pen.is_none());
    assert_ne!(h.output().platform_output.cursor_image.as_ref().unwrap().rgba, new.rgba, "the closed anchor advertises Delete");
    h.hover_at(first + vec2(120.0, 120.0));
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_image.as_ref().unwrap().rgba, new.rgba);

    h.state_mut().run("prefs.set", json!({"path": "cursors.other", "value": "precise"})).unwrap();
    h.run_steps(2);
    assert_ne!(h.output().platform_output.cursor_image.as_ref().unwrap().rgba, new.rgba);
    h.state_mut().run("prefs.set", json!({"path": "cursors.other", "value": "standard"})).unwrap();
    h.event(Event::ModifiersChanged(Modifiers { command: true, ctrl: true, ..Default::default() }));
    h.run_steps(2);
    assert!(h.output().platform_output.cursor_image.is_none());
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.event(Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_ne!(h.output().platform_output.cursor_image.as_ref().unwrap().rgba, new.rgba);
    h.event(Event::Key { key: Key::Space, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    h.hover_at(pos2(20.0, 12.0));
    h.run_steps(2);
    assert!(h.output().platform_output.cursor_image.is_none(), "leaving the canvas clears the Pen bitmap");
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn native_convert_cursor_keeps_the_captured_action_when_command_is_pressed_mid_drag() {
    use egui::{Event, Modifiers, PointerButton};
    use egui_kittest::Harness;
    for committed in [false, true] {
        let mut h = Harness::builder().with_size(vec2(600.0, 500.0)).build_ui_state(
            |ui, app: &mut PhotocraftApp| {
                if !ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                    return;
                }
                egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
            },
            app(),
        );
        PhotocraftApp::setup_context(&h.ctx, Default::default());
        h.run_steps(4);
        let first = h.state().last_canvas_rect.center() - vec2(40.0, 40.0);
        let click = |h: &mut Harness<'_, PhotocraftApp>, pos| {
            h.hover_at(pos);
            for pressed in [true, false] {
                h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
                h.step();
            }
            h.run_steps(2);
        };
        for p in [first, first + vec2(80.0, 0.0), first + vec2(80.0, 80.0)] {
            click(&mut h, p);
        }
        if committed {
            click(&mut h, first);
        }
        let command = Modifiers { command: true, ctrl: true, ..Default::default() };
        if !committed {
            h.hover_at(first);
            h.event(Event::ModifiersChanged(Modifiers::ALT));
            h.event(Event::PointerButton { pos: first, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::ALT });
            h.step();
            h.event(Event::ModifiersChanged(command));
            h.event(Event::PointerButton { pos: first, button: PointerButton::Primary, pressed: false, modifiers: command });
            h.run_steps(2);
            assert_eq!(h.state().ui.pen.as_ref().expect("Alt press must not become a Command click at release").knots.len(), 3);
            assert!(!pen_converting(h.state()));
        }
        h.hover_at(first);
        h.event(Event::ModifiersChanged(Modifiers::ALT));
        h.run_steps(2);
        let convert = h.output().platform_output.cursor_image.clone().expect("native Convert cursor before press");
        h.event(Event::PointerButton { pos: first, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::ALT });
        h.step();
        h.event(Event::ModifiersChanged(command));
        let to = first + vec2(0.0, 20.0);
        h.hover_at(to);
        h.run_steps(2);
        assert!(pen_converting(h.state()), "Alt press must start Convert despite Command arriving before the drag threshold: committed={committed}");
        assert_eq!(
            h.output().platform_output.cursor_image.as_ref().expect("captured Convert must retain its native cursor").rgba,
            convert.rgba,
            "captured Convert wins over mid-drag command: committed={committed}"
        );
        h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: command });
        h.run_steps(2);
        assert!(!pen_converting(h.state()));
        assert!(h.output().platform_output.cursor_image.is_none(), "idle command returns the system arrow");
    }
}
