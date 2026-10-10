use super::*;
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn harness(tool: Tool) -> Harness<'static, PhotocraftApp> {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 400, "height": 300})).unwrap();
    app.ui.tool = tool;
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).with_max_steps(64).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        app
    });
    h.run_steps(6);
    let p = h.state().last_canvas_rect.center();
    h.event(Event::PointerMoved(p));
    h.run_steps(2);
    h
}

fn key(h: &mut Harness<'_, PhotocraftApp>, pressed: bool, repeat: bool) {
    h.input_mut().events.push(Event::Key { key: Key::Z, physical_key: None, pressed, repeat, modifiers: Modifiers::NONE });
    h.step();
}

fn button(h: &mut Harness<'_, PhotocraftApp>, p: Pos2, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.extend([
        Event::ModifiersChanged(modifiers),
        Event::PointerMoved(p),
        Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers },
    ]);
    h.step();
}

#[test]
fn holding_z_zooms_with_alt_direction_then_returns_to_brush_without_painting() {
    let mut h = harness(Tool::Brush);
    let history = h.state().session.active().unwrap().history.past_len();
    let layers = h.state().session.active().unwrap().doc.layers.clone();
    key(&mut h, true, false);
    h.run_steps(15);
    assert_eq!(h.state().ui.tool, Tool::Brush, "the selected tool is retained during a hold");
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ZoomIn);
    let before = h.state().current_zoom();
    let p = h.state().last_canvas_rect.center();
    button(&mut h, p, true, Modifiers::ALT);
    button(&mut h, p, false, Modifiers::ALT);
    assert!(h.state().current_zoom() < before, "Alt+Z zooms out");
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ZoomOut);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    key(&mut h, false, false);
    h.run_steps(2);
    assert_eq!(h.state().ui.tool, Tool::Brush);
    assert_eq!(h.state().session.active().unwrap().history.past_len(), history);
    assert_eq!(h.state().session.active().unwrap().doc.layers, layers);
    assert!(h.state().drag.is_none());
}

#[test]
fn short_taps_select_zoom_but_key_repeat_keeps_the_original_tool() {
    let mut h = harness(Tool::Brush);
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key: Key::Z, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
    }
    h.step();
    assert_eq!(h.state().ui.tool, Tool::Zoom);
    h.state_mut().ui.tool = Tool::Brush;
    key(&mut h, true, false);
    h.run_steps(15);
    key(&mut h, true, true);
    key(&mut h, false, false);
    h.run_steps(2);
    assert_eq!(h.state().ui.tool, Tool::Brush);
    // A normal quick press and release in separate frames is still a permanent tool switch.
    key(&mut h, true, false);
    key(&mut h, false, false);
    assert_eq!(h.state().ui.tool, Tool::Zoom);
}

#[test]
fn releasing_z_during_a_zoom_drag_never_hands_the_gesture_to_the_brush() {
    let mut h = harness(Tool::Brush);
    let history = h.state().session.active().unwrap().history.past_len();
    let layers = h.state().session.active().unwrap().doc.layers.clone();
    key(&mut h, true, false);
    h.run_steps(15);
    let p = h.state().last_canvas_rect.center();
    let before = h.state().current_zoom();
    button(&mut h, p, true, Modifiers::NONE);
    h.event(Event::PointerMoved(p + vec2(60.0, 25.0)));
    h.run_steps(2);
    key(&mut h, false, false);
    h.event(Event::PointerMoved(p + vec2(90.0, 30.0)));
    h.run_steps(2);
    button(&mut h, p + vec2(90.0, 30.0), false, Modifiers::NONE);
    h.run_steps(2);
    assert_ne!(h.state().current_zoom(), before);
    assert_eq!(h.state().ui.tool, Tool::Brush);
    assert_eq!(h.state().session.active().unwrap().history.past_len(), history);
    assert_eq!(h.state().session.active().unwrap().doc.layers, layers);
    assert!(h.state().drag.is_none());
}

#[test]
fn holding_z_preserves_an_unfinished_pen_path_and_crop_frame() {
    let mut h = harness(Tool::Pen);
    h.state_mut().ui.pen = Some(crate::vector_ui::PenPath {
        knots: vec![[[40.0, 40.0]; 3], [[160.0, 40.0]; 3], [[160.0, 180.0]; 3]],
        unlinked: Vec::new(),
        dragging: false,
        adjusting_last: false,
    });
    key(&mut h, true, false);
    h.run_steps(15);
    assert!(h.state().ui.pen.is_some());
    assert!(h.state().session.active().unwrap().doc.work_path.is_none(), "temporary zoom must not commit the Pen path");
    key(&mut h, false, false);
    h.run_steps(2);
    assert_eq!(h.state().ui.tool, Tool::Pen);
    assert!(h.state().ui.pen.is_some());
    let mut h = harness(Tool::Crop);
    let frame = h.state().ui.crop_rect;
    assert!(frame.is_some());
    key(&mut h, true, false);
    h.run_steps(15);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ZoomIn);
    key(&mut h, false, false);
    h.run_steps(2);
    assert_eq!(h.state().ui.tool, Tool::Crop);
    assert_eq!(h.state().ui.crop_rect, frame);
}

#[test]
fn focus_loss_and_explicit_tool_changes_cancel_a_held_zoom() {
    let mut h = harness(Tool::Brush);
    key(&mut h, true, false);
    h.run_steps(15);
    h.input_mut().focused = false;
    h.step();
    h.input_mut().focused = true;
    key(&mut h, false, false);
    assert_eq!(h.state().ui.tool, Tool::Brush);
    key(&mut h, true, false);
    h.state_mut().ui.tool = Tool::Move;
    h.run_steps(15);
    key(&mut h, false, false);
    assert_eq!(h.state().ui.tool, Tool::Move);
}

#[test]
fn alt_can_be_held_before_z_to_temporarily_zoom_out() {
    let mut h = harness(Tool::Brush);
    h.input_mut().events.extend([
        Event::ModifiersChanged(Modifiers::ALT),
        Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::ALT },
    ]);
    h.step();
    h.run_steps(15);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ZoomOut);
    let p = h.state().last_canvas_rect.center();
    let before = h.state().current_zoom();
    button(&mut h, p, true, Modifiers::ALT);
    button(&mut h, p, false, Modifiers::ALT);
    assert!(h.state().current_zoom() < before);
    h.input_mut().events.push(Event::Key { key: Key::Z, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::ALT });
    h.step();
    h.run_steps(2);
    assert_eq!(h.state().ui.tool, Tool::Brush);
}
