use super::*;
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};

fn app() -> PhotocraftApp {
    PhotocraftApp::new(photocraft_engine::Session::new(), Default::default())
}

#[test]
fn panel_commands_group_split_reorder_float_and_restore_without_duplicate_ownership() {
    let mut app = app();
    app.run("window.panel.move", json!({"panel":"swatches", "anchor":"layers", "zone":"top"})).unwrap();
    app.run("window.panel.move", json!({"panel":"properties", "anchor":"swatches", "before":"swatches"})).unwrap();
    let before_float = app.ui.docking.clone().unwrap();
    app.run("window.panel.float", json!({"panel":"properties", "x":50,"y":70,"width":330,"height":410})).unwrap();
    assert!(app.ui.docking.as_ref().unwrap().floating.iter().any(|g| g.panels == ["properties"]));
    app.run("window.panel.dock", json!({"panel":"properties"})).unwrap();
    assert_eq!(app.ui.docking.as_ref().unwrap(), &before_float);
    app.run("window.panel.close", json!({"panel":"properties"})).unwrap();
    assert!(!app.ui.docking.as_ref().unwrap().contains(&"properties".into()));
    app.run("window.panel.activate", json!({"panel":"properties"})).unwrap();
    assert_eq!(app.ui.docking.as_ref().unwrap(), &before_float);
    let layout = app.ui.docking.as_ref().unwrap();
    assert!(valid(layout));
    let unique: std::collections::HashSet<_> = layout.panels().into_iter().collect();
    assert_eq!(unique.len(), layout.panels().len());
    let saved = serde_json::to_value(&app.ui).unwrap();
    let restored: crate::state::UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.docking, app.ui.docking);
    assert_eq!(restored.docking_hidden, app.ui.docking_hidden);
}

#[test]
fn invalid_panel_requests_are_atomic_and_do_not_enter_a_custom_workspace() {
    let mut app = app();
    for (id, params) in [
        ("window.panel.move", json!({"panel":"layers","anchor":"unknown"})),
        ("window.panel.move", json!({"panel":"layers","anchor":"properties","before":"unknown"})),
        ("window.panel.move", json!({"panel":"layers","anchor":"properties","zone":42})),
        ("window.panel.float", json!({"panel":"layers","width":-10})),
        ("window.panel.float", json!({"panel":"unknown"})),
        ("window.panel.float", json!({"panel":"layers","x":"invalid"})),
    ] {
        let old = app.ui.docking.clone();
        let hidden = app.ui.docking_hidden.clone();
        assert!(app.run(id, params).is_err(), "{id}");
        assert_eq!(app.ui.docking, old);
        assert_eq!(app.ui.docking_hidden, hidden);
    }
}

fn harness(app: PhotocraftApp, size: egui::Vec2) -> Harness<'static, PhotocraftApp> {
    let mut ready = false;
    let mut harness = Harness::builder().with_size(size).with_step_dt(1.0 / 60.0).build_ui_state(
        move |ui, app: &mut PhotocraftApp| {
            if !ready {
                PhotocraftApp::setup_context(ui.ctx(), crate::theme::ThemeKind::ProMedium);
                ready = true;
                return;
            }
            crate::panels::right_dock(app, ui);
            if !app.ui.dock_regions.enabled {
                egui::CentralPanel::default().show(ui, |_| {});
            }
            crate::document_docking::show(app, ui.ctx());
        },
        app,
    );
    harness.run_steps(4);
    harness
}

fn drag(h: &mut Harness<'static, PhotocraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.run_steps(1);
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for step in 1..=8 {
        h.event(Event::PointerMoved(from + (to - from) * (step as f32 / 8.0)));
        h.run_steps(1);
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

#[test]
fn ordinary_panel_tab_drag_enters_shared_docking_and_can_redock_into_another_tab_group() {
    let mut h = harness(app(), vec2(1280.0, 800.0));
    let source = h.query_all_by_label("Layers").find(|node| node.rect().height() <= 45.0).unwrap().rect().center();
    drag(&mut h, source, egui::pos2(160.0, 140.0));
    let layout = h.state().ui.docking.as_ref().expect("ordinary tab drag starts shared docking");
    assert!(layout.floating.iter().any(|g| g.panels.contains(&"layers".into())), "{layout:?}");
    let source = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("tab", &"layers".to_string()))).unwrap().rect.center();
    let destination = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("tab", &"properties".to_string()))).unwrap().rect.center();
    drag(&mut h, source, destination);
    let layout = h.state().ui.docking.as_ref().unwrap();
    assert!(!layout.floating.iter().any(|g| g.panels.contains(&"layers".into())));
    assert!(valid(layout));
}

#[test]
fn saved_workspace_restores_the_custom_tree_and_hidden_panel_placement() {
    let mut app = app();
    let ctx = egui::Context::default();
    app.run("window.panel.move", json!({"panel":"swatches","anchor":"layers","zone":"top"})).unwrap();
    app.run("window.panel.float", json!({"panel":"properties","x":50,"y":80})).unwrap();
    app.run("window.panel.close", json!({"panel":"swatches"})).unwrap();
    let layout = app.ui.docking.clone();
    let hidden = app.ui.docking_hidden.clone();
    crate::menus::invoke(&mut app, &ctx, "window.workspace.newWorkspace", json!({"name":"Custom docking"})).unwrap();
    crate::menus::invoke(&mut app, &ctx, "window.workspace.essentials", json!({})).unwrap();
    assert!(app.ui.docking.is_none());
    crate::menus::invoke(&mut app, &ctx, "window.workspace.select", json!({"name":"Custom docking"})).unwrap();
    assert_eq!(app.ui.docking, layout);
    assert_eq!(app.ui.docking_hidden, hidden);
}

#[test]
fn shared_layout_keeps_panel_identity_across_themes_and_respects_workspace_lock() {
    let mut app = app();
    app.run("window.panel.move", json!({"panel":"color","anchor":"layers"})).unwrap();
    let layout = app.ui.docking.clone();
    app.ui.theme = crate::theme::ThemeKind::Studio;
    assert_eq!(app.ui.docking, layout);
    app.session.prefs.edit(|prefs| prefs.workspace_locked = true);
    assert!(app.run("window.panel.float", json!({"panel":"color"})).is_err());
    assert_eq!(app.ui.docking, layout);
    assert!(app.run("window.panel.activate", json!({"panel":"color"})).is_ok());
}

/// Explicit offscreen evidence; this creates no native windows.
#[test]
fn capture_custom_panel_docking_visual_fixtures() {
    let Some(directory) = std::env::var_os("CRAFT_UI_DOCKING_FIXTURES").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&directory).unwrap();
    for theme in crate::theme::ThemeKind::ALL {
        for width in [800.0, 1280.0] {
            for scale in [1.0, 1.5, 2.0] {
                let mut app = app();
                app.run("file.new", json!({})).unwrap();
                app.run("window.panel.move", json!({"panel":"swatches", "anchor":"layers", "zone":"top"})).unwrap();
                app.run("window.panel.float", json!({"panel":"properties","x":30,"y":90,"width":300,"height":440})).unwrap();
                let mut ready = false;
                let mut harness = Harness::builder().with_size(vec2(width, 800.0)).with_pixels_per_point(scale).wgpu().build_ui_state(
                    move |ui, app: &mut PhotocraftApp| {
                        if !ready {
                            PhotocraftApp::setup_context(ui.ctx(), theme);
                            app.ui.theme = theme;
                            ready = true;
                            return;
                        }
                        crate::panels::right_dock(app, ui);
                        egui::CentralPanel::default().show(ui, |ui| {
                            ui.label("Custom panel workspace");
                        });
                    },
                    app,
                );
                harness.input_mut().max_texture_side = Some(8192);
                harness.run_steps(5);
                assert!(valid(harness.state().ui.docking.as_ref().unwrap()));
                harness.render().unwrap().save(directory.join(format!("photocraft-shared-docking-{}-{width}-{scale}x.png", theme.id()))).unwrap();
            }
        }
    }
}

