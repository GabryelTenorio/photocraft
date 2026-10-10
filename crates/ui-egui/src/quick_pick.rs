//! ⌘⌥⌃-click on the canvas, with any tool: select the topmost visible layer with pixels under the
//! pointer (`layer.pickAt`), without switching to the Move tool. The whole press–drag–release is
//! swallowed so the active tool never sees it; it runs before the ⌃⌥ brush-resize gesture, which
//! would otherwise claim it.
//!
//! Platforms: the gesture is macOS-only, because ⌘ is the only key that tells it apart from the
//! ⌃⌥ brush resize. egui never reports `mac_cmd` on Windows or Linux, so there it is a no-op and
//! ⌃⌥-drag keeps resizing the brush; the Move tool's Auto-Select picks layers on every platform.
//! A failed pick is shown as a status-bar error and never panics.

use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::ToolEvent;

/// ⌘ + ⌥ + ⌃ held together. Always false off macOS, where egui never sets `mac_cmd`.
pub fn is_gesture(mods: egui::Modifiers) -> bool {
    mods.mac_cmd && mods.alt && mods.ctrl
}

/// Keyboard/palette counterpart of Auto-Select. Resolve the latest canvas mapping so zoom,
/// rotation, mirroring and display scale behave exactly like a pointer pick. An idle pointer
/// outside the image leaves the current layer selected; an in-progress edit keeps its target.
pub fn under_cursor(app: &mut PhotocraftApp, ctx: &egui::Context, params: serde_json::Value) -> Result<serde_json::Value, String> {
    if ctx.text_edit_focused() || app.ui.text_edit.is_some() || !app.ui.dialogs.is_empty() || app.drag.is_some() || app.ui.transform.is_some() {
        return Ok(json!({"layer": null}));
    }
    let Some(xf) = crate::canvas::ViewXform::active(app) else { return Ok(json!({"layer": null})) };
    let Some(p) = ctx.input(|i| i.pointer.hover_pos()).filter(|p| xf.rect.contains(*p)) else { return Ok(json!({"layer": null})) };
    let [x, y] = xf.to_doc(p);
    let Some(st) = app.session.active() else { return Ok(json!({"layer": null})) };
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 || x >= f64::from(st.doc.size.width) || y >= f64::from(st.doc.size.height) {
        return Ok(json!({"layer": null}));
    }
    let mut fields = match params {
        serde_json::Value::Null => serde_json::Map::new(),
        serde_json::Value::Object(fields) => fields,
        _ => return Err("layer.pickAt parameters must be an object".into()),
    };
    let select = fields.get("select").and_then(serde_json::Value::as_bool).unwrap_or(true);
    let mode = fields.get("mode").cloned().unwrap_or_else(|| json!("replace"));
    fields.insert("x".into(), json!(x));
    fields.insert("y".into(), json!(y));
    fields.insert("select".into(), json!(false));
    let picked = app.run("layer.pickAt", serde_json::Value::Object(fields))?;
    if select && let Some(layer) = picked.get("layer").and_then(serde_json::Value::as_u64) {
        // Auto-Select clicks preserve an existing multiselection for the following drag.
        // A dedicated keyboard pick instead makes the pointed layer the active selection.
        app.run("layer.select", json!({"layer": layer, "mode": mode}))?;
    }
    Ok(picked)
}

/// Handles one canvas pointer event; true when it was consumed.
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: egui::Modifiers) -> bool {
    match ev {
        ToolEvent::Down { x, y, .. } => {
            if app.drag.is_some() || app.session.active().is_none() || !is_gesture(mods) {
                return false;
            }
            if let Err(e) = app.run("layer.pickAt", json!({"x": x, "y": y, "target": "layer", "mode": "replace"})) {
                app.ui.status = e;
                app.ui.status_error = true;
            }
            app.quick_pick = true;
            true
        }
        ToolEvent::Move { .. } => app.quick_pick,
        ToolEvent::Up { .. } => std::mem::take(&mut app.quick_pick),
    }
}

