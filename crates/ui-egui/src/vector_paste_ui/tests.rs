use std::cell::{Cell, RefCell};
use std::rc::Rc;

use photocraft_engine::Session;

use super::*;
use crate::{Services, VectorClipboard, VectorClipboardSource};

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="8"><rect width="12" height="8" fill="red"/></svg>"#;

type ClipboardFixture = (PhotocraftApp, Rc<Cell<usize>>, Rc<RefCell<Vec<u8>>>);

fn app_with_clipboard(source: &str) -> ClipboardFixture {
    let reads = Rc::new(Cell::new(0));
    let payload = Rc::new(RefCell::new(source.as_bytes().to_vec()));
    let (count, bytes) = (reads.clone(), payload.clone());
    let services = Services {
        clipboard_get_vector: Some(Box::new(move || {
            count.set(count.get() + 1);
            Ok(Some(VectorClipboard { svg: bytes.borrow().clone(), source: VectorClipboardSource::RegisteredSvg, sequence: Some(1) }))
        })),
        ..Default::default()
    };
    let mut app = PhotocraftApp::new(Session::new(), services);
    app.session.execute("file.new", json!({"width": 64, "height": 64})).unwrap();
    app.sync_views();
    (app, reads, payload)
}

fn open(app: &mut PhotocraftApp) {
    assert_eq!(intercept(app, "edit.paste", &json!({})).unwrap().unwrap()["deferred"], true);
}

#[test]
fn control_menu_invoke_preserves_pending_paste_and_rejects_other_dialogs() {
    let (mut app, reads, clipboard) = app_with_clipboard(SVG);
    open(&mut app);
    app.vector_paste.as_mut().unwrap().mode = PasteMode::SmartObject;
    let before = app.session.active().unwrap().doc.clone();
    let revision = app.session.active().unwrap().revision;
    let history = app.session.active().unwrap().history.past_len();
    let journal = app.session.journal.clone();
    let target = Target::capture(&app).unwrap();
    let view = (app.ui.views[0].center, app.ui.views[0].zoom);
    let pending = app.vector_paste.as_ref().unwrap();
    let payload =
        (pending.source.clone(), pending.mode, pending.center, pending.availability.clone(), pending.prepared.size(), pending.prepared.warnings().to_vec());
    let authorizations = Rc::new(Cell::new(0));
    let authorized = authorizations.clone();
    app.services.automation_command = Some(Box::new(move |_, _| {
        authorized.set(authorized.get() + 1);
        Ok(())
    }));
    let ctx = egui::Context::default();
    for id in ["layer.duplicate", "image.imageSize", "edit.paste", "view.zoomIn"] {
        let (request, _) = crate::control::ControlRequest::new("ui.menu.invoke", json!({"id": id}));
        let crate::control::Outcome::Done(result) = crate::control::handle(&mut app, &ctx, &request) else {
            panic!("blocked menu call must return immediately")
        };
        assert_eq!(result["ok"], false, "{id}: {result}");
        assert!(result.to_string().contains("modal dialog"));
        assert_eq!(authorizations.get(), 0);
        assert_eq!(reads.get(), 1);
        assert_eq!(clipboard.borrow().as_slice(), SVG.as_bytes());
        assert!(app.session.clipboard.is_none());
        assert!(app.ui.dialogs.is_empty());
        let state = app.session.active().unwrap();
        assert_eq!(state.doc, before);
        assert_eq!(state.revision, revision);
        assert_eq!(state.history.past_len(), history);
        assert_eq!(app.session.journal, journal);
        assert_eq!((app.ui.views[0].center, app.ui.views[0].zoom), view);
        let pending = app.vector_paste.as_ref().unwrap();
        assert!(pending.target == target);
        assert_eq!(
            (pending.source.clone(), pending.mode, pending.center, pending.availability.clone(), pending.prepared.size(), pending.prepared.warnings().to_vec()),
            payload
        );
    }
}

#[test]
fn idle_frames_and_unrelated_commands_never_read_clipboard() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui.ctx()));
    output.textures_delta.clear();
    assert!(intercept(&mut app, "edit.copy", &json!({})).is_none());
    assert!(intercept(&mut app, "edit.paste", &json!({"mode": "pixels"})).is_none());
    app.automation_input = true;
    assert!(intercept(&mut app, "edit.paste", &json!({})).is_none());
    assert_eq!(reads.get(), 0);
}

