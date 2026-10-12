//! Primary document presentation and the single canvas input owner.
//! Extra New Window views keep an independent camera and never own editing tools.
use crate::PhotocraftApp;
use photocraft_doc::DocId;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Owner {
    document: DocId,
    window: Option<u64>,
}
#[derive(Default)]
pub(crate) struct State {
    owner: Option<Owner>,
    root_document: Option<DocId>,
    pointer_held: bool,
}

pub(crate) fn primary_window(app: &PhotocraftApp, document: usize) -> Option<u64> {
    app.ui.windows.iter().find(|w| w.document == document && w.open && w.primary).map(|w| w.id)
}
pub(crate) fn docked(app: &PhotocraftApp) -> Vec<usize> {
    (0..app.session.documents().len()).filter(|&d| primary_window(app, d).is_none()).collect()
}
pub(crate) fn index(app: &PhotocraftApp, document: DocId) -> Option<usize> {
    app.session.documents().iter().position(|d| d.doc.id == document)
}
pub(crate) fn root_index(app: &PhotocraftApp) -> Option<usize> {
    app.session
        .active_index()
        .filter(|&d| primary_window(app, d).is_none())
        .or_else(|| app.document_windows.root_document.and_then(|id| index(app, id)).filter(|&d| primary_window(app, d).is_none()))
        .or_else(|| docked(app).first().copied())
}
fn owner_for(app: &PhotocraftApp, document: usize) -> Option<Owner> {
    Some(Owner { document: app.session.documents().get(document)?.doc.id, window: primary_window(app, document) })
}
pub(crate) fn prune(app: &mut PhotocraftApp) {
    if app.document_windows.owner.is_some_and(|owner| index(app, owner.document).is_none_or(|d| primary_window(app, d) != owner.window)) {
        app.document_windows.owner = None;
        app.document_windows.pointer_held = false;
    }
    if app.document_windows.root_document.is_some_and(|id| index(app, id).is_none_or(|d| primary_window(app, d).is_some())) {
        app.document_windows.root_document = None;
    }
}
fn select(app: &mut PhotocraftApp, document: usize, window: Option<u64>) {
    let Some(doc) = app.session.documents().get(document) else { return };
    let owner = Owner { document: doc.doc.id, window };
    if app.document_windows.pointer_held && app.document_windows.owner != Some(owner) {
        return;
    }
    app.document_windows.owner = Some(owner);
    if window.is_none() {
        app.document_windows.root_document = Some(owner.document);
    }
    app.session.set_active(document);
}
/// Before root command dispatch, use native focus metadata once, never child render order.
pub(crate) fn prepare(app: &mut PhotocraftApp, ctx: &egui::Context) {
    prune(app);
    if app.document_windows.pointer_held || modal(app) {
        return;
    }
    let focused = ctx.input(|i| i.raw.viewports.iter().find_map(|(id, info)| (info.focused == Some(true)).then_some(*id)));
    if let Some((document, window)) =
        focused.and_then(|id| app.ui.windows.iter().find(|w| w.open && w.primary && viewport(w.id) == id).map(|w| (w.document, w.id)))
    {
        select(app, document, Some(window));
    } else if let Some(document) = app.session.active_index() {
        app.document_windows.owner = owner_for(app, document);
        if primary_window(app, document).is_none() {
            app.document_windows.root_document = app.document_windows.owner.map(|o| o.document);
        }
    }
}
pub(crate) fn owns(app: &PhotocraftApp, document: usize, window: Option<u64>) -> bool {
    app.session
        .documents()
        .get(document)
        .is_some_and(|d| app.session.active_index() == Some(document) && app.document_windows.owner == Some(Owner { document: d.doc.id, window }))
}
pub(crate) fn root_keys(app: &PhotocraftApp, ctx: &egui::Context) -> bool {
    app.automation_input || (ctx.input(|i| i.focused) && !(ctx.embed_viewports() && app.document_windows.owner.is_some_and(|o| o.window.is_some())))
}
/// The root UI retains the selected floating tool target until an actual docked canvas click.
pub(crate) fn root_canvas(app: &mut PhotocraftApp, ui: &egui::Ui, document: usize, rect: egui::Rect) -> bool {
    if canvas_pressed(ui, rect)
        || (app.document_windows.owner.is_none() && app.session.active_index() == Some(document) && primary_window(app, document).is_none())
    {
        select(app, document, None);
    }
    owns(app, document, None)
}
fn canvas_pressed(ui: &egui::Ui, rect: egui::Rect) -> bool {
    let at = ui.input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()).flatten());
    at.is_some_and(|p| rect.contains(p) && ui.ctx().layer_id_at(p) == Some(ui.layer_id()))
}
pub(crate) fn child_canvas(app: &mut PhotocraftApp, ui: &egui::Ui, document: usize, window: u64, embedded: bool, rect: egui::Rect) -> bool {
    if !modal(app) && ((!embedded && ui.input(|i| i.focused)) || canvas_pressed(ui, rect)) {
        select(app, document, Some(window));
    }
    owns(app, document, Some(window))
}
fn modal(app: &PhotocraftApp) -> bool {
    app.discard.is_some() || app.camera_raw.is_some() || !app.ui.dialogs.is_empty()
}

pub(crate) fn after_canvas(app: &mut PhotocraftApp, ctx: &egui::Context) {
    app.document_windows.pointer_held = ctx.input(|i| i.pointer.any_down());
}
pub(crate) fn consolidate(app: &mut PhotocraftApp) {
    app.document_windows = State::default();
    if let Some(document) = app.session.active_index() {
        select(app, document, None);
    }
}
pub(crate) fn viewport(window: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("docwin", window))
}