#[cfg(test)]
mod tests {
    use egui::Modifiers;
    use photocraft_geom::Rect;
    use serde_json::json;

    use super::*;
    use crate::canvas::tool_event;
    use crate::state::Tool;

    const PICK: Modifiers = Modifiers { alt: true, ctrl: true, shift: false, mac_cmd: true, command: true };

    /// A 64×64 document with A painted at (8..24)² and B at (40..56)²; B is active.
    fn app() -> (PhotocraftApp, photocraft_doc::LayerId) {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.session.execute("file.new", json!({"width": 64, "height": 64})).unwrap();
        app.sync_views();
        let mut first = None;
        for at in [8, 40] {
            app.session.execute("layer.new.layer", json!({})).unwrap();
            app.session
                .edit("paint", |doc, a| {
                    doc.layer_mut(a.unwrap()).unwrap().surface_mut().unwrap().fill_rect(Rect::new(at, at, at + 16, at + 16), &[1.0, 0.0, 0.0, 1.0]);
                    Ok(())
                })
                .unwrap();
            first.get_or_insert(app.session.active().unwrap().active_layer.unwrap());
        }
        (app, first.unwrap())
    }

    #[test]
    fn picks_the_layer_under_the_pointer_with_a_painting_tool() {
        let (mut app, a) = app();
        app.ui.tool = Tool::Brush;
        let before = app.session.active().unwrap().history.past_len();
        tool_event(&mut app, ToolEvent::Down { x: 12.0, y: 12.0, pressure: 1.0 }, PICK);
        tool_event(&mut app, ToolEvent::Move { x: 20.0, y: 20.0, pressure: 1.0 }, PICK);
        tool_event(&mut app, ToolEvent::Up { x: 20.0, y: 20.0 }, PICK);
        let st = app.session.active().unwrap();
        assert_eq!(st.active_layer, Some(a));
        assert!(app.drag.is_none() && app.brush_resize.is_none(), "neither painted nor resized the brush");
        assert_eq!(st.history.past_len(), before, "no paint stroke was recorded");
    }

    #[test]
    fn needs_all_three_modifiers() {
        let (mut app, a) = app();
        app.ui.tool = Tool::RectMarquee;
        let no_ctrl = Modifiers { ctrl: false, ..PICK };
        assert!(!pointer(&mut app, ToolEvent::Down { x: 12.0, y: 12.0, pressure: 1.0 }, no_ctrl));
        assert_ne!(app.session.active().unwrap().active_layer, Some(a));
    }

    /// Windows and Linux report Ctrl+Alt (+Shift) but never `mac_cmd`: the click falls through to
    /// the tool and the ⌃⌥ brush resize instead of picking a layer.
    #[test]
    fn is_a_no_op_without_the_mac_command_key() {
        let (mut app, a) = app();
        app.ui.tool = Tool::Brush;
        let pc = Modifiers { alt: true, ctrl: true, shift: false, mac_cmd: false, command: true };
        for mods in [pc, Modifiers { shift: true, ..pc }] {
            assert!(!pointer(&mut app, ToolEvent::Down { x: 12.0, y: 12.0, pressure: 1.0 }, mods));
            assert!(!app.quick_pick);
            assert_ne!(app.session.active().unwrap().active_layer, Some(a));
        }
    }

    #[test]
    fn a_failed_pick_is_a_status_error_and_swallows_the_click() {
        let (mut app, _) = app();
        // Far outside the canvas no layer has pixels: the pick fails gracefully.
        assert!(pointer(&mut app, ToolEvent::Down { x: 1e9, y: -1e9, pressure: 1.0 }, PICK));
        assert!(pointer(&mut app, ToolEvent::Up { x: 1e9, y: -1e9 }, PICK));
        assert!(!app.quick_pick);
    }
}
