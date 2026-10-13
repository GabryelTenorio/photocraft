use super::*;

#[test]
fn pdf_drop_on_canvas_selects_then_places_a_smart_object() {
    let ctx = egui::Context::default();
    let mut app = app_with_canvas();
    let bytes = photocraft_io::export(&app.session.active().unwrap().doc, "pdf", &photocraft_io::ExportOptions::default()).unwrap().bytes;
    app.open_dropped(&ctx, vec![dropped("pages.PDF", Ok(bytes))], Some(egui::pos2(400.0, 300.0)));
    assert_eq!(app.session.documents().len(), 1);
    assert_eq!(app.pending_pdf_pages(), Some(1));
    assert!(app.drop_places.is_empty());
    let before = app.session.active().unwrap().doc.layers.len();
    app.confirm_pdf_pages(&[0]).unwrap();
    assert_eq!(app.session.documents().len(), 1);
    assert_eq!(app.session.active().unwrap().doc.layers.len(), before + 1);
    let state = app.session.active().unwrap();
    assert!(matches!(state.doc.layer(state.active_layer.unwrap()).unwrap().content, photocraft_doc::LayerContent::Smart(_)));
}

#[cfg(not(target_arch = "wasm32"))]
fn two_page_pdf() -> Vec<u8> {
    let mut session = photocraft_engine::Session::new();
    session.execute("file.new", json!({"width": 4, "height": 4, "background": "white"})).unwrap();
    let jpeg = photocraft_io::export(&session.active().unwrap().doc, "page.jpg", &Default::default()).unwrap().bytes;
    photocraft_io::pdf_writer::raster_pdf(&[(4, 4, 72.0, jpeg.clone()), (4, 4, 72.0, jpeg)])
}

#[cfg(not(target_arch = "wasm32"))]
fn pdf_open_batch(tag: &str) -> (PhotocraftApp, std::path::PathBuf) {
    let (dir, paths) = files(tag, &["A.pdf", "B.png", "C.png"], &png_bytes(4, 4));
    std::fs::write(&paths[0], two_page_pdf()).unwrap();
    let (mut app, _) = app_with(Vec::new());
    app.background_jobs = true;
    assert_eq!(app.open_paths(&paths), 3);
    assert!(app.session.documents().is_empty(), "a pending picker creates no tab");
    assert_eq!(app.pending_pdf_pages(), Some(2));
    assert_eq!(app.jobs.opens.len(), 2);
    (app, dir)
}