#[test]
fn escape_cancels_the_first_drag_without_replacing_the_legacy_workspace() {
    let mut h = harness(app(), vec2(1280.0, 800.0));
    let source = h.query_all_by_label("Layers").find(|node| node.rect().height() <= 45.0).unwrap().rect().center();
    h.event(Event::PointerMoved(source));
    h.run_steps(1);
    h.event(Event::PointerButton { pos: source, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.event(Event::PointerMoved(source - vec2(90.0, 0.0)));
    h.run_steps(2);
    assert!(h.state().ui.docking.is_some());
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    assert!(h.state().ui.docking.is_none());
    h.event(Event::PointerButton { pos: source, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert!(h.state().ui.docking.is_none());
}

#[test]
fn serialized_layout_actions_cover_geometry_reorder_accordion_and_atomic_rejection() {
    let mut app = app();
    app.run("window.panel.move", json!({"panel":"swatches","anchor":"layers","zone":"top"})).unwrap();
    app.run("window.panel.layout", json!({"action":{"ResizeSplit":{"path":[],"size":{"Ratio":0.35}}}})).unwrap();
    assert!(
        matches!(app.ui.docking.as_ref().unwrap().root, Some(Node::Split { size: craft_ui::layout::SplitSize::Ratio(value), .. }) if (value - 0.35).abs() < 0.001)
    );
    app.run("window.panel.float", json!({"panel":"properties"})).unwrap();
    app.run("window.panel.layout", json!({"action":{"MoveFloating":{"panel":"properties","rect":[44,66,360,450]}}})).unwrap();
    assert_eq!(
        app.ui.docking.as_ref().unwrap().floating.iter().find(|group| group.panels.iter().any(|p| p == "properties")).unwrap().rect,
        [44.0, 66.0, 360.0, 450.0]
    );
    app.run("window.panel.layout", json!({"action":{"Move":{"panel":"properties","anchor":"swatches","placement":{"Tab":{"before":"swatches"}}}}})).unwrap();
    assert_eq!(group_members_for_test(app.ui.docking.as_ref().unwrap(), "swatches"), ["properties", "swatches"]);
    let before = app.ui.docking.clone();
    let hidden = app.ui.docking_hidden.clone();
    for action in [
        json!({"MoveFloating":{"panel":"properties","rect":[0,0,-5,30]}}),
        json!({"ResizeSplit":{"path":[true,true,true],"size":{"Ratio":0.2}}}),
        json!({"Open":{"panel":"unknown","anchor":null}}),
        json!({"Move":{"panel":"properties","anchor":"swatches","placement":{"Tab":{"before":"unknown"}}}}),
    ] {
        assert!(app.run("window.panel.layout", json!({"action":action})).is_err());
        assert_eq!(app.ui.docking, before);
        assert_eq!(app.ui.docking_hidden, hidden);
    }
    app.ui.docking = Some(Layout {
        root: Some(Node::Stack { entries: vec![craft_ui::docking::StackEntry { panel: "layers".into(), open: true, height: Some(100.0) }] }),
        floating: Vec::new(),
    });
    app.run("window.panel.layout", json!({"action":{"SetStackOpen":{"panel":"layers","open":false}}})).unwrap();
    app.run("window.panel.layout", json!({"action":{"ResizeStack":{"panel":"layers","height":150}}})).unwrap();
    let Some(Node::Stack { entries }) = &app.ui.docking.as_ref().unwrap().root else { panic!("expected stack") };
    assert!(!entries[0].open);
    assert_eq!(entries[0].height, Some(150.0));
}

fn group_members_for_test(layout: &Layout<String>, panel: &str) -> Vec<String> {
    let mut pending: Vec<_> = layout.root.iter().collect();
    while let Some(node) = pending.pop() {
        match node {
            Node::Tabs { panels, .. } if panels.iter().any(|p| p == panel) => return panels.clone(),
            Node::Split { first, second, .. } => pending.extend([first.as_ref(), second.as_ref()]),
            _ => {}
        }
    }
    Vec::new()
}

#[test]
fn customization_retains_native_collapsed_groups_until_their_tab_is_activated() {
    let mut app = app();
    app.ui.dock.set_collapsed(Group::Color, true);
    app.run("window.panel.float", json!({"panel":"properties"})).unwrap();
    let layout = app.ui.docking.as_ref().unwrap();
    assert!(collapsed_members(&app, layout).contains("swatches"));
    let saved = serde_json::to_value(&app.ui).unwrap();
    let restored: crate::state::UiState = serde_json::from_value(saved).unwrap();
    assert!(restored.dock.is_collapsed(Group::Color));
    app.run("window.panel.layout", json!({"action":{"Activate":{"panel":"swatches"}}})).unwrap();
    assert!(!app.ui.dock.is_collapsed(Group::Color));
}

#[test]
fn splitting_a_short_column_keeps_the_layers_footer_visible_and_usable() {
    let mut app = app();
    app.run("file.new", json!({"width":480,"height":360})).unwrap();
    app.run("window.panel.move", json!({"panel":"swatches","anchor":"layers","zone":"top"})).unwrap();
    app.run("window.panel.float", json!({"panel":"properties","x":30,"y":90,"width":300,"height":440})).unwrap();
    let mut h = harness(app, vec2(800.0, 800.0));
    let button = h.get_by_label("Create a new layer").rect();
    assert!(button.bottom() <= 800.0, "Layers footer is clipped below the viewport: {button:?}");
    let before = h.state().session.active().unwrap().doc.layers.len();
    let at = button.center();
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    assert_eq!(h.state().session.active().unwrap().doc.layers.len(), before + 1);
}

#[test]
fn populated_canvas_keeps_a_real_opacity_edit_after_floating_and_redocking() {
    let mut app = app();
    app.ui.theme = crate::theme::ThemeKind::Studio;
    app.run("file.new", json!({"width":480,"height":360})).unwrap();
    app.run("layer.new.layer", json!({"name":"Docking sample"})).unwrap();
    app.run("select.rect", json!({"x":80,"y":70,"width":250,"height":160})).unwrap();
    app.run("edit.fill", json!({"color":"#5297bb"})).unwrap();
    app.run("select.deselect", json!({})).unwrap();
    let layer = app.session.active().unwrap().active_layer.unwrap();
    app.run("window.panel.float", json!({"panel":"properties","x":40,"y":140,"width":310,"height":490})).unwrap();
    let directory = std::env::var_os("CRAFT_UI_DOCKING_FIXTURES").map(std::path::PathBuf::from);
    let builder = Harness::builder().with_size(vec2(1280.0, 800.0));
    let builder = if directory.is_some() { builder.wgpu() } else { builder };
    let mut ready = false;
    let mut h = builder.build_ui_state(
        move |ui, app: &mut PhotocraftApp| {
            if !ready {
                PhotocraftApp::setup_context(ui.ctx(), crate::theme::ThemeKind::Studio);
                ready = true;
                return;
            }
            crate::panels::title_bar(app, ui);
            crate::panels::options_bar(app, ui);
            crate::panels::toolbar(app, ui);
            crate::panels::right_dock(app, ui);
            egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
        },
        app,
    );
    h.input_mut().max_texture_side = Some(8192);
    h.run_steps(5);
    let floating = h.ctx.memory(|m| m.area_rect(egui::Id::new("photocraft-panel-docking").with(("floating", &"properties".to_string())))).unwrap();
    let mut sliders: Vec<_> = h.ctx.viewport(|vp| {
        vp.prev_pass
            .widgets
            .layers()
            .flat_map(|(_, widgets)| widgets.iter())
            .filter(|widget| widget.sense.senses_drag() && widget.rect.height() == 18.0 && floating.contains_rect(widget.rect))
            .map(|widget| widget.rect)
            .collect()
    });
    sliders.sort_by(|a, b| a.top().total_cmp(&b.top()));
    let at = sliders.first().expect("Properties opacity slider").center();
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
    let opacity = h.state().session.active().unwrap().doc.layer(layer).unwrap().opacity;
    assert!((opacity - 0.5).abs() < 0.03, "actual opacity {opacity}");
    if let Some(directory) = &directory {
        std::fs::create_dir_all(directory).unwrap();
        h.render().unwrap().save(directory.join("photocraft-populated-opacity-floating.png")).unwrap();
    }
    let from = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("tab", &"properties".to_string()))).unwrap().rect.center();
    let to = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("tab", &"layers".to_string()))).unwrap().rect.center();
    drag(&mut h, from, to);
    assert!(h.state().ui.docking.as_ref().unwrap().floating.iter().all(|group| !group.panels.iter().any(|panel| panel == "properties")));
    assert_eq!(h.state().session.active().unwrap().doc.layer(layer).unwrap().opacity, opacity);
    if let Some(directory) = &directory {
        h.render().unwrap().save(directory.join("photocraft-populated-opacity-redocked.png")).unwrap();
    }
}

