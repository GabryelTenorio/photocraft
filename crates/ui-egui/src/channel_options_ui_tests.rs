use serde_json::json;

use super::*;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.run("file.new", json!({"width": 16, "height": 16})).unwrap();
    app.run("select.rect", json!({"x": 2, "y": 2, "width": 8, "height": 8})).unwrap();
    app.run("select.saveSelection", json!({})).unwrap();
    app
}

fn channel(app: &PhotocraftApp) -> photocraft_doc::AlphaChannel {
    app.session.active().unwrap().doc.channels[0].clone()
}

#[test]
fn saved_selection_becomes_a_spot_channel_with_its_ink_and_solidity() {
    let mut app = app();
    assert!(channel(&app).spot.is_none());
    let id = open(&mut app, &json!(0)).unwrap();
    let f = &app.ui.dialogs.last().unwrap().fields;
    assert!(owns(f));
    assert_eq!(f["indicates"], json!("masked"));
    assert_eq!(f["name"], json!(channel(&app).name));
    let d = app.ui.dialog_mut(id).unwrap();
    d.fields.insert("name".into(), json!("PMS 185"));
    d.fields.insert("indicates".into(), json!("spot"));
    d.fields.insert("color".into(), json!("#ff0000"));
    d.fields.insert("opacity".into(), json!(40.0));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert!(app.ui.dialogs.is_empty());
    let ch = channel(&app);
    assert_eq!(ch.name, "PMS 185");
    let (ink, solidity) = ch.spot.expect("a spot channel");
    assert_eq!(ink.to_rgb(), [1.0, 0.0, 0.0]);
    assert!((solidity - 0.4).abs() < 1e-6);

    // Reopened, the dialog shows the spot settings; switching back makes it an alpha channel.
    let id = open(&mut app, &json!(0)).unwrap();
    let f = &app.ui.dialog_mut(id).unwrap().fields;
    assert_eq!((f["indicates"].clone(), f["color"].clone(), f["opacity"].clone()), (json!("spot"), json!("#ff0000"), json!(40.0)));
    app.ui.dialog_mut(id).unwrap().fields.insert("indicates".into(), json!("selected"));
    crate::dialogs::confirm(&mut app, id).unwrap();
    let ch = channel(&app);
    assert!(ch.spot.is_none());
    assert_eq!(ch.indicates, ColorIndicates::SelectedAreas);
}

#[test]
fn cancel_leaves_the_channel_alone() {
    let mut app = app();
    let before = app.session.active().unwrap().history.past_len();
    let id = open(&mut app, &json!(0)).unwrap();
    app.ui.dialog_mut(id).unwrap().fields.insert("indicates".into(), json!("spot"));
    app.ui.close_dialog(id);
    assert!(channel(&app).spot.is_none());
    assert_eq!(app.session.active().unwrap().history.past_len(), before);
}

#[test]
fn spot_params_send_solidity_and_alpha_params_send_opacity() {
    let mut f = Map::new();
    f.insert(MARK.into(), json!(true));
    f.insert("channel".into(), json!(0));
    f.insert("indicates".into(), json!("spot"));
    f.insert("opacity".into(), json!(25.0));
    assert_eq!(params(&f), json!({"channel": 0, "indicates": "spot", "solidity": 25.0}));
    f.insert("indicates".into(), json!("masked"));
    assert_eq!(params(&f), json!({"channel": 0, "indicates": "masked", "opacity": 25.0}));
}

#[test]
fn menu_opens_the_dialog_on_the_targeted_channel_and_refuses_otherwise() {
    let ctx = egui::Context::default();
    let mut empty = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    assert!(crate::menus::invoke(&mut empty, &ctx, COMMAND, json!({})).is_err());

    let mut app = app();
    // The composite is targeted: nothing to edit.
    assert!(crate::menus::invoke(&mut app, &ctx, COMMAND, json!({})).is_err());
    assert!(app.ui.dialogs.is_empty());
    app.run("channel.target", json!({"channel": 0})).unwrap();
    let r = crate::menus::invoke(&mut app, &ctx, COMMAND, json!({})).unwrap();
    assert!(r["dialog"].is_u64());
    assert_eq!(app.ui.dialogs.last().unwrap().fields["channel"], json!(0));
    // Explicit params run the engine directly, no dialog.
    app.ui.dialogs.clear();
    crate::menus::invoke(&mut app, &ctx, COMMAND, json!({"channel": 0, "indicates": "spot"})).unwrap();
    assert!(app.ui.dialogs.is_empty());
    assert!(channel(&app).spot.is_some());
}

#[test]
fn quick_mask_options_have_no_name_or_spot_and_bad_channels_open_nothing() {
    let mut app = app();
    let f = fields(&app, &json!("quickMask")).unwrap();
    assert!(!f.contains_key("name"));
    assert_eq!(f["__label"], json!("Quick Mask Options…"));
    assert!(fields(&app, &json!(7)).is_none());
    assert!(fields(&app, &json!("nope")).is_none());
    assert!(open(&mut app, &json!(-1)).is_none());
    assert!(app.ui.dialogs.is_empty());
}

#[test]
fn dialog_body_renders_for_alpha_spot_and_quick_mask() {
    let mut app = app();
    app.run("channel.options", json!({"channel": 0, "indicates": "spot"})).unwrap();
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    for channel in [json!(0), json!("quickMask")] {
        let mut f = fields(&app, &channel).unwrap();
        ctx.run_ui(Default::default(), |ui| body(ui, &mut f)).textures_delta.clear();
        assert!(owns(&f));
    }
}
