use super::*;
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable};
use photocraft_doc::{LayerId, SmartFilter};

pub(super) fn app(depth: u32) -> (PhotocraftApp, LayerId) {
    let mut s = photocraft_engine::Session::new();
    s.execute("file.new", json!({"width": 32, "height": 24, "depth": depth})).unwrap();
    s.execute("layer.new.layer", json!({"name": "Filtered source"})).unwrap();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 16, "height": 24})).unwrap();
    s.execute("edit.fill", json!({"contents": "color", "color": "#d06030"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    s.execute("layer.smartObjects.convertToSmartObject", json!({})).unwrap();
    let layer = s.active().unwrap().active_layer.unwrap();
    s.execute("filter.other.offset", json!({"horizontal": 5, "undefinedAreas": "transparent"})).unwrap();
    s.execute("filter.blur.gaussianBlur", json!({"radius": 2})).unwrap();
    s.execute("layer.new.layer", json!({"name": "Other active layer"})).unwrap();
    let mut app = PhotocraftApp::new(s, Default::default());
    app.background_jobs = false;
    (app, layer)
}

pub(super) fn smart(app: &PhotocraftApp, layer: LayerId) -> &SmartObject {
    let LayerContent::Smart(sm) = &app.session.active().unwrap().doc.layer(layer).unwrap().content else { panic!("Smart Object") };
    sm
}

pub(super) fn filters(app: &PhotocraftApp, layer: LayerId) -> Vec<SmartFilter> {
    smart(app, layer).smart_filters.clone()
}

pub(super) fn pixels(app: &PhotocraftApp, layer: LayerId) -> Vec<f32> {
    let doc = &app.session.active().unwrap().doc;
    smart(app, layer).cache.as_ref().unwrap().read_region(doc.bounds())
}

pub(super) fn text_position(h: &Harness<'_, PhotocraftApp>, name: &str) -> Pos2 {
    fn text(shape: &egui::Shape, name: &str) -> Option<Pos2> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == name => Some(Rect::from_min_size(t.pos, t.galley.size()).center()),
            egui::Shape::Vec(v) => v.iter().find_map(|s| text(s, name)),
            _ => None,
        }
    }
    h.output().shapes.iter().find_map(|s| text(&s.shape, name)).unwrap_or_else(|| panic!("row text {name} not drawn"))
}

pub(super) fn click(h: &mut Harness<'_, PhotocraftApp>, at: Pos2, button: PointerButton) {
    h.hover_at(at);
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

pub(super) fn harness() -> (Harness<'static, PhotocraftApp>, LayerId) {
    harness_at(8, 1.0)
}

pub(super) fn harness_at(depth: u32, ppp: f32) -> (Harness<'static, PhotocraftApp>, LayerId) {
    let (app, layer) = app(depth);
    let mut h =
        Harness::builder().with_size(vec2(1440.0, 900.0)).with_pixels_per_point(ppp).with_step_dt(1.0 / 60.0).with_max_steps(64).build_eframe(move |cc| {
            PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
            app
        });
    let ctx = h.ctx.clone();
    let (req, _rx) = crate::control::ControlRequest::new("ui.set", json!({"dock": {"collapsed": ["color", "properties", "history", "navigator"]}}));
    crate::control::handle(h.state_mut(), &ctx, &req);
    h.run_steps(8);
    (h, layer)
}

#[test]
fn row_menu_moves_and_deletes_the_clicked_filter_with_one_undo_step() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    for depth in [8, 16, 32] {
        for ppp in [1.0, 2.0] {
            let (mut h, layer) = harness_at(depth, ppp);
            let before = filters(h.state(), layer);
            let before_pixels = pixels(h.state(), layer);
            let source = photocraft_engine::smart_cmds::contents_of(&h.state().session.active().unwrap().doc, layer).unwrap();
            let st = h.state().session.active().unwrap();
            let active = st.active_layer;
            let selected = st.selected_layers.clone();
            let history = st.history.past_len();
            let other = st.doc.layer(active.unwrap()).unwrap().clone();
            let offset = text_position(&h, "Offset");
            click(&mut h, offset, PointerButton::Secondary);
            assert!(h.query_by_label("Move Up").is_some(), "actual filter row has Move Up");
            h.get_by_label("Move Up").click();
            h.run_steps(4);
            assert_eq!(filters(h.state(), layer).iter().map(|f| f.command.as_str()).collect::<Vec<_>>(), ["filter.blur.gaussianBlur", "filter.other.offset"]);
            assert!(text_position(&h, "Offset").y < text_position(&h, "Gaussian Blur").y);
            assert_ne!(pixels(h.state(), layer), before_pixels);
            assert_eq!(h.state().session.active().unwrap().history.past_len(), history + 1);
            assert_eq!(h.state().session.active().unwrap().active_layer, active);
            assert_eq!(h.state().session.active().unwrap().selected_layers, selected);
            assert_eq!(photocraft_engine::smart_cmds::contents_of(&h.state().session.active().unwrap().doc, layer).unwrap(), source);
            h.state_mut().run("edit.undo", json!({})).unwrap();
            h.run_steps(3);
            assert_eq!(serde_json::to_value(filters(h.state(), layer)).unwrap(), serde_json::to_value(&before).unwrap());
            assert_eq!(pixels(h.state(), layer), before_pixels);
            h.state_mut().run("edit.redo", json!({})).unwrap();
            h.run_steps(3);
            let offset = text_position(&h, "Offset");
            click(&mut h, offset, PointerButton::Secondary);
            h.get_by_label("Move Down").click();
            h.run_steps(4);
            assert_eq!(serde_json::to_value(filters(h.state(), layer)).unwrap(), serde_json::to_value(&before).unwrap());
            assert_eq!(pixels(h.state(), layer), before_pixels);
            assert_eq!(h.state().session.active().unwrap().history.past_len(), history + 2);
            let offset = text_position(&h, "Offset");
            click(&mut h, offset, PointerButton::Secondary);
            h.get_by_label("Delete Smart Filter").click();
            h.run_steps(4);
            assert_eq!(filters(h.state(), layer).len(), 1);
            assert_eq!(filters(h.state(), layer)[0].command, "filter.blur.gaussianBlur");
            assert_eq!(h.state().session.active().unwrap().history.past_len(), history + 3);
            assert_eq!(h.state().session.active().unwrap().active_layer, active);
            assert_eq!(h.state().session.active().unwrap().selected_layers, selected);
            assert_eq!(photocraft_engine::smart_cmds::contents_of(&h.state().session.active().unwrap().doc, layer).unwrap(), source);
            let unchanged = h.state().session.active().unwrap().doc.layer(other.id).unwrap();
            assert_eq!(unchanged, &other);
            h.state_mut().run("edit.undo", json!({})).unwrap();
            assert_eq!(pixels(h.state(), layer), before_pixels);
            h.state_mut().run("edit.redo", json!({})).unwrap();
            assert_eq!(filters(h.state(), layer).len(), 1);
        }
    }
}

