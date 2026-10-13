//! The full-size preview refusal names the workaround, and `doc.save` keeps full resolution.

use photocraft_automation::{AuthorizedWorkspace, Headless};
use serde_json::json;

#[test]
fn full_size_preview_refusal_names_the_workaround() {
    let mut h = Headless::new();
    h.handle("doc.new", json!({"width": 3000, "height": 2000})).unwrap();
    let error = h.handle("doc.render", json!({"maxSide": 0})).unwrap_err().to_string();
    assert!(error.contains("automation preview side exceeds 2048 pixels"), "{error}");
    assert!(error.contains("maxSide 0 checks the document's longest edge"), "{error}");
    assert!(error.contains("doc.save a .png for full resolution"), "{error}");
    assert!(h.handle("doc.render", json!({"maxSide": 2048})).is_ok());
}

#[test]
fn full_resolution_survives_via_save_where_full_size_render_is_refused() {
    let dir = std::env::temp_dir().join(format!(
        "pc-preview-hint-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = Headless::with_workspace(AuthorizedWorkspace::new(None, Some(&dir)).unwrap());
    h.handle("doc.new", json!({"width": 3000, "height": 2000})).unwrap();
    assert!(h.handle("doc.render", json!({"maxSide": 0})).is_err());
    h.handle("doc.save", json!({"path": "full.png"})).unwrap();
    let png = std::fs::read(dir.join("full.png")).unwrap();
    assert_eq!(photocraft_codecs::decode(&png).unwrap().dimensions(), (3000, 2000));
    // Release directory capabilities before cleanup on Windows.
    drop(h);
    std::fs::remove_dir_all(&dir).unwrap();
}
