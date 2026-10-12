//! Auto-Show Hidden Panels (Preferences ▸ Workspace ▸ Auto-show hidden panels, #204).

use egui::{pos2, vec2};

use crate::PhotocraftApp;
use crate::auto_show_peek;
use crate::record_panel_strip;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.ui.panels.toolbar = false;
    app.ui.panels.dock = false;
    app
}

/// A frame with the pointer at `p` on a 1200×800 window.
fn ctx_at(p: egui::Pos2) -> egui::Context {
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 800.0));
    let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events: vec![egui::Event::PointerMoved(p)], ..Default::default() }, |_ui| {});
    out.textures_delta.clear();
    ctx
}

/// The window edges reveal the hidden toolbar (start edge) and dock (end edge); the middle
/// reveals nothing, and a shown panel is not peeked.
#[test]
fn the_window_edges_reveal_the_hidden_panels() {
    let mut app = app();
    let peek = |app: &PhotocraftApp, p: egui::Pos2| auto_show_peek(app, &ctx_at(p));
    assert!(peek(&app, pos2(6.0, 400.0)).toolbar, "the start edge reveals the toolbar");
    assert!(!peek(&app, pos2(6.0, 400.0)).dock);
    assert!(peek(&app, pos2(1194.0, 400.0)).dock, "the end edge reveals the dock");
    assert_eq!(peek(&app, pos2(600.0, 400.0)), crate::Peek::default(), "the middle reveals nothing");
    app.ui.panels.toolbar = true;
    assert!(!peek(&app, pos2(6.0, 400.0)).toolbar, "a shown toolbar is not peeked");
    app.ui.panels.dock = true;
    assert_eq!(peek(&app, pos2(1194.0, 400.0)), crate::Peek::default(), "a shown dock is not peeked");
}

/// The peek follows the preference, and holds while the pointer is over the strip it revealed
/// (so the pointer can reach into the panel it brought back).
#[test]
fn the_peek_follows_the_preference_and_keeps_over_the_revealed_panel() {
    let mut app = app();
    let p = pos2(1000.0, 400.0);
    let ctx = ctx_at(p);
    // Nothing recorded yet: the pointer is in the canvas, far from the end edge.
    assert_eq!(auto_show_peek(&app, &ctx), crate::Peek::default());
    // The dock was drawn 260 points wide at the end edge: the pointer over it keeps the peek.
    record_panel_strip(
        &ctx,
        crate::dock_strip_id(),
        egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 800.0)),
        egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(940.0, 800.0)),
        false,
    );
    assert!(auto_show_peek(&app, &ctx).dock, "the pointer over the revealed dock keeps it");
    // The preference off: the edge hover does nothing.
    app.session.prefs.edit(|p| p.workspace.auto_show_hidden_panels = false);
    assert_eq!(auto_show_peek(&app, &ctx_at(pos2(6.0, 400.0))), crate::Peek::default(), "preference off");
}