#[test]
fn repeated_paste_and_cancel_do_not_change_document_or_history() {
    for keys in [vec![egui::Key::Escape], vec![egui::Key::Escape, egui::Key::Enter], vec![egui::Key::Enter, egui::Key::Escape]] {
        let (mut app, reads, _) = app_with_clipboard(SVG);
        let before = app.session.active().unwrap().doc.clone();
        let revision = app.session.active().unwrap().revision;
        let journal = app.session.journal.len();
        open(&mut app);
        assert_eq!(app.vector_paste.as_ref().unwrap().mode, PasteMode::Pixels);
        open(&mut app);
        assert_eq!(reads.get(), 1);
        let ctx = egui::Context::default();
        PhotocraftApp::setup_context(&ctx, Default::default());
        let mut input = egui::RawInput::default();
        input.events.extend(keys.into_iter().map(|key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }));
        let mut output = ctx.run_ui(input, |ui| show(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert!(app.vector_paste.is_none());
        let st = app.session.active().unwrap();
        assert_eq!(st.doc, before);
        assert_eq!(st.revision, revision);
        assert!(!st.history.can_undo());
        assert_eq!(app.session.journal.len(), journal);
    }
}

fn prepare_cas_layer(app: &mut PhotocraftApp) {
    app.session.execute("layer.new.layer", json!({})).unwrap();
    app.session.execute("select.rect", json!({"x":8,"y":8,"width":40,"height":24})).unwrap();
    app.session.execute("edit.fill", json!({"color":"#c03020"})).unwrap();
    app.session.execute("select.deselect", json!({})).unwrap();
    app.sync_views();
}

fn begin_resized_cas(app: &mut PhotocraftApp) {
    let ctx = egui::Context::default();
    crate::cas_transform::begin(app, &ctx).unwrap();
    let shift = egui::Modifiers { shift: true, ..Default::default() };
    assert!(crate::transform_tool::pointer(app, crate::canvas::ToolEvent::Down { x: 48.0, y: 20.0, pressure: 1.0 }, shift));
    crate::transform_tool::pointer(app, crate::canvas::ToolEvent::Move { x: 32.0, y: 20.0, pressure: 1.0 }, shift);
    crate::transform_tool::pointer(app, crate::canvas::ToolEvent::Up { x: 32.0, y: 20.0 }, shift);
    let t = app.ui.transform.as_ref().unwrap();
    assert!(t.content_aware.is_some());
    assert_eq!(t.quad, [[8.0, 8.0], [32.0, 8.0], [32.0, 32.0], [8.0, 32.0]]);
    assert!(app.transform_preview.as_ref().unwrap().content_aware.is_some());
}

#[test]
fn resized_cas_rejects_paste_before_clipboard_read_then_cancel_allows_paste() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    prepare_cas_layer(&mut app);
    begin_resized_cas(&mut app);
    let before = app.session.active().unwrap().doc.clone();
    let active = app.session.active().unwrap().active_layer;
    let revision = app.session.active().unwrap().revision;
    let history = app.session.active().unwrap().history.past_len();
    let transform = app.ui.transform.clone();
    let preview = app.transform_preview.as_ref().unwrap();
    let (preview_doc, preview_session, preview_size) = (preview.doc.clone(), preview.session, preview.texture.size());

    assert_eq!(intercept(&mut app, "edit.paste", &json!({})).unwrap().unwrap_err(), "Commit or cancel the transform first");
    assert_eq!(reads.get(), 0);
    assert!(app.vector_paste.is_none());
    assert_eq!(app.session.active().unwrap().doc, before);
    assert_eq!(app.session.active().unwrap().active_layer, active);
    assert_eq!(app.session.active().unwrap().revision, revision);
    assert_eq!(app.session.active().unwrap().history.past_len(), history);
    assert_eq!(app.ui.transform, transform);
    let preview = app.transform_preview.as_ref().unwrap();
    assert!(std::sync::Arc::ptr_eq(&preview.doc, &preview_doc));
    assert_eq!((preview.session, preview.texture.size()), (preview_session, preview_size));
    assert!(preview.content_aware.is_some());

    crate::transform_tool::cancel(&mut app);
    open(&mut app);
    confirm(&mut app).unwrap();
    let st = app.session.active().unwrap();
    assert_eq!(reads.get(), 1);
    assert_eq!(st.doc.layer_count(), before.layer_count() + 1);
    assert_eq!(st.doc.layer(active.unwrap()), before.layer(active.unwrap()));
    assert_eq!(st.history.past_len(), history + 1);
    assert!(app.vector_paste.is_none() && app.ui.transform.is_none() && app.transform_preview.is_none());
}

