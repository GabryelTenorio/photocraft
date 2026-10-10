//! #2849: a complete secondary tap can arrive in one frame. Keep its opening press from
//! dismissing a canvas menu whose position rounds to a physical pixel.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;

use crate::{PhotocraftApp, Services, Tool};

fn harness(tool: Tool, selection: bool, scale: f32) -> Harness<'static, PhotocraftApp> {
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).with_pixels_per_point(scale).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Services::default());
        app.run("file.new", json!({"width": 400, "height": 300})).unwrap();
        app.run("layer.new.layer", json!({"name": "Color"})).unwrap();
        app.run("edit.fill", json!({"color": "#cc6644"})).unwrap();
        if selection {
            app.run("select.rect", json!({"x": 40, "y": 40, "width": 200, "height": 100})).unwrap();
        }
        app.ui.tool = tool;
        app
    });
    h.run_steps(5);
    h
}

fn fractional_point(h: &Harness<'_, PhotocraftApp>, scale: f32) -> Pos2 {
    let center = crate::rulers::content_rect(h.state(), h.state().last_canvas_rect).center();
    pos2(((center.x * scale).floor() + 0.75) / scale, ((center.y * scale).floor() + 0.75) / scale)
}

fn press(h: &mut Harness<'_, PhotocraftApp>, at: Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerButton { pos: at, button, pressed, modifiers });
}

fn tap(h: &mut Harness<'_, PhotocraftApp>, at: Pos2, modifiers: Modifiers, same_frame: bool) {
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::PointerMoved(at));
    h.run_steps(2);
    for pressed in [true, false] {
        // Harness::event separates queued events into frames; RawInput preserves a quick tap.
        press(h, at, PointerButton::Secondary, pressed, modifiers);
        if !same_frame {
            h.run_steps(1);
        }
    }
    h.run_steps(3);
}

fn is_open(h: &Harness<'_, PhotocraftApp>, layers: bool) -> bool {
    if layers { h.state().ui.layer_menu.is_some() } else { h.state().ui.canvas_tool_menu.is_some() }
}

fn dismiss_outside(h: &mut Harness<'_, PhotocraftApp>, layers: bool) {
    // The status-bar corner is outside both menus and does not edit the document.
    let at = pos2(1190.0, 790.0);
    h.event(Event::PointerMoved(at));
    h.run_steps(2);
    press(h, at, PointerButton::Primary, true, Modifiers::NONE);
    h.run_steps(1);
    assert!(!is_open(h, layers), "a later outside press closes immediately");
    press(h, at, PointerButton::Primary, false, Modifiers::NONE);
    h.run_steps(2);
}

fn check_reopening(tool: Tool, modifiers: Modifiers, layers: bool) {
    for scale in [1.0, 2.0] {
        for selection in [false, true] {
            let mut h = harness(tool, selection, scale);
            let center = fractional_point(&h, scale);
            for at in [center, center + vec2(40.0, 20.0), center - vec2(40.0, 20.0), center.round()] {
                for same_frame in [false, true] {
                    for attempt in 0..3 {
                        let before = h.state().session.active().unwrap();
                        let (revision, history, active, selected) = (before.revision, before.history.past_len(), before.active_layer, before.selected_layers());
                        let mask = before.doc.selection.clone();
                        tap(&mut h, at, modifiers, same_frame);
                        assert!(is_open(&h, layers), "{tool:?} scale={scale} selection={selection} same_frame={same_frame} reopen={attempt} at {at:?}");
                        let after = h.state().session.active().unwrap();
                        assert_eq!(
                            (after.revision, after.history.past_len(), after.active_layer, after.selected_layers()),
                            (revision, history, active, selected),
                            "opening a menu does not edit or select a layer"
                        );
                        assert_eq!(after.doc.selection, mask, "opening a menu preserves the selection mask");
                        if !layers {
                            let menu = h.state().ui.canvas_tool_menu.as_ref().unwrap();
                            assert_eq!(menu.has_selection, selection);
                            assert!(!menu.pasteboard);
                        }
                        dismiss_outside(&mut h, layers);
                        let after = h.state().session.active().unwrap();
                        assert_eq!(after.doc.selection, mask, "outside dismissal preserves the selection mask");
                        assert_eq!((after.revision, after.history.past_len()), (revision, history));
                    }
                }
            }
            tap(&mut h, center, modifiers, true);
            h.key_press(egui::Key::Escape);
            h.run_steps(2);
            assert!(!is_open(&h, layers));
        }
    }
}

#[test]
fn secondary_taps_keep_selection_menus_open() {
    for tool in [Tool::RectMarquee, Tool::EllipseMarquee, Tool::Lasso, Tool::PolygonLasso, Tool::MagneticLasso, Tool::MagicWand, Tool::ObjectSelection] {
        check_reopening(tool, Modifiers::NONE, false);
    }
}

