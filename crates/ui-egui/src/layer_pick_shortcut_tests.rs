//! #2298: a rebindable keyboard layer pick through the real canvas and shortcut dispatcher.
use crate::{PhotocraftApp, state::Tool};
use egui::{Event, Key, Modifiers, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn command() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers { command: true, mac_cmd: true, ..Modifiers::NONE }
    } else {
        Modifiers { command: true, ctrl: true, ..Modifiers::NONE }
    }
}

fn press(h: &mut Harness<'_, PhotocraftApp>, key: Key, modifiers: Modifiers) {
    h.input_mut().events.extend([
        Event::ModifiersChanged(modifiers),
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
        Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers },
    ]);
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

fn harness() -> (Harness<'static, PhotocraftApp>, u64, u64) {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 200, "height": 100, "background": "transparent"})).unwrap();
    let mut ids = Vec::new();
    for (name, x) in [("Left", 10), ("Right", 120)] {
        app.run("layer.new.layer", json!({"name": name})).unwrap();
        app.run("select.rect", json!({"x": x, "y": 10, "width": 40, "height": 60})).unwrap();
        app.run("edit.fill", json!({"color": "#ff0000"})).unwrap();
        app.run("select.deselect", json!({})).unwrap();
        ids.push(app.session.active().unwrap().active_layer.unwrap().0);
    }
    app.run("layer.new.layer", json!({"name": "Hidden"})).unwrap();
    app.run("edit.fill", json!({"color": "#0000ff"})).unwrap();
    app.run("layer.setProps", json!({"visible": false})).unwrap();
    app.run("layer.new.layer", json!({"name": "Locked"})).unwrap();
    app.run("edit.fill", json!({"color": "#0000ff"})).unwrap();
    app.run("layer.setProps", json!({"locks": {"all": true}})).unwrap();
    app.run("layer.new.layer", json!({"name": "Empty"})).unwrap();
    app.run("prefs.set", json!({"path": "shortcuts.layer.selectUnderCursor", "value": "Cmd+Alt+Shift+U"})).unwrap();
    app.ui.tool = Tool::Brush;
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).with_max_steps(64).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        app
    });
    h.run_steps(6);
    (h, ids[0], ids[1])
}

fn pointer(h: &mut Harness<'_, PhotocraftApp>, x: f32, y: f32) {
    let p = crate::canvas::ViewXform::active(h.state()).unwrap().to_screen(x, y);
    h.event(Event::PointerMoved(p));
    h.run_steps(2);
}

#[test]
fn layer_pick_shortcut_follows_the_transformed_pointer_without_editing() {
    let (mut h, left, right) = harness();
    for (zoom, rotation, flip) in [(1.0, 0.0, false), (2.0, 30.0, true), (0.75, -90.0, false)] {
        {
            let app = h.state_mut();
            app.ui.views[0].fit_pending = false;
            app.ui.views[0].zoom = zoom;
            app.ui.views[0].center = [100.0, 50.0];
            app.ui.views[0].rotation = rotation;
            app.ui.view.flip_horizontal = flip;
        }
        h.run_steps(2);
        for (x, expected) in [(20.0, left), (130.0, right)] {
            pointer(&mut h, x, 30.0);
            let st = h.state().session.active().unwrap();
            let (layers, selection, history) = (st.doc.layers.clone(), st.doc.selection.clone(), st.history.past_len());
            press(&mut h, Key::U, command() | Modifiers::ALT | Modifiers::SHIFT);
            let st = h.state().session.active().unwrap();
            assert_eq!(st.active_layer.unwrap().0, expected);
            assert_eq!(st.doc.layers, layers);
            assert_eq!(st.doc.selection, selection);
            assert_eq!(st.history.past_len(), history);
            assert_eq!(h.state().ui.tool, Tool::Brush);
            assert!(h.state().drag.is_none());
        }
    }
}

