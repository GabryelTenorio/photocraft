//! Exercise the embedded browser through the editor and toolbar, not a replacement test UI.

use super::editor_tests::{editor, type_number};
use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};

#[test]
fn editor_browser_expands_groups_and_selects_without_opening_the_sidebar() {
    let (mut h, sink) = editor(false);
    let tab = h.state().ui.dock_tabs.color;
    h.get_by_label("Purples").click();
    h.run_steps(3);
    h.get_by_label("Purple 03").click();
    h.run_steps(3);
    assert_eq!(h.state().session.presets.gradient.name, "Purple 03");
    assert!(matches!(sink.fill(h.state()), Some(Fill::Gradient { stops, .. }) if stops.len() == 3));
    assert_eq!(h.state().ui.dock_tabs.color, tab);
    assert!(h.state().ui.panels.gradient_editor);
    assert_eq!(h.state().session.active().unwrap().doc.layers.len(), 1);
}

#[test]
fn layer_browser_replaces_color_and_opacity_preserves_geometry_and_undoes_once() {
    let (mut h, sink) = editor(true);
    let before = sink.fill(h.state()).unwrap();
    let count = h.state().session.active().unwrap().history.past_len();
    h.get_by_label("Foreground to Transparent").click();
    h.run_steps(3);
    let after = sink.fill(h.state()).unwrap();
    let Fill::Gradient { angle, scale, style, reverse, offset, dither, align, .. } = before.clone() else { panic!() };
    let Fill::Gradient { angle: a, scale: s, style: st, reverse: r, offset: o, dither: d, align: al, stops, opacity_stops, midpoints } = &after else {
        panic!()
    };
    assert_eq!((*a, *s, *st, *r, *o, *d, *al), (angle, scale, style, reverse, offset, dither, align));
    assert_eq!(stops.len(), 2);
    assert_eq!(opacity_stops, &vec![(0.0, 1.0), (1.0, 0.0)]);
    assert!(midpoints.is_empty());
    assert_eq!(h.state().session.active().unwrap().doc.layers.len(), 2);
    assert_eq!(h.state().session.active().unwrap().history.past_len(), count + 1);
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(sink.fill(h.state()).unwrap(), before);
    h.state_mut().run("edit.redo", json!({})).unwrap();
    assert_eq!(sink.fill(h.state()).unwrap(), after);
}

#[test]
fn classic_browser_and_editor_edit_the_tool_even_with_a_gradient_layer_selected() {
    let (mut h, layer) = editor(true);
    h.state_mut().ui.tool_options.gradient_classic = true;
    h.run_steps(3);
    let before = layer.fill(h.state());
    let count = h.state().session.active().unwrap().history.past_len();
    h.get_by_label("Foreground to Transparent").click();
    h.run_steps(3);
    assert_eq!(layer.fill(h.state()), before);
    assert_eq!(h.state().session.presets.gradient.name, "Foreground to Transparent");
    assert_eq!(h.state().session.active().unwrap().history.past_len(), count);
    assert!(h.query_by_label("Gradient Options").is_none());
    h.query_all_by_label("Color").find(|n| n.accesskit_node().role() == egui::accesskit::Role::Button).unwrap().click();
    h.run_steps(2);
    assert!(!h.state().ui.dialogs.is_empty());
}

#[test]
fn picking_a_preset_discards_an_uncommitted_location_draft() {
    let (mut h, sink) = editor(true);
    let count = h.state().session.active().unwrap().history.past_len();
    type_number(&mut h, 0, "70", false);
    h.get_by_label("Foreground to Transparent").click();
    h.run_steps(4);
    assert!(matches!(sink.fill(h.state()), Some(Fill::Gradient { stops, .. }) if stops[0].0 == 0.0));
    assert_eq!(h.state().session.active().unwrap().history.past_len(), count + 1);
}

#[test]
fn toolbar_popup_selects_in_both_modes_without_creating_a_layer() {
    for classic in [false, true] {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 200, "height": 120})).unwrap();
        app.ui.tool = Tool::Gradient;
        app.ui.tool_options.gradient_classic = classic;
        let mut h = Harness::builder().with_size(vec2(1000.0, 800.0)).build_ui_state(
            |ui, app: &mut PhotocraftApp| {
                if preset_swatch(app, ui) {
                    app.ui.panels.gradient_editor = true;
                }
            },
            app,
        );
        PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ProMedium);
        h.run_steps(3);
        h.get_by_label("Presets").click();
        h.run_steps(3);
        h.get_by_label("Foreground to Transparent").click();
        h.run_steps(3);
        assert_eq!(h.state().session.presets.gradient.name, "Foreground to Transparent");
        assert_eq!(h.state().session.active().unwrap().doc.layers.len(), 1);
        assert!(!h.state().ui.panels.gradient_editor);
        assert!(h.query_by_label("Basics").is_none(), "popup closes after choosing a preset");
    }
}

#[test]
fn editor_browser_can_choose_presets_without_a_document() {
    let app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    let mut h = Harness::builder().with_size(vec2(1000.0, 900.0)).build_ui_state(|ui, app: &mut PhotocraftApp| editor_window(app, ui.ctx()), app);
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ProMedium);
    h.state_mut().ui.panels.gradient_editor = true;
    h.run_steps(3);
    h.get_by_label("Foreground to Transparent").click();
    h.run_steps(3);
    assert!(h.state().session.active().is_none());
    assert_eq!(h.state().session.presets.gradient.name, "Foreground to Transparent");
}

#[test]
fn browser_new_saves_the_edited_layer_instead_of_the_tool_gradient() {
    let (mut h, sink) = editor(true);
    let before = sink.fill(h.state());
    let count = h.state().session.active().unwrap().history.past_len();
    h.get_by_label("Create new gradient").click();
    h.run_steps(3);
    let saved = h.state().session.presets.gradients.iter().flat_map(|g| &g.items).find(|p| p.name == "Custom").unwrap();
    assert_eq!(saved.stops.len(), 3);
    assert_eq!(saved.opacity, vec![(0.0, 0.25), (0.5, 0.6), (1.0, 1.0)]);
    assert_eq!(sink.fill(h.state()), before);
    assert_eq!(h.state().session.active().unwrap().history.past_len(), count);
}

#[test]
fn double_clicking_an_editor_preset_does_not_create_another_layer() {
    let (mut h, _) = editor(true);
    h.get_by_label("Foreground to Transparent").click();
    h.run_steps(2);
    h.get_by_label("Foreground to Transparent").click();
    h.run_steps(3);
    assert_eq!(h.state().session.active().unwrap().doc.layers.len(), 2);
    assert_eq!(h.state().session.presets.gradient.name, "Foreground to Transparent");
}

#[test]
fn presets_can_collapse_to_reach_controls_in_a_small_window_across_themes() {
    for theme in crate::theme::ThemeKind::ALL {
        let (mut h, _) = editor(true);
        PhotocraftApp::setup_context(&h.ctx, theme);
        h.set_size(vec2(440.0, 560.0));
        h.run_steps(4);
        h.get_by_label("Presets").click();
        h.run_steps(8);
        let reset = h.get_by_label("Reset Alignment").rect();
        assert!(reset.left() >= 0.0 && reset.right() <= 440.0 && reset.bottom() <= 560.0, "{theme:?}: {reset:?}");
        h.get_by_label("Reset Alignment").click();
        h.run_steps(3);
        assert!(h.state().ui.panels.gradient_editor);
    }
}