#[test]
fn deferred_paste_waits_for_cas_cancel_without_losing_captured_payload() {
    let (mut app, reads, payload) = app_with_clipboard(SVG);
    prepare_cas_layer(&mut app);
    open(&mut app);
    let captured = app.vector_paste.as_ref().unwrap().source.clone();
    begin_resized_cas(&mut app);
    let before = app.session.active().unwrap().doc.clone();
    let active = app.session.active().unwrap().active_layer;
    let revision = app.session.active().unwrap().revision;
    let history = app.session.active().unwrap().history.past_len();
    let transform = app.ui.transform.clone();
    let preview = app.transform_preview.as_ref().unwrap();
    let (preview_doc, preview_session, preview_size) = (preview.doc.clone(), preview.session, preview.texture.size());
    *payload.borrow_mut() = b"not the captured SVG".to_vec();

    assert_eq!(confirm(&mut app).unwrap_err(), "Commit or cancel the transform first");
    assert_eq!(app.vector_paste.as_ref().unwrap().source, captured);
    assert_eq!(reads.get(), 1);
    assert_eq!(app.session.active().unwrap().doc, before);
    assert_eq!(app.session.active().unwrap().active_layer, active);
    assert_eq!(app.session.active().unwrap().revision, revision);
    assert_eq!(app.session.active().unwrap().history.past_len(), history);
    assert_eq!(app.ui.transform, transform);
    let preview = app.transform_preview.as_ref().unwrap();
    assert!(std::sync::Arc::ptr_eq(&preview.doc, &preview_doc));
    assert_eq!((preview.session, preview.texture.size()), (preview_session, preview_size));
    assert!(preview.content_aware.is_some());

    crate::transform_tool::cancel(&mut app);
    confirm(&mut app).unwrap();
    let st = app.session.active().unwrap();
    assert_eq!(reads.get(), 1);
    assert_eq!(st.doc.layer_count(), before.layer_count() + 1);
    assert_eq!(st.doc.layer(active.unwrap()), before.layer(active.unwrap()));
    assert_eq!(st.history.past_len(), history + 1);
    assert!(app.vector_paste.is_none() && app.ui.transform.is_none() && app.transform_preview.is_none());
}

#[test]
fn ordinary_transform_keeps_existing_paste_as_routing() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    prepare_cas_layer(&mut app);
    crate::transform_tool::begin(&mut app, &egui::Context::default()).unwrap();
    assert!(app.ui.transform.as_ref().unwrap().content_aware.is_none());
    open(&mut app);
    assert_eq!(reads.get(), 1);
    assert!(app.vector_paste.is_some() && app.ui.transform.is_some());
}

#[test]
fn idle_cam_box_rejects_paste_before_clipboard_read_then_cancel_allows_paste() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    app.session.execute("select.rect", json!({"x":8,"y":8,"width":12,"height":8})).unwrap();
    app.ui.tool = crate::state::Tool::ContentAwareMove;
    assert!(crate::cam_transform::begin(&mut app, [8, 4]));
    assert!(app.drag.is_none() && app.patch_preview.is_none() && app.ui.transform.is_none());
    let before = app.session.active().unwrap().doc.clone();
    let revision = app.session.active().unwrap().revision;
    let history = app.session.active().unwrap().history.past_len();
    let cam = app.ui.cam_transform.clone();

    let error = intercept(&mut app, "edit.paste", &json!({})).unwrap().unwrap_err();
    assert_eq!(error, "Commit or cancel the transform first");
    assert_eq!(reads.get(), 0);
    assert!(app.vector_paste.is_none());
    assert_eq!(app.session.active().unwrap().doc, before);
    assert_eq!(app.session.active().unwrap().revision, revision);
    assert_eq!(app.session.active().unwrap().history.past_len(), history);
    assert_eq!(app.ui.cam_transform, cam);

    crate::cam_transform::cancel(&mut app);
    open(&mut app);
    confirm(&mut app).unwrap();
    let st = app.session.active().unwrap();
    assert_eq!(reads.get(), 1);
    assert_eq!(st.doc.layer_count(), before.layer_count() + 1);
    assert_eq!(st.doc.selection, before.selection);
    assert_eq!(st.history.past_len(), history + 1);
    assert!(app.vector_paste.is_none() && app.ui.cam_transform.is_none());
}

