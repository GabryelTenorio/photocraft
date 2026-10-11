//! "Rasterize?" prompt: painting the pixels of a type, shape, Smart Object or fill layer with a
//! pixel tool asks first, as Photoshop does ("This type layer must be rasterized before
//! proceeding. Its text will no longer be editable. Rasterize the type?"), instead of failing
//! with a status-bar message.
//!
//! Edit › Transform › Warp on editable type asks the same question (#3004): OK rasterizes the
//! type, then Warp opens on the pixels. (Distort and Perspective are greyed for type instead, as
//! in Photoshop.)
//!
//! The prompt is an ordinary dialog in the UI state (`ui.inspect` shows it, `ui.dialog.confirm`
//! and `ui.dialog.cancel` answer it). OK runs the matching `layer.rasterize.*` command, then paints
//! where the click was (or invokes the command it was asked for, `then`), so rasterizing and the
//! edit are two history states, as in Photoshop. Cancel does nothing.

use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::canvas::ToolEvent;
use crate::state::{DialogKind, Tool};

const MARK: &str = "__rasterize";

/// The commands a prompt may continue with. Dialog fields can be edited (`ui.dialog.set`), so a
/// prompt never runs anything else.
const CONTINUES: [&str; 1] = ["edit.transform.warp"];

/// A layer kind that must be rasterized before its pixels can be painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Type,
    Shape,
    SmartObject,
    Fill,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Type, Kind::Shape, Kind::SmartObject, Kind::Fill];

    fn key(self) -> &'static str {
        match self {
            Kind::Type => "type",
            Kind::Shape => "shape",
            Kind::SmartObject => "smartObject",
            Kind::Fill => "fill",
        }
    }

    /// The command that rasterizes it.
    pub fn command(self) -> &'static str {
        match self {
            Kind::Type => "layer.rasterize.type",
            Kind::Shape => "layer.rasterize.shape",
            Kind::SmartObject => "layer.rasterize.smartObject",
            Kind::Fill => "layer.rasterize.fillContent",
        }
    }

    /// Photoshop's question.
    pub fn message(self) -> &'static str {
        match self {
            Kind::Type => "This type layer must be rasterized before proceeding. Its text will no longer be editable. Rasterize the type?",
            Kind::Shape => "This shape layer must be rasterized before proceeding. It will no longer be editable as a shape. Rasterize the shape?",
            Kind::SmartObject => "This Smart Object must be rasterized before proceeding. Its contents will no longer be editable. Rasterize the Smart Object?",
            Kind::Fill => "This fill layer must be rasterized before proceeding. Its fill content will no longer be editable. Rasterize the layer?",
        }
    }

    fn from_key(k: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|x| x.key() == k)
    }
}

/// Does `tool` paint the layer's pixels (so a vector or Smart Object layer must be rasterized)?
pub fn paints_pixels(app: &PhotocraftApp, tool: Tool) -> bool {
    tool.is_brushlike()
        || matches!(tool, Tool::PaintBucket | Tool::MagicEraser | Tool::RedEye)
        || (tool == Tool::Gradient && app.ui.tool_options.gradient_classic)
}

/// The active layer's kind when painting its pixels needs rasterizing first: the tool targets
/// the layer's pixels (not its mask, an alpha channel or the Quick Mask) and the layer is a
/// type, shape, Smart Object or fill layer that isn't locked.
pub fn needed(app: &PhotocraftApp) -> Option<(Kind, photocraft_doc::LayerId)> {
    if crate::canvas::paint_target(app) != json!("pixels") {
        return None;
    }
    let st = app.session.active()?;
    let id = st.active_layer?;
    let l = st.doc.layer(id)?;
    let locks = st.doc.effective_locks(id);
    if locks.all || locks.pixels {
        return None;
    }
    use photocraft_doc::LayerContent as C;
    let kind = match &l.content {
        C::Text(_) => Kind::Type,
        C::Shape(_) => Kind::Shape,
        C::Smart(_) => Kind::SmartObject,
        C::Fill(_) => Kind::Fill,
        _ => return None,
    };
    Some((kind, id))
}

/// A tool's pointer press: when it would paint a layer that must be rasterized first, park it
/// behind the prompt and return true (the tool must not see the press).
pub fn intercept(app: &mut PhotocraftApp, tool: Tool, x: f64, y: f64, pressure: f32) -> bool {
    if !paints_pixels(app, tool) {
        return false;
    }
    let Some((kind, layer)) = needed(app) else { return false };
    if !app.ui.dialogs.iter().any(|d| owns(&d.fields)) {
        open(app, kind, layer, tool, [x, y, f64::from(pressure)]);
    }
    true
}

/// Open the prompt for `layer`; OK rasterizes it, then `tool` paints at `at` (x, y, pressure).
pub fn open(app: &mut PhotocraftApp, kind: Kind, layer: photocraft_doc::LayerId, tool: Tool, at: [f64; 3]) -> u64 {
    let mut f = Map::new();
    f.insert(MARK.into(), json!(kind.key()));
    f.insert("__command".into(), json!(kind.command()));
    f.insert("__label".into(), json!("PhotoCraft"));
    f.insert("message".into(), json!(kind.message()));
    f.insert("layer".into(), json!(layer.0));
    f.insert("tool".into(), json!(format!("{tool:?}")));
    f.insert("at".into(), json!(at));
    app.ui.open_dialog(DialogKind::Command, f)
}

