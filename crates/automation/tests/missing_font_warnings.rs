//! A session server (`photocraft-cli serve` or `mcp`) must warn when a type command leaves a
//! layer naming a font the host does not have, and when `doc.save` / `doc.open` would export or
//! surface such a document (#3391). `photocraft-cli convert`, `run` and `info` already do.

use photocraft_automation::Headless;
use serde_json::{Value, json};

const MISSING: &str = "DefinitelyMissingPhotoCraftTypeface3391";

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pc-missing-font-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn warnings(v: &Value) -> Vec<String> {
    v.get("warnings").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter_map(|w| w.as_str().map(str::to_owned)).collect()
}

fn has_missing_font_warning(v: &Value) -> bool {
    warnings(v).iter().any(|w| w.contains(MISSING) && w.contains("fallback"))
}

#[test]
fn type_create_set_style_edit_save_and_open_warn_about_a_missing_font() {
    let dir = tmp("rpc");
    let path = dir.join("missing.pcraft");
    let path_str = path.to_str().unwrap();
    let mut h = Headless::trusted_local();
    h.handle("doc.new", json!({"width": 128, "height": 64, "background": "white"})).unwrap();

    // type.create names a missing family: the reply carries the warning.
    let created = h
        .handle("engine.execute", json!({"command": "type.create", "params": {"x": 4, "y": 32, "text": "Hi", "font": MISSING, "size": 20}}))
        .unwrap();
    assert!(has_missing_font_warning(&created), "type.create: {created}");

    // type.setStyle on the same layer still names the same missing family.
    let layer = created["layer"].as_u64().unwrap();
    let styled = h.handle("engine.execute", json!({"command": "type.setStyle", "params": {"layer": layer, "font": MISSING, "size": 24}})).unwrap();
    assert!(has_missing_font_warning(&styled), "type.setStyle: {styled}");

    // type.edit edits the text, keeping the missing family.
    let edited = h.handle("engine.execute", json!({"command": "type.edit", "params": {"layer": layer, "text": "Hello"}})).unwrap();
    assert!(has_missing_font_warning(&edited), "type.edit: {edited}");

    // doc.save (layered .pcraft) carries the warning next to format warnings.
    let saved = h.handle("doc.save", json!({"path": path_str})).unwrap();
    assert!(has_missing_font_warning(&saved), "doc.save pcraft: {saved}");

    // doc.save as .png (flattened export) also warns before the fallback bakes in.
    let png = dir.join("missing.png");
    let png_str = png.to_str().unwrap();
    let saved_png = h.handle("doc.save", json!({"path": png_str})).unwrap();
    assert!(has_missing_font_warning(&saved_png), "doc.save png: {saved_png}");

    // Close and reopen: the warning rides on doc.open too.
    h.handle("doc.close", json!({})).unwrap();
    let opened = h.handle("doc.open", json!({"path": path_str})).unwrap();
    assert!(has_missing_font_warning(&opened), "doc.open: {opened}");

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn no_type_layers_do_not_add_a_missing_font_warning() {
    let mut h = Headless::trusted_local();
    let created = h.handle("doc.new", json!({"width": 16, "height": 16, "background": "white"})).unwrap();
    assert!(warnings(&created).iter().all(|w| !w.contains("font '")), "{created}");
    let dir = tmp("blank");
    let path = dir.join("blank.pcraft");
    let saved = h.handle("doc.save", json!({"path": path.to_str().unwrap()})).unwrap();
    assert!(warnings(&saved).iter().all(|w| !w.contains("font '")), "{saved}");
    let opened = h.handle("doc.open", json!({"path": path.to_str().unwrap()})).unwrap();
    assert!(warnings(&opened).iter().all(|w| !w.contains("font '")), "{opened}");
    std::fs::remove_dir_all(dir).unwrap();
}
