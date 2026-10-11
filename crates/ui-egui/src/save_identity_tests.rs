//! Save identity belongs to the editable document, after a successful write only.

use crate::{ExportSettings, PhotocraftApp, Services, menus};
use photocraft_engine::Session;
use serde_json::json;
use std::{cell::RefCell, rc::Rc};

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(
        Session::new(),
        Services {
            import: Some(Box::new(|name, bytes, max_svg_group_depth| {
                photocraft_io::import_with_svg_group_depth(name, bytes, max_svg_group_depth).map(|r| (r.document, r.warnings)).map_err(|e| e.to_string())
            })),
            export: Some(Box::new(|doc, path, _| {
                photocraft_io::export(doc, path, &Default::default()).map(|r| (r.bytes, r.warnings)).map_err(|e| e.to_string())
            })),
            write: Some(Box::new(|_, _| Ok(()))),
            automation_write: Some(Box::new(|_, _| Ok(()))),
            ..Default::default()
        },
    );
    app.run("file.new", json!({"width": 4, "height": 4, "name": "Untitled-1"})).unwrap();
    app.run("layer.new.layer", json!({})).unwrap();
    app
}

fn identity(app: &PhotocraftApp) -> (String, Option<String>, u64, u64) {
    let st = app.session.active().unwrap();
    (st.doc.name.clone(), st.path.clone(), st.revision, st.saved_revision)
}

#[test]
fn new_document_psd_and_subsequent_save_as_adopt_the_written_file_name() {
    for depth in [8, 16, 32] {
        let mut app = app();
        app.run("file.new", json!({"width": 4, "height": 4, "depth": depth, "name": "Untitled-1"})).unwrap();
        app.run("layer.new.layer", json!({})).unwrap();
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let written = bytes.clone();
        app.services.write = Some(Box::new(move |_, data| {
            *written.borrow_mut() = data.to_vec();
            Ok(())
        }));
        let id = app.session.active().unwrap().doc.id;
        let revision = app.session.active().unwrap().revision;
        let ctx = egui::Context::default();
        menus::invoke(&mut app, &ctx, "file.saveAs", json!({"path": "作品/My image.PSD"})).unwrap();
        assert_eq!(identity(&app), ("My image.PSD".into(), Some("作品/My image.PSD".into()), revision, revision));
        assert_eq!(app.session.active().unwrap().doc.id, id);
        let reopened = photocraft_io::import("My image.PSD", &bytes.borrow()).unwrap();
        assert_eq!(reopened.document.size, app.session.active().unwrap().doc.size);
        assert_eq!(reopened.document.depth.bits(), depth);
        menus::invoke(&mut app, &ctx, "file.saveAs", json!({"path": "作品/次の画像.psd"})).unwrap();
        assert_eq!(app.session.active().unwrap().doc.name, "次の画像.psd");
        menus::invoke(&mut app, &ctx, "file.save", json!({})).unwrap();
        assert_eq!(app.session.active().unwrap().path.as_deref(), Some("作品/次の画像.psd"));
        app.open_bytes("existing.psd", &bytes.borrow()).unwrap();
        let id = app.active_doc_id().unwrap();
        assert_eq!(app.session.active().unwrap().doc.name, "existing.psd");
        app.save_as(Some("renamed existing.psd".into())).unwrap();
        assert_eq!(app.active_doc_id().unwrap(), id);
        assert_eq!(app.session.active().unwrap().doc.name, "renamed existing.psd");
    }
}

#[test]
fn failed_encode_or_write_keeps_the_previous_name_path_and_dirty_state() {
    for saved_before in [false, true] {
        let mut app = app();
        if saved_before {
            app.save_as(Some("before.psd".into())).unwrap();
            app.run("layer.new.layer", json!({})).unwrap();
        }
        let before = identity(&app);
        app.services.write = Some(Box::new(|_, _| Err("disk full".into())));
        assert!(app.save_as(Some("after.psd".into())).is_err());
        assert_eq!(identity(&app), before);
        assert!(app.save_automation(Some("after.psd".into())).is_ok());
        app.run("layer.new.layer", json!({})).unwrap();
        let before = identity(&app);
        app.services.automation_write = Some(Box::new(|_, _| Err("denied".into())));
        assert!(app.save_automation(Some("another.psd".into())).is_err());
        assert_eq!(identity(&app), before);
        app.services.export = Some(Box::new(|_, _, _| Err("cannot encode".into())));
        assert!(app.save_as(Some("after.psd".into())).is_err());
        assert_eq!(identity(&app), before);
    }
}