#[test]
fn changing_workspace_during_a_drag_cancels_the_stale_gesture() {
    let mut app = app();
    app.run("window.panel.move", json!({"panel":"swatches","anchor":"layers","zone":"top"})).unwrap();
    let destination = app.ui.docking.clone();
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, "window.workspace.newWorkspace", json!({"name":"Destination"})).unwrap();
    crate::menus::invoke(&mut app, &ctx, "window.workspace.essentials", json!({})).unwrap();
    let mut h = harness(app, vec2(1280.0, 800.0));
    let source = h.query_all_by_label("Layers").find(|node| node.rect().height() <= 45.0).unwrap().rect().center();
    h.event(Event::PointerMoved(source));
    h.run_steps(1);
    h.event(Event::PointerButton { pos: source, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.event(Event::PointerMoved(source - vec2(90.0, 0.0)));
    h.run_steps(2);
    let ctx = h.ctx.clone();
    crate::menus::invoke(h.state_mut(), &ctx, "window.workspace.select", json!({"name":"Destination"})).unwrap();
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    assert_eq!(h.state().ui.docking, destination);
    h.event(Event::PointerButton { pos: source, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().ui.docking, destination);
}

#[test]
fn first_legacy_drag_workspace_switch_preserves_custom_and_legacy_destinations() {
    let layout_state = |app: &PhotocraftApp| json!({"panels":app.ui.panels,"dock":app.ui.dock,"tabs":app.ui.dock_tabs,"layout":app.ui.docking,"hidden":app.ui.docking_hidden,"collapsed":app.ui.docking_collapsed,"timeline":app.ui.timeline.open});
    for customized_destination in [true, false] {
        for cancellation in ["release", "escape", "focus-loss"] {
            let mut app = app();
            let ctx = egui::Context::default();
            if customized_destination {
                app.run("window.panel.move", json!({"panel":"swatches","anchor":"layers","zone":"top"})).unwrap();
                crate::menus::invoke(&mut app, &ctx, "window.workspace.newWorkspace", json!({"name":"Destination"})).unwrap();
                crate::menus::invoke(&mut app, &ctx, "window.workspace.essentials", json!({})).unwrap();
            }
            let mut h = harness(app, vec2(1280.0, 800.0));
            let source = h.query_all_by_label("Layers").find(|node| node.rect().height() <= 45.0).unwrap().rect().center();
            h.event(Event::PointerMoved(source));
            h.run_steps(1);
            h.event(Event::PointerButton { pos: source, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
            h.run_steps(1);
            h.event(Event::PointerMoved(source - vec2(90.0, 0.0)));
            // One frame installs the first legacy drag; no shared dock has rendered yet.
            h.run_steps(1);
            assert!(h.state().ui.docking.is_some());
            let origin = egui::Id::new("photocraft-panel-docking").with("legacy-origin");
            assert!(h.ctx.data_mut(|data| data.get_temp::<LegacyDrag>(origin)).unwrap().layout.is_none());
            let ctx = h.ctx.clone();
            let command = if customized_destination { "window.workspace.select" } else { "window.workspace.painting" };
            crate::menus::invoke(h.state_mut(), &ctx, command, json!({"name":"Destination"})).unwrap();
            assert_eq!(h.state().ui.docking.is_some(), customized_destination);
            let destination = layout_state(h.state());
            let tabs = serde_json::to_value(h.state().ui.dock_tabs).unwrap();
            match cancellation {
                "escape" => h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }),
                "focus-loss" => h.event(Event::WindowFocused(false)),
                _ => {
                    h.event(Event::PointerButton { pos: source - vec2(90.0, 0.0), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE })
                }
            }
            h.run_steps(1);
            assert_eq!(layout_state(h.state()), destination, "{customized_destination}/{cancellation}");
            if cancellation == "focus-loss" {
                let at = source - vec2(90.0, 0.0);
                h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
                h.run_steps(1);
                assert_eq!(layout_state(h.state()), destination, "secondary press must not resume the stale primary drag");
                h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
                h.run_steps(1);
            }
            assert_eq!(serde_json::to_value(h.state().ui.dock_tabs).unwrap(), tabs);
            assert!(h.ctx.data_mut(|data| data.get_temp::<LegacyDrag>(origin)).is_none(), "stale origin: {customized_destination}/{cancellation}");
            h.event(Event::WindowFocused(true));
            h.event(Event::PointerButton { pos: source, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
            h.run_steps(2);
            assert_eq!(layout_state(h.state()), destination);
        }
    }
}

fn control_set(app: &mut PhotocraftApp, ctx: &egui::Context, params: Value) -> Value {
    let (request, _) = crate::control::ControlRequest::new("ui.set", params);
    match crate::control::handle(app, ctx, &request) {
        crate::control::Outcome::Done(value) => value,
        _ => panic!("ui.set must reply synchronously"),
    }
}

#[test]
fn current_main_hidden_tabs_migrate_by_name_and_selected_identity() {
    for theme in [crate::theme::ThemeKind::ProMedium, crate::theme::ThemeKind::Studio] {
        let mut app = app();
        app.ui.theme = theme;
        let pro = matches!(theme, crate::theme::ThemeKind::Pro | crate::theme::ThemeKind::ProMedium);
        let color = Group::Color.tabs(pro).iter().position(|name| *name == "Color").unwrap();
        let swatches = Group::Color.tabs(pro).iter().position(|name| *name == "Swatches").unwrap();
        app.ui.dock.hide_tab(Group::Color, swatches, pro);
        app.ui.dock.hide_tab(Group::Layers, 2, pro);
        app.ui.dock_tabs.color = color;
        let saved = json!({"panels":app.ui.panels,"dock":app.ui.dock,"dockTabs":app.ui.dock_tabs});
        assert_eq!(saved["dock"]["hidden_tabs"]["layers"], json!(["Paths"]));
        let mut restored = self::app();
        restored.ui.theme = theme;
        crate::dock::apply(&mut restored, &saved);
        restored.run("window.panel.float", json!({"panel":"properties"})).unwrap();
        let layout = restored.ui.docking.as_ref().unwrap();
        assert!(!layout.contains(&"swatches".into()) && !layout.contains(&"paths".into()));
        assert!(panel_active(layout, "color"));
        assert!(restored.ui.docking_hidden.contains_key("swatches") && restored.ui.docking_hidden.contains_key("paths"));
        restored.run("window.panel.swatches", json!({"show":true})).unwrap();
        assert!(panel_active(restored.ui.docking.as_ref().unwrap(), "swatches"));
    }
}

#[test]
fn legacy_control_updates_authoritative_custom_visibility_and_selection_atomically() {
    let mut app = app();
    app.ui.theme = crate::theme::ThemeKind::ProMedium;
    app.run("window.panel.float", json!({"panel":"properties"})).unwrap();
    let ctx = egui::Context::default();
    assert_eq!(control_set(&mut app, &ctx, json!({"dockTabs":{"layers":1}}))["ok"], true);
    assert!(panel_active(app.ui.docking.as_ref().unwrap(), "channels"));
    assert_eq!(control_set(&mut app, &ctx, json!({"panels":{"layers":false}}))["ok"], true);
    assert!(PANELS.iter().filter(|p| p.2 == Group::Layers).all(|p| !app.ui.docking.as_ref().unwrap().contains(&p.0.into())));
    assert!(!app.ui.panels.layers);
    assert_eq!(control_set(&mut app, &ctx, json!({"panels":{"layers":true},"dockTabs":{"layers":2}}))["ok"], true);
    assert!(panel_active(app.ui.docking.as_ref().unwrap(), "paths"));
    let before = serde_json::to_value(&app.ui).unwrap();
    for request in [
        json!({"panels":{"color":false},"dockTabs":{"layers":999}}),
        json!({"tool":"brush","dock":{"order":["layers","color"]}}),
        json!({"panels":{"layers":false},"dockTabs":{"layers":1}}),
    ] {
        assert_eq!(control_set(&mut app, &ctx, request)["ok"], false);
        assert_eq!(serde_json::to_value(&app.ui).unwrap(), before);
    }
}

#[test]
fn legacy_dock_width_is_consumed_by_custom_renderer_and_clamped_to_existing_range() {
    let mut app = app();
    app.run("window.panel.float", json!({"panel":"properties"})).unwrap();
    let mut h = harness(app, vec2(1280.0, 800.0));
    let ctx = h.ctx.clone();
    for (requested, expected) in [(410.0, 410.0), (900.0, 520.0), (20.0, 250.0)] {
        assert_eq!(control_set(h.state_mut(), &ctx, json!({"dockWidth":requested}))["ok"], true);
        h.run_steps(2);
        let state = egui::containers::panel::PanelState::load(&h.ctx, egui::Id::new("shared-panel-dock")).unwrap();
        assert!((state.outer_rect.width() - expected).abs() < 1.0, "{} / {expected}", state.outer_rect.width());
        assert!(h.ctx.data(|data| data.get_temp::<f32>(crate::panels::dock_width_id())).is_none());
    }
}

#[test]
fn explicit_reveal_activates_existing_tab_and_expands_custom_group() {
    let mut app = app();
    app.run("window.panel.move", json!({"panel":"properties","anchor":"layers"})).unwrap();
    app.run("window.panel.group", json!({"panel":"layers","operation":"collapse"})).unwrap();
    app.run("window.panel.layers", json!({"show":true})).unwrap();
    assert!(panel_active(app.ui.docking.as_ref().unwrap(), "layers"));
    assert!(!collapsed_members(&app, app.ui.docking.as_ref().unwrap()).contains("layers"));
    app.run("window.panel.layers", json!({"show":true})).unwrap();
    assert!(app.ui.docking.as_ref().unwrap().contains(&"layers".into()));
    let before = app.ui.docking.clone();
    assert!(app.run("window.panel.layers", json!({"show":"yes"})).is_err());
    assert_eq!(app.ui.docking, before);
}

#[test]
fn custom_group_commands_preserve_members_and_workspace_timeline() {
    let mut app = app();
    app.run("window.panel.move", json!({"panel":"properties","anchor":"layers"})).unwrap();
    let members = group_members(app.ui.docking.as_ref().unwrap(), "layers");
    app.run("window.panel.group", json!({"panel":"layers","operation":"up"})).unwrap();
    assert_eq!(group_members(app.ui.docking.as_ref().unwrap(), "layers"), members);
    app.ui.timeline.open = true;
    app.run("window.panel.group", json!({"panel":"layers","operation":"collapse"})).unwrap();
    let saved = json!({"panels":app.ui.panels,"dock":app.ui.dock,"dockTabs":app.ui.dock_tabs,"docking":app.ui.docking,"dockingHidden":app.ui.docking_hidden,"dockingCollapsed":app.ui.docking_collapsed,"timelineOpen":true});
    let mut restored = self::app();
    crate::dock::apply(&mut restored, &saved);
    assert_eq!(restored.ui.docking, app.ui.docking);
    assert!(restored.ui.timeline.open);
    assert!(collapsed_members(&restored, restored.ui.docking.as_ref().unwrap()).contains("layers"));
    app.run("window.panel.group", json!({"panel":"layers","operation":"close"})).unwrap();
    assert!(members.iter().all(|id| !app.ui.docking.as_ref().unwrap().contains(id)));
    assert!(members.iter().all(|id| app.ui.docking_hidden.contains_key(id)));
    app.session.prefs.edit(|p| p.workspace_locked = true);
    let before = serde_json::to_value(&app.ui).unwrap();
    assert!(app.run("window.panel.group", json!({"panel":"color","operation":"close"})).is_err());
    assert_eq!(serde_json::to_value(&app.ui).unwrap(), before);
    restored.ui.timeline.playing = true;
    crate::dock::apply(&mut restored, &json!({"dock":{}}));
    assert!(!restored.ui.timeline.open && !restored.ui.timeline.playing);
}

#[test]
fn customized_panel_menus_and_rail_remain_reachable_after_float_and_restore() {
    let mut app = app();
    app.run("window.panel.float", json!({"panel":"swatches","x":20,"y":40})).unwrap();
    let mut h = harness(app, vec2(1280.0, 800.0));
    assert!(h.query_all_by_label("Panel menu").count() > 0);
    assert!(h.query_all_by_label("Color & Swatches").count() > 0);
    let floating = h.ctx.memory(|memory| memory.area_rect(egui::Id::new("photocraft-panel-docking").with(("floating", &"swatches".to_string())))).unwrap();
    let first = h.query_all_by_label("Panel menu").find(|node| floating.contains_rect(node.rect())).unwrap().rect().center();
    h.hover_at(first);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: first, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
    assert!(h.query_all_by_label("Close Tab Group").count() > 0);
    assert!(h.query_all_by_label("New Swatch…").count() > 0);
    // Panel-specific menu body is shared by both its context and group menu seams.
    let mut ready = false;
    let mut menu = Harness::builder().build_ui_state(
        move |ui, app: &mut PhotocraftApp| {
            if !ready {
                PhotocraftApp::setup_context(ui.ctx(), crate::theme::ThemeKind::ProMedium);
                ready = true;
            }
            panel_menu(app, ui, "swatches");
        },
        self::app(),
    );
    menu.run_steps(2);
    assert!(menu.query_all_by_label("New Swatch…").count() > 0);
}

#[test]
fn custom_workspace_saves_shared_layout_and_timeline_without_embedding_flags() {
    let mut app = app();
    app.run("window.panel.float", json!({"panel":"properties"})).unwrap();
    app.ui.timeline.open = true;
    app.ui.panels.rail = false;
    app.ui.panels.menu_bar = false;
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, "window.workspace.newWorkspace", json!({"name":"Docked Motion"})).unwrap();
    let saved = app.session.prefs().workspaces.get("Docked Motion").unwrap().clone();
    assert!(saved["docking"].is_object() && saved["timelineOpen"] == true);
    assert!(saved["panels"].get("rail").is_none() && saved["panels"].get("menu_bar").is_none());
    let layout = app.ui.docking.clone();
    app.ui.workspace = "Motion".into();
    crate::menus::apply_workspace(&mut app);
    assert!(app.ui.docking.is_none() && app.ui.timeline.open);
    crate::dock::apply(&mut app, &saved);
    assert_eq!(app.ui.docking, layout);
    assert!(app.ui.timeline.open);
}

#[test]
fn closing_and_reopening_a_whole_group_retains_its_return_position_and_tab_order() {
    let mut app = app();
    app.run("window.panel.float", json!({"panel":"properties"})).unwrap();
    let before = app.ui.docking.clone().unwrap();
    let members = group_members(&before, "layers");
    app.run("window.panel.group", json!({"panel":"layers","operation":"close"})).unwrap();
    for panel in &members {
        app.run("window.panel.activate", json!({"panel":panel})).unwrap();
    }
    let after = app.ui.docking.as_ref().unwrap();
    assert_eq!(group_members(after, "layers"), members);
    assert_eq!(root_groups(after), root_groups(&before));
    assert!(valid(after));
}

#[test]
fn floating_group_grip_redocks_all_tabs_as_one_atomic_move() {
    let mut app = app();
    app.run("window.panel.float", json!({"panel":"swatches","x":30,"y":60})).unwrap();
    app.run("window.panel.move", json!({"panel":"patterns","anchor":"swatches"})).unwrap();
    let mut h = harness(app, vec2(1280.0, 800.0));
    let from = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("move-group", Some(&"swatches".to_string())))).unwrap().rect.center();
    let to = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("tab", &"layers".to_string()))).unwrap().rect.center();
    drag(&mut h, from, to);
    let layout = h.state().ui.docking.as_ref().unwrap();
    assert!(layout.floating.iter().all(|group| !group.panels.iter().any(|id| id == "swatches" || id == "patterns")));
    let members = group_members(layout, "layers");
    let swatches = members.iter().position(|id| id == "swatches").unwrap();
    assert_eq!(members.get(swatches + 1).map(String::as_str), Some("patterns"));
    assert!(valid(layout));
    let before = h.state().ui.docking.clone();
    assert!(h.state_mut().run("window.panel.group", json!({"panel":"swatches","operation":"move","anchor":"patterns"})).is_err());
    assert_eq!(h.state().ui.docking, before);
}