#[test]
fn deferred_paste_waits_for_cam_cancel_without_losing_captured_payload() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    app.session.execute("select.rect", json!({"x":8,"y":8,"width":12,"height":8})).unwrap();
    app.ui.tool = crate::state::Tool::ContentAwareMove;
    open(&mut app);
    assert!(crate::cam_transform::begin(&mut app, [8, 4]));
    let before = app.session.active().unwrap().doc.clone();
    let revision = app.session.active().unwrap().revision;
    let history = app.session.active().unwrap().history.past_len();
    let cam = app.ui.cam_transform.clone();

    assert_eq!(confirm(&mut app).unwrap_err(), "Commit or cancel the transform first");
    assert!(app.vector_paste.is_some());
    assert_eq!(reads.get(), 1);
    assert_eq!(app.session.active().unwrap().doc, before);
    assert_eq!(app.session.active().unwrap().revision, revision);
    assert_eq!(app.session.active().unwrap().history.past_len(), history);
    assert_eq!(app.ui.cam_transform, cam);

    crate::cam_transform::cancel(&mut app);
    confirm(&mut app).unwrap();
    assert_eq!(reads.get(), 1);
    assert_eq!(app.session.active().unwrap().doc.layer_count(), before.layer_count() + 1);
    assert_eq!(app.session.active().unwrap().history.past_len(), history + 1);
    assert!(app.vector_paste.is_none());
}

#[test]
fn confirm_uses_captured_snapshot_and_one_undo_step() {
    let (mut app, reads, payload) = app_with_clipboard(SVG);
    let before = app.session.active().unwrap().doc.clone();
    open(&mut app);
    *payload.borrow_mut() = b"not SVG anymore".to_vec();
    confirm(&mut app).unwrap();
    assert_eq!(reads.get(), 1);
    assert!(app.vector_paste.is_none());
    let after = app.session.active().unwrap().doc.clone();
    assert_ne!(after, before);
    assert_eq!(app.session.journal.last().unwrap().0, "edit.pasteVector");
    app.session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(app.session.active().unwrap().doc, before);
    assert!(!app.session.active().unwrap().history.can_undo());
    app.session.execute("edit.redo", json!({})).unwrap();
    assert_eq!(app.session.active().unwrap().doc, after);
}

#[test]
fn stale_document_and_view_targets_fail_without_paste() {
    for change in 0..5 {
        let (mut app, _, _) = app_with_clipboard(SVG);
        open(&mut app);
        match change {
            0 => app.session.execute("layer.new.layer", json!({})).map(|_| ()).unwrap(),
            1 => app.ui.mask_target = true,
            2 => app.ui.vector_mask_target = true,
            3 => app.session.active_mut().unwrap().channel_view.target = ChannelTarget::Color(0),
            _ => app.ui.selected_path = Some("work".into()),
        }
        let before_confirm = app.session.active().unwrap().doc.clone();
        assert!(confirm(&mut app).unwrap_err().contains("changed"));
        assert_eq!(app.session.active().unwrap().doc, before_confirm);
        assert!(app.vector_paste.is_none());
    }
}

#[test]
fn mask_and_channel_targets_keep_legacy_path_without_vector_read() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    app.ui.mask_target = true;
    assert!(intercept(&mut app, "edit.paste", &json!({})).is_none());
    app.ui.mask_target = false;
    app.ui.vector_mask_target = true;
    assert!(intercept(&mut app, "edit.paste", &json!({})).is_none());
    app.ui.vector_mask_target = false;
    app.session.active_mut().unwrap().channel_view.target = ChannelTarget::Alpha(0);
    assert!(intercept(&mut app, "edit.paste", &json!({})).is_none());
    assert_eq!(reads.get(), 0);
}