#[test]
fn save_copy_and_export_keep_the_editable_document_identity() {
    let mut app = app();
    app.save_as(Some("original.psd".into())).unwrap();
    app.run("layer.new.layer", json!({})).unwrap();
    let before = identity(&app);
    app.write_document("copy.psd".into(), &ExportSettings::default(), true).unwrap();
    assert_eq!(identity(&app), before);
    let (dialog, _) = crate::file_dialog::fake(vec![Some(crate::file_dialog::FileDialogAnswer::SaveTo("export.png".into()))]);
    app.services.file_dialog = Some(dialog);
    let ctx = egui::Context::default();
    crate::export_dialog::confirm(&mut app, json!({"format": "png", "scale": 100}).as_object().unwrap()).unwrap();
    app.poll_file_dialog(&ctx, None);
    assert_eq!(identity(&app), before);
    assert!(app.ui.status.starts_with("Exported export.png"), "{}", app.ui.status);
}

#[test]
fn web_download_and_automation_saves_use_the_same_identity_completion() {
    let mut app = app();
    let (dialog, _) = crate::file_dialog::fake(vec![Some(crate::file_dialog::FileDialogAnswer::SaveTo("Untitled-1.psd".into()))]);
    app.services.file_dialog = Some(dialog);
    app.save_as(None).unwrap();
    app.poll_file_dialog(&egui::Context::default(), None);
    assert_eq!(app.session.active().unwrap().doc.name, "Untitled-1.psd");
    app.save_automation(Some("保存/agent save.psd".into())).unwrap();
    assert_eq!(app.session.active().unwrap().doc.name, "agent save.psd");
}

/// Save As to a flat format writes a copy when the flat file can't hold the document: its layers,
/// or the layered file it already lives in. The document keeps its file, so Save still writes the
/// layered original and the edits stay unsaved (#2550).
#[test]
fn save_as_flat_keeps_the_layered_original() {
    let ctx = egui::Context::default();
    // A layered document, and a one-layer one (the issue's case: the dialog offered a JPEG).
    for layered in [true, false] {
        let mut app = app();
        if !layered {
            app.run("file.new", json!({"width": 4, "height": 4, "name": "Untitled-2"})).unwrap();
        }
        let written = Rc::new(RefCell::new(Vec::new()));
        let w = written.clone();
        app.services.write = Some(Box::new(move |path, _| {
            w.borrow_mut().push(path.to_string());
            Ok(())
        }));
        menus::invoke(&mut app, &ctx, "file.saveAs", json!({"path": "art.psd"})).unwrap();
        app.run("edit.fill", json!({"color": "#000000"})).unwrap();
        let before = identity(&app);
        for flat in ["art.jpg", "art.png"] {
            menus::invoke(&mut app, &ctx, "file.saveAs", json!({"path": flat})).unwrap();
            assert_eq!(identity(&app), before, "{flat}, layered: {layered}");
        }
        menus::invoke(&mut app, &ctx, "file.save", json!({})).unwrap();
        assert_eq!(*written.borrow(), ["art.psd", "art.jpg", "art.png", "art.psd"]);
        let st = app.session.active().unwrap();
        assert_eq!(st.saved_revision, st.revision);
    }
    // An untitled layered document saved as a JPEG is still untitled and unsaved.
    let mut app = app();
    let before = identity(&app);
    app.save_as(Some("flat.jpg".into())).unwrap();
    assert_eq!(identity(&app), before);
}

/// A flat document that isn't in a layered file becomes the flat file it is saved as, as before.
#[test]
fn save_as_flat_retargets_a_flat_document() {
    let mut app = app();
    app.run("file.new", json!({"width": 4, "height": 4, "name": "Untitled-2"})).unwrap();
    app.save_as(Some("shot.png".into())).unwrap();
    assert_eq!(app.session.active().unwrap().path.as_deref(), Some("shot.png"));
    app.save_as(Some("shot.jpg".into())).unwrap();
    let st = app.session.active().unwrap();
    assert_eq!((st.doc.name.as_str(), st.path.as_deref()), ("shot.jpg", Some("shot.jpg")));
    assert_eq!(st.saved_revision, st.revision);
}

/// `app.save` (control channel, MCP bridge) to a flat format is a copy, as in the headless server
/// (#1547); only a layered format becomes the document's file (#2579).
#[test]
fn automation_flat_save_is_a_copy() {
    let mut app = app();
    let before = identity(&app);
    for flat in ["flat.png", "flat.jpg", "flat.tif"] {
        app.save_automation(Some(flat.into())).unwrap();
        assert_eq!(identity(&app), before, "{flat}");
    }
    app.save_automation(Some("layers.psd".into())).unwrap();
    let st = app.session.active().unwrap();
    assert_eq!((st.doc.name.as_str(), st.path.as_deref()), ("layers.psd", Some("layers.psd")));
    assert_eq!(st.saved_revision, st.revision);
}