#[test]
fn mixed_theme_and_legacy_selection_projects_the_selected_panel_in_the_new_theme() {
    let mut app = app();
    app.ui.theme = crate::theme::ThemeKind::ProMedium;
    app.run("window.panel.float", json!({"panel":"properties"})).unwrap();
    let ctx = egui::Context::default();
    assert_eq!(control_set(&mut app, &ctx, json!({"theme":"studio","dockTabs":{"color":0}}))["ok"], true);
    assert!(panel_active(app.ui.docking.as_ref().unwrap(), "swatches"));
    assert_eq!(app.ui.dock_tabs.color, 0);
    assert_eq!(control_set(&mut app, &ctx, json!({"theme":"proMedium","dockTabs":{"color":0}}))["ok"], true);
    assert!(panel_active(app.ui.docking.as_ref().unwrap(), "color"));
    assert_eq!(app.ui.dock_tabs.color, 0);
}

#[test]
fn native_tab_reorder_stays_native_until_drag_crosses_the_strip() {
    let mut h = harness(app(), vec2(1280.0, 800.0));
    let source = h.query_all_by_label("Layers").find(|node| node.rect().height() <= 45.0).unwrap().rect().center();
    let target = h.query_all_by_label("Channels").find(|node| node.rect().height() <= 45.0).unwrap().rect().right_center() - vec2(1.0, 0.0);
    h.event(Event::PointerMoved(source));
    h.run_steps(1);
    h.event(Event::PointerButton { pos: source, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    let near_edge = egui::pos2(target.x, source.y - 21.0);
    h.event(Event::PointerMoved(near_edge));
    h.run_steps(1);
    assert!(h.state().ui.docking.is_none(), "native near-edge reorder tolerance must not promote to custom docking");
    h.event(Event::PointerMoved(target));
    h.run_steps(1);
    assert!(h.state().ui.docking.is_none(), "native horizontal reorder must not promote to custom docking");
    h.event(Event::PointerButton { pos: target, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert!(h.state().ui.docking.is_none());
    let tabs = h.state().ui.dock.visible_tabs(Group::Layers, false);
    let layers = tabs.iter().position(|(_, label)| *label == "Layers").unwrap();
    let channels = tabs.iter().position(|(_, label)| *label == "Channels").unwrap();
    assert!(channels < layers, "upstream native reorder remains effective");
}

fn tab_rect(h: &Harness<'static, PhotocraftApp>, id: &str) -> egui::Rect {
    h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("tab", &id.to_owned()))).unwrap().rect
}

fn canvas_region(h: &Harness<'static, PhotocraftApp>) -> egui::Rect {
    h.ctx.data(|data| data.get_temp(egui::Id::new("photocraft-workspace-canvas"))).unwrap()
}

#[test]
fn four_regions_surround_a_protected_canvas_and_release_empty_sides() {
    let mut app = app();
    for (panel, side) in [("color", "left"), ("properties", "top"), ("history", "bottom"), ("layers", "right")] {
        app.run("window.panel.dock", json!({"panel":panel,"side":side})).unwrap();
    }
    let rendered = app.ui.dock_regions.render(app.ui.docking.as_ref().unwrap());
    rendered.layout.validate().unwrap();
    assert_eq!(rendered.layout.panels().iter().filter(|id| id.as_str() == CANVAS).count(), 1);
    let mut h = harness(app, vec2(1440.0, 1000.0));
    let canvas = canvas_region(&h);
    assert!(tab_rect(&h, "color").right() < canvas.left());
    assert!(tab_rect(&h, "layers").left() > canvas.right());
    assert!(tab_rect(&h, "properties").bottom() < canvas.top());
    assert!(tab_rect(&h, "history").top() > canvas.bottom());
    h.state_mut().run("window.panel.float", json!({"panel":"color"})).unwrap();
    h.run_steps(3);
    assert!(canvas_region(&h).left() < canvas.left());
    assert!(!h.state().ui.docking.as_ref().unwrap().contains(&CANVAS.to_owned()));
}

#[test]
fn group_float_and_side_dock_preserve_members_order_selection_and_panel_return_side() {
    let mut app = app();
    app.run("window.panel.activate", json!({"panel":"channels"})).unwrap();
    app.run("window.panel.group", json!({"panel":"channels","operation":"dock","side":"left"})).unwrap();
    let members = group_members(app.ui.docking.as_ref().unwrap(), "channels");
    assert_eq!(members, ["layers", "channels", "paths"]);
    app.run("window.panel.group", json!({"panel":"channels","operation":"float","x":40,"y":60})).unwrap();
    let floating = app.ui.docking.as_ref().unwrap().floating.last().unwrap();
    assert_eq!(floating.panels, members);
    assert_eq!(floating.panels[floating.active], "channels");
    app.run("window.panel.group", json!({"panel":"channels","operation":"dock","side":"bottom"})).unwrap();
    assert_eq!(group_members(app.ui.docking.as_ref().unwrap(), "channels"), members);
    for id in &members {
        assert_eq!(app.ui.dock_regions.side(id), DockSide::Bottom);
    }
    app.run("window.panel.float", json!({"panel":"channels"})).unwrap();
    app.run("window.panel.dock", json!({"panel":"channels"})).unwrap();
    assert_eq!(app.ui.dock_regions.side("channels"), DockSide::Bottom);
    assert!(panel_active(app.ui.docking.as_ref().unwrap(), "channels"));
}

#[test]
fn cross_region_tab_and_group_moves_follow_destination_side_and_split_paths_resize_real_tree() {
    let mut app = app();
    app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
    app.run("window.panel.move", json!({"panel":"properties","anchor":"color","zone":"bottom"})).unwrap();
    app.run("window.panel.group", json!({"panel":"layers","operation":"move","anchor":"color"})).unwrap();
    for id in ["properties", "layers", "channels", "paths"] {
        assert_eq!(app.ui.dock_regions.side(id), DockSide::Left);
    }
    let rendered = app.ui.dock_regions.render(app.ui.docking.as_ref().unwrap());
    for target in rendered.splits.values() {
        if let SplitTarget::Tree(path) = target {
            app.run("window.panel.layout", json!({"action":Action::<String>::ResizeSplit {path:path.clone(),size:SplitSize::Ratio(0.4)}})).unwrap();
        }
    }
    assert!(valid(app.ui.docking.as_ref().unwrap()));
    assert!(app.ui.dock_regions.render(app.ui.docking.as_ref().unwrap()).layout.validate().is_ok());
    app.run("window.panel.close", json!({"panel":"properties"})).unwrap();
    app.run("window.panel.layout", json!({"action":Action::<String>::Open {panel:"properties".into(),anchor:Some("swatches".into())}})).unwrap();
    assert_eq!(app.ui.dock_regions.side("properties"), DockSide::Right);
}

#[test]
fn regions_persist_migrate_reset_and_restore_hidden_panels() {
    let mut app = app();
    app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
    app.run("window.panel.close", json!({"panel":"color"})).unwrap();
    app.run("window.panel.layout", json!({"side":"left","size":350})).unwrap();
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, "window.workspace.newWorkspace", json!({"name":"Four sides"})).unwrap();
    let saved = app.session.prefs().workspaces.get("Four sides").unwrap().clone();
    let expected = app.ui.dock_regions.clone();
    crate::dock::apply(&mut app, &saved);
    assert_eq!(app.ui.dock_regions, expected);
    app.run("window.panel.activate", json!({"panel":"color"})).unwrap();
    assert_eq!(app.ui.dock_regions.side("color"), DockSide::Left);
    assert!(app.ui.docking.as_ref().unwrap().contains(&"color".into()));
    app.session.prefs.edit(|p| p.workspace.remember_workspace_changes = true);
    crate::dock::persist(&mut app, &ctx);
    assert_eq!(app.session.prefs().panel_layout["dockRegions"], serde_json::to_value(&app.ui.dock_regions).unwrap());
    let old = json!({"docking":app.ui.docking});
    crate::dock::apply(&mut app, &old);
    assert!(!app.ui.dock_regions.enabled);
    assert!(app.ui.docking.is_some());
    let corrupt = json!({"docking":app.ui.docking,"dockRegions":{"enabled":true,"sizes":[-1,1,1,1]}});
    crate::dock::apply(&mut app, &corrupt);
    assert!(!app.ui.dock_regions.enabled);
    crate::menus::invoke(&mut app, &ctx, "window.workspace.essentials", json!({})).unwrap();
    assert!(!app.ui.dock_regions.enabled);
    assert!(app.ui.docking.is_none());
}

#[test]
fn invalid_region_and_group_requests_and_locked_workspace_are_atomic() {
    let mut app = app();
    app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
    let before = serde_json::to_value(&app.ui).unwrap();
    for (id, params) in [
        ("window.panel.dock", json!({"panel":"color","side":"center"})),
        ("window.panel.dock", json!({"panel":"color","side":4})),
        ("window.panel.dock", json!({"panel":"unknown","side":"left"})),
        ("window.panel.move", json!({"panel":"color","side":"left","anchor":"layers"})),
        ("window.panel.group", json!({"panel":"color","operation":"close","side":"left"})),
        ("window.panel.group", json!({"panel":"color","operation":"float","width":-1})),
        ("window.panel.layout", json!({"side":"left","size":"bad"})),
        ("window.panel.layout", json!({"side":"left","size":1e100})),
        ("window.panel.layout", json!({"side":"left","size":0})),
    ] {
        assert!(app.run(id, params).is_err());
        assert_eq!(serde_json::to_value(&app.ui).unwrap(), before);
    }
    app.session.prefs.edit(|p| p.workspace_locked = true);
    for (id, params) in [
        ("window.panel.dock", json!({"panel":"color","side":"right"})),
        ("window.panel.group", json!({"panel":"color","operation":"float"})),
        ("window.panel.layout", json!({"side":"left","size":350})),
    ] {
        assert!(app.run(id, params).is_err());
        assert_eq!(serde_json::to_value(&app.ui).unwrap(), before);
    }
}

#[test]
fn pointer_tab_and_docked_group_drags_can_create_regions_and_float_whole_groups() {
    let mut app = app();
    app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
    let mut h = harness(app, vec2(1440.0, 1000.0));
    let from = tab_rect(&h, "layers").center();
    drag(&mut h, from, egui::pos2(500.0, 12.0));
    assert_eq!(h.state().ui.dock_regions.side("layers"), DockSide::Top);
    assert_ne!(h.state().ui.dock_regions.side("channels"), DockSide::Top);
    h.state_mut().run("window.panel.move", json!({"panel":"channels","anchor":"layers"})).unwrap();
    h.run_steps(3);
    let grip = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("dock-group-grip", &"channels".to_owned()))).unwrap().rect.center();
    let to = canvas_region(&h).center();
    drag(&mut h, grip, to);
    let layout = h.state().ui.docking.as_ref().unwrap();
    assert!(layout.floating.iter().any(|g| g.panels.contains(&"layers".into()) && g.panels.contains(&"channels".into())));
}

