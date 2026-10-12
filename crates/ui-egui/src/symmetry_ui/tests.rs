use super::*;
use egui::{Context, Modifiers, Pos2, RawInput, Rect};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use photocraft_engine::Session;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    app.run("file.new", json!({"width":100,"height":80})).unwrap();
    app.sync_views();
    app.ui.tool = Tool::Brush;

    app.ui.views[0].zoom = 1.0;
    app.last_canvas_rect = Rect::from_min_size(Pos2::ZERO, vec2(500.0, 400.0));
    app
}
fn down(app: &mut PhotocraftApp, p: [f64; 2]) {
    crate::canvas::tool_event(app, ToolEvent::Down { x: p[0], y: p[1], pressure: 1.0 }, Modifiers::NONE);
}
fn move_to(app: &mut PhotocraftApp, p: [f64; 2], mods: Modifiers) {
    crate::canvas::tool_event(app, ToolEvent::Move { x: p[0], y: p[1], pressure: 1.0 }, mods);
}
fn up(app: &mut PhotocraftApp, p: [f64; 2]) {
    crate::canvas::tool_event(app, ToolEvent::Up { x: p[0], y: p[1] }, Modifiers::NONE);
}
fn key(app: &mut PhotocraftApp, key: Key) {
    let ctx = Context::default();
    let input = RawInput {
        focused: true,
        events: vec![egui::Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: Modifiers::NONE }],
        ..Default::default()
    };
    ctx.run_ui(input, |ui| track(app, ui.ctx())).textures_delta.clear();
}
#[test]
fn transform_moves_rotates_snaps_and_commits_without_painting_or_history() {
    let mut app = app();
    app.run("paint.setSymmetry", json!({"mode":"dual"})).unwrap();
    let before = app.session.active().unwrap().doc.clone();
    let history = app.session.active().unwrap().history.past_len();
    crate::menus::invoke(&mut app, &Context::default(), "ui.symmetryTransform", json!({})).unwrap();
    down(&mut app, [50.0, 40.0]);
    move_to(&mut app, [60.0, 45.0], Modifiers::NONE);
    up(&mut app, [60.0, 45.0]);
    let p = app.ui.symmetry_transform.as_ref().unwrap().preview;
    assert_eq!(p.center, [60.0, 45.0]);
    assert_eq!(preset(&app).unwrap().center, [50.0, 40.0]);
    let handle = rotation_handle(p, [100.0, 80.0]);
    down(&mut app, handle);
    let angle = 112_f64.to_radians();
    move_to(&mut app, [60.0 + 28.0 * angle.cos(), 45.0 + 28.0 * angle.sin()], Modifiers { shift: true, ..Modifiers::NONE });
    up(&mut app, [0.0, 0.0]);
    assert!((app.ui.symmetry_transform.as_ref().unwrap().preview.rotation - 15.0).abs() < 1e-8);
    key(&mut app, Key::Enter);
    assert!(app.ui.symmetry_transform.is_none());
    assert_eq!(preset(&app).unwrap().center, [60.0, 45.0]);
    assert_eq!(app.session.active().unwrap().doc, before);
    assert_eq!(app.session.active().unwrap().history.past_len(), history);
    assert_eq!(app.session.journal.last().unwrap().0, "paint.setSymmetry");
}
#[test]
fn cancel_competing_edits_tool_document_and_focus_changes_drop_only_the_preview() {
    let mut app = app();
    app.run("paint.setSymmetry", json!({"mode":"vertical"})).unwrap();
    begin(&mut app).unwrap();
    down(&mut app, [50.0, 40.0]);
    move_to(&mut app, [10.0, 20.0], Modifiers::NONE);
    key(&mut app, Key::Escape);
    assert_eq!(preset(&app).unwrap().center, [50.0, 40.0]);
    assert!(app.ui.symmetry_transform.is_none());
    begin(&mut app).unwrap();
    app.run("document.inspect", json!({})).unwrap();
    assert!(app.ui.symmetry_transform.is_some());
    app.run("paint.setSymmetry", json!({"mode":"horizontal"})).unwrap();
    assert!(app.ui.symmetry_transform.is_none());
    for change in 0..4 {
        begin(&mut app).unwrap();
        match change {
            0 => app.ui.tool = Tool::Pencil,
            1 => {
                app.session.execute("file.new", json!({"width":10,"height":10})).unwrap();
            }
            2 => {
                app.session.execute("layer.new.layer", json!({})).unwrap();
            }
            _ => {}
        }
        let ctx = Context::default();
        ctx.run_ui(RawInput { focused: change != 3, ..Default::default() }, |ui| track(&mut app, ui.ctx())).textures_delta.clear();
        assert!(app.ui.symmetry_transform.is_none());
        if change == 1 {
            assert!(app.session.set_active(0));
        }
    }
    begin(&mut app).unwrap();
    let ctx = Context::default();
    let input = RawInput {
        focused: true,
        events: vec![egui::Event::Key { key: Key::V, physical_key: Some(Key::V), pressed: true, repeat: false, modifiers: Modifiers::NONE }],
        ..Default::default()
    };
    ctx.run_ui(input, |ui| {
        crate::shortcuts::handle(&mut app, ui.ctx());
        track(&mut app, ui.ctx());
    })
    .textures_delta
    .clear();
    assert_eq!(app.ui.tool, Tool::Move);
    assert!(app.ui.symmetry_transform.is_none());
}
#[test]
fn guides_never_consume_normal_paint_and_transform_background_clicks_never_paint() {
    let mut app = app();
    app.run("paint.setSymmetry", json!({"mode":"vertical"})).unwrap();
    let before = app.session.active().unwrap().history.past_len();
    down(&mut app, [20.0, 20.0]);
    up(&mut app, [20.0, 20.0]);
    assert_eq!(app.session.active().unwrap().history.past_len(), before + 1);
    begin(&mut app).unwrap();
    down(&mut app, [10.0, 10.0]);
    move_to(&mut app, [30.0, 30.0], Modifiers::NONE);
    up(&mut app, [30.0, 30.0]);
    assert_eq!(app.session.active().unwrap().history.past_len(), before + 1);
}
#[test]
fn menu_selects_all_modes_resets_positions_and_preserves_work_path_choice() {
    let mut app = app();
    app.session.edit_prefs(|p| p.interface.language = "en".into());
    app.run("path.set", json!({"name":"work","path":{"subpaths":[{"knots":[[20,0],[20,80]]}]}})).unwrap();
    let mut h = Harness::builder().with_size(vec2(500.0, 450.0)).build_ui_state(
        |ctx, app| {
            crate::i18n::with_language(crate::i18n::Lang::EN, || menu(app, ctx));
        },
        app,
    );
    for mode in SymmetryMode::ALL {
        h.get_by_label("Set painting symmetry options").click();
        h.run_steps(3);
        h.get_by_label(label(mode)).click();
        h.run_steps(3);
        assert_eq!(preset(h.state()).unwrap().mode, mode);
        assert_eq!(preset(h.state()).unwrap().center, [50.0, 40.0]);
        h.get_by_label("Set painting symmetry options").click();
        h.run_steps(3);
        assert_eq!(h.get_by_label(label(mode)).accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
        h.get_by_label("Transform Symmetry").click();
        h.run_steps(3);
        assert!(h.state().ui.symmetry_transform.is_some());
        h.state_mut().ui.symmetry_transform = None;
    }
    h.get_by_label("Set painting symmetry options").click();
    h.run_steps(3);
    h.get_by_label("Work Path").click();
    h.run_steps(3);
    assert_eq!(h.state().session.active().unwrap().symmetry.as_ref().unwrap().path_source(), Some("work"));
    assert!(!can_transform(h.state()));
    h.get_by_label("Set painting symmetry options").click();
    h.run_steps(3);
    h.get_by_label("Symmetry Off").click();
    h.run_steps(3);
    assert!(preset(h.state()).is_none());
}
#[test]
fn overlays_follow_document_view_transforms_in_every_theme() {
    for theme in crate::theme::ThemeKind::ALL {
        let mut app = app();
        app.run("paint.setSymmetry", json!({"mode":"dual"})).unwrap();
        let ctx = Context::default();
        crate::theme::apply(&ctx, theme);
        let xf =
            ViewXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(500.0, 400.0)), zoom: 1.7, center: [50.0, 40.0], rotation: 37.0, flip: true, aspect: 1.0 };
        let mut out = ctx.run_ui(RawInput { screen_rect: Some(xf.rect), ..Default::default() }, |ui| {
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("symmetry")));
            draw(&app, &painter, &xf);
        });
        let lines: Vec<_> =
            out.shapes.iter().filter_map(|s| if let egui::Shape::LineSegment { points, .. } = &s.shape { Some(*points) } else { None }).collect();
        assert_eq!(lines.len(), 2);
        let c = xf.to_screen(50.0, 40.0);
        for [a, b] in lines {
            assert!((egui::pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0) - c).length() < 0.01);
        }
        out.textures_delta.clear();
    }
    let de = crate::i18n::Lang::from_code("de").unwrap();
    assert_eq!(crate::i18n::tr(de, "Dual Axis"), "Duale Achse");
    assert_eq!(crate::i18n::tr(de, "Transform Symmetry"), "Symmetrie transformieren");
}
/// The canvas at 100 %, a 400 x 300 document centred, with a dual-axis symmetry on Brush.
fn canvas() -> Harness<'static, PhotocraftApp> {
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 400, "height": 300})).unwrap();
    app.sync_views();
    app.ui.extras.rulers = false;
    app.ui.tool = Tool::Brush;
    app.run("paint.setSymmetry", json!({"mode":"dual"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1000.0, 700.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            let ctx = ui.ctx().clone();
            if !ctx.fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                return;
            }
            crate::shortcuts::handle(app, &ctx);
            egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
    h.run_steps(4);
    let v = &mut h.state_mut().ui.views[0];
    v.zoom = 1.0;
    v.center = [200.0, 150.0];
    v.fit_pending = false;
    h.run_steps(2);
    h
}
fn hover(h: &mut Harness<'static, PhotocraftApp>, p: [f64; 2]) -> (egui::CursorIcon, bool) {
    let app = h.state();
    let v = &app.ui.views[0];
    let xf = ViewXform {
        rect: crate::rulers::content_rect(app, app.last_canvas_rect),
        zoom: v.zoom,
        center: v.center,
        flip: app.ui.view.flip_horizontal,
        rotation: v.rotation,
        aspect: app.ui.view.display_aspect(),
    };
    h.event(egui::Event::PointerMoved(xf.to_screen(p[0] as f32, p[1] as f32)));
    h.run_steps(2);
    let out = &h.output().platform_output;
    (out.cursor_icon, out.cursor_image.is_some())
}
/// #4495: while the symmetry is transformed the brush tip is not the cursor. The centre moves it,
/// the handle turns it (the drawn arc, no tip bitmap), and elsewhere a press does nothing: the arrow.
#[test]
fn transform_shows_move_rotate_and_arrow_cursors_instead_of_the_brush_tip() {
    let mut h = canvas();
    let (_, tip) = hover(&mut h, [100.0, 100.0]);
    assert!(tip, "outside the transform the brush tip is the cursor");
    begin(h.state_mut()).unwrap();
    h.run_steps(1);
    let p = h.state().ui.symmetry_transform.as_ref().unwrap().preview;
    assert_eq!(hover(&mut h, p.center), (egui::CursorIcon::Move, false), "centre");
    assert_eq!(hover(&mut h, rotation_handle(p, [400.0, 300.0])), (egui::CursorIcon::None, false), "rotation handle");
    assert_eq!(hover(&mut h, [100.0, 100.0]), (egui::CursorIcon::Default, false), "background");
}
/// #4495: Enter and Esc were the only way out of the transform. The options bar carries Photoshop's
/// commit and cancel buttons while it is open, so a pen or a touch screen can finish it too.
#[test]
fn options_bar_commits_or_cancels_the_symmetry_transform() {
    for commit in [true, false] {
        let mut app = app();
        app.session.edit_prefs(|p| p.interface.language = "en".into());
        app.run("paint.setSymmetry", json!({"mode":"dual"})).unwrap();
        let mut h = Harness::builder().with_size(vec2(1600.0, 300.0)).build_ui_state(|ui, app| crate::panels::options_bar(app, ui), app);
        PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
        h.run_steps(4);
        assert!(h.query_by_label("Commit transform").is_none() && h.query_by_label("Cancel transform").is_none());
        begin(h.state_mut()).unwrap();
        h.state_mut().ui.symmetry_transform.as_mut().unwrap().preview.center = [60.0, 45.0];
        h.run_steps(2);
        h.get_by_label(if commit { "Commit transform" } else { "Cancel transform" }).click();
        h.run_steps(2);
        assert!(h.state().ui.symmetry_transform.is_none());
        assert_eq!(preset(h.state()).unwrap().center, if commit { [60.0, 45.0] } else { [50.0, 40.0] });
        assert!(h.query_by_label("Commit transform").is_none());
    }
}