/// A flat 16-bit RGB TIFF, as another app writes the copy it hands over for editing.
fn handoff_tiff() -> Vec<u8> {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 4, "height": 4, "depth": 16, "name": "beach-Edit"})).unwrap();
    let doc = (*s.active().unwrap().doc).clone();
    assert!(!photocraft_io::tiff_layers::would_write_layers(&doc), "a lone Background: a flat TIFF");
    photocraft_io::export(&doc, "beach-Edit.tif", &Default::default()).unwrap().bytes
}

/// What the write service was given, in order.
type Writes = Rc<RefCell<Vec<(String, Vec<u8>)>>>;

/// [`app`] with `path` opened from [`handoff_tiff`], as an edit copy another app handed over
/// (`in_place`) or as any other file, and its writes recorded.
fn app_with_tiff(path: &str, in_place: bool) -> (PhotocraftApp, Writes) {
    let mut app = app();
    let writes: Writes = Default::default();
    let record = writes.clone();
    app.services.write = Some(Box::new(move |path, data| {
        record.borrow_mut().push((path.to_string(), data.to_vec()));
        Ok(())
    }));
    // as the desktop app's exporter does: the keep-layers setting reaches the TIFF writer
    app.services.export = Some(Box::new(|doc, path, settings| {
        let opts = photocraft_io::ExportOptions { tiff_layers: settings.tiff_layers, ..Default::default() };
        photocraft_io::export(doc, path, &opts).map(|r| (r.bytes, r.warnings)).map_err(|e| e.to_string())
    }));
    if in_place {
        app.in_place_opens.insert(path.to_string());
    }
    app.open_file(path, &handoff_tiff()).unwrap();
    (app, writes)
}

/// File › Save on an edit copy another app handed over (`PHOTOCRAFT_EDIT_IN_PLACE`) writes back to
/// that TIFF: no Save As, the layers kept in the file, the depth unchanged, the document clean.
#[test]
fn a_handed_over_tiff_is_saved_back_in_place() {
    let ctx = egui::Context::default();
    let path = "照片/beach-Edit.tif";
    let (mut app, writes) = app_with_tiff(path, true);
    let (dialog, asked) = crate::file_dialog::fake(vec![]);
    app.services.file_dialog = Some(dialog);
    assert!(app.session.active().unwrap().edit_in_place);
    assert!(app.in_place_opens.is_empty(), "the mark is for that one open");

    // untouched: the same flat file again
    menus::invoke(&mut app, &ctx, "file.save", json!({})).unwrap();
    // edited: a layer, which the TIFF keeps
    app.run("layer.new.layer", json!({})).unwrap();
    assert!(app.session.active().unwrap().is_dirty());
    menus::invoke(&mut app, &ctx, "file.save", json!({})).unwrap();

    assert!(asked.borrow().is_empty(), "no Save As dialog");
    let writes = writes.borrow();
    assert_eq!(writes.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), [path, path]);
    let flat = photocraft_io::import("beach-Edit.tif", &writes[0].1).unwrap().document;
    assert_eq!((flat.layers.len(), flat.depth.bits()), (1, 16));
    let layered = photocraft_io::import("beach-Edit.tif", &writes[1].1).unwrap().document;
    assert_eq!((layered.layers.len(), layered.depth.bits()), (2, 16), "the new layer is in the file");
    let st = app.session.active().unwrap();
    assert_eq!((st.path.as_deref(), st.is_dirty(), st.edit_in_place), (Some(path), false, true));
    assert_eq!(st.doc.name, "beach-Edit.tif");
}

/// The same TIFF opened any other way keeps the rule for flat files: Save asks where to save, so
/// an original is never written over (#416). So does the next file opened after a handed-over one.
#[test]
fn a_tiff_opened_normally_still_goes_through_save_as() {
    let ctx = egui::Context::default();
    let (mut app, writes) = app_with_tiff("photos/beach.tif", false);
    assert!(!app.session.active().unwrap().edit_in_place);
    assert_eq!(app.save_target(), None);
    assert!(app.replay_save("file.save", &json!({})).is_err(), "a recorded Save doesn't write over it either");
    let (dialog, asked) = crate::file_dialog::fake(vec![None]);
    app.services.file_dialog = Some(dialog);
    menus::invoke(&mut app, &ctx, "file.save", json!({})).unwrap();
    app.poll_file_dialog(&ctx, None);
    assert_eq!(asked.borrow().len(), 1, "Save As was shown");
    assert!(writes.borrow().is_empty(), "and cancelled: nothing written");

    let (mut app, _) = app_with_tiff("photos/a-Edit.tif", true);
    app.open_file("photos/b.tif", &handoff_tiff()).unwrap();
    assert!(!app.session.active().unwrap().edit_in_place, "only the handed-over file is marked");
    assert_eq!(app.save_target(), None);
}