#[test]
fn region_drag_escape_and_focus_loss_preserve_the_workspace() {
    for (escape, whole_group) in [(true, false), (false, false), (true, true), (false, true)] {
        let mut app = app();
        app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
        let mut h = harness(app, vec2(1280.0, 800.0));
        let before = h.state().ui.docking.clone();
        let regions = h.state().ui.dock_regions.clone();
        let from = if whole_group {
            h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("dock-group-grip", &"layers".to_owned()))).unwrap().rect.center()
        } else {
            tab_rect(&h, "layers").center()
        };
        h.event(Event::PointerMoved(from));
        h.run_steps(1);
        h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.run_steps(1);
        let to = egui::pos2(300.0, 12.0);
        h.event(Event::PointerMoved(to));
        h.run_steps(2);
        if escape {
            h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        } else {
            h.input_mut().focused = false;
        }
        h.run_steps(1);
        h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(2);
        assert_eq!(h.state().ui.docking, before);
        assert_eq!(h.state().ui.dock_regions, regions);
    }
}

#[test]
fn regional_layout_keeps_canvas_and_floating_headers_reachable_at_small_sizes() {
    let mut app = app();
    for (panel, side) in [("color", "left"), ("properties", "top"), ("history", "bottom")] {
        app.run("window.panel.dock", json!({"panel":panel,"side":side})).unwrap();
    }
    app.run("window.panel.float", json!({"panel":"swatches","x":900,"y":700})).unwrap();
    let h = harness(app, vec2(640.0, 480.0));
    let canvas = canvas_region(&h);
    assert!(canvas.is_finite() && canvas.width() > 0.0 && canvas.height() > 0.0);
    let floating = h.ctx.memory(|memory| memory.area_rect(egui::Id::new("photocraft-panel-docking").with(("floating", &"swatches".to_owned())))).unwrap();
    assert!(floating.left() >= 0.0 && floating.top() >= 0.0 && floating.right() <= 640.0 && floating.top() < 480.0);
}

#[test]
fn capture_regional_docking_visual_fixtures() {
    let Some(directory) = std::env::var_os("PHOTOCRAFT_REGIONAL_DOCK_FIXTURES").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&directory).unwrap();
    for theme in crate::theme::ThemeKind::ALL {
        for (width, height, scale) in [(800.0, 700.0, 1.0), (1440.0, 1000.0, 1.5)] {
            let mut app = app();
            app.run("file.new", json!({"width":600,"height":400})).unwrap();
            for (panel, side) in [("color", "left"), ("properties", "top"), ("history", "bottom")] {
                app.run("window.panel.dock", json!({"panel":panel,"side":side})).unwrap();
            }
            app.run("window.panel.float", json!({"panel":"swatches","x":80,"y":360,"width":270,"height":240})).unwrap();
            let mut ready = false;
            let mut h = Harness::builder().with_size(vec2(width, height)).with_pixels_per_point(scale).wgpu().build_ui_state(
                move |ui, app: &mut PhotocraftApp| {
                    if !ready {
                        PhotocraftApp::setup_context(ui.ctx(), theme);
                        app.ui.theme = theme;
                        ready = true;
                        return;
                    }
                    crate::panels::title_bar(app, ui);
                    crate::panels::options_bar(app, ui);
                    crate::panels::status_bar(app, ui);
                    crate::panels::toolbar(app, ui);
                    crate::panels::right_dock(app, ui);
                },
                app,
            );
            h.input_mut().max_texture_side = Some(8192);
            h.run_steps(5);
            assert!(canvas_region(&h).is_finite());
            h.render().unwrap().save(directory.join(format!("regional-{}-{width}-{scale}x.png", theme.id()))).unwrap();
            let tab = tab_rect(&h, "layers");
            hold_drag(&mut h, tab.center(), egui::pos2(tab.left() + 3.0, tab.center().y));
            assert_insertion_only(&h, "layers");
            h.render().unwrap().save(directory.join(format!("insertion-{}-{width}-{scale}x.png", theme.id()))).unwrap();
        }
    }
}

#[test]
fn region_dividers_resize_through_commands_and_group_up_down_stay_on_their_side() {
    let mut app = app();
    app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
    let mut h = harness(app, vec2(1280.0, 800.0));
    let rendered = h.state().ui.dock_regions.render(h.state().ui.docking.as_ref().unwrap());
    let left_path = rendered.splits.iter().find(|(_, target)| matches!(target, SplitTarget::Region(DockHost::Workspace, DockSide::Left))).unwrap().0;
    let divider = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("split", left_path))).unwrap().rect.center();
    let width = canvas_region(&h).left();
    drag(&mut h, divider, divider + vec2(40.0, 0.0));
    assert!(canvas_region(&h).left() > width + 20.0);
    assert!(h.state().ui.dock_regions.sizes[0] > 300.0);
    let before = h.state().ui.docking.clone();
    assert!(h.state_mut().run("window.panel.group", json!({"panel":"color","operation":"up"})).is_err());
    assert_eq!(h.state().ui.docking, before);
}

#[test]
fn legacy_group_header_can_float_all_tabs_without_first_customizing() {
    let mut h = harness(app(), vec2(1280.0, 800.0));
    let layers = h.query_all_by_label("Layers").find(|node| node.rect().height() <= 45.0).unwrap().rect();
    // Empty strip space between the last tab and menu belongs to the native group header.
    let from = egui::pos2(1230.0, layers.center().y);
    drag(&mut h, from, egui::pos2(500.0, 300.0));
    let layout = h.state().ui.docking.as_ref().expect("header drag promotes the legacy dock");
    assert!(layout.floating.iter().any(|g| g.panels == ["layers", "channels", "paths"]));
}

#[test]
fn floating_group_grip_can_dock_at_a_workspace_edge_and_canvas_edge() {
    for workspace_edge in [true, false] {
        let mut app = app();
        app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
        app.run("window.panel.activate", json!({"panel":"channels"})).unwrap();
        app.run("window.panel.group", json!({"panel":"layers","operation":"float","x":400,"y":300})).unwrap();
        let mut h = harness(app, vec2(1440.0, 1000.0));
        let from = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("move-group", Some(&"layers".to_owned())))).unwrap().rect.center();
        let to = if workspace_edge {
            egui::pos2(500.0, 12.0)
        } else {
            let canvas = canvas_region(&h);
            egui::pos2(canvas.center().x, canvas.bottom() - 8.0)
        };
        drag(&mut h, from, to);
        let expected = if workspace_edge { DockSide::Top } else { DockSide::Bottom };
        for id in ["layers", "channels", "paths"] {
            assert_eq!(h.state().ui.dock_regions.side(id), expected);
        }
        assert!(h.state().ui.docking.as_ref().unwrap().floating.is_empty());
        assert!(panel_active(h.state().ui.docking.as_ref().unwrap(), "channels"));
    }
}

