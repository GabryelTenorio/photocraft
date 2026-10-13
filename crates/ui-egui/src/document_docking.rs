//! In-app primary document views. Additional native views remain in canvas::extra_windows.
use crate::{PhotocraftApp, workspace_dock::DockHost};
use egui::{Id, Pos2, Rect, Sense, Vec2};
use serde_json::{Value, json};

pub(crate) fn canvas_id(id: u64) -> String {
    format!("__photocraft_document_{id}")
}
pub(crate) fn canvas_document(id: &str) -> Option<u64> {
    id.strip_prefix("__photocraft_document_")?.parse().ok()
}
pub(crate) fn is_canvas(id: &str) -> bool {
    id == crate::workspace_dock::CANVAS || canvas_document(id).is_some()
}
pub(crate) fn canvas_host(id: &str) -> DockHost {
    canvas_document(id).map(DockHost::DocumentView).unwrap_or(DockHost::Document)
}
pub(crate) fn detached(app: &PhotocraftApp, index: usize) -> bool {
    app.session.documents().get(index).is_some_and(|d| app.ui.floating_documents.contains_key(&d.doc.id.0))
}
fn index(app: &PhotocraftApp, id: u64) -> Option<usize> {
    app.session.documents().iter().position(|d| d.doc.id.0 == id)
}
pub(crate) fn content_rect(ctx: &egui::Context, id: u64) -> Option<Rect> {
    ctx.data(|d| d.get_temp::<Rect>(Id::new(("document-content", id))))
}
fn checked_rect(params: &Value, default: [f32; 4]) -> Result<[f32; 4], String> {
    let mut rect = default;
    for (slot, key) in rect.iter_mut().zip(["x", "y", "width", "height"]) {
        if let Some(v) = params.get(key) {
            *slot = v.as_f64().filter(|v| v.is_finite() && v.abs() <= 1_000_000.0).ok_or("geometry must contain finite coordinates")? as f32;
        }
    }
    if !rect.iter().all(|v| v.is_finite() && v.abs() <= 1_000_000.0) {
        return Err("geometry must contain finite coordinates".into());
    }
    if !(320.0..=4096.0).contains(&rect[2]) || !(240.0..=4096.0).contains(&rect[3]) {
        return Err("document size must be 320–4096 by 240–4096 points".into());
    }
    Ok(rect)
}
/// Place a primary view using its independent preferred size, keeping the header reachable.
pub(crate) fn placement(app: &PhotocraftApp, doc: usize, at: Pos2, available: Rect) -> Result<[f32; 4], String> {
    let id = app.session.documents().get(doc).ok_or("unknown document")?.doc.id.0;
    let preferred = app.ui.floating_documents.get(&id).or_else(|| app.ui.document_float_memory.get(&id)).copied().unwrap_or([100.0, 120.0, 800.0, 600.0]);
    let preferred = checked_rect(&json!({}), preferred)?;
    if !at.is_finite() || !available.is_finite() || !available.is_positive() {
        return Err("invalid document placement geometry".into());
    }
    let visible = clamp(Rect::from_min_size(at, egui::vec2(preferred[2], preferred[3])), available);
    // Clamping must not overwrite preferred dimensions on a narrow viewport.
    checked_rect(&json!({}), [visible.left(), visible.top(), preferred[2], preferred[3]])
}

