//! Paste As keeps one immutable SVG clipboard snapshot until the user confirms or cancels.

use photocraft_doc::{DocId, LayerId};
use photocraft_engine::channel_cmds::{ChannelTarget, ChannelView};
use photocraft_engine::vector_paste_cmds::{PasteMode, PreparedSvg, prepare_svg};
use photocraft_geom::Size;
use serde_json::{Value, json};

use crate::PhotocraftApp;

const MODES: [PasteMode; 5] = [PasteMode::Layers, PasteMode::SmartObject, PasteMode::Pixels, PasteMode::Path, PasteMode::Shape];

/// View targets are not document revisions, so keep them alongside the document snapshot.
#[derive(PartialEq)]
struct Target {
    doc: DocId,
    revision: u64,
    layer: Option<LayerId>,
    selected_layers: Vec<LayerId>,
    size: Size,
    channel: ChannelView,
    quick_mask: bool,
    mask: bool,
    vector_mask: bool,
    path: Option<String>,
    foreground: [f32; 4],
}

impl Target {
    fn capture(app: &PhotocraftApp) -> Option<Self> {
        let st = app.session.active()?;
        Some(Self {
            doc: st.doc.id,
            revision: st.revision,
            layer: st.active_layer,
            selected_layers: st.selected_layers.clone(),
            size: st.doc.size,
            channel: st.channel_view.clone(),
            quick_mask: st.doc.quick_mask.is_some(),
            mask: app.ui.mask_target || app.session.tools.mask_targeted || app.layer_mask_targeted(),
            vector_mask: app.ui.vector_mask_target,
            path: app.ui.selected_path.clone(),
            foreground: app.session.tools.foreground,
        })
    }

    fn normal_layers(&self) -> bool {
        self.channel.target == ChannelTarget::Composite && self.channel.layer_mask.is_none() && !self.quick_mask && !self.mask && !self.vector_mask
    }
}

/// Pending payload is never replaced by clipboard changes while the modal is open.
pub(crate) struct PendingPaste {
    target: Target,
    source: String,
    prepared: PreparedSvg,
    mode: PasteMode,
    center: [f32; 2],
    availability: [Result<(), String>; 5],
}

impl PendingPaste {
    fn capability(&self, mode: PasteMode) -> Result<(), String> {
        MODES
            .iter()
            .zip(&self.availability)
            .find(|(candidate, _)| **candidate == mode)
            .map(|(_, available)| available.clone())
            .unwrap_or_else(|| Err("Unknown vector paste mode".into()))
    }
}

/// Called only for an explicit command, before the existing image clipboard path.
pub(crate) fn intercept(app: &mut PhotocraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != "edit.paste" || app.automation_input || !(params.is_null() || params.as_object().is_some_and(|p| p.is_empty())) {
        return None;
    }
    if app.save_options_open() {
        let error = "Answer TIFF or Targa Options before starting Paste As".to_string();
        app.ui.status = error.clone();
        app.ui.status_error = true;
        return Some(Err(error));
    }
    if app.ui.cam_transform.is_some() || app.ui.transform.as_ref().is_some_and(|t| t.content_aware.is_some()) {
        let error = "Commit or cancel the transform first".to_string();
        app.ui.status = error.clone();
        app.ui.status_error = true;
        return Some(Err(error));
    }
    if app.vector_paste.is_some() {
        return Some(Ok(json!({"pasted": false, "deferred": true})));
    }
    // Inline type retains its existing text-paste handler. Mask and
    // channel targets retain the image clipboard path, which knows their coverage semantics.
    if app.ui.text_edit.is_some() {
        return None;
    }
    let target = Target::capture(app)?;
    if !target.normal_layers() {
        return None;
    }
    let get = app.services.clipboard_get_vector.as_mut()?;
    let pending: Result<Option<PendingPaste>, String> = (|| {
        let Some(clip) = get()? else { return Ok(None) };
        let prepared = prepare_svg(&clip.svg).map_err(|e| e.to_string())?;
        let source = String::from_utf8(clip.svg).map_err(|_| "The vector clipboard is not UTF-8 SVG".to_string())?;
        let center = app
            .session
            .active_index()
            .and_then(|i| app.ui.views.get(i))
            .map_or([target.size.width as f32 / 2.0, target.size.height as f32 / 2.0], |view| view.center);
        let availability = MODES.map(|mode| prepared.capability_at(mode, target.size, center.map(f64::from)));
        Ok(Some(PendingPaste { target, source, prepared, mode: PasteMode::Pixels, center, availability }))
    })();
    match pending {
        Ok(None) => None,
        Ok(Some(pending)) => {
            app.vector_paste = Some(pending);
            Some(Ok(json!({"pasted": false, "deferred": true})))
        }
        Err(e) => {
            app.ui.status = e.clone();
            app.ui.status_error = true;
            Some(Err(e))
        }
    }
}