#[test]
fn implicit_mask_channel_visibility_keeps_legacy_paste_until_composite_is_restored() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    let st = app.session.active_mut().unwrap();
    let layer = st.active_layer.unwrap();
    std::sync::Arc::make_mut(&mut st.doc).layer_mut(layer).unwrap().mask = Some(photocraft_doc::LayerMask::reveal_all());
    st.channel_view.color_hidden = vec![true; photocraft_engine::channel_cmds::color_count(&st.doc)];
    st.channel_view.alpha_visible.clear();
    assert_eq!(st.channel_view.target, ChannelTarget::Composite);
    assert!(st.channel_view.layer_mask.is_none());
    assert!(!app.ui.mask_target);
    assert!(!app.session.tools.mask_targeted);
    assert!(app.layer_mask_targeted());
    let before = app.session.active().unwrap().doc.clone();

    assert!(intercept(&mut app, "edit.paste", &json!({})).is_none());
    assert_eq!(reads.get(), 0);
    assert!(app.vector_paste.is_none());
    assert_eq!(app.session.active().unwrap().doc, before);

    app.session.active_mut().unwrap().channel_view.color_hidden.clear();
    assert!(!app.layer_mask_targeted());
    open(&mut app);
    assert_eq!(reads.get(), 1);
    assert!(app.vector_paste.is_some());
}

#[test]
fn explicit_mask_channel_roundtrip_restores_paste_as_without_removing_masks() {
    for channel in ["mask", "vectorMask"] {
        let (mut app, reads, _) = app_with_clipboard(SVG);
        app.session.execute("layer.layerMask.revealAll", json!({})).unwrap();
        app.session.execute("layer.vectorMask.revealAll", json!({})).unwrap();
        let before = app.session.active().unwrap().doc.clone();

        app.run("channel.target", json!({"channel":channel})).unwrap();
        assert_eq!(app.ui.mask_target, channel == "mask");
        assert_eq!(app.ui.vector_mask_target, channel == "vectorMask");
        assert!(intercept(&mut app, "edit.paste", &json!({})).is_none());
        assert_eq!(reads.get(), 0);

        app.run("channel.target", json!({"channel":"composite"})).unwrap();
        assert_eq!(app.session.active().unwrap().channel_view.target, ChannelTarget::Composite);
        assert!(!app.ui.mask_target);
        assert!(!app.ui.vector_mask_target);
        assert!(!app.session.tools.mask_targeted);
        assert_eq!(app.session.active().unwrap().doc, before);
        assert_eq!(app.run("edit.paste", json!({})).unwrap()["deferred"], true);
        assert!(app.vector_paste.is_some());
        assert_eq!(reads.get(), 1);
    }
}

#[test]
fn thumbnail_mask_selection_survives_view_sync_and_rejected_channel_target() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    app.run("layer.layerMask.revealAll", json!({})).unwrap();
    assert_eq!(app.session.active().unwrap().channel_view.target, ChannelTarget::Composite);
    app.sync_views();
    assert!(app.ui.mask_target);
    assert!(app.session.tools.mask_targeted);
    assert!(app.run("channel.target", json!({"channel":"missing channel"})).is_err());
    assert!(app.ui.mask_target);
    assert!(app.session.tools.mask_targeted);
    assert!(intercept(&mut app, "edit.paste", &json!({})).is_none());
    assert_eq!(reads.get(), 0);
}

#[test]
fn absent_vector_payload_preserves_legacy_path() {
    let (mut app, _, _) = app_with_clipboard(SVG);
    app.services.clipboard_get_vector = Some(Box::new(|| Ok(None)));
    assert!(intercept(&mut app, "edit.paste", &json!({})).is_none());
    assert!(app.vector_paste.is_none());
}

#[test]
fn null_params_open_prompt_but_other_wrong_types_do_not_read() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    for params in [json!(false), json!(""), json!([]), json!(0)] {
        assert!(intercept(&mut app, "edit.paste", &params).is_none());
    }
    assert_eq!(reads.get(), 0);
    assert!(intercept(&mut app, "edit.paste", &Value::Null).unwrap().is_ok());
    assert_eq!(reads.get(), 1);
    assert!(app.vector_paste.is_some());
}

#[test]
fn path_confirm_selects_imported_saved_path() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    open(&mut app);
    app.vector_paste.as_mut().unwrap().mode = PasteMode::Path;
    let result = confirm(&mut app).unwrap();
    let name = result["path"].as_str().unwrap();
    assert_eq!(app.ui.selected_path.as_deref(), Some(name));
    assert!(app.session.active().unwrap().doc.paths.iter().any(|path| path.name == name));
    assert_eq!(reads.get(), 1);
}

