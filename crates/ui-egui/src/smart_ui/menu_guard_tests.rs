use super::context_tests::*;
use super::*;
use egui::{PointerButton, pos2};
use egui_kittest::kittest::{NodeT, Queryable};
use std::sync::Arc;

#[test]
fn row_menu_disables_edges_and_deletes_the_only_remaining_filter() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    for ppp in [1.0, 2.0] {
        let (mut h, layer) = harness_at(8, ppp);
        let top = text_position(&h, "Gaussian Blur");
        click(&mut h, top, PointerButton::Secondary);
        assert!(h.get_by_label("Move Up").accesskit_node().is_disabled());
        assert!(!h.get_by_label("Move Down").accesskit_node().is_disabled());
        let before = h.state().session.active().unwrap().doc.clone();
        let history = h.state().session.active().unwrap().history.past_len();
        h.get_by_label("Move Up").click();
        h.run_steps(3);
        assert!(Arc::ptr_eq(&before, &h.state().session.active().unwrap().doc));
        assert_eq!(h.state().session.active().unwrap().history.past_len(), history);
        egui::Popup::close_all(&h.ctx);
        h.state_mut().run("layer.smartFilter.delete", json!({"layer":layer.0,"index":0})).unwrap();
        h.run_steps(3);
        let only = text_position(&h, "Gaussian Blur");
        click(&mut h, only, PointerButton::Secondary);
        assert!(h.get_by_label("Move Up").accesskit_node().is_disabled());
        assert!(h.get_by_label("Move Down").accesskit_node().is_disabled());
        assert!(!h.get_by_label("Delete Smart Filter").accesskit_node().is_disabled());
        h.get_by_label("Delete Smart Filter").click();
        h.run_steps(4);
        assert!(filters(h.state(), layer).is_empty());
        assert_eq!(h.state().session.active().unwrap().history.past_len(), history + 2);
        assert!(!egui::Popup::is_any_open(&h.ctx));
        h.state_mut().run("edit.undo", json!({})).unwrap();
        assert_eq!(filters(h.state(), layer).len(), 1);
        h.state_mut().run("edit.redo", json!({})).unwrap();
        assert!(filters(h.state(), layer).is_empty());
    }
}

#[test]
fn stale_row_menu_closes_on_edits_undo_redo_document_switch_and_same_id_reopen() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    for change in ["move", "delete", "undo", "redo", "switch", "reopen", "close"] {
        let (mut h, layer) = harness();
        let offset = text_position(&h, "Offset");
        click(&mut h, offset, PointerButton::Secondary);
        assert!(h.query_by_label("Delete Smart Filter").is_some());
        match change {
            "move" => {
                h.state_mut().run("layer.smartFilter.move", json!({"layer":layer.0,"index":0,"to":1})).unwrap();
            }
            "delete" => {
                h.state_mut().run("layer.smartFilter.delete", json!({"layer":layer.0,"index":0})).unwrap();
            }
            "undo" => {
                h.state_mut().run("edit.undo", json!({})).unwrap();
            }
            "redo" => {
                h.state_mut().run("edit.undo", json!({})).unwrap();
                h.state_mut().run("edit.redo", json!({})).unwrap();
            }
            "switch" => {
                h.state_mut().run("file.new", json!({"width":8,"height":8})).unwrap();
            }
            "reopen" => {
                let session = &mut h.state_mut().session;
                let old = session.close(session.active_index().unwrap()).unwrap();
                let index = session.add_document((*old.doc).clone(), None);
                assert_eq!(session.documents()[index].doc.id, old.doc.id);
                session.active_mut().unwrap().revision = old.revision;
                assert!(!Arc::ptr_eq(&session.active().unwrap().doc, &old.doc));
            }
            "close" => {
                let session = &mut h.state_mut().session;
                session.close(session.active_index().unwrap()).unwrap();
            }
            other => panic!("unrecognized test mutation {other}"),
        }
        let before = h.state().session.active().map(|st| (st.doc.clone(), st.revision, st.history.past_len()));
        h.run_steps(4);
        assert!(h.query_by_label("Delete Smart Filter").is_none(), "{change} closes stale choices");
        assert!(!egui::Popup::is_any_open(&h.ctx), "{change} closes popup memory too");
        if let Some((doc, revision, history)) = before {
            let st = h.state().session.active().unwrap();
            assert!(Arc::ptr_eq(&doc, &st.doc), "{change} adds no accidental edit");
            assert_eq!((st.revision, st.history.past_len()), (revision, history));
        }
    }
}

#[test]
fn parameter_and_camera_raw_modals_close_the_row_menu_without_mutation() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    for raw in [false, true] {
        let (mut h, layer) = harness();
        let offset = text_position(&h, "Offset");
        click(&mut h, offset, PointerButton::Secondary);
        let ctx = h.ctx.clone();
        if raw {
            crate::camera_raw_ui::open(h.state_mut(), &ctx).unwrap();
        } else {
            let sm = smart(h.state(), layer).clone();
            open_editor(h.state_mut(), &ctx, layer.0, &sm, 0);
            assert!(h.state().ui.dialogs.last().unwrap().fields.contains_key("__smartFilter"));
        }
        let before = h.state().session.active().unwrap().doc.clone();
        let history = h.state().session.active().unwrap().history.past_len();
        h.run_steps(4);
        assert!(h.query_by_label("Delete Smart Filter").is_none());
        assert!(!egui::Popup::is_any_open(&h.ctx));
        assert!(Arc::ptr_eq(&before, &h.state().session.active().unwrap().doc));
        assert_eq!(h.state().session.active().unwrap().history.past_len(), history);
    }
}