pub(crate) fn detach(app: &mut PhotocraftApp, doc: usize, rect: Option<[f32; 4]>) -> Result<u64, String> {
    if app.session.prefs().workspace_locked {
        return Err("workspace is locked".into());
    }
    let id = app.session.documents().get(doc).ok_or("unknown document")?.doc.id.0;
    let preferred = rect
        .or_else(|| app.ui.floating_documents.get(&id).copied())
        .or_else(|| app.ui.document_float_memory.get(&id).copied())
        .unwrap_or([100.0, 120.0, 800.0, 600.0]);
    let preferred = checked_rect(&json!({}), preferred)?;
    if app.ui.floating_documents.len() >= 64 && !app.ui.floating_documents.contains_key(&id) {
        return Err("too many floating document views".into());
    }
    crate::panel_docking::enable_workspace(app);
    app.ui.dock_regions.view_sizes.entry(id).or_insert(app.ui.dock_regions.document_sizes);
    app.ui.floating_documents.insert(id, preferred);
    app.ui.document_float_memory.insert(id, preferred);
    Ok(id)
}
fn return_panels(app: &mut PhotocraftApp, id: u64) {
    for host in app.ui.dock_regions.hosts.values_mut() {
        if *host == DockHost::DocumentView(id) {
            *host = DockHost::Document;
        }
    }
    app.ui.dock_regions.view_sizes.remove(&id);
}
pub(crate) fn consolidate(app: &mut PhotocraftApp) {
    let ids: Vec<_> = app.ui.floating_documents.keys().copied().collect();
    for id in ids {
        return_panels(app, id);
    }
    app.ui.floating_documents.clear();
}
pub(crate) fn command(app: &mut PhotocraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if !matches!(id, "window.document.float" | "window.document.dock" | "window.document.geometry") {
        return None;
    }
    Some((|| {
        if app.session.prefs().workspace_locked {
            return Err("workspace is locked".into());
        }
        let doc = match params.get("documentView") {
            Some(v) => v.as_u64().and_then(|id| index(app, id)).ok_or("unknown documentView")?,
            None => match params.get("document") {
                Some(v) => v.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n < app.session.documents().len()).ok_or("unknown document")?,
                None => app.session.active_index().ok_or("no document")?,
            },
        };
        let document = app.session.documents().get(doc).ok_or("unknown document")?.doc.id.0;
        if id == "window.document.dock" {
            return_panels(app, document);
            app.ui.floating_documents.remove(&document);
            app.session.set_active(doc);
        } else {
            if id == "window.document.geometry" && !app.ui.floating_documents.contains_key(&document) {
                return Err("document view is not floating".into());
            }
            let rect = checked_rect(
                params,
                app.ui
                    .floating_documents
                    .get(&document)
                    .copied()
                    .or_else(|| app.ui.document_float_memory.get(&document).copied())
                    .unwrap_or([100.0, 120.0, 800.0, 600.0]),
            )?;
            detach(app, doc, Some(rect))?;
        }
        Ok(json!({"documentView":document,"floating":app.ui.floating_documents.get(&document)}))
    })())
}

pub(crate) fn canvas(app: &mut PhotocraftApp, ui: &mut egui::Ui, id: u64) {
    let Some(doc) = index(app, id) else {
        return;
    };
    let rect = ui.available_rect_before_wrap();
    if ui.is_enabled() && ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.primary_pressed()) {
        app.session.set_active(doc);
    }
    let Some(mut view) = app.ui.views.get(doc).cloned() else {
        return;
    };
    if !ui.is_enabled() {
        view.fit_pending = false;
        view.fill_pending = false;
    }
    let primary = ui.is_enabled() && app.session.active_index() == Some(doc);
    if primary {
        app.last_canvas_rect = rect;
        app.drop_canvas_rect = Some(rect);
    }
    let view = crate::canvas::canvas_view(app, ui, doc, rect, view, primary);
    if ui.is_enabled()
        && let Some(slot) = app.ui.views.get_mut(doc)
    {
        *slot = view;
    }
}

#[derive(Clone)]
struct DocumentDrag {
    id: u64,
    origin: Pos2,
    rect: Rect,
    resize: Vec2,
    generation: u64,
    tab: bool,
}
fn drag_key() -> Id {
    Id::new("photocraft-document-drag")
}