fn translated(source: &str) -> &str {
    crate::i18n::tr_ctx(crate::i18n::current(), "vectorPaste", source)
}

fn confirm(app: &mut PhotocraftApp) -> Result<Value, String> {
    if app.ui.cam_transform.is_some() || app.ui.transform.as_ref().is_some_and(|t| t.content_aware.is_some()) {
        return Err("Commit or cancel the transform first".into());
    }
    let Some(pending) = app.vector_paste.take() else { return Ok(json!({"pasted": false})) };
    if Target::capture(app).as_ref() != Some(&pending.target) || app.ui.text_edit.is_some() {
        return Err("Paste cancelled: the document or editing target changed".into());
    }
    pending.capability(pending.mode)?;
    let mode = pending.mode;
    let result =
        app.run("edit.pasteVector", json!({"svg": pending.source, "mode": mode.as_str(), "document": pending.target.doc.0, "center": pending.center}))?;
    if mode == PasteMode::Path {
        app.ui.selected_path = result.get("path").and_then(Value::as_str).map(str::to_owned);
    }
    Ok(result)
}

pub(crate) fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let Some(pending) = app.vector_paste.as_mut() else { return };
    let mut ok = false;
    let mut cancel = false;
    let modal = egui::Modal::new(egui::Id::new("vector-paste-as")).show(ctx, |ui| {
        let tokens = crate::theme::Tokens::get(ctx);
        ui.set_max_width(440.0);
        ui.label(egui::RichText::new(translated("Paste")).font(crate::theme::semibold(15.0)));
        ui.add_space(4.0);
        crate::widgets::hairline(ui);
        ui.add_space(8.0);
        ui.label(translated("Paste as:"));
        for mode in MODES {
            let capability = pending.capability(mode);
            ui.add_enabled_ui(capability.is_ok(), |ui| {
                ui.radio_value(&mut pending.mode, mode, translated(mode.label()));
            });
            if let Err(reason) = capability {
                ui.label(egui::RichText::new(translated(&reason)).small().color(tokens.text_dim));
            }
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new(translated("SVG text is converted to outlines, not editable type.")).small().color(tokens.text_dim));
        if !pending.prepared.warnings().is_empty() {
            ui.label(translated("Import warnings:"));
            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for warning in pending.prepared.warnings() {
                    ui.label(egui::RichText::new(warning).small().color(tokens.text_dim));
                }
            });
        }
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ok = ui.add_enabled_ui(pending.capability(pending.mode).is_ok(), |ui| crate::widgets::primary_button(ui, "OK", 84.0)).inner.clicked();
            cancel = crate::widgets::secondary_button(ui, "Cancel", 84.0).clicked();
        });
    });
    // egui records the top modal at the end of the frame. A newly opened modal owns
    // its keys immediately when no earlier modal owns them.
    let owns_keys = modal.is_top_modal || ctx.memory(|memory| memory.top_modal_layer().is_none());
    // Escape/backdrop always wins over Enter, including a frame that contains both keys.
    cancel |=
        modal.should_close() || (owns_keys && !modal.any_popup_open && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)));
    if cancel {
        app.vector_paste = None;
        return;
    }
    if (ok || (owns_keys && !modal.any_popup_open && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))))
        && let Err(error) = confirm(app)
    {
        app.ui.status = error;
        app.ui.status_error = true;
    }
}

#[cfg(test)]
mod tests;
