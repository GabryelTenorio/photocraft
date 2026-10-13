//! Automation opens dedupe: a path that is already an open document's file activates that
//! document instead of loading a second copy (`doc.open`, MCP `doc_open`).

use photocraft_automation::{AuthorizedWorkspace, Headless};
use serde_json::json;
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pc-open-dedupe-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_png(dir: &Path, name: &str, w: u32, h: u32) {
    let img = photocraft_codecs::Image::from_u8(w, h, photocraft_codecs::ChannelLayout::Rgb, vec![200; (w * h * 3) as usize]).unwrap();
    let bytes = photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &Default::default()).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
}

#[test]
fn trusted_local_open_of_an_already_open_path_returns_the_existing_document() {
    let dir = tmp("local");
    write_png(&dir, "x.png", 8, 4);
    write_png(&dir, "other.png", 6, 6);
    let mut h = Headless::trusted_local();
    let x = dir.join("x.png").to_string_lossy().into_owned();

    let first = h.handle("doc.open", json!({"path": x})).unwrap();
    assert!(first["deduped"].is_null(), "{first}");
    let (doc_id, revision) = {
        let st = h.session.documents().first().unwrap();
        (st.doc.id, st.revision)
    };

    let second = h.handle("doc.open", json!({"path": x})).unwrap();
    assert_eq!(second["deduped"], json!(true), "{second}");
    assert_eq!(second["index"], first["index"]);
    assert_eq!(second["name"], first["name"]);
    assert_eq!(h.session.documents().len(), 1, "the same path opens no second document");
    let st = h.session.active().unwrap();
    assert_eq!((st.doc.id, st.revision), (doc_id, revision), "the existing document is returned untouched");

    // A different path still opens a second document.
    let other = dir.join("other.png").to_string_lossy().into_owned();
    let third = h.handle("doc.open", json!({"path": other})).unwrap();
    assert!(third["deduped"].is_null(), "{third}");
    assert_eq!(h.session.documents().len(), 2);

    // A spelling that normalizes to the same key dedupes too.
    let again = h.handle("doc.open", json!({"path": dir.join("./x.png").to_string_lossy()})).unwrap();
    assert_eq!(again["deduped"], json!(true), "{again}");
    assert_eq!(again["index"], first["index"]);
    assert_eq!(h.session.documents().len(), 2);

    // Closing the document makes the next open load the file again.
    h.handle("doc.close", json!({"index": 0})).unwrap();
    let reopened = h.handle("doc.open", json!({"path": x})).unwrap();
    assert!(reopened["deduped"].is_null(), "{reopened}");
    assert_eq!(h.session.documents().len(), 2);

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn workspace_open_dedupes_by_the_requested_path() {
    let root = tmp("workspace");
    write_png(&root, "in.png", 8, 4);
    let workspace = AuthorizedWorkspace::new(Some(&root), Some(&root)).unwrap();
    let mut h = Headless::with_workspace(workspace);

    let first = h.handle("doc.open", json!({"path": "in.png"})).unwrap();
    assert!(first["deduped"].is_null(), "{first}");
    let second = h.handle("doc.open", json!({"path": "in.png"})).unwrap();
    assert_eq!(second["deduped"], json!(true), "{second}");
    assert_eq!(h.session.documents().len(), 1);

    // Release directory capabilities before cleanup on Windows.
    drop(h);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn open_of_a_path_a_still_open_document_was_saved_to_activates_it() {
    let dir = tmp("saved");
    let mut h = Headless::trusted_local();
    h.handle("doc.new", json!({"width": 8, "height": 8, "name": "Work"})).unwrap();
    let saved = h.handle("doc.save", json!({"path": dir.join("work.pcraft").to_string_lossy()})).unwrap();
    assert!(saved["warnings"].as_array().is_none_or(Vec::is_empty), "{saved}");
    let opened = h.handle("doc.open", json!({"path": dir.join("work.pcraft").to_string_lossy()})).unwrap();
    assert_eq!(opened["deduped"], json!(true), "the saved document is already the file at that path: {opened}");
    assert_eq!(opened["index"], 0);
    assert_eq!(h.session.documents().len(), 1);

    std::fs::remove_dir_all(dir).unwrap();
}