/// Open the prompt for `layer`; OK rasterizes it, then invokes the menu command `then` with
/// `params`, as if it had been chosen on the rasterized layer.
pub fn open_then(app: &mut PhotocraftApp, kind: Kind, layer: photocraft_doc::LayerId, then: &str, params: Value) -> u64 {
    let mut f = Map::new();
    f.insert(MARK.into(), json!(kind.key()));
    f.insert("__command".into(), json!(kind.command()));
    f.insert("__label".into(), json!("PhotoCraft"));
    f.insert("message".into(), json!(kind.message()));
    f.insert("layer".into(), json!(layer.0));
    f.insert("then".into(), json!({ "command": then, "params": params }));
    app.ui.open_dialog(DialogKind::Command, f)
}

/// The editable type layer Edit › Transform › Warp would warp: the open transform box's layer
/// (when it moves the layer, not a mask, channel, path or the selection), else the active layer
/// when its pixels are targeted. Warp can't keep type editable, so it must be rasterized first.
fn warp_needs_raster(app: &PhotocraftApp) -> Option<photocraft_doc::LayerId> {
    let st = app.session.active()?;
    let id = match &app.ui.transform {
        Some(t) if t.selection || t.target.is_some() || t.path.is_some() || t.warp.is_some() => return None,
        Some(t) => photocraft_doc::LayerId(t.layer),
        None => {
            if crate::canvas::paint_target(app) != json!("pixels") || crate::vector_ui::free_transform_path(app).is_some() {
                return None;
            }
            st.active_layer?
        }
    };
    matches!(st.doc.layer(id)?.content, photocraft_doc::LayerContent::Text(_)).then_some(id)
}

/// Edit › Transform › Warp (the menu, or the warp toggle of an open transform) on editable type:
/// open the prompt instead and return its id (an open one is reused). `None`: warp as usual.
pub fn ask_before_warp(app: &mut PhotocraftApp) -> Option<u64> {
    let layer = warp_needs_raster(app)?;
    if let Some(d) = app.ui.dialogs.iter().find(|d| owns(&d.fields)) {
        return Some(d.id);
    }
    Some(open_then(app, Kind::Type, layer, "edit.transform.warp", json!({})))
}

/// Run the command a confirmed prompt continues with (it needs the egui context, which
/// [`confirm`] doesn't have). Called right after a confirm and every frame.
pub fn resume(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let Some((command, params)) = app.rasterize_then.take() else { return };
    if let Err(e) = crate::menus::invoke(app, ctx, &command, params) {
        app.ui.status = e;
    }
}

/// Is this dialog the prompt?
pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}

/// The prompt's body: the question, with a caution mark.
pub fn body(ui: &mut egui::Ui, f: &Map<String, Value>) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let text = f.get("message").and_then(Value::as_str).unwrap_or("This layer must be rasterized before proceeding.");
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (r, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
        crate::icons::paint(ui, r, "triangle-alert", 30.0, t.warning);
        ui.add(egui::Label::new(egui::RichText::new(text).color(t.text).size(13.0)).wrap());
    });
}

/// OK: rasterize, then paint at the click or queue the `then` command (two history states).
pub fn confirm(app: &mut PhotocraftApp, f: &Map<String, Value>) -> Result<Value, String> {
    let kind = f.get(MARK).and_then(Value::as_str).and_then(Kind::from_key).ok_or("not a rasterize prompt")?;
    let layer = f.get("layer").and_then(Value::as_u64).ok_or("the prompt has no layer")?;
    if let Some(then) = f.get("then") {
        let command = then.get("command").and_then(Value::as_str).filter(|c| CONTINUES.contains(c)).ok_or("the prompt has no command to continue with")?;
        // A transform box open on the layer (the warp toggle) is applied first: scaling and
        // rotating keep type editable, and Warp then starts from the transformed type.
        if app.ui.transform.as_ref().is_some_and(|t| t.layer == layer) {
            crate::transform_tool::commit(app);
        }
        let r = app.run(kind.command(), json!({ "layer": layer }))?;
        app.rasterize_then = Some((command.to_string(), then.get("params").cloned().unwrap_or_else(|| json!({}))));
        return Ok(json!({ "rasterized": r, "then": command }));
    }
    let r = app.run(kind.command(), json!({ "layer": layer }))?;
    let tool = f.get("tool").and_then(Value::as_str).and_then(Tool::from_name);
    let at: Vec<f64> = f.get("at").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let (Some(tool), Some(&x), Some(&y)) = (tool, at.first(), at.get(1)) else {
        return Ok(json!({ "rasterized": r, "painted": false }));
    };
    let pressure = at.get(2).copied().unwrap_or(1.0) as f32;
    // The same tool, layer and point as when the prompt opened.
    app.ui.tool = tool;
    let before = app.session.active().map(|st| st.history.past_len());
    if app.session.active().and_then(|st| st.active_layer) == Some(photocraft_doc::LayerId(layer)) && x.is_finite() && y.is_finite() {
        crate::canvas::tool_event(app, ToolEvent::Down { x, y, pressure: pressure.clamp(0.0, 1.0) }, egui::Modifiers::NONE);
        crate::canvas::tool_event(app, ToolEvent::Up { x, y }, egui::Modifiers::NONE);
    }
    let painted = app.session.active().map(|st| st.history.past_len()) != before;
    Ok(json!({ "rasterized": r, "painted": painted }))
}

#[cfg(test)]
#[path = "rasterize_prompt_tests.rs"]
mod tests;