#[test]
fn pending_document_job_closes_row_menu_and_refuses_new_choices() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    let (mut h, _layer) = harness();
    let offset = text_position(&h, "Offset");
    click(&mut h, offset, PointerButton::Secondary);
    let before = h.state().session.active().unwrap().doc.clone();
    let started = h
        .state_mut()
        .session
        .start_job(
            "test.wait",
            json!({}),
            "Waiting filter",
            true,
            |ctx| -> photocraft_engine::Result<()> {
                loop {
                    ctx.check()?;
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            },
            |_, ()| Ok(Value::Null),
        )
        .unwrap();
    let photocraft_engine::jobs::Started::Job(id) = started else { panic!("native worker") };
    h.run_steps(3);
    assert!(h.query_by_label("Delete Smart Filter").is_none());
    assert!(!egui::Popup::is_any_open(&h.ctx));
    click(&mut h, offset, PointerButton::Secondary);
    assert!(h.query_by_label("Delete Smart Filter").is_none());
    assert!(Arc::ptr_eq(&before, &h.state().session.active().unwrap().doc));
    h.state_mut().session.cancel_job(id);
    h.state_mut().session.join_cancelled_jobs();
}

#[test]
fn existing_eye_and_name_double_click_remain_distinct_from_secondary_actions() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    for ppp in [1.0, 2.0] {
        let (mut h, layer) = harness_at(8, ppp);
        let offset = text_position(&h, "Offset");
        let layer_row = crate::layer_row_ui::recorded(&h.ctx).into_iter().find(|r| r.layer == layer.0).unwrap().row;
        let eye = pos2(layer_row.left() + 15.0, offset.y);
        let history = h.state().session.active().unwrap().history.past_len();
        click(&mut h, eye, PointerButton::Primary);
        assert!(!filters(h.state(), layer)[0].visible);
        assert!(h.query_by_label("Delete Smart Filter").is_none());
        assert!(h.state().ui.dialogs.is_empty());
        h.state_mut().run("edit.undo", json!({})).unwrap();
        h.run_steps(40);
        click(&mut h, offset, PointerButton::Primary);
        click(&mut h, offset, PointerButton::Primary);
        let dialog = h.state().ui.dialogs.last().unwrap();
        assert_eq!(dialog.fields["__command"], json!("filter.other.offset"));
        assert_eq!(dialog.fields["__smartFilter"], json!({"layer":layer.0,"index":0}));
        assert_eq!(h.state().session.active().unwrap().history.past_len(), history);
    }
}

#[test]
#[ignore = "offscreen native popup captures; set PHOTOCRAFT_SMART_FILTER_CAPTURE_DIR"]
fn capture_native_popup_at_two_scales_and_in_french() {
    let out = std::env::var("PHOTOCRAFT_SMART_FILTER_CAPTURE_DIR").expect("capture destination");
    for (language, ppp) in [("en", 1.0), ("fr", 2.0)] {
        let (mut app, layer) = app(8);
        app.run("prefs.set", json!({"path":"interface.language","value":language})).unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_pixels_per_point(ppp).with_max_steps(64).wgpu().build_eframe(
            move |cc| {
                PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
                app
            },
        );
        let ctx = h.ctx.clone();
        let (req, _) = crate::control::ControlRequest::new("ui.set", json!({"zoom":12.0,"dock":{"collapsed":["color","properties","history","navigator"]}}));
        crate::control::handle(h.state_mut(), &ctx, &req);
        h.run_steps(12);
        let label = filter_label(&filters(h.state(), layer)[0]);
        let offset = text_position(&h, &label);
        click(&mut h, offset, PointerButton::Secondary);
        for label in [tl!("Move Up"), tl!("Move Down"), tl!("Delete Smart Filter")] {
            assert!(h.query_by_label(label).is_some(), "localized popup item {label}");
        }
        h.run_steps(12);
        let file = std::path::Path::new(&out).join(format!("smart-filter-menu-{language}-{}x.png", ppp as u32));
        h.render().unwrap().save(&file).unwrap();
        println!("capture {} at {offset:?}, doc {:?}, inactive target {:?}", file.display(), h.state().session.active().unwrap().doc.id, layer);
    }
}
/// Selecting a row is a primary action; its subsequent context edit keeps that filter selected.
#[test]
fn row_menu_reorder_keeps_the_selected_filter_identity() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    for ppp in [1.0, 2.0] {
        for (target, choice) in [("Offset", "Move Up"), ("Gaussian Blur", "Move Down")] {
            let (mut h, layer) = harness_at(8, ppp);
            let at = text_position(&h, "Offset");
            click(&mut h, at, PointerButton::Primary);
            assert_eq!(h.get_by_label("Offset").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
            assert_eq!(h.get_by_label("Gaussian Blur").accesskit_node().toggled(), Some(egui::accesskit::Toggled::False));
            let st = h.state().session.active().unwrap();
            let (active, selected, history) = (st.active_layer, st.selected_layers.clone(), st.history.past_len());
            let at = text_position(&h, target);
            click(&mut h, at, PointerButton::Secondary);
            h.get_by_label(choice).click();
            h.run_steps(4);
            assert_eq!(filters(h.state(), layer).iter().map(|f| f.command.as_str()).collect::<Vec<_>>(), ["filter.blur.gaussianBlur", "filter.other.offset"]);
            assert_eq!(h.get_by_label("Offset").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True), "{target}/{choice} keeps Offset selected");
            assert_eq!(h.get_by_label("Gaussian Blur").accesskit_node().toggled(), Some(egui::accesskit::Toggled::False));
            let st = h.state().session.active().unwrap();
            assert_eq!((st.active_layer, &st.selected_layers, st.history.past_len()), (active, &selected, history + 1));
        }
    }
}

