//! Per-filter reorder/delete convenience actions. The engine owns edits and Undo/Redo; a
//! transient weak snapshot prevents an open menu from following a different filter or document.

use std::sync::{Arc, Weak};

use photocraft_doc::{DocId, Document, LayerContent, LayerId};
use serde_json::{Value, json};

use crate::PhotocraftApp;

#[derive(Clone)]
struct Target {
    snapshot: Weak<Document>,
    document: DocId,
    revision: u64,
    layer: LayerId,
    index: usize,
    popup: egui::Id,
}

fn state_id() -> egui::Id {
    egui::Id::new("smart-filter-row-menu-target")
}

#[derive(Clone)]
struct PendingSelection {
    target: Target,
    command: &'static str,
    params: Value,
    to: Option<usize>,
}

fn pending_id() -> egui::Id {
    egui::Id::new("smart-filter-row-menu-selection")
}

/// A menu edit changes storage indices. Apply its row selection only after the engine succeeds.
pub(crate) struct SelectionUpdate {
    document: DocId,
    before: super::FilterDrag,
    after: Option<super::FilterDrag>,
}

impl SelectionUpdate {
    pub(crate) fn apply(self, ctx: &egui::Context) {
        ctx.data_mut(|data| {
            let key = super::selected_key(self.document);
            if data.get_temp::<super::FilterDrag>(key) == Some(self.before) {
                if let Some(after) = self.after {
                    data.insert_temp(key, after);
                } else {
                    data.remove::<super::FilterDrag>(key);
                }
            }
        });
    }
}

/// Only choices queued by this row menu remap selection; other command origins keep their policy.
pub(crate) fn selection_update(app: &PhotocraftApp, ctx: &egui::Context, command: &str, params: &Value) -> Option<SelectionUpdate> {
    let pending = ctx.data(|d| d.get_temp::<PendingSelection>(pending_id()))?;
    if pending.command != command || &pending.params != params {
        return None;
    }
    ctx.data_mut(|d| d.remove::<PendingSelection>(pending_id()));
    let count = pending.target.count(app)?;
    let before = ctx.data(|d| d.get_temp::<super::FilterDrag>(super::selected_key(pending.target.document)))?;
    let (layer, Some(picked)) = before else { return None };
    if layer != pending.target.layer.0 || picked >= count {
        return None;
    }
    let from = pending.target.index;
    let next = match pending.to {
        Some(to) if picked == from => Some(to),
        Some(to) if from < to && picked > from && picked <= to => picked.checked_sub(1),
        Some(to) if from > to && picked >= to && picked < from => picked.checked_add(1),
        None if picked == from => None,
        None if picked > from => picked.checked_sub(1),
        _ => Some(picked),
    };
    Some(SelectionUpdate { document: pending.target.document, before, after: next.map(|index| (layer, Some(index))) })
}

fn queue(ctx: &egui::Context, target: &Target, command: &'static str, params: Value, to: Option<usize>, actions: &mut Vec<(String, Value)>) {
    ctx.data_mut(|d| d.insert_temp(pending_id(), PendingSelection { target: target.clone(), command, params: params.clone(), to }));
    actions.push((command.into(), params));
}

impl Target {
    fn count(&self, app: &PhotocraftApp) -> Option<usize> {
        let st = app.session.active()?;
        if st.doc.id != self.document
            || st.revision != self.revision
            || !self.snapshot.ptr_eq(&Arc::downgrade(&st.doc))
            || app.session.job_on(self.document).is_some()
            || !crate::menus::modal_allows(app, "layer.smartFilter.move")
        {
            return None;
        }
        let LayerContent::Smart(sm) = &st.doc.layer(self.layer)?.content else { return None };
        sm.smart_filters.get(self.index)?;
        Some(sm.smart_filters.len())
    }

    fn params(&self) -> Value {
        json!({"layer": self.layer.0, "index": self.index})
    }

    fn enabled(&self, app: &mut PhotocraftApp, command: &str) -> bool {
        // The immutable menu query checks the active layer. Query the named layer, like the
        // engine's explicit-layer dispatch, then restore it without changing selection/history.
        let Some(st) = app.session.active_mut() else { return false };
        let active = st.active_layer.replace(self.layer);
        let enabled = app.session.is_enabled_with(command, &self.params());
        if let Some(st) = app.session.active_mut() {
            st.active_layer = active;
        }
        enabled
    }
}

/// Runs even when the new document has no filter rows, so a stale popup cannot remain open.
pub(crate) fn close_stale(app: &PhotocraftApp, ctx: &egui::Context) {
    let target: Option<Target> = ctx.data(|d| d.get_temp(state_id()));
    if let Some(target) = target
        && (target.count(app).is_none() || !egui::Popup::is_id_open(ctx, target.popup))
    {
        egui::Popup::close_id(ctx, target.popup);
        ctx.data_mut(|d| d.remove::<Target>(state_id()));
        ctx.data_mut(|d| d.remove::<PendingSelection>(pending_id()));
    }
}

pub(crate) fn show(app: &mut PhotocraftApp, response: &egui::Response, layer: LayerId, index: usize, actions: &mut Vec<(String, Value)>) {
    let ctx = &response.ctx;
    let popup = egui::Id::new(("smart-filter-row-menu", layer.0, index));
    if response.secondary_clicked()
        && let Some(st) = app.session.active()
    {
        let target = Target { snapshot: Arc::downgrade(&st.doc), document: st.doc.id, revision: st.revision, layer, index, popup };
        if target.count(app).is_some() {
            ctx.data_mut(|d| d.insert_temp(state_id(), target));
        }
    }
    let Some(target) = ctx.data(|d| d.get_temp::<Target>(state_id())).filter(|t| t.popup == popup) else { return };
    let Some(count) = target.count(app) else {
        egui::Popup::close_id(ctx, popup);
        ctx.data_mut(|d| d.remove::<Target>(state_id()));
        return;
    };
    egui::Popup::context_menu(response).id(popup).show(|ui| {
        ui.set_min_width(180.0);
        let enabled = target.enabled(app, "layer.smartFilter.move");
        // Filters display in reverse storage order: index + 1 moves the row visually up.
        let up = index.checked_add(1).filter(|to| *to < count);
        let down = index.checked_sub(1);
        for (label, to) in [(tl!("Move Up"), up), (tl!("Move Down"), down)] {
            if ui.add_enabled(enabled && to.is_some(), egui::Button::new(label)).clicked()
                && let Some(to) = to
            {
                queue(ctx, &target, "layer.smartFilter.move", json!({"layer": target.layer.0, "index": target.index, "to": to}), Some(to), actions);
                ui.close();
            }
        }
        ui.separator();
        if ui.add_enabled(target.enabled(app, "layer.smartFilter.delete"), egui::Button::new(tl!("Delete Smart Filter"))).clicked() {
            queue(ctx, &target, "layer.smartFilter.delete", target.params(), None, actions);
            ui.close();
        }
    });
}