fn hold_drag(h: &mut Harness<'static, PhotocraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.run_steps(1);
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for step in 1..=8 {
        h.event(Event::PointerMoved(from.lerp(to, step as f32 / 8.0)));
        h.run_steps(1);
    }
    h.run_steps(2);
}
fn rendered_group(h: &Harness<'static, PhotocraftApp>, panel: &str) -> egui::Rect {
    h.ctx.data(|data| data.get_temp(egui::Id::new("photocraft-panel-docking").with(("group-rect", panel.to_owned())))).unwrap()
}
fn drop_preview(h: &Harness<'static, PhotocraftApp>) -> Option<craft_ui::docking::DropFeedback> {
    h.ctx.data(|data| data.get_temp(egui::Id::new("photocraft-panel-docking").with("resolved-preview")))
}
fn release_and_compare_preview(h: &mut Harness<'static, PhotocraftApp>, to: Pos2, panel: &str) {
    let preview = drop_preview(h).and_then(|feedback| feedback.preview).expect("resolved destination preview");
    let capture = std::env::var_os("PHOTOCRAFT_DOCK_PREVIEW_FIXTURES").map(std::path::PathBuf::from).map(|directory| {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        std::fs::create_dir_all(&directory).unwrap();
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        h.render().unwrap().save(directory.join(format!("{id:02}-preview.png"))).unwrap();
        (directory, id)
    });
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
    let actual = rendered_group(h, panel);
    if let Some((directory, id)) = capture {
        h.render().unwrap().save(directory.join(format!("{id:02}-settled.png"))).unwrap();
    }
    assert!((actual.min - preview.min).length() <= 1.5 && (actual.max - preview.max).length() <= 1.5, "{panel}: preview={preview:?}, actual={actual:?}");
}

#[test]
fn pointer_edge_previews_match_final_group_bounds_for_all_sides_and_sizes() {
    for size in [vec2(1440.0, 1000.0), vec2(700.0, 500.0)] {
        for side in DockSide::ALL {
            for whole in [false, true] {
                let mut app = app();
                app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
                app.run("window.panel.layout", json!({"side":side,"size":340.0})).unwrap();
                let mut h = harness(app, size);
                let from = if whole {
                    h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("dock-group-grip", &"layers".to_owned()))).unwrap().rect.center()
                } else {
                    tab_rect(&h, "layers").center()
                };
                let workspace: egui::Rect = h.ctx.data(|data| data.get_temp(egui::Id::new("photocraft-panel-docking").with("workspace-rect"))).unwrap();
                let to = match side {
                    DockSide::Left => egui::pos2(workspace.left() + 10.0, workspace.bottom() - 30.0),
                    DockSide::Right => egui::pos2(workspace.right() - 10.0, workspace.bottom() - 30.0),
                    DockSide::Top => egui::pos2(workspace.center().x, workspace.top() + 10.0),
                    DockSide::Bottom => egui::pos2(workspace.center().x, workspace.bottom() - 10.0),
                };
                hold_drag(&mut h, from, to);
                assert!(
                    drop_preview(&h).and_then(|feedback| feedback.preview).is_some(),
                    "missing preview: {size:?}/{side:?}/whole={whole}, from={from:?}, to={to:?}"
                );
                release_and_compare_preview(&mut h, to, "layers");
                assert_eq!(h.state().ui.dock_regions.side("layers"), side);
            }
        }
    }
}

#[test]
fn split_canvas_join_float_and_source_cleanup_previews_match_final_bounds() {
    for scenario in ["split", "canvas", "join", "float", "cleanup", "floating-group"] {
        let mut app = app();
        app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
        if scenario == "cleanup" {
            app.run("window.panel.dock", json!({"panel":"layers","side":"top"})).unwrap();
        }
        if scenario == "floating-group" {
            app.run("window.panel.group", json!({"panel":"layers","operation":"float","x":450,"y":320})).unwrap();
        }
        let mut h = harness(app, vec2(1440.0, 1000.0));
        let from = if scenario == "floating-group" {
            h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("move-group", Some(&"layers".to_owned())))).unwrap().rect.center()
        } else {
            tab_rect(&h, "layers").center()
        };
        let color = rendered_group(&h, "color");
        let canvas = canvas_region(&h);
        let to = match scenario {
            "split" => egui::pos2(color.center().x, color.bottom() - 30.0),
            "join" => tab_rect(&h, "color").center(),
            "canvas" => egui::pos2(canvas.center().x, canvas.bottom() - 30.0),
            "float" => canvas.center(),
            "cleanup" => egui::pos2(canvas.right() - 30.0, canvas.center().y),
            _ => egui::pos2(500.0, 10.0),
        };
        hold_drag(&mut h, from, to);
        release_and_compare_preview(&mut h, to, "layers");
    }
}

#[test]
fn unchanged_tab_insertions_show_only_a_bounded_line_and_leave_order_unchanged() {
    for panel in ["layers", "channels", "paths"] {
        for after_midpoint in [false, true] {
            let mut app = app();
            app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
            let mut h = harness(app, vec2(1440.0, 1000.0));
            let rect = tab_rect(&h, panel);
            let from = rect.center();
            let to = egui::pos2(if after_midpoint { rect.right() - 3.0 } else { rect.left() + 3.0 }, rect.center().y);
            let before = h.state().ui.docking.clone();
            hold_drag(&mut h, from, to);
            assert_insertion_only(&h, panel);
            capture_insertion(&mut h, &format!("unchanged-{panel}-{after_midpoint}"));
            h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
            h.run_steps(3);
            // Clicking may select a different tab; its membership and order must stay fixed.
            assert_eq!(group_members(h.state().ui.docking.as_ref().unwrap(), panel), group_members(before.as_ref().unwrap(), panel));
            assert!(h.state().ui.docking.as_ref().unwrap().floating.is_empty());
        }
    }
}

#[test]
fn first_regional_drop_from_legacy_custom_dock_accounts_for_the_icon_rail() {
    for theme in [crate::theme::ThemeKind::ProMedium, crate::theme::ThemeKind::Studio] {
        let mut app = app();
        app.ui.theme = theme;
        app.run("window.panel.float", json!({"panel":"properties","x":400,"y":300})).unwrap();
        let mut h = harness(app, vec2(1440.0, 1000.0));
        PhotocraftApp::setup_context(&h.ctx, theme);
        h.run_steps(3);
        let from = tab_rect(&h, "layers").center();
        let to = egui::pos2(500.0, 12.0);
        hold_drag(&mut h, from, to);
        release_and_compare_preview(&mut h, to, "layers");
        assert_eq!(h.state().ui.dock_regions.side("layers"), DockSide::Top);
    }
}

fn assert_insertion_only(h: &Harness<'static, PhotocraftApp>, panel: &str) {
    assert_drag_opacity(h, 0.7);
    let feedback = drop_preview(h).expect("tab-strip feedback");
    assert!(feedback.preview.is_none(), "same-group reorder must not highlight the group");
    let marker = feedback.insertion.expect("insertion marker even at unchanged slots");
    assert!(rendered_group(h, panel).contains_rect(marker), "marker {marker:?} outside group");
    let tab = tab_rect(h, panel);
    assert!(marker.top() >= tab.top() - 0.1 && marker.bottom() <= tab.bottom() + 0.1);
    assert!(marker.width() > 0.0 && marker.width() <= 2.01);
    let accent = crate::theme::Tokens::get(&h.ctx).accent;
    assert!(h.output().shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(r) if r.rect == marker && r.fill == accent)));
}
fn assert_drag_opacity(h: &Harness<'static, PhotocraftApp>, opacity: f32) {
    fn large_fill(shape: &egui::Shape, fill: egui::Color32) -> bool {
        match shape {
            egui::Shape::Rect(r) => r.fill == fill && r.rect.width() >= 200.0 && r.rect.height() >= 200.0,
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| large_fill(shape, fill)),
            _ => false,
        }
    }
    let card = crate::theme::Tokens::get(&h.ctx).card;
    assert!(h.output().shapes.iter().any(|shape| large_fill(&shape.shape, card.gamma_multiply(opacity))), "moving UI opacity {opacity}");
    if opacity == 1.0 {
        assert!(!h.output().shapes.iter().any(|shape| large_fill(&shape.shape, card.gamma_multiply(0.7))));
    }
}

#[test]
fn drag_opacity_tracks_same_and_cross_group_strips_and_restores_outside() {
    for floating in [false, true] {
        let mut app = app();
        enable_workspace(&mut app);
        if floating {
            app.run("window.panel.group", json!({"panel":"properties","operation":"float","x":150,"y":130,"width":330,"height":450})).unwrap();
        }
        let mut h = harness(app, vec2(1440.0, 1000.0));
        let before = h.state().ui.docking.clone();
        let from = tab_rect(&h, "layers").center();
        let own = egui::pos2(tab_rect(&h, "layers").left() + 3.0, from.y);
        hold_drag(&mut h, from, own);
        assert_insertion_only(&h, "layers");
        capture_insertion(&mut h, &format!("opacity-own-{floating}"));
        let outside = rendered_group(&h, CANVAS).center();
        h.event(Event::PointerMoved(outside));
        h.run_steps(2);
        assert_drag_opacity(&h, 1.0);
        capture_insertion(&mut h, &format!("opacity-outside-{floating}"));
        let target = tab_rect(&h, "properties").center();
        h.event(Event::PointerMoved(target));
        h.run_steps(2);
        assert_drag_opacity(&h, 0.7);
        let feedback = drop_preview(&h).unwrap();
        assert!(feedback.insertion.is_some() && feedback.preview.is_some());
        assert_eq!(h.state().ui.docking, before);
        capture_insertion(&mut h, &format!("opacity-cross-{floating}"));
        release_and_compare_preview(&mut h, target, "layers");
        assert!(group_members(h.state().ui.docking.as_ref().unwrap(), "properties").contains(&"layers".to_owned()));
    }
}
fn capture_insertion(h: &mut Harness<'static, PhotocraftApp>, name: &str) {
    if let Some(directory) = std::env::var_os("PHOTOCRAFT_DOCK_PREVIEW_FIXTURES").map(std::path::PathBuf::from) {
        std::fs::create_dir_all(&directory).unwrap();
        h.render().unwrap().save(directory.join(format!("{name}.png"))).unwrap();
    }
}