#[test]
fn row_menu_delete_before_selection_keeps_the_remaining_filter_selected() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    for ppp in [1.0, 2.0] {
        let (mut h, layer) = harness_at(8, ppp);
        let at = text_position(&h, "Gaussian Blur");
        click(&mut h, at, PointerButton::Primary);
        assert_eq!(h.get_by_label("Gaussian Blur").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
        let st = h.state().session.active().unwrap();
        let (active, selected, history) = (st.active_layer, st.selected_layers.clone(), st.history.past_len());
        let at = text_position(&h, "Offset");
        click(&mut h, at, PointerButton::Secondary);
        h.get_by_label("Delete Smart Filter").click();
        h.run_steps(4);
        assert_eq!(filters(h.state(), layer).iter().map(|f| f.command.as_str()).collect::<Vec<_>>(), ["filter.blur.gaussianBlur"]);
        assert_eq!(h.get_by_label("Gaussian Blur").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
        let st = h.state().session.active().unwrap();
        assert_eq!((st.active_layer, &st.selected_layers, st.history.past_len()), (active, &selected, history + 1));
    }
}

#[test]
fn row_menu_delete_selected_filter_clears_its_row_selection() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    for ppp in [1.0, 2.0] {
        let (mut h, layer) = harness_at(8, ppp);
        let at = text_position(&h, "Offset");
        click(&mut h, at, PointerButton::Primary);
        assert_eq!(h.get_by_label("Offset").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
        let st = h.state().session.active().unwrap();
        let (active, selected, history) = (st.active_layer, st.selected_layers.clone(), st.history.past_len());
        let at = text_position(&h, "Offset");
        click(&mut h, at, PointerButton::Secondary);
        h.get_by_label("Delete Smart Filter").click();
        h.run_steps(4);
        assert_eq!(filters(h.state(), layer).iter().map(|f| f.command.as_str()).collect::<Vec<_>>(), ["filter.blur.gaussianBlur"]);
        assert_eq!(h.get_by_label("Gaussian Blur").accesskit_node().toggled(), Some(egui::accesskit::Toggled::False));
        let doc = h.state().session.active().unwrap().doc.id;
        assert!(h.ctx.data(|d| d.get_temp::<FilterDrag>(selected_key(doc))).is_none());
        let st = h.state().session.active().unwrap();
        assert_eq!((st.active_layer, &st.selected_layers, st.history.past_len()), (active, &selected, history + 1));
    }
}

#[test]
fn row_menu_refused_edit_preserves_filter_order_and_selection() {
    let _language = crate::i18n::language_scope(crate::i18n::Lang::EN);
    let (mut h, layer) = harness();
    let at = text_position(&h, "Offset");
    click(&mut h, at, PointerButton::Primary);
    assert_eq!(h.get_by_label("Offset").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    let before = h.state().session.active().unwrap().doc.clone();
    let st = h.state().session.active().unwrap();
    let (active, selected, history) = (st.active_layer, st.selected_layers.clone(), st.history.past_len());
    h.state_mut().session.authorize =
        Some(|command, _| if command == "layer.smartFilter.move" { Err(photocraft_engine::EngineError::Other("refused menu edit".into())) } else { Ok(()) });
    let at = text_position(&h, "Offset");
    click(&mut h, at, PointerButton::Secondary);
    h.get_by_label("Move Up").click();
    h.run_steps(4);
    let st = h.state().session.active().unwrap();
    assert!(Arc::ptr_eq(&before, &st.doc));
    assert_eq!((st.active_layer, &st.selected_layers, st.history.past_len()), (active, &selected, history));
    assert_eq!(filters(h.state(), layer).iter().map(|f| f.command.as_str()).collect::<Vec<_>>(), ["filter.other.offset", "filter.blur.gaussianBlur"]);
    assert_eq!(h.get_by_label("Offset").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    assert_eq!(h.get_by_label("Gaussian Blur").accesskit_node().toggled(), Some(egui::accesskit::Toggled::False));
}