#[test]
fn layer_pick_shortcut_can_be_reassigned_removed_and_ignores_empty_space() {
    let (mut h, left, right) = harness();
    h.state_mut().run("prefs.set", json!({"path": "shortcuts.layer.selectUnderCursor", "value": "Cmd+Alt+K"})).unwrap();
    pointer(&mut h, 20.0, 30.0);
    press(&mut h, Key::K, command() | Modifiers::ALT);
    assert_eq!(h.state().session.active().unwrap().active_layer.unwrap().0, left);
    pointer(&mut h, 130.0, 30.0);
    press(&mut h, Key::U, command() | Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(h.state().session.active().unwrap().active_layer.unwrap().0, left, "old binding no longer picks");
    press(&mut h, Key::K, command() | Modifiers::ALT);
    assert_eq!(h.state().session.active().unwrap().active_layer.unwrap().0, right);
    // Neither transparent pixels nor the toolbar selects a different layer.
    pointer(&mut h, 90.0, 30.0);
    press(&mut h, Key::K, command() | Modifiers::ALT);
    assert_eq!(h.state().session.active().unwrap().active_layer.unwrap().0, right);
    h.event(Event::PointerMoved(Pos2::new(5.0, 5.0)));
    h.run_steps(2);
    press(&mut h, Key::K, command() | Modifiers::ALT);
    assert_eq!(h.state().session.active().unwrap().active_layer.unwrap().0, right);
    h.state_mut().run("prefs.set", json!({"path": "shortcuts.layer.selectUnderCursor", "value": ""})).unwrap();
    pointer(&mut h, 20.0, 30.0);
    press(&mut h, Key::K, command() | Modifiers::ALT);
    assert_eq!(h.state().session.active().unwrap().active_layer.unwrap().0, right);
}

#[test]
fn layer_pick_shortcut_respects_nested_visibility_and_replaces_multiselection() {
    let (mut h, left, right) = harness();
    h.state_mut().run("layer.new.group", json!({"name": "Folder"})).unwrap();
    let group = h.state().session.active().unwrap().active_layer.unwrap().0;
    h.state_mut().run("layer.moveTo", json!({"layer": left, "target": group, "position": "into"})).unwrap();
    h.state_mut().run("layer.select", json!({"layer": left})).unwrap();
    h.state_mut().run("layer.select", json!({"layer": right, "mode": "add"})).unwrap();
    h.run_steps(2);
    pointer(&mut h, 20.0, 30.0);
    press(&mut h, Key::U, command() | Modifiers::ALT | Modifiers::SHIFT);
    let st = h.state().session.active().unwrap();
    assert_eq!(st.active_layer.unwrap().0, left);
    assert_eq!(st.selected_layers().len(), 1);
    h.state_mut().run("layer.setProps", json!({"layer": group, "visible": false})).unwrap();
    h.state_mut().run("layer.select", json!({"layer": right})).unwrap();
    press(&mut h, Key::U, command() | Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(h.state().session.active().unwrap().active_layer.unwrap().0, right, "a hidden parent's children are skipped");
    h.state_mut().run("layer.setProps", json!({"layer": group, "visible": true, "locks": {"all": true}})).unwrap();
    press(&mut h, Key::U, command() | Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(h.state().session.active().unwrap().active_layer.unwrap().0, right, "a fully locked parent also excludes its children");
}

#[test]
fn layer_pick_shortcut_is_unique_discoverable_and_keeps_inline_typing() {
    let (mut h, left, _) = harness();
    let binding = crate::shortcuts::parse("Cmd+Alt+Shift+U").unwrap();
    let ids: Vec<_> = crate::shortcut_dispatch::bindings(h.state()).into_iter().filter(|(_, sc)| *sc == binding).map(|(id, _)| id).collect();
    assert_eq!(ids, ["layer.selectUnderCursor"]);
    let row = crate::prefs_ui::shortcut_items(h.state()).into_iter().find(|r| r.0 == "layer.selectUnderCursor").unwrap();
    assert_eq!(row.1, "Select Layer Under Cursor");
    assert_eq!(row.2, ["Layer"]);
    assert_eq!(row.3, None, "the command has no factory binding");
    let original = photocraft_engine::command_specs().into_iter().find(|c| c.id == "layer.pickAt").unwrap();
    assert_eq!(original.label, "Auto-Select Layer");
    assert_eq!(original.shortcut, None);
    let active = h.state().session.active().unwrap().active_layer.unwrap().0;
    let ctx = h.ctx.clone();
    crate::layer_row_ui::start_rename(&ctx, active, "Empty");
    h.run_steps(2);
    assert!(ctx.text_edit_focused());
    pointer(&mut h, 20.0, 30.0);
    press(&mut h, Key::U, command() | Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(h.state().session.active().unwrap().active_layer.unwrap().0, active);
    assert_ne!(active, left);
}

#[test]
#[ignore = "remote performance evidence"]
fn layer_pick_shortcut_large_stack() {
    let (mut h, left, _) = harness();
    h.state_mut()
        .session
        .edit("Synthetic layer stack", |doc, _| {
            let template = doc.layers.last().unwrap().clone();
            let add = 2048 - doc.walk().len();
            for i in 0..add {
                let mut empty = template.clone();
                empty.id = photocraft_doc::LayerId(u64::MAX - i as u64);
                doc.layers.push(empty);
            }
            Ok(())
        })
        .unwrap();
    h.run_steps(2);
    pointer(&mut h, 20.0, 30.0);
    let ctx = h.ctx.clone();
    let history = h.state().session.active().unwrap().history.past_len();
    let started = std::time::Instant::now();
    for _ in 0..100 {
        let result = crate::menus::invoke(h.state_mut(), &ctx, "layer.selectUnderCursor", json!({})).unwrap();
        assert_eq!(result["layer"], left);
    }
    println!("{}", json!({"layers": 2048, "picks": 100, "meanMs": started.elapsed().as_secs_f64() * 10.0}));
    assert_eq!(h.state().session.active().unwrap().active_layer.unwrap().0, left);
    assert_eq!(h.state().session.active().unwrap().history.past_len(), history);
}