#[test]
fn same_group_reorders_have_only_lines_in_both_directions_and_floating_groups() {
    for size in [vec2(1440.0, 1000.0), vec2(700.0, 500.0)] {
        for floating in [false, true] {
            for forward in [false, true] {
                let mut app = app();
                app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
                if floating {
                    app.run("window.panel.group", json!({"panel":"layers","operation":"float","x":300,"y":200,"width":240,"height":300})).unwrap();
                }
                let mut h = harness(app, size);
                let panel = if forward { "layers" } else { "paths" };
                let target = tab_rect(&h, if forward { "paths" } else { "layers" });
                let to = egui::pos2(if forward { target.right() - 3.0 } else { target.left() + 3.0 }, target.center().y);
                let from = tab_rect(&h, panel).center();
                hold_drag(&mut h, from, to);
                assert_insertion_only(&h, panel);
                capture_insertion(&mut h, &format!("reorder-{}-{floating}-{forward}", size.x));
                h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
                h.run_steps(3);
                let order = group_members(h.state().ui.docking.as_ref().unwrap(), panel);
                assert_eq!(order, if forward { vec!["channels", "paths", "layers"] } else { vec!["paths", "layers", "channels"] });
            }
        }
    }
}

#[test]
fn singleton_strip_reorder_wins_over_workspace_edge() {
    let mut app = app();
    app.run("window.panel.dock", json!({"panel":"layers","side":"top"})).unwrap();
    let mut h = harness(app, vec2(700.0, 500.0));
    let tab = tab_rect(&h, "layers");
    let to = egui::pos2(tab.left() + 3.0, tab.center().y);
    let before = h.state().ui.docking.clone();
    hold_drag(&mut h, tab.center(), to);
    assert_insertion_only(&h, "layers");
    capture_insertion(&mut h, "singleton-top-edge");
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(h.state().ui.docking, before);
}

#[test]
fn floating_top_border_and_blank_header_move_the_whole_group() {
    for locked in [false, true] {
        for hit in ["move-group-border", "move-group-header"] {
            let mut app = app();
            app.run("window.panel.group", json!({"panel":"layers","operation":"float","x":300,"y":200,"width":500,"height":300})).unwrap();
            app.session.prefs.edit(|prefs| prefs.workspace_locked = locked);
            let mut h = harness(app, vec2(1440.0, 1000.0));
            let before = rendered_group(&h, "layers");
            let from = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with((hit, Some(&"layers".to_owned())))).unwrap().rect.center();
            let to = from + vec2(80.0, 60.0);
            drag(&mut h, from, to);
            let after = rendered_group(&h, "layers");
            assert!((after.min - before.min - if locked { egui::Vec2::ZERO } else { vec2(80.0, 60.0) }).length() <= 1.5, "{hit}: {before:?} -> {after:?}");
            assert_eq!(group_members(h.state().ui.docking.as_ref().unwrap(), "layers"), ["layers", "channels", "paths"]);
            capture_insertion(&mut h, &format!("{hit}-{locked}"));
        }
    }
}

#[test]
fn floating_edges_and_corners_resize_without_detaching_tabs() {
    for locked in [false, true] {
        for (hit, delta) in [
            ("resize", vec2(40.0, 50.0)),
            ("resize-left", vec2(-40.0, 0.0)),
            ("resize-right", vec2(40.0, 0.0)),
            ("resize-bottom", vec2(0.0, 50.0)),
            ("resize-bottom-left", vec2(-40.0, 50.0)),
        ] {
            let mut app = app();
            app.run("window.panel.group", json!({"panel":"layers","operation":"float","x":300,"y":200,"width":400,"height":300})).unwrap();
            app.session.prefs.edit(|prefs| prefs.workspace_locked = locked);
            let mut h = harness(app, vec2(1440.0, 1000.0));
            let before = rendered_group(&h, "layers");
            let from = h.ctx.read_response(egui::Id::new("photocraft-panel-docking").with(("floating", &"layers".to_owned())).with(hit)).unwrap().rect.center();
            drag(&mut h, from, from + delta);
            let after = rendered_group(&h, "layers");
            assert!((after.width() - before.width() - if locked { 0.0 } else { delta.x.abs() }).abs() <= 1.5, "{hit}: {before:?} -> {after:?}");
            assert!((after.height() - before.height() - if locked { 0.0 } else { delta.y.abs() }).abs() <= 1.5, "{hit}: {before:?} -> {after:?}");
            assert_eq!(group_members(h.state().ui.docking.as_ref().unwrap(), "layers"), ["layers", "channels", "paths"]);
            assert!((after.top() - before.top()).abs() <= 1.5);
            if delta.x < 0.0 {
                assert!((after.right() - before.right()).abs() <= 1.5);
            } else {
                assert!((after.left() - before.left()).abs() <= 1.5);
            }
            capture_insertion(&mut h, &format!("{hit}-{locked}"));
        }
    }
}

#[test]
fn unchanged_first_slot_in_an_overflowing_float_preserves_hidden_tab_order() {
    let mut app = app();
    for panel in ["color", "properties", "swatches", "history", "navigator"] {
        app.run("window.panel.move", json!({"panel":panel,"anchor":"layers"})).unwrap();
    }
    app.run("window.panel.group", json!({"panel":"layers","operation":"float","x":250,"y":100,"width":220,"height":300})).unwrap();
    app.run("window.panel.activate", json!({"panel":"layers"})).unwrap();
    let mut h = harness(app, vec2(700.0, 500.0));
    let before = group_members(h.state().ui.docking.as_ref().unwrap(), "layers");
    let tab = tab_rect(&h, "layers");
    let to = egui::pos2(tab.left() + 3.0, tab.center().y);
    hold_drag(&mut h, tab.center(), to);
    assert_insertion_only(&h, "layers");
    capture_insertion(&mut h, "overflow-first-slot");
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(group_members(h.state().ui.docking.as_ref().unwrap(), "layers"), before);
    assert_eq!(h.state().ui.docking.as_ref().unwrap().floating.len(), 1);
}

#[test]
fn floating_memory_is_independent_of_dock_sizes_and_persists() {
    let mut app = app();
    app.run("window.panel.float", json!({"panel":"layers","width":355,"height":455})).unwrap();
    for side in DockSide::ALL {
        app.run("window.panel.dock", json!({"panel":"layers","side":side})).unwrap();
        app.run("window.panel.layout", json!({"side":side,"size":700})).unwrap();
        app.run("window.panel.float", json!({"panel":"layers"})).unwrap();
        let rect = app.ui.docking.as_ref().unwrap().floating.iter().find(|g| g.panels.contains(&"layers".into())).unwrap().rect;
        assert_eq!(&rect[2..], &[355.0, 455.0]);
        assert_eq!(app.ui.dock_regions.sizes[side.index()], 700.0);
    }
    let value = json!({"docking":app.ui.docking,"dockRegions":app.ui.dock_regions});
    let mut restored = self::app();
    crate::dock::apply(&mut restored, &value);
    assert_eq!(restored.ui.dock_regions.floating_memory, app.ui.dock_regions.floating_memory);
    restored.run("window.panel.layout", json!({"action":Action::<String>::MoveFloating{panel:"layers".into(),rect:[40.0,50.0,380.0,480.0]}})).unwrap();
    restored.run("window.panel.dock", json!({"panel":"layers","scope":"document","side":"bottom"})).unwrap();
    restored.run("window.panel.float", json!({"panel":"layers"})).unwrap();
    assert_eq!(restored.ui.dock_regions.floating_memory["layers"], [40.0, 50.0, 380.0, 480.0]);
}

#[test]
fn live_preview_of_an_inactive_tab_preserves_selection_on_cancel() {
    let mut app = app();
    enable_workspace(&mut app);
    let mut h = harness(app, vec2(1440.0, 1000.0));
    let before = serde_json::to_value(h.state().ui.dock_tabs).unwrap();
    let from = tab_rect(&h, "channels").center();
    let to = from - vec2(400.0, 0.0);
    hold_drag(&mut h, from, to);
    assert_eq!(serde_json::to_value(h.state().ui.dock_tabs).unwrap(), before);
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(serde_json::to_value(h.state().ui.dock_tabs).unwrap(), before);
    assert_eq!(group_members(h.state().ui.docking.as_ref().unwrap(), "layers"), ["layers", "channels", "paths"]);
}

#[test]
fn document_host_ownership_is_independent_and_returns_on_redock() {
    let mut app = app();
    app.run("file.new", json!({"width":80,"height":60})).unwrap();
    let doc = app.session.active().unwrap().doc.id.0;
    app.run("window.panel.dock", json!({"panel":"color","scope":"document","side":"left"})).unwrap();
    app.run("window.document.float", json!({"documentView":doc,"width":700,"height":500})).unwrap();
    let inner_sizes = app.ui.dock_regions.sizes_for(DockHost::DocumentView(doc));
    app.run("window.panel.layout", json!({"scope":"document","side":"left","size":350})).unwrap();
    assert_eq!(app.ui.dock_regions.sizes_for(DockHost::DocumentView(doc)), inner_sizes);
    assert_eq!(app.ui.dock_regions.host("color"), DockHost::Document);
    for side in DockSide::ALL {
        app.run("window.panel.dock", json!({"panel":"layers","scope":"document","documentView":doc,"side":side})).unwrap();
        assert_eq!(app.ui.dock_regions.host("layers"), DockHost::DocumentView(doc));
        let layout = app.ui.docking.as_ref().unwrap();
        assert!(!app.ui.dock_regions.render(layout).layout.panels().contains(&&"layers".to_string()));
        assert!(
            app.ui
                .dock_regions
                .render_host(layout, DockHost::DocumentView(doc), &crate::document_docking::canvas_id(doc))
                .layout
                .panels()
                .contains(&&"layers".to_string())
        );
    }
    let preferred = app.ui.views[0].clone();
    app.run("window.document.dock", json!({"documentView":doc})).unwrap();
    assert!(!crate::document_docking::detached(&app, 0));
    assert_eq!(app.ui.views[0], preferred);
    assert_eq!(app.ui.dock_regions.host("layers"), DockHost::Document);
    assert_eq!(app.ui.dock_regions.side("layers"), DockSide::Bottom);
}