/// Observe both document tab implementations without changing their reorder behavior.
pub(crate) fn tab_gesture(app: &PhotocraftApp, ui: &egui::Ui, doc: usize, response: &egui::Response) {
    if response.drag_started()
        && !app.session.prefs().workspace_locked
        && let Some(st) = app.session.documents().get(doc)
    {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(response.rect.center());
        let preferred = app.ui.document_float_memory.get(&st.doc.id.0).copied().unwrap_or([100.0, 120.0, 800.0, 600.0]);
        let source = Rect::from_min_size(origin - egui::vec2(100.0, 14.0), egui::vec2(preferred[2], preferred[3]));
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                drag_key(),
                DocumentDrag { id: st.doc.id.0, origin, rect: source, resize: Vec2::ZERO, generation: app.ui.docking_generation, tab: true },
            )
        });
    }
}
pub(crate) fn tab_outside(ctx: &egui::Context, tab: Rect) -> bool {
    ctx.pointer_interact_pos().is_some_and(|p| p.y < tab.top() - 18.0 || p.y > tab.bottom() + 18.0)
}
pub(crate) fn blank_source(app: &PhotocraftApp, ctx: &egui::Context, doc: usize) -> bool {
    ctx.data(|d| d.get_temp::<DocumentDrag>(drag_key()))
        .is_some_and(|drag| drag.tab && index(app, drag.id) == Some(doc) && app.tab_strip.as_ref().is_some_and(|s| tab_outside(ctx, s.rect)))
}
fn clamp(rect: Rect, screen: Rect) -> Rect {
    let size = rect.size().min(screen.size()).max(Vec2::splat(1.0));
    Rect::from_min_size(rect.min.max(screen.min).min(screen.max - size), size)
}
fn resize_rect(drag: &DocumentDrag, pointer: Pos2, screen: Rect) -> Option<Rect> {
    craft_ui::docking::AnchoredResize::new(drag.rect, drag.origin, drag.resize)?.rect(
        pointer,
        screen,
        craft_ui::docking::PanelLimits { min: egui::vec2(320.0, 240.0), max: Vec2::splat(4096.0) },
    )
}
fn moved(drag: &DocumentDrag, pointer: Pos2, screen: Rect) -> Rect {
    if drag.resize == Vec2::ZERO { drag.rect.translate(pointer - drag.origin) } else { resize_rect(drag, pointer, screen).unwrap_or(drag.rect) }
}