/// Apply one real worker's result, controlling completion order without timing assumptions.
#[cfg(not(target_arch = "wasm32"))]
fn finish_named_open(app: &mut PhotocraftApp, name: &str) {
    let i = app.jobs.opens.iter().position(|tab| tab.name == name).unwrap();
    let tab = app.jobs.opens.remove(i);
    let result = app.session.wait_job(tab.job).unwrap();
    crate::jobs_ui::finish_open_tab(app, &tab, &result).unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn pending_pdf_picker_does_not_count_as_an_open_tab() {
    let (mut app, dir) = pdf_open_batch("pending-pdf-cancel");
    finish_named_open(&mut app, "C.png");
    finish_named_open(&mut app, "B.png");
    assert_eq!(doc_names(&app), ["B.png", "C.png"]);
    assert_eq!(app.jobs.batches.len(), 1, "the picker retains its input order");
    app.background_jobs = false;
    app.open_bytes("unrelated.psd", b"x").unwrap();
    app.cancel_pdf_import();
    assert!(app.jobs.batches.is_empty());
    assert_eq!(doc_names(&app), ["B.png", "C.png", "unrelated.psd"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn confirmed_pdf_tabs_keep_page_and_input_order() {
    for background in [false, true] {
        for after_rasters in [false, true] {
            let (mut app, dir) = pdf_open_batch(&format!("pending-pdf-confirm-{background}-{after_rasters}"));
            finish_named_open(&mut app, "C.png");
            if after_rasters {
                finish_named_open(&mut app, "B.png");
            }
            app.background_jobs = background;
            assert!(app.confirm_pdf_pages(&[2]).is_err(), "an invalid selection keeps the picker retryable");
            assert_eq!(app.pending_pdf_pages(), Some(2));
            assert_eq!(app.jobs.batches.len(), 1);
            app.confirm_pdf_pages(&[1, 0]).unwrap();
            if background {
                finish_named_open(&mut app, "A.pdf");
            }
            if !after_rasters {
                finish_named_open(&mut app, "B.png");
            }
            assert_eq!(doc_names(&app), ["A - Page 1.pdf", "A - Page 2.pdf", "B.png", "C.png"]);
            assert!(app.session.documents()[..2].iter().all(|state| state.source_read_only && state.path.is_none()));
            assert_eq!(app.ui.views.len(), 4);
            assert!(app.jobs.batches.is_empty());
            assert_eq!(app.pending_pdf_pages(), None);
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn separate_pending_pdf_batches_keep_their_order() {
    for background in [false, true] {
        for later_first in [false, true] {
            for cancel_first in [false, true] {
                let (dir, paths) = files(&format!("separate-pdfs-{background}-{later_first}-{cancel_first}"), &["A.pdf", "B.pdf", "C.png"], &two_page_pdf());
                std::fs::write(&paths[2], png_bytes(4, 4)).unwrap();
                let (mut app, _) = app_with(Vec::new());
                app.background_jobs = true;
                assert_eq!(app.open_paths(&paths[..1]), 1);
                assert_eq!(app.open_paths(&paths[1..2]), 1);
                assert_eq!(app.open_paths(&paths[2..]), 1);
                app.background_jobs = background;
                if cancel_first {
                    app.cancel_pdf_import();
                } else {
                    app.confirm_pdf_pages(&[0, 1]).unwrap();
                }
                app.confirm_pdf_pages(&[0, 1]).unwrap();
                if background {
                    if later_first {
                        finish_named_open(&mut app, "B.pdf");
                    }
                    if !cancel_first {
                        finish_named_open(&mut app, "A.pdf");
                    }
                    if !later_first {
                        finish_named_open(&mut app, "B.pdf");
                    }
                }
                finish_named_open(&mut app, "C.png");
                let expected = if cancel_first {
                    vec!["B - Page 1.pdf", "B - Page 2.pdf", "C.png"]
                } else {
                    vec!["A - Page 1.pdf", "A - Page 2.pdf", "B - Page 1.pdf", "B - Page 2.pdf", "C.png"]
                };
                assert_eq!(doc_names(&app), expected, "background={background}, later_first={later_first}, cancel_first={cancel_first}");
                assert!(app.jobs.batches.is_empty());
                assert_eq!(app.pending_pdf_pages(), None);
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn cancelled_pdf_picker_and_failed_raster_release_the_batch() {
    let (dir, paths) = files("pending-pdf-failure", &["A.pdf", "B.png", "C.png"], &png_bytes(4, 4));
    let mut session = photocraft_engine::Session::new();
    session.execute("file.new", json!({"width": 4, "height": 4})).unwrap();
    let pdf = photocraft_io::export(&session.active().unwrap().doc, "page.pdf", &Default::default()).unwrap().bytes;
    std::fs::write(&paths[0], pdf).unwrap();
    std::fs::write(&paths[1], b"bad").unwrap();
    let (mut app, _) = app_with(Vec::new());
    app.background_jobs = true;
    assert_eq!(app.open_paths(&paths), 3);
    finish_named_open(&mut app, "C.png");
    app.cancel_pdf_import();
    assert_eq!(app.jobs.batches.len(), 1, "the raster job still belongs to the batch");
    let later_pdf = dir.join("D.pdf");
    let later_raster = dir.join("E.png");
    std::fs::write(&later_pdf, two_page_pdf()).unwrap();
    std::fs::write(&later_raster, png_bytes(4, 4)).unwrap();
    assert_eq!(app.open_paths(&[later_pdf.to_string_lossy().into_owned(), later_raster.to_string_lossy().into_owned()]), 2);
    finish_named_open(&mut app, "E.png");
    let job = app.jobs.opens[0].job;
    assert!(app.session.wait_job(job).is_err());
    crate::jobs_ui::tick(&mut app, &egui::Context::default());
    assert!(app.jobs.opens.is_empty());
    assert_eq!(app.jobs.batches.len(), 1, "only the later PDF selection remains pending");
    app.background_jobs = false;
    app.open_pdf_pages(&[0, 1]).unwrap();
    assert!(app.jobs.batches.is_empty());
    assert_eq!(doc_names(&app), ["C.png", "D - Page 1.pdf", "D - Page 2.pdf", "E.png"]);
    let _ = std::fs::remove_dir_all(dir);
}