#[test]
fn malformed_document_and_host_commands_are_atomic() {
    let mut app = app();
    app.run("file.new", json!({"width":50,"height":50})).unwrap();
    for (id, params) in [
        ("window.document.float", json!({"width":-1})),
        ("window.document.float", json!({"documentView":999999})),
        ("window.document.geometry", json!({"width":800})),
        ("window.document.float", json!({"x":"bad"})),
        ("window.panel.dock", json!({"panel":"layers","side":"top","scope":"document","documentView":999999})),
        ("window.panel.dock", json!({"panel":"layers","side":"top","scope":17})),
        ("window.panel.layout", json!({"side":"left","size":-20,"scope":"document"})),
    ] {
        let before = serde_json::to_value(&app.ui).unwrap();
        assert!(app.run(id, params).is_err(), "{id}");
        assert_eq!(serde_json::to_value(&app.ui).unwrap(), before, "{id}");
    }
    app.session.edit_prefs(|p| p.workspace_locked = true);
    assert!(app.run("window.document.float", json!({})).is_err());
}

#[test]
fn pointer_inner_document_edges_match_measured_drop_layout() {
    for side in DockSide::ALL {
        let mut app = app();
        app.run("file.new", json!({"width":80,"height":60})).unwrap();
        app.run("window.panel.dock", json!({"panel":"color","side":"left"})).unwrap();
        let mut h = harness(app, vec2(1440.0, 1000.0));
        let canvas = rendered_group(&h, CANVAS);
        let to = match side {
            DockSide::Left => egui::pos2(canvas.left() + 40.0, canvas.center().y),
            DockSide::Right => egui::pos2(canvas.right() - 40.0, canvas.center().y),
            DockSide::Top => egui::pos2(canvas.center().x, canvas.top() + 40.0),
            DockSide::Bottom => egui::pos2(canvas.center().x, canvas.bottom() - 40.0),
        };
        let from = tab_rect(&h, "layers").center();
        hold_drag(&mut h, from, to);
        capture_insertion(&mut h, &format!("live-inner-{side:?}-drag"));
        release_and_compare_preview(&mut h, to, "layers");
        assert_eq!(h.state().ui.dock_regions.host("layers"), DockHost::Document);
        assert_eq!(h.state().ui.dock_regions.side("layers"), side);
        capture_insertion(&mut h, &format!("live-inner-{side:?}-settled"));
    }
}

#[test]
fn floating_document_host_accepts_pointer_drops_and_keeps_geometry_on_cancel() {
    let mut app = app();
    app.run("file.new", json!({"width":80,"height":60})).unwrap();
    let id = app.session.active().unwrap().doc.id.0;
    app.run("window.document.float", json!({"documentView":id,"x":50,"y":150,"width":650,"height":500})).unwrap();
    let mut h = harness(app, vec2(1440.0, 1000.0));
    let original = h.state().ui.floating_documents.clone();
    let canvas = rendered_group(&h, &crate::document_docking::canvas_id(id));
    let to = egui::pos2(canvas.right() - 40.0, canvas.center().y);
    let from = tab_rect(&h, "layers").center();
    hold_drag(&mut h, from, to);
    capture_insertion(&mut h, "live-floating-document-drag");
    release_and_compare_preview(&mut h, to, "layers");
    assert_eq!(h.state().ui.dock_regions.host("layers"), DockHost::DocumentView(id));
    assert_eq!(h.state().ui.floating_documents, original);
    capture_insertion(&mut h, "live-floating-document-settled");
}

#[test]
fn document_header_movement_and_resize_commit_only_on_release_and_cancel() {
    let mut app = app();
    app.run("file.new", json!({"width":80,"height":60})).unwrap();
    let id = app.session.active().unwrap().doc.id.0;
    app.run("window.document.float", json!({"x":70,"y":150,"width":600,"height":500})).unwrap();
    let mut h = harness(app, vec2(1440.0, 1000.0));
    for name in ["header", "corner"] {
        let original = h.state().ui.floating_documents.clone();
        let control = if name == "header" { egui::Id::new(("document-header", id)) } else { egui::Id::new(("document-resize", id, "corner")) };
        let from = h.ctx.read_response(control).unwrap().rect.center();
        hold_drag(&mut h, from, from + vec2(60.0, 40.0));
        assert_eq!(h.state().ui.floating_documents, original, "transient {name} must not modify saved geometry");
        h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        h.run_steps(1);
        h.event(Event::PointerButton { pos: from + vec2(60.0, 40.0), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(2);
        assert_eq!(h.state().ui.floating_documents, original);
        let from = h.ctx.read_response(control).unwrap().rect.center();
        drag(&mut h, from, from + vec2(60.0, 40.0));
        let rect = h.state().ui.floating_documents[&id];
        let old = original[&id];
        if name == "header" {
            assert_eq!(&rect[..2], &[old[0] + 60.0, old[1] + 40.0]);
        } else {
            assert_eq!(&rect[2..], &[old[2] + 60.0, old[3] + 40.0]);
        }
    }
}

#[test]
fn document_tab_detachment_redocking_and_stable_identity_after_reorder() {
    let mut app = app();
    for _ in 0..2 {
        app.run("file.new", json!({"width":80,"height":60})).unwrap();
    }
    enable_workspace(&mut app);
    let id = app.session.documents()[0].doc.id.0;
    let mut h = harness(app, vec2(1440.0, 1000.0));
    let tab = h.state().tab_strip.as_ref().unwrap().tabs.iter().find(|(i, _)| *i == 0).unwrap().1;
    drag(&mut h, tab.center(), tab.center() + vec2(120.0, 180.0));
    assert!(h.state().ui.floating_documents.contains_key(&id));
    assert!(!h.state().tab_strip.as_ref().unwrap().tabs.iter().any(|(i, _)| *i == 0));
    h.state_mut().run("document.move", json!({"document":0,"to":1})).unwrap();
    h.run_steps(2);
    assert!(crate::document_docking::detached(h.state(), 1));
    let from = h.ctx.read_response(egui::Id::new(("document-header", id))).unwrap().rect.center();
    let to = h.state().tab_strip.as_ref().unwrap().rect.right_center() - vec2(20.0, 0.0);
    drag(&mut h, from, to);
    assert!(!h.state().ui.floating_documents.contains_key(&id));
    assert_eq!(h.state().session.documents().len(), 2);
    h.state_mut().run("window.document.float", json!({"documentView":id,"width":680,"height":420})).unwrap();
    h.state_mut().run("window.document.dock", json!({"documentView":id})).unwrap();
    h.run_steps(2);
    let tab = h.state().tab_strip.as_ref().unwrap().tabs.iter().find(|(i, _)| *i == 1).unwrap().1;
    drag(&mut h, tab.center(), tab.center() + vec2(80.0, 150.0));
    assert_eq!(&h.state().ui.floating_documents[&id][2..], &[680.0, 420.0]);
    h.state_mut().run("window.panel.dock", json!({"panel":"color","scope":"document","documentView":id,"side":"top"})).unwrap();
    h.state_mut().run("file.close", json!({"document":1})).unwrap();
    h.run_steps(2);
    assert!(!h.state().ui.floating_documents.contains_key(&id));
    assert_eq!(h.state().ui.dock_regions.host("color"), DockHost::Document);
    assert_eq!(h.state().ui.dock_regions.side("color"), DockSide::Top);
}

#[test]
fn capture_live_document_docking_themes() {
    let Some(directory) = std::env::var_os("PHOTOCRAFT_LIVE_DOCK_FIXTURES").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&directory).unwrap();
    for theme in crate::theme::ThemeKind::ALL {
        for size in [vec2(960.0, 700.0), vec2(1440.0, 1000.0)] {
            let mut app = app();
            app.run("file.new", json!({"width":120,"height":90})).unwrap();
            app.ui.theme = theme;
            let id = app.session.active().unwrap().doc.id.0;
            app.run("window.document.float", json!({"x":30,"y":140,"width":600,"height":450})).unwrap();
            app.run("window.panel.dock", json!({"panel":"color","scope":"document","documentView":id,"side":"left"})).unwrap();
            app.run("window.panel.dock", json!({"panel":"swatches","scope":"document","side":"bottom"})).unwrap();
            let mut ready = false;
            let mut h = Harness::builder().with_size(size).wgpu().build_ui_state(
                move |ui, app: &mut PhotocraftApp| {
                    if !ready {
                        PhotocraftApp::setup_context(ui.ctx(), theme);
                        ready = true;
                        return;
                    }
                    crate::panels::right_dock(app, ui);
                    crate::document_docking::show(app, ui.ctx());
                },
                app,
            );
            h.input_mut().max_texture_side = Some(8192);
            h.run_steps(4);
            let stem = format!("{}-{}", theme.label(), size.x as u32);
            h.render().unwrap().save(directory.join(format!("{stem}-settled.png"))).unwrap();
            let from = tab_rect(&h, "layers").center();
            let canvas = rendered_group(&h, &crate::document_docking::canvas_id(id));
            let to = egui::pos2(canvas.right() - 35.0, canvas.center().y);
            hold_drag(&mut h, from, to);
            h.render().unwrap().save(directory.join(format!("{stem}-drag.png"))).unwrap();
        }
    }
}

#[test]
fn hidden_floating_panel_keeps_its_dock_return_and_floating_memory() {
    let mut app = app();
    app.run("window.panel.dock", json!({"panel":"properties","scope":"document","side":"bottom"})).unwrap();
    app.run("window.panel.float", json!({"panel":"properties","width":345,"height":465})).unwrap();
    app.run("window.panel.close", json!({"panel":"properties"})).unwrap();
    app.run("window.panel.activate", json!({"panel":"properties"})).unwrap();
    app.run("window.panel.dock", json!({"panel":"properties"})).unwrap();
    assert_eq!(app.ui.dock_regions.host("properties"), DockHost::Document);
    assert_eq!(app.ui.dock_regions.side("properties"), DockSide::Bottom);
    app.run("window.panel.float", json!({"panel":"properties"})).unwrap();
    assert_eq!(&app.ui.dock_regions.floating_memory["properties"][2..], &[345.0, 465.0]);
}