pub(crate) fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let stale: Vec<_> = app
        .ui
        .floating_documents
        .iter()
        .filter(|(id, rect)| index(app, **id).is_none() || checked_rect(&json!({}), **rect).is_err())
        .map(|(id, _)| *id)
        .collect();
    let live: std::collections::BTreeSet<_> = app.session.documents().iter().map(|d| d.doc.id.0).collect();
    app.ui.document_float_memory.retain(|id, _| live.contains(id));
    for id in stale {
        return_panels(app, id);
        app.ui.floating_documents.remove(&id);
    }
    for host in app.ui.dock_regions.hosts.values_mut() {
        if let DockHost::DocumentView(id) = host
            && !app.ui.floating_documents.contains_key(id)
        {
            *host = DockHost::Document;
        }
    }
    let cancelled = ctx.input(|i| !i.focused || i.key_pressed(egui::Key::Escape)) || app.session.prefs().workspace_locked;
    if cancelled {
        ctx.data_mut(|d| d.remove::<DocumentDrag>(drag_key()));
    }
    let mut drag = ctx.data(|d| d.get_temp::<DocumentDrag>(drag_key()));
    if drag.as_ref().is_some_and(|d| d.generation != app.ui.docking_generation || index(app, d.id).is_none()) {
        ctx.data_mut(|d| d.remove::<DocumentDrag>(drag_key()));
        drag = None;
    }
    let screen = ctx.content_rect();
    let mut windows = app.ui.floating_documents.clone();
    if let Some(drag) = &drag
        && drag.tab
        && app.tab_strip.as_ref().is_some_and(|s| tab_outside(ctx, s.rect))
    {
        let rect = moved(drag, ctx.pointer_interact_pos().unwrap_or(drag.origin), screen);
        windows.insert(drag.id, [rect.left(), rect.top(), rect.width(), rect.height()]);
        if let Some(doc) = index(app, drag.id)
            && let Some(tab) = app.tab_strip.as_ref().and_then(|s| s.tabs.iter().find(|(i, _)| *i == doc).map(|(_, r)| *r))
        {
            ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, Id::new("document-source-placeholder"))).rect_filled(
                tab,
                0.0,
                crate::theme::Tokens::get(ctx).tab_strip,
            );
        }
    }
    for (id, stored) in windows {
        let active_drag = drag.as_ref().filter(|d| d.id == id);
        let original = Rect::from_min_size(egui::pos2(stored[0], stored[1]), egui::vec2(stored[2], stored[3]));
        let rect = clamp(active_drag.map(|d| moved(d, ctx.pointer_interact_pos().unwrap_or(d.origin), screen)).unwrap_or(original), screen);
        if active_drag.is_some_and(|d| !d.tab) {
            ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, Id::new("document-source-placeholder"))).rect_filled(
                clamp(original, screen),
                0.0,
                crate::theme::Tokens::get(ctx).card,
            );
        }
        let title = index(app, id).and_then(|i| app.session.documents().get(i)).map(|s| s.doc.name.clone()).unwrap_or_default();
        let locked = app.session.prefs().workspace_locked;
        let preview = active_drag.is_some_and(|d| d.tab);
        let mut close = false;
        let mut dock = false;
        egui::Area::new(Id::new(("floating-document", id)))
            .fade_in(false)
            .order(if active_drag.is_some() { egui::Order::Foreground } else { egui::Order::Middle })
            .fixed_pos(rect.min)
            .movable(false)
            .show(ctx, |ui| {
                ui.set_min_size(rect.size());
                ui.set_clip_rect(rect);
                let t = crate::theme::Tokens::get(ctx);
                ui.painter().rect_filled(rect, 0.0, t.card);
                let header = Rect::from_min_size(rect.min, egui::vec2(rect.width(), 28.0_f32.min(rect.height())));
                ui.painter().rect_filled(header, 0.0, t.tab_strip);
                let hit = Rect::from_min_max(header.min, egui::pos2(header.right() - 88.0, header.bottom()));
                let response = ui.interact(hit, Id::new(("document-header", id)), Sense::click_and_drag());
                ui.painter().with_clip_rect(hit).text(
                    hit.left_center() + egui::vec2(8.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &title,
                    egui::FontId::proportional(12.0),
                    t.text,
                );
                if response.clicked()
                    && let Some(doc) = index(app, id)
                {
                    app.session.set_active(doc);
                }
                if !locked && response.drag_started() {
                    let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(header.center());
                    ctx.data_mut(|d| {
                        d.insert_temp(
                            drag_key(),
                            DocumentDrag {
                                id,
                                origin,
                                rect: Rect::from_min_size(rect.min, original.size()),
                                resize: Vec2::ZERO,
                                generation: app.ui.docking_generation,
                                tab: false,
                            },
                        )
                    });
                }
                let mut buttons = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(Rect::from_min_max(egui::pos2(header.right() - 88.0, header.top()), header.max))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                dock = buttons.add_enabled(!locked, egui::Button::new(tl!("Dock")).small()).clicked();
                close = crate::icons::button(&mut buttons, "x", 20.0, false, tl!("Close")).clicked();
                let content = Rect::from_min_max(egui::pos2(rect.left(), header.bottom()), rect.max).shrink(1.0);
                ctx.data_mut(|d| d.insert_temp(Id::new(("document-content", id)), content));
                let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("document-host", id)).max_rect(content));
                child.set_clip_rect(content);
                if active_drag.is_some() {
                    child.visuals_mut().disabled_alpha = 1.0;
                    child.disable();
                }
                if preview {
                    canvas(app, &mut child, id);
                } else {
                    crate::panel_docking::show_document_host(app, &mut child, id);
                }
                let handles = [
                    ("left", Rect::from_min_max(rect.min + egui::vec2(0.0, 28.0), egui::pos2(rect.left() + 4.0, rect.bottom() - 12.0)), egui::vec2(-1.0, 0.0)),
                    ("right", Rect::from_min_max(egui::pos2(rect.right() - 4.0, rect.top() + 28.0), rect.max - egui::vec2(0.0, 12.0)), egui::vec2(1.0, 0.0)),
                    ("bottom", Rect::from_min_max(egui::pos2(rect.left() + 12.0, rect.bottom() - 4.0), rect.max - egui::vec2(12.0, 0.0)), egui::vec2(0.0, 1.0)),
                    ("corner", Rect::from_min_size(rect.max - egui::vec2(12.0, 12.0), egui::vec2(12.0, 12.0)), Vec2::splat(1.0)),
                    ("corner-left", Rect::from_min_size(egui::pos2(rect.left(), rect.bottom() - 12.0), egui::vec2(12.0, 12.0)), egui::vec2(-1.0, 1.0)),
                ];
                for (name, hit, resize) in handles {
                    let r = ui.interact(hit, Id::new(("document-resize", id, name)), Sense::drag()).on_hover_cursor(if resize.x == 0.0 {
                        egui::CursorIcon::ResizeVertical
                    } else if resize.y == 0.0 {
                        egui::CursorIcon::ResizeHorizontal
                    } else if resize.x < 0.0 {
                        egui::CursorIcon::ResizeNeSw
                    } else {
                        egui::CursorIcon::ResizeNwSe
                    });
                    if !locked && r.drag_started() {
                        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(hit.center());
                        ctx.data_mut(|d| {
                            d.insert_temp(drag_key(), DocumentDrag { id, origin, rect, resize, generation: app.ui.docking_generation, tab: false })
                        });
                    }
                }
                ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, t.card_border), egui::StrokeKind::Inside);
            });
        if dock {
            let _ = app.run("window.document.dock", json!({"documentView":id}));
        }
        if close && let Some(doc) = index(app, id) {
            let _ = crate::menus::invoke(app, ctx, "file.close", json!({"document":doc}));
        }
    }
    if let Some(drag) = drag {
        if !drag.tab && drag.resize == Vec2::ZERO {
            ctx.set_cursor_icon(egui::CursorIcon::Default);
        }
        let pointer = ctx.pointer_interact_pos().unwrap_or(drag.origin);
        let target = app.tab_strip.as_ref().is_some_and(|s| s.rect.contains(pointer));
        if target
            && drag.resize == Vec2::ZERO
            && !drag.tab
            && let Some(strip) = &app.tab_strip
        {
            ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("document-dock-preview"))).rect_stroke(
                strip.rect,
                0.0,
                egui::Stroke::new(2.0, crate::theme::Tokens::get(ctx).accent),
                egui::StrokeKind::Inside,
            );
        }
        if ctx.input(|i| i.pointer.primary_released()) {
            ctx.data_mut(|d| d.remove::<DocumentDrag>(drag_key()));
            if !drag.tab && target && drag.resize == Vec2::ZERO {
                let _ = app.run("window.document.dock", json!({"documentView":drag.id}));
            } else if (!drag.tab || app.tab_strip.as_ref().is_some_and(|s| tab_outside(ctx, s.rect)))
                && (drag.resize == Vec2::ZERO || resize_rect(&drag, pointer, screen).is_some_and(|rect| rect != drag.rect))
            {
                let rect = moved(&drag, pointer, screen);
                if app
                    .run("window.document.float", json!({"documentView":drag.id,"x":rect.left(),"y":rect.top(),"width":rect.width(),"height":rect.height()}))
                    .is_ok()
                    && let Some(doc) = index(app, drag.id)
                {
                    app.session.set_active(doc);
                }
            }
        }
    }
    crate::panel_docking::finish_frame(app, ctx);
}