/// A document the TIFF can't hold whole goes through Save As even when it was handed over: writing
/// it back would drop what it can't keep (an alpha channel, the layers of a Lab document) and
/// mark the document saved.
#[test]
fn a_handed_over_tiff_that_cannot_hold_the_document_asks_where_to_save() {
    let path = "photos/beach-Edit.tif";
    let (mut app, _) = app_with_tiff(path, true);
    assert_eq!(app.save_target().as_deref(), Some(path));
    app.run("select.all", json!({})).unwrap();
    app.run("select.saveSelection", json!({"name": "sky"})).unwrap();
    assert_eq!(app.session.active().unwrap().doc.channels.len(), 1);
    assert_eq!(app.save_target(), None, "an alpha channel is not written to a TIFF");
    assert!(app.session.active().unwrap().edit_in_place, "still the handed-over copy");

    let (mut app, _) = app_with_tiff(path, true);
    app.run("layer.new.layer", json!({})).unwrap();
    app.run("image.mode.lab", json!({})).unwrap();
    assert!(app.session.active().unwrap().doc.layers.len() > 1, "the test needs the layers kept");
    assert_eq!(app.save_target(), None, "a Lab document is saved as a flat TIFF");

    // a palette is not written to a TIFF either: the picture would come back as plain RGB
    let (mut app, _) = app_with_tiff(path, true);
    app.run("image.mode.bits8", json!({})).unwrap();
    app.run("image.mode.indexedColor", json!({})).unwrap();
    assert_eq!(app.session.active().unwrap().doc.mode, photocraft_doc::ColorMode::Indexed);
    assert_eq!(app.save_target(), None, "an Indexed document loses its palette in a TIFF");
}

/// Once the document is saved to another file the handoff is over; a copy (a flat format that
/// can't hold the layers) leaves the document on the handed-over TIFF, where Save still writes.
#[test]
fn saving_a_handed_over_tiff_elsewhere_ends_the_handoff() {
    let ctx = egui::Context::default();
    let path = "photos/beach-Edit.tif";
    let (mut app, writes) = app_with_tiff(path, true);
    app.run("layer.new.layer", json!({})).unwrap();
    menus::invoke(&mut app, &ctx, "file.saveAs", json!({"path": "photos/web.png"})).unwrap();
    let st = app.session.active().unwrap();
    assert_eq!((st.path.as_deref(), st.edit_in_place, st.is_dirty()), (Some(path), true, true), "a copy: still on the TIFF");
    assert!(app.replay_save("file.save", &json!({})).is_ok(), "a recorded Save writes back like the menu's");
    assert_eq!(writes.borrow().last().map(|(p, _)| p.as_str()), Some(path));

    menus::invoke(&mut app, &ctx, "file.saveAs", json!({"path": "photos/final.psd"})).unwrap();
    let st = app.session.active().unwrap();
    assert_eq!((st.path.as_deref(), st.edit_in_place), (Some("photos/final.psd"), false));
    assert_eq!(app.save_target().as_deref(), Some("photos/final.psd"));
}

/// `open_paths_in_place` marks the files it is given and nothing else; one that can't be opened
/// leaves no mark behind for a later open of that path.
#[test]
fn open_paths_in_place_marks_only_the_files_it_opens() {
    let dir = std::env::temp_dir().join(format!("photocraft-in-place-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = |name: &str| dir.join(name).to_string_lossy().to_string();
    for name in ["a-Edit.tif", "b.tif"] {
        std::fs::write(dir.join(name), handoff_tiff()).unwrap();
    }
    std::fs::write(dir.join("broken-Edit.tif"), b"not a TIFF").unwrap();
    let mut app = app();
    assert_eq!(app.open_paths_in_place(&[file("a-Edit.tif"), file("missing-Edit.tif"), file("broken-Edit.tif")]), 1);
    assert!(app.in_place_opens.is_empty(), "{:?}", app.in_place_opens);
    assert_eq!(app.open_paths(&[file("b.tif")]), 1);
    std::fs::write(dir.join("broken-Edit.tif"), handoff_tiff()).unwrap();
    assert_eq!(app.open_paths(&[file("broken-Edit.tif")]), 1);
    let marked: Vec<(String, bool)> =
        app.session.documents().iter().filter_map(|d| Some((crate::file_open::display_name(d.path.as_deref()?), d.edit_in_place))).collect();
    assert_eq!(marked, [("a-Edit.tif".to_string(), true), ("b.tif".to_string(), false), ("broken-Edit.tif".to_string(), false)]);
    let _ = std::fs::remove_dir_all(&dir);
}