#[test]
fn malformed_svg_and_clipboard_errors_do_not_create_history() {
    let (mut app, _, _) = app_with_clipboard("<svg><");
    assert!(intercept(&mut app, "edit.paste", &json!({})).unwrap().is_err());
    assert!(app.vector_paste.is_none());
    assert!(!app.session.active().unwrap().history.can_undo());
    app.services.clipboard_get_vector = Some(Box::new(|| Err("Clipboard is busy".into())));
    assert_eq!(intercept(&mut app, "edit.paste", &json!({})).unwrap().unwrap_err(), "Clipboard is busy");
    assert!(app.ui.status_error);
}

#[test]
fn warnings_and_capabilities_are_kept_in_pending_snapshot() {
    let (mut app, _, _) = app_with_clipboard(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><defs><filter id="blur"><feGaussianBlur stdDeviation="1"/></filter></defs><rect width="12" height="12" filter="url(#blur)"/></svg>"#,
    );
    open(&mut app);
    let pending = app.vector_paste.as_ref().unwrap();
    assert!(!pending.prepared.warnings().is_empty());
    assert!(pending.prepared.capability(PasteMode::Pixels).is_ok());
    assert!(pending.prepared.capability(PasteMode::Path).is_err());
}

#[test]
fn german_paste_dialog_strings_have_translations() {
    let de = crate::i18n::lang_from_tag("de").unwrap();
    for source in ["Paste as:", "SVG text is converted to outlines, not editable type.", "Import warnings:"] {
        assert_ne!(crate::i18n::tr_ctx(de, "vectorPaste", source), source);
    }
    for mode in MODES {
        assert!(!crate::i18n::tr_ctx(de, "vectorPaste", mode.label()).is_empty());
    }
}

#[test]
fn vector_only_service_enables_paste_without_reading_clipboard() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    assert!(crate::menus::is_enabled(&app, "edit.paste"));
    assert!(!crate::menus::is_enabled(&app, "file.newFromClipboard"));
    assert_eq!(reads.get(), 0);
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, "edit.paste", json!({})).unwrap();
    assert!(app.vector_paste.is_some());
    assert_eq!(reads.get(), 1);
}

#[test]
fn save_options_reject_paste_before_reading_clipboard() {
    for targa in [false, true] {
        let (mut app, reads, _) = app_with_clipboard(SVG);
        if targa {
            crate::targa_options_ui::park(&mut app, "synthetic.tga".into(), false, "file.saveAs").unwrap();
        } else {
            crate::tiff_options_ui::park(&mut app, "synthetic.tif".into()).unwrap();
        }
        let before = app.session.active().unwrap().doc.clone();
        let ctx = egui::Context::default();
        let error = crate::menus::invoke(&mut app, &ctx, "edit.paste", json!({})).unwrap_err();
        assert!(error.contains("Answer TIFF or Targa Options"));
        assert_eq!(reads.get(), 0);
        assert!(app.vector_paste.is_none());
        assert!(app.save_options_open());
        assert_eq!(app.session.active().unwrap().doc, before);
        assert!(!app.session.active().unwrap().history.can_undo());
    }
}

#[test]
fn pending_vector_paste_rejects_new_and_deferred_save_prompts() {
    let (mut app, reads, _) = app_with_clipboard(SVG);
    open(&mut app);
    for path in ["synthetic.tga", "synthetic.tif"] {
        assert!(app.save_as(Some(path.into())).unwrap_err().contains("Answer Paste As"));
        // A native save picker may finish after another command has opened Paste As.
        assert!(app.save_to(path.into(), None).unwrap_err().contains("Answer Paste As"));
        assert!(app.vector_paste.is_some());
        assert!(!app.save_options_open());
    }
    assert_eq!(reads.get(), 1);
    assert!(!app.session.active().unwrap().history.can_undo());
}