#[test]
fn secondary_taps_keep_canvas_layer_menus_open() {
    check_reopening(Tool::Move, Modifiers::NONE, true);
    check_reopening(Tool::RectMarquee, Modifiers::COMMAND, true);
}

#[test]
fn reopened_canvas_menus_still_choose_their_actions() {
    let mut h = harness(Tool::RectMarquee, true, 1.0);
    let at = fractional_point(&h, 1.0);
    tap(&mut h, at, Modifiers::NONE, true);
    assert!(is_open(&h, false));
    dismiss_outside(&mut h, false);
    tap(&mut h, at, Modifiers::NONE, true);
    assert!(is_open(&h, false));
    h.get_by_label(crate::i18n::t("Deselect")).click();
    h.run_steps(3);
    assert!(!is_open(&h, false));
    assert!(h.state().session.active().unwrap().doc.selection.is_none());
    h.state_mut().ui.tool = Tool::Move;
    h.run_steps(2);
    tap(&mut h, at, Modifiers::NONE, true);
    assert!(is_open(&h, true));
    dismiss_outside(&mut h, true);
    tap(&mut h, at, Modifiers::NONE, true);
    assert!(is_open(&h, true));
    let menu_rect = h.ctx.memory(|m| m.area_rect(egui::Id::new("canvas-layer-menu"))).unwrap();
    h.query_all_by_label("Background").find(|node| menu_rect.contains(node.rect().center())).expect("Background in the layer menu").click();
    h.run_steps(3);
    assert!(!is_open(&h, true));
    let state = h.state().session.active().unwrap();
    assert_eq!(state.doc.layer(state.active_layer.unwrap()).unwrap().name, "Background");
}

#[test]
fn secondary_taps_keep_other_canvas_tool_menus_open() {
    for scale in [1.0, 2.0] {
        for tool in [Tool::Pen, Tool::RectMarquee, Tool::Move] {
            let mut h = harness(tool, true, scale);
            let center = fractional_point(&h, scale);
            let pasteboard = tool != Tool::Pen;
            let at = if pasteboard { center + vec2(240.0, 0.0) } else { center };
            for _ in 0..3 {
                tap(&mut h, at, Modifiers::NONE, true);
                let menu = h.state().ui.canvas_tool_menu.as_ref().expect("Pen or pasteboard menu stays open");
                assert_eq!(menu.pasteboard, pasteboard);
                dismiss_outside(&mut h, false);
            }
        }
        let mut h = harness(Tool::Move, false, scale);
        let ctx = h.ctx.clone();
        crate::transform_tool::begin(h.state_mut(), &ctx).unwrap();
        h.run_steps(3);
        let at = fractional_point(&h, scale);
        for _ in 0..3 {
            tap(&mut h, at, Modifiers::NONE, true);
            assert!(h.state().ui.canvas_tool_menu.as_ref().is_some_and(|m| m.transform));
            dismiss_outside(&mut h, false);
            assert!(h.state().ui.transform.is_some(), "dismissing its menu keeps the transform session");
        }
    }
}

#[test]
fn control_channel_opening_uses_the_same_dismissal_guard() {
    use crate::control::{ControlRequest, Outcome, handle};

    for (tool, layers, pasteboard) in [(Tool::RectMarquee, false, false), (Tool::Move, true, false), (Tool::RectMarquee, false, true)] {
        let mut h = harness(tool, true, 1.0);
        let mut at = fractional_point(&h, 1.0);
        if pasteboard {
            at.x += 240.0;
        }
        let [x, y] = crate::canvas::ViewXform::active(h.state()).unwrap().to_doc(at);
        let (req, _) = ControlRequest::new("ui.pointer", json!({"button":"secondary", "events":[{"kind":"down", "x":x, "y":y}, {"kind":"up", "x":x, "y":y}]}));
        let ctx = h.ctx.clone();
        let mut input = h.input_mut().clone();
        input.events = vec![
            Event::PointerMoved(at),
            Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE },
            Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE },
        ];
        let mut opened = false;
        ctx.run_ui(input, |ui| {
            if !opened {
                let Outcome::Done(reply) = handle(h.state_mut(), ui.ctx(), &req) else { panic!("pointer reply") };
                assert_eq!(reply["ok"], true, "{reply}");
                opened = true;
            }
            crate::canvas_tool_menu::show(h.state_mut(), ui.ctx());
            crate::layer_pick_ui::show(h.state_mut(), ui.ctx());
            assert!(is_open(&h, layers), "the control opening press does not dismiss its menu");
        })
        .drop_without_applying_deltas();
        if !layers {
            assert_eq!(h.state().ui.canvas_tool_menu.as_ref().unwrap().pasteboard, pasteboard);
        }
        dismiss_outside(&mut h, layers);
    }
}
