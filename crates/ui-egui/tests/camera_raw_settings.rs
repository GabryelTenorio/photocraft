//! Camera Raw Save/Load Settings as an `.xmp` sidecar (issue #4274).

use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use photocraft_ui_egui::{PhotocraftApp, camera_raw_ui};
use serde_json::json;

fn fixture() -> Harness<'static, PhotocraftApp> {
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_eframe(|cc| {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.run("file.new", json!({"width":64,"height":48})).unwrap();
        app.run("layer.new.layer", json!({})).unwrap();
        app.run("edit.fill", json!({"color":"#808080"})).unwrap();
        camera_raw_ui::open(&mut app, &cc.egui_ctx).unwrap();
        app
    });
    h.run_steps(3);
    h
}

/// The dialog's footer offers both Settings buttons.
#[test]
fn dialog_offers_save_and_load_settings() {
    let h = fixture();
    assert!(h.query_by_label("Save Settings…").is_some(), "Save Settings button");
    assert!(h.query_by_label("Load Settings…").is_some(), "Load Settings button");
}

/// The engine commands the buttons dispatch round-trip a look through the XMP packet.
#[test]
fn settings_round_trip_through_the_engine() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    let settings = json!({"exposure": 0.5, "contrast": 12, "vibrance": 20, "gradeShadows": {"hue": 200.0, "sat": 15.0, "lum": -5.0}});
    let out = app.run("filter.cameraRaw.exportSettings", json!({"settings": settings})).unwrap();
    let data = out["data"].as_str().expect("base64 data").to_string();
    assert!(out["bytes"].as_u64().unwrap() > 0);
    let back = app.run("filter.cameraRaw.importSettings", json!({"data": data})).unwrap();
    assert_eq!(back["settings"]["exposure"], json!(0.5));
    assert_eq!(back["settings"]["contrast"], json!(12.0));
    assert_eq!(back["settings"]["vibrance"], json!(20.0));
    assert_eq!(back["settings"]["gradeShadows"]["hue"], json!(200.0));
}