#[test]
fn two_filter_row_context_offers_reorder_and_delete() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    let (mut h, layer) = harness();
    let top = text_position(&h, "Gaussian Blur");
    let bottom = text_position(&h, "Offset");
    assert!(top.y < bottom.y, "index1 is visually above index0");
    let before = h.state().session.active().unwrap().doc.clone();
    let history = h.state().session.active().unwrap().history.past_len();
    click(&mut h, bottom, PointerButton::Secondary);
    assert!(std::sync::Arc::ptr_eq(&before, &h.state().session.active().unwrap().doc));
    assert_eq!(h.state().session.active().unwrap().history.past_len(), history);
    let missing = ["Move Up", "Move Down", "Delete Smart Filter"].into_iter().filter(|label| h.query_by_label(label).is_none()).collect::<Vec<_>>();
    assert!(missing.is_empty(), "filter row lacks context choices {missing:?}; stack has {} filters", smart(h.state(), layer).smart_filters.len());
}

#[test]
fn existing_filter_move_delete_commands_preserve_source_and_undo_at_every_depth() {
    for depth in [8, 16, 32] {
        let (mut app, layer) = app(depth);
        let original = filters(&app, layer);
        let original_pixels = pixels(&app, layer);
        let source = photocraft_engine::smart_cmds::contents_of(&app.session.active().unwrap().doc, layer).unwrap();
        let active = app.session.active().unwrap().active_layer;
        let history = app.session.active().unwrap().history.past_len();
        app.run("layer.smartFilter.move", json!({"layer": layer.0, "index": 0, "to": 1})).unwrap();
        assert_eq!(filters(&app, layer).iter().map(|f| f.command.as_str()).collect::<Vec<_>>(), ["filter.blur.gaussianBlur", "filter.other.offset"]);
        assert_ne!(pixels(&app, layer), original_pixels, "noncommuting operations really rerender");
        assert_eq!(app.session.active().unwrap().history.past_len(), history + 1);
        assert_eq!(app.session.active().unwrap().active_layer, active);
        assert_eq!(photocraft_engine::smart_cmds::contents_of(&app.session.active().unwrap().doc, layer).unwrap(), source);
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(serde_json::to_value(filters(&app, layer)).unwrap(), serde_json::to_value(&original).unwrap());
        assert_eq!(pixels(&app, layer), original_pixels);
        app.run("edit.redo", json!({})).unwrap();
        let reordered_pixels = pixels(&app, layer);
        app.run("layer.smartFilter.delete", json!({"layer": layer.0, "index": 1})).unwrap();
        assert_eq!(filters(&app, layer).len(), 1);
        assert_eq!(filters(&app, layer)[0].command, "filter.blur.gaussianBlur");
        assert_eq!(app.session.active().unwrap().history.past_len(), history + 2);
        assert_eq!(app.session.active().unwrap().active_layer, active);
        assert_eq!(photocraft_engine::smart_cmds::contents_of(&app.session.active().unwrap().doc, layer).unwrap(), source);
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(pixels(&app, layer), reordered_pixels);
        app.run("edit.redo", json!({})).unwrap();
        assert_eq!(filters(&app, layer).len(), 1);
    }
}