#[test]
fn automated_mask_trash_keeps_pending_paste_without_opening_another_dialog() {
    for (create, delete, vector) in
        [("layer.layerMask.revealAll", "layer.layerMask.delete", false), ("layer.vectorMask.revealAll", "layer.vectorMask.delete", true)]
    {
        let (mut app, reads, _) = app_with_clipboard(SVG);
        app.session.execute(create, json!({})).unwrap();
        let layer = app.session.active().unwrap().active_layer.unwrap();
        let before = app.session.active().unwrap().doc.clone();
        let past = app.session.active().unwrap().history.past_len();
        open(&mut app);
        let pending_source = app.vector_paste.as_ref().unwrap().source.clone();
        let authorized = Rc::new(Cell::new(0));
        let count = authorized.clone();
        app.services.automation_command = Some(Box::new(move |id, _| {
            assert_eq!(id, delete);
            count.set(count.get() + 1);
            Ok(())
        }));

        app.automation_input = true;
        let result = app.run(delete, json!({"layer": layer.0, "__maskTrash": true})).unwrap();
        app.automation_input = false;

        assert!(result.get("confirmationPending").is_none());
        assert!(app.ui.dialogs.is_empty());
        assert_eq!(authorized.get(), 1, "automation must reach its command authorization");
        assert_eq!(app.vector_paste.as_ref().unwrap().source, pending_source);
        assert_eq!(reads.get(), 1);
        let st = app.session.active().unwrap();
        let after = st.doc.layer(layer).unwrap();
        assert!(if vector { after.vector_mask.is_none() } else { after.mask.is_none() });
        assert_eq!(st.history.past_len(), past + 1);
        app.session.undo();
        assert_eq!(app.session.active().unwrap().doc, before);
    }
}

#[test]
fn paste_shortcut_during_save_options_keeps_the_save_prompt() {
    for targa in [false, true] {
        let (mut app, reads, _) = app_with_clipboard(SVG);
        if targa {
            crate::targa_options_ui::park(&mut app, "synthetic.tga".into(), false, "file.saveAs").unwrap();
        } else {
            crate::tiff_options_ui::park(&mut app, "synthetic.tif".into()).unwrap();
        }
        let before = app.session.active().unwrap().doc.clone();
        let journal = app.session.journal.len();
        let modifiers = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key { key: egui::Key::V, physical_key: None, pressed: true, repeat: false, modifiers },
            ],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(input, |ui| crate::shortcuts::handle(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert_eq!(reads.get(), 0);
        assert!(app.vector_paste.is_none());
        assert!(app.save_options_open());
        assert_eq!(app.session.active().unwrap().doc, before);
        assert!(!app.session.active().unwrap().history.can_undo());
        assert_eq!(app.session.journal.len(), journal);
        assert!(crate::shortcut_dispatch::take_log(&ctx).is_empty());
    }
}

#[test]
fn availability_uses_the_captured_view_center_and_destination_budget() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="300000" height="300000"/></svg>"#;
    let (mut app, reads, _) = app_with_clipboard(svg);
    app.session.execute("file.new", json!({"width":8000,"height":8000})).unwrap();
    app.sync_views();
    let index = app.session.active_index().unwrap();
    app.ui.views[index].center = [1000.0, 1000.0];
    open(&mut app);
    let pending = app.vector_paste.as_ref().unwrap();
    assert_eq!(pending.center, [1000.0, 1000.0]);
    assert!(pending.capability(PasteMode::Layers).is_err());
    assert!(pending.capability(PasteMode::Shape).is_err());
    assert!(pending.capability(PasteMode::Pixels).is_ok());
    assert!(pending.capability(PasteMode::SmartObject).is_ok());
    assert_eq!(reads.get(), 1);
}

/// Explicit visual evidence, rendered without native windows or the user's clipboard.
#[test]
fn capture_vector_paste_visual_fixtures() {
    let Some(directory) = std::env::var_os("CRAFT_VECTOR_PASTE_FIXTURES").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&directory).unwrap();
    for theme in crate::theme::ThemeKind::ALL {
        for (width, height) in [(800.0, 480.0), (1280.0, 720.0)] {
            for language in ["en", "de"] {
                let (mut app, reads, _) = app_with_clipboard(SVG);
                open(&mut app);
                let mut ready = false;
                let lang = crate::i18n::lang_from_tag(language).unwrap();
                let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(width, height)).wgpu().build_ui_state(
                    move |ui, app: &mut PhotocraftApp| {
                        let _language = crate::i18n::language_scope(lang);
                        if !ready {
                            PhotocraftApp::setup_context(ui.ctx(), theme);
                            ready = true;
                            return;
                        }
                        show(app, ui.ctx());
                    },
                    app,
                );
                harness.ctx.set_os(egui::os::OperatingSystem::Windows);
                harness.run_steps(5);
                assert_eq!(reads.get(), 1);
                assert!(harness.state().vector_paste.is_some());
                harness.render().unwrap().save(directory.join(format!("photocraft-vector-paste-{}-{width}-{height}-{language}.png", theme.id()))).unwrap();
            }
        }
    }
}
