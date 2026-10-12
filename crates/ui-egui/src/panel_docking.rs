//! Application-owned panel IDs and commands around the shared docking layout.

use crate::{PhotocraftApp, modules};
use craft_ui::docking::{Action, Layout, Node, Placement, Zone};
use craft_ui::layout::{SplitAxis, SplitSize};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct LegacyDrag {
    generation: u64,
    layout: Option<Layout<String>>,
    dock: crate::dock::DockLayout,
    hidden: std::collections::BTreeMap<String, craft_ui::docking::Location<String>>,
}

fn cancel_legacy_drag(app: &mut PhotocraftApp, ctx: &egui::Context) -> bool {
    let key = egui::Id::new("photocraft-panel-docking").with("legacy-origin");
    let previous = ctx.data_mut(|data| data.remove_temp::<LegacyDrag>(key));
    if let Some(previous) = previous {
        craft_ui::docking::cancel_drag::<String>(ctx, egui::Id::new("photocraft-panel-docking"));
        if previous.generation != app.ui.docking_generation {
            return false;
        }
        app.ui.dock = previous.dock;
        app.ui.docking = previous.layout;
        app.ui.docking_hidden = previous.hidden;
        true
    } else {
        false
    }
}

pub(crate) fn normalize(raw: &str) -> Option<&'static str> {
    modules::ALL.iter().find(|m| m.id.eq_ignore_ascii_case(raw.trim()) || m.title.eq_ignore_ascii_case(raw.trim())).map(|m| m.id)
}

pub(crate) fn valid(layout: &Layout<String>) -> bool {
    layout.validate().is_ok() && layout.panels().iter().all(|id| normalize(id) == Some(id.as_str()))
}

fn legacy(app: &PhotocraftApp) -> Layout<String> {
    legacy_layout(app, false)
}

fn legacy_layout(app: &PhotocraftApp, all: bool) -> Layout<String> {
    let mut dock = app.ui.dock.clone();
    if all {
        for module in modules::ALL {
            if !dock.visible(module.id) {
                dock.show(module.id);
            }
        }
    }
    let mut layout = Layout::default();
    for pane in dock.panes.iter().rev() {
        let panels: Vec<String> = pane.tabs.iter().filter(|id| modules::get(id).is_some()).cloned().collect();
        if panels.is_empty() {
            continue;
        }
        let active = panels.iter().position(|id| id == pane.front()).unwrap_or(0);
        let node = Node::Tabs { panels, active };
        layout.root = Some(match layout.root.take() {
            Some(second) => {
                Node::Split { axis: SplitAxis::Vertical, size: SplitSize::FixedFirst(pane.height()), first: Box::new(node), second: Box::new(second) }
            }
            None => node,
        });
    }
    layout
}

// Native collapse flags remain app-owned. An intact native tab group keeps its collapsed
// presentation when the workspace first becomes customizable; mixed custom groups are expanded.
fn collapsed_members(app: &PhotocraftApp, layout: &Layout<String>) -> std::collections::HashSet<String> {
    let mut collapsed: std::collections::HashSet<String> = app.ui.docking_collapsed.iter().cloned().collect();
    for members in root_groups(layout) {
        if app.ui.dock.panes.iter().any(|pane| pane.collapsed && pane.tabs.len() == members.len() && members.iter().all(|id| pane.tabs.contains(id))) {
            collapsed.extend(members);
        }
    }
    collapsed
}

fn first_root(layout: &Layout<String>, except: &str) -> Option<String> {
    let root = Layout { root: layout.root.clone(), floating: Vec::new() };
    root.panels().into_iter().find(|p| p.as_str() != except).cloned()
}

fn ensure(layout: &mut Layout<String>, panel: &str) -> Result<(), String> {
    let anchor = first_root(layout, panel);
    layout.apply(Action::Open { panel: panel.into(), anchor }).map_err(|e| e.to_string())
}

fn number(params: &Value, key: &str, default: f32) -> Result<f32, String> {
    match params.get(key) {
        None => Ok(default),
        Some(v) => v.as_f64().filter(|v| v.is_finite() && v.abs() <= 1_000_000.0).map(|v| v as f32).ok_or_else(|| format!("{key} must be a finite coordinate")),
    }
}

fn placement(params: &Value) -> Result<Placement<String>, String> {
    if let Some(before) = params.get("before") {
        let id = before.as_str().and_then(normalize).ok_or("before must name a panel")?;
        return Ok(Placement::Tab { before: Some(id.into()) });
    }
    let zone = match params.get("zone") {
        None => "tab",
        Some(value) => value.as_str().ok_or("zone must be a string")?,
    };
    Ok(match zone {
        "tab" => Placement::Tab { before: None },
        "center" => Placement::Split(Zone::Center),
        "left" => Placement::Split(Zone::Left),
        "right" => Placement::Split(Zone::Right),
        "top" => Placement::Split(Zone::Top),
        "bottom" => Placement::Split(Zone::Bottom),
        _ => return Err("zone must be tab, center, left, right, top or bottom".into()),
    })
}

pub(crate) fn command(app: &mut PhotocraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id == "window.panel.group" {
        return Some(group_command(app, params));
    }
    if id == "window.panel.layout" {
        return Some(
            params
                .get("action")
                .ok_or_else(|| "action is required".to_string())
                .and_then(|value| serde_json::from_value::<Action<String>>(value.clone()).map_err(|error| error.to_string()))
                .and_then(|action| apply_action(app, action)),
        );
    }
    if let Some(layout) = &app.ui.docking
        && let Some(panel) = modules::by_menu_id(id).map(|m| m.id).or_else(|| id.strip_prefix("window.toggle.").and_then(normalize))
    {
        let showing = layout.contains(&panel.to_string());
        let show = match params.get("show").or_else(|| params.get("on")) {
            None => !showing || !panel_active(layout, panel) || collapsed_members(app, layout).contains(panel),
            Some(value) => match value.as_bool() {
                Some(show) => show,
                None => return Some(Err("show must be a boolean".into())),
            },
        };
        return Some(change(app, if show { "activate" } else { "close" }, &json!({"panel":panel})));
    }
    let operation = match id {
        "window.panel.move" => "move",
        "window.panel.float" => "float",
        "window.panel.dock" => "dock",
        "window.panel.activate" => "activate",
        "window.panel.close" => "close",
        "window.panel" if app.ui.docking.is_some() => "activate",
        _ => return None,
    };
    Some(change(app, operation, params))
}

pub(crate) fn checked(app: &PhotocraftApp, id: &str) -> Option<bool> {
    let layout = app.ui.docking.as_ref()?;
    let panel = modules::by_menu_id(id).map(|m| m.id).or_else(|| id.strip_prefix("window.toggle.").and_then(normalize))?;
    Some(layout.contains(&panel.to_string()))
}

// UI output and automation share this atomic dispatcher. The application owns the registry
// and saved return locations; craft-ui owns tree validation and structural changes.
fn apply_action(app: &mut PhotocraftApp, action: Action<String>) -> Result<Value, String> {
    if app.session.prefs().workspace_locked && !matches!(action, Action::Activate { .. }) {
        return Err("workspace is locked".into());
    }
    let registered = |id: &String| normalize(id) == Some(id.as_str());
    let known = match &action {
        Action::Move { panel, anchor, placement } | Action::OpenAt { panel, anchor, placement } => {
            registered(panel)
                && registered(anchor)
                && match placement {
                    Placement::Tab { before: Some(id) } => registered(id),
                    _ => true,
                }
        }
        Action::Open { panel, anchor } => registered(panel) && anchor.as_ref().is_none_or(registered),
        Action::Float { panel, .. }
        | Action::Close { panel }
        | Action::Activate { panel }
        | Action::MoveFloating { panel, .. }
        | Action::SetStackOpen { panel, .. }
        | Action::ResizeStack { panel, .. } => registered(panel),
        Action::ResizeSplit { path, .. } => path.len() <= craft_ui::docking::MAX_DEPTH,
    };
    if !known {
        return Err("docking action contains an unknown panel or an excessive split path".into());
    }
    if app.ui.docking.as_ref().is_some_and(|layout| !valid(layout)) {
        return Err("saved panel layout is invalid; reset the workspace first".into());
    }
    let mut layout = app.ui.docking.clone().unwrap_or_else(|| legacy(app));
    let returning = match &action {
        Action::Close { panel } => layout.location(panel).ok().map(|location| (panel.clone(), location)),
        Action::Float { panel, .. } => layout.location(panel).ok().filter(|location| location.floating.is_none()).map(|location| (panel.clone(), location)),
        _ => None,
    };
    let reveal = match &action {
        Action::Activate { panel } => Some(panel.clone()),
        _ => None,
    };
    let activate = match &action {
        Action::Activate { panel } => Some(panel.clone()),
        _ => None,
    };
    layout.apply(action).map_err(|error| error.to_string())?;
    if let Some(panel) = activate
        && let Some(index) = app.ui.dock.pane_of(&panel)
    {
        app.ui.dock.set_collapsed(index, false);
    }
    if let Some((panel, location)) = returning {
        app.ui.docking_hidden.insert(panel, location);
    }
    if app.ui.docking.is_none() {
        app.ui.docking_hidden.extend(migration_hidden(app, &layout));
    }
    app.ui.docking = Some(layout);
    app.ui.panels.dock = true;
    sync_visibility(app);
    if let Some(panel) = reveal {
        app.ui.docking_reveal = Some(panel.clone());
        reveal_group(app, &panel);
    }
    Ok(json!({"layout": app.ui.docking}))
}

fn change(app: &mut PhotocraftApp, operation: &str, params: &Value) -> Result<Value, String> {
    if app.session.prefs().workspace_locked && operation != "activate" {
        return Err("workspace is locked".into());
    }
    let panel = params.get("panel").and_then(Value::as_str).and_then(normalize).ok_or("panel must name a registered panel")?;
    if app.ui.docking.as_ref().is_some_and(|layout| !valid(layout)) {
        return Err("saved panel layout is invalid; reset the workspace first".into());
    }
    let mut layout = app.ui.docking.clone().unwrap_or_else(|| legacy(app));
    let migrated = if app.ui.docking.is_none() { migration_hidden(app, &layout) } else { app.ui.docking_hidden.clone() };
    let mut hidden: std::collections::BTreeMap<_, _> =
        migrated.iter().filter(|(id, _)| normalize(id).is_some()).take(64).map(|(id, at)| (id.clone(), at.clone())).collect();
    let mut restored = false;
    if operation == "dock"
        && params.get("anchor").or_else(|| params.get("onto")).is_none()
        && let Some(location) = hidden.get(panel).filter(|location| {
            location.floating.is_none()
                && location.anchor.as_ref().is_none_or(|anchor| Layout { root: layout.root.clone(), floating: Vec::new() }.contains(anchor))
        })
    {
        let mut candidate = layout.clone();
        if candidate.contains(&panel.to_string()) {
            candidate.apply(Action::Close { panel: panel.into() }).map_err(|e| e.to_string())?;
        }
        if candidate.restore(panel.into(), location).is_ok() {
            layout = candidate;
            restored = true;
            hidden.remove(panel);
        }
    }
    match operation {
        "dock" if restored => {}
        "float" => {
            let rect = [number(params, "x", 400.0)?, number(params, "y", 140.0)?, number(params, "width", 300.0)?, number(params, "height", 400.0)?];
            ensure(&mut layout, panel)?;
            if let Ok(location) = layout.location(&panel.to_string())
                && location.floating.is_none()
            {
                hidden.insert(panel.into(), location);
            }
            layout.apply(Action::Float { panel: panel.into(), rect }).map_err(|e| e.to_string())?;
        }
        "move" | "dock" => {
            if operation == "move" && params.get("anchor").or_else(|| params.get("onto")).is_none() {
                return Err("move requires a destination panel".into());
            }
            let anchor = match params.get("anchor").or_else(|| params.get("onto")) {
                Some(v) => Some(v.as_str().and_then(normalize).ok_or("anchor must name a registered panel")?.to_string()),
                None => first_root(&layout, panel),
            };
            let placement = placement(params)?;
            if let Some(anchor) = anchor {
                ensure(&mut layout, panel)?;
                layout.apply(Action::Move { panel: panel.into(), anchor, placement }).map_err(|e| e.to_string())?;
            } else if operation == "dock" {
                if layout.contains(&panel.to_string()) {
                    layout.apply(Action::Close { panel: panel.into() }).map_err(|e| e.to_string())?;
                }
                layout.apply(Action::Open { panel: panel.into(), anchor: None }).map_err(|e| e.to_string())?;
            } else {
                return Err("move requires an existing destination panel".into());
            }
        }
        "close" => {
            hidden.insert(panel.into(), layout.location(&panel.to_string()).map_err(|e| e.to_string())?);
            layout.apply(Action::Close { panel: panel.into() }).map_err(|e| e.to_string())?;
        }
        _ => {
            if !layout.contains(&panel.to_string()) && hidden.contains_key(panel) {
                // A missing neighbor is allowed: the normal visible dock becomes the fallback.
                restore_hidden(&mut layout, &mut hidden, panel);
            }
            ensure(&mut layout, panel)?;
            layout.apply(Action::Activate { panel: panel.into() }).map_err(|e| e.to_string())?;
            hidden.remove(panel);
        }
    }
    app.ui.docking = Some(layout);
    app.ui.docking_hidden = hidden;
    app.ui.panels.dock = true;
    sync_visibility(app);
    if operation == "activate" {
        if let Some(index) = app.ui.dock.pane_of(panel) {
            app.ui.dock.set_collapsed(index, false);
        }
        app.ui.docking_reveal = Some(panel.to_owned());
        reveal_group(app, panel);
    }
    Ok(json!({"panel": panel, "layout": app.ui.docking}))
}

pub(crate) fn show(app: &mut PhotocraftApp, ui: &mut egui::Ui) -> bool {
    let area = egui::Id::new("photocraft-panel-docking");
    let stale_origin =
        ui.ctx().data_mut(|data| data.get_temp::<LegacyDrag>(area.with("legacy-origin")).is_some_and(|origin| origin.generation != app.ui.docking_generation));
    let changed_workspace = ui.ctx().data_mut(|data| {
        let previous = data.get_temp::<u64>(area.with("generation"));
        data.insert_temp(area.with("generation"), app.ui.docking_generation);
        previous.is_some_and(|generation| generation != app.ui.docking_generation)
    });
    if changed_workspace || stale_origin {
        ui.ctx().data_mut(|data| data.insert_temp(area.with("legacy-suppressed"), true));
        craft_ui::docking::cancel_drag::<String>(ui.ctx(), area);
        ui.ctx().data_mut(|data| data.remove::<LegacyDrag>(area.with("legacy-origin")));
    }

    let cancelled =
        ui.input(|input| !input.focused || input.key_pressed(egui::Key::Escape) || (!input.pointer.primary_down() && !input.pointer.any_released()));
    if cancelled && cancel_legacy_drag(app, ui.ctx()) {
        ui.ctx().data_mut(|data| data.insert_temp(area.with("legacy-suppressed"), true));
        if app.ui.docking.is_none() {
            return false;
        }
    }
    let Some(layout) = app.ui.docking.as_ref() else { return false };
    if !valid(layout) {
        app.ui.docking = None;
        let message = "Invalid panel layout; restored the default dock";
        app.ui.status = tl!(message).into();
        return false;
    }
    let layout = layout.clone();
    let reveal = app.ui.docking_reveal.take();
    let t = crate::theme::Tokens::get(ui.ctx());
    let locked = app.session.prefs().workspace_locked;
    let collapsed = collapsed_members(app, &layout);
    let requested_width = ui.ctx().data_mut(|data| data.remove_temp::<f32>(crate::panels::dock_width_id()));
    let mut commands = Vec::new();
    let mut targets = Vec::new();
    let mut draw = |ui: &mut egui::Ui| {
        let mut style = craft_ui::docking::DockStyle::from_ui(ui);
        style.tab_height = if t.pro { 28.0 } else { 40.0 };
        style.min_pane = 0.0;
        style.background = t.card;
        style.tab_background = t.tab_strip;
        style.active_background = t.card;
        style.text = t.text;
        style.inactive_text = t.text_dim;
        style.border = egui::Stroke::new(1.0, t.card_border);
        style.accent = t.accent;
        style.float_label = tl!("Float Panel").to_string();
        style.close_label = tl!("Close Panel").to_string();
        style.move_label = tl!("Group with").to_string();
        style.panels_label = tl!("Panels").to_string();
        style.resize_label = tl!("Resize panels").to_string();
        style.resize_window_label = tl!("Resize panel window").to_string();
        let mut area = craft_ui::docking::DockArea::new(egui::Id::new("photocraft-panel-docking"));
        if let Some(panel) = &reveal {
            area = area.raise_panel(panel.clone());
        }
        area.show_customized(
            ui,
            &layout,
            &style,
            |id| modules::get(id).map(|m| tl!(m.title).to_string()).unwrap_or_else(|| id.clone()),
            |_| craft_ui::docking::Permissions { movable: !locked, floatable: !locked, closable: !locked, accepts_tabs: !locked },
            |id| {
                if collapsed.contains(id) {
                    craft_ui::docking::PanelLimits { min: egui::vec2(220.0, 0.0), max: egui::vec2(1_000_000.0, 0.0) }
                } else {
                    // Layers owns its row scroll and footer; leave room for its fixed controls
                    // plus a scrolling row area instead of clipping the footer after a split.
                    let height = if id == "layers" { 220.0 } else { 80.0 };
                    craft_ui::docking::PanelLimits { min: egui::vec2(220.0, height), ..Default::default() }
                }
            },
            &mut PanelContent { app, collapsed: &collapsed, commands: &mut commands, targets: &mut targets },
        )
    };
    let output = if layout.root.is_some() {
        let mut panel = crate::direction::end_panel("shared-panel-dock")
            .default_size(if t.pro { 290.0 } else { 300.0 })
            .size_range(crate::panels::DOCK_WIDTH)
            .resizable(!locked)
            .frame(egui::Frame::NONE.fill(t.card));
        if let Some(width) = requested_width {
            panel = panel.exact_size(width);
        }
        panel.show(ui, &mut draw).inner
    } else {
        draw(ui)
    };
    if let Some(error) = output.error {
        app.ui.status = error.to_string();
    }
    if ui.input(|input| input.pointer.any_released()) {
        if !output.actions.iter().any(|action| matches!(action, Action::Move { .. } | Action::Float { .. })) && cancel_legacy_drag(app, ui.ctx()) {
            return app.ui.docking.is_some();
        }
        ui.ctx().data_mut(|data| data.remove::<LegacyDrag>(egui::Id::new("photocraft-panel-docking").with("legacy-origin")));
    }
    if let Some(panel) = output.group_drag.as_ref()
        && let Some(pointer) = ui.ctx().pointer_interact_pos()
    {
        let members = group_members(&layout, panel);
        let eligible: Vec<_> = targets.iter().filter(|target| !members.contains(&target.anchor)).cloned().collect();
        if let Some(proposal) = craft_ui::docking::resolve_drop(panel, pointer, &eligible) {
            ui.painter().rect_stroke(proposal.preview, 0.0, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        }
    }
    if let Some((panel, pointer)) = output.group_drop {
        let members = group_members(&layout, &panel);
        let eligible: Vec<_> = targets.iter().filter(|target| !members.contains(&target.anchor)).cloned().collect();
        if let Some(proposal) = craft_ui::docking::resolve_drop(&panel, pointer, &eligible) {
            commands.push(("window.panel.group", json!({"panel":panel,"operation":"move","anchor":proposal.anchor,"placement":proposal.placement})));
        }
    }
    for action in output.actions {
        if let Err(error) = app.run("window.panel.layout", json!({"action": action})) {
            app.ui.status = error;
        }
    }
    for (id, params) in commands {
        if let Err(error) = app.run(id, params) {
            app.ui.status = error;
        }
    }
    crate::dock::persist(app, ui.ctx());
    true
}

pub(crate) fn legacy_tab(app: &mut PhotocraftApp, ui: &mut egui::Ui, panel: &str, response: &egui::Response) {
    let suppressed = egui::Id::new("photocraft-panel-docking").with("legacy-suppressed");
    if ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary)) {
        ui.ctx().data_mut(|data| data.remove::<bool>(suppressed));
    }
    if ui.ctx().data(|data| data.get_temp::<bool>(suppressed)).unwrap_or(false) || app.session.prefs().workspace_locked {
        return;
    }
    let Some(panel) = normalize(panel) else { return };
    // Native tab reordering and vertical whole-group moves stay in the dock.
    // Crossing the dock column horizontally detaches through the shared layout.
    let strip = egui::Rect::from_min_max(egui::pos2(ui.max_rect().left(), response.rect.top()), egui::pos2(ui.max_rect().right(), response.rect.bottom()));
    if response.dragged()
        && ui.input(|input| input.focused && input.pointer.primary_down() && !input.key_pressed(egui::Key::Escape))
        && ui.ctx().pointer_interact_pos().is_some_and(|position| position.x < strip.left() - 24.0 || position.x > strip.right() + 24.0)
        && let Some(id) = normalize(panel)
    {
        let mut layout = app.ui.docking.clone().unwrap_or_else(|| legacy(app));
        let bounds = ui.max_rect();
        let source = egui::Rect::from_min_size(
            egui::pos2(bounds.left(), response.rect.top()),
            egui::vec2(bounds.width().clamp(240.0, 600.0), bounds.height().clamp(200.0, 600.0)),
        );
        if ensure(&mut layout, id).is_ok() && craft_ui::docking::begin_drag(ui.ctx(), egui::Id::new("photocraft-panel-docking"), id.to_string(), source) {
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    egui::Id::new("photocraft-panel-docking").with("legacy-origin"),
                    LegacyDrag {
                        generation: app.ui.docking_generation,
                        layout: app.ui.docking.clone(),
                        dock: app.ui.dock.clone(),
                        hidden: app.ui.docking_hidden.clone(),
                    },
                )
            });
            if app.ui.docking.is_none() {
                app.ui.docking_hidden = migration_hidden(app, &layout);
            }
            app.ui.docking = Some(layout);
        }
    }
}

fn body(app: &mut PhotocraftApp, ui: &mut egui::Ui, id: &str) {
    if let Some(module) = modules::get(id) {
        crate::dock::module_body(app, ui, module, "shared-panel-body");
    }
}

#[cfg(test)]
#[path = "panel_docking_tests.rs"]
mod tests;

/// Return positions for panels already hidden in a legacy workspace.
fn migration_hidden(app: &PhotocraftApp, visible: &Layout<String>) -> std::collections::BTreeMap<String, craft_ui::docking::Location<String>> {
    let full = legacy_layout(app, true);
    modules::ALL
        .iter()
        .filter(|m| !visible.contains(&m.id.to_owned()))
        .filter_map(|m| full.location(&m.id.to_owned()).ok().map(|at| (m.id.to_owned(), at)))
        .collect()
}

fn group_members(layout: &Layout<String>, panel: &str) -> Vec<String> {
    let mut pending: Vec<_> = layout.root.iter().collect();
    while let Some(node) = pending.pop() {
        match node {
            Node::Split { first, second, .. } => pending.extend([second.as_ref(), first.as_ref()]),
            Node::Tabs { panels, .. } if panels.iter().any(|id| id == panel) => return panels.clone(),
            Node::Stack { entries } if entries.iter().any(|entry| entry.panel == panel) => return vec![panel.to_owned()],
            _ => {}
        }
    }
    layout.floating.iter().find(|group| group.panels.iter().any(|id| id == panel)).map(|group| group.panels.clone()).unwrap_or_default()
}

fn reveal_group(app: &mut PhotocraftApp, panel: &str) {
    if let Some(layout) = &app.ui.docking {
        let members = group_members(layout, panel);
        app.ui.docking_collapsed.retain(|id| !members.contains(id));
        for id in members {
            if let Some(index) = app.ui.dock.pane_of(&id) {
                app.ui.dock.set_collapsed(index, false);
            }
        }
    }
}

pub(crate) fn rail_on(app: &PhotocraftApp, panel: &str) -> bool {
    app.ui.docking.as_ref().is_some_and(|layout| panel_active(layout, panel) && !collapsed_members(app, layout).contains(panel))
}

pub(crate) fn rail_click(app: &mut PhotocraftApp, panel: &str) {
    let result =
        if rail_on(app, panel) { group_command(app, &json!({"panel":panel,"operation":"collapse"})) } else { change(app, "activate", &json!({"panel":panel})) };
    if let Err(error) = result {
        app.ui.status = error;
    }
}

fn root_groups(layout: &Layout<String>) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut pending: Vec<_> = layout.root.iter().collect();
    while let Some(node) = pending.pop() {
        match node {
            Node::Split { first, second, .. } => pending.extend([second.as_ref(), first.as_ref()]),
            Node::Tabs { panels, .. } => out.push(panels.clone()),
            Node::Stack { entries } => out.extend(entries.iter().map(|e| vec![e.panel.clone()])),
        }
    }
    out
}

fn group_command(app: &mut PhotocraftApp, params: &Value) -> Result<Value, String> {
    let panel = params.get("panel").and_then(Value::as_str).and_then(normalize).ok_or("panel must name a registered panel")?;
    let operation = params.get("operation").and_then(Value::as_str).ok_or("operation is required")?;
    if app.session.prefs().workspace_locked {
        return Err("workspace is locked".into());
    }
    let mut layout = app.ui.docking.clone().unwrap_or_else(|| legacy(app));
    if !valid(&layout) {
        return Err("invalid panel layout".into());
    }
    let members = group_members(&layout, panel);
    if members.is_empty() {
        return Err("panel is hidden".into());
    }
    let mut hidden = if app.ui.docking.is_none() { migration_hidden(app, &layout) } else { app.ui.docking_hidden.clone() };
    let mut collapsed = collapsed_members(app, &layout);
    match operation {
        "close" => {
            for id in &members {
                hidden.insert(id.clone(), layout.location(id).map_err(|e| e.to_string())?);
                layout.apply(Action::Close { panel: id.clone() }).map_err(|e| e.to_string())?;
            }
            collapsed.retain(|id| !members.contains(id));
        }
        "collapse" => {
            collapsed.extend(members.iter().cloned());
        }
        "expand" => {
            collapsed.retain(|id| !members.contains(id));
        }
        "move" => {
            let anchor = params.get("anchor").and_then(Value::as_str).and_then(normalize).ok_or("anchor must name a registered panel")?;
            let placement = params
                .get("placement")
                .map(|v| serde_json::from_value::<Placement<String>>(v.clone()).map_err(|e| e.to_string()))
                .unwrap_or_else(|| placement(params))?;
            if members.iter().any(|id| id == anchor) {
                return Err("group cannot be moved into itself".into());
            }
            let first = members.first().ok_or("group is empty")?;
            layout.apply(Action::Move { panel: first.clone(), anchor: anchor.to_owned(), placement }).map_err(|e| e.to_string())?;
            let destination = group_members(&layout, first);
            let before = destination.iter().position(|id| id == first).and_then(|index| destination.get(index + 1)).cloned();
            for id in members.iter().skip(1) {
                layout
                    .apply(Action::Move { panel: id.clone(), anchor: first.clone(), placement: Placement::Tab { before: before.clone() } })
                    .map_err(|e| e.to_string())?;
            }
            layout.apply(Action::Activate { panel: panel.to_owned() }).map_err(|e| e.to_string())?;
        }
        "up" | "down" => {
            let groups = root_groups(&layout);
            let index = groups.iter().position(|g| g.contains(&panel.to_owned())).ok_or("floating groups cannot move within the dock")?;
            let neighbor = if operation == "up" { index.checked_sub(1) } else { index.checked_add(1).filter(|i| *i < groups.len()) };
            let anchor = neighbor.and_then(|i| groups.get(i)).and_then(|g| g.first()).ok_or("group is already at the edge")?;
            let first = members.first().ok_or("group is empty")?;
            layout
                .apply(Action::Move {
                    panel: first.clone(),
                    anchor: anchor.clone(),
                    placement: Placement::Split(if operation == "up" { Zone::Top } else { Zone::Bottom }),
                })
                .map_err(|e| e.to_string())?;
            for id in members.iter().skip(1) {
                layout
                    .apply(Action::Move { panel: id.clone(), anchor: first.clone(), placement: Placement::Tab { before: None } })
                    .map_err(|e| e.to_string())?;
            }
            layout.apply(Action::Activate { panel: panel.to_owned() }).map_err(|e| e.to_string())?;
        }
        _ => return Err("operation must be close, collapse, expand, move, up or down".into()),
    }
    app.ui.docking = Some(layout);
    app.ui.docking_hidden = hidden;
    app.ui.docking_collapsed = collapsed.into_iter().collect();
    sync_visibility(app);
    // Native collapse flags must not re-collapse a custom group after explicit expansion.
    if operation == "expand" || operation == "move" {
        reveal_group(app, panel);
    }
    Ok(json!({"layout":app.ui.docking}))
}

pub(crate) struct LegacyUpdate {
    layout: Layout<String>,
    hidden: std::collections::BTreeMap<String, craft_ui::docking::Location<String>>,
    clear_collapse: Vec<String>,
}

/// Validate and stage the legacy controls before ui.set commits any fields.
pub(crate) fn prepare_legacy_update(app: &PhotocraftApp, request: &Value, dock: Option<&crate::dock::DockLayout>) -> Result<Option<LegacyUpdate>, String> {
    let Some(current) = &app.ui.docking else { return Ok(None) };
    let Some(dock) = dock else { return Ok(None) };
    if !valid(current) {
        return Err("invalid panel layout; reset the workspace first".into());
    }
    if app.session.prefs().workspace_locked {
        return Err("workspace is locked".into());
    }
    // A column cannot describe an arbitrary split or floating workspace. Preserve that tree;
    // legacy group visibility/selection remains supported through the module-based parser.
    if request.get("dock").is_some() {
        let shape = |d: &crate::dock::DockLayout| d.panes.iter().map(|p| (p.tabs.clone(), p.height)).collect::<Vec<_>>();
        if shape(dock) != shape(&app.ui.dock) {
            return Err("dock order/heights cannot replace a customized layout; use window.panel.layout".into());
        }
    }
    let mut layout = current.clone();
    let mut hidden = app.ui.docking_hidden.clone();
    for module in modules::ALL {
        let id = module.id;
        let show = dock.visible(id);
        if !show && layout.contains(&id.to_owned()) {
            hidden.insert(id.to_owned(), layout.location(&id.to_owned()).map_err(|e| e.to_string())?);
            layout.apply(Action::Close { panel: id.to_owned() }).map_err(|e| e.to_string())?;
        } else if show && !layout.contains(&id.to_owned()) {
            if hidden.contains_key(id) {
                restore_hidden(&mut layout, &mut hidden, id);
            }
            ensure(&mut layout, id)?;
        }
    }
    let mut clear_collapse = Vec::new();
    for pane in &dock.panes {
        if pane.front() != app.ui.dock.panes.iter().find(|old| old.tabs == pane.tabs).map_or("", |old| old.front()) {
            layout.apply(Action::Activate { panel: pane.front().to_owned() }).map_err(|e| e.to_string())?;
            clear_collapse.extend(pane.tabs.iter().cloned());
        }
        if !pane.collapsed && app.ui.dock.panes.iter().any(|old| old.tabs == pane.tabs && old.collapsed) {
            clear_collapse.extend(pane.tabs.iter().cloned());
        }
    }
    Ok(Some(LegacyUpdate { layout, hidden, clear_collapse }))
}

pub(crate) fn commit_legacy_update(app: &mut PhotocraftApp, update: LegacyUpdate, _request: &Value) {
    app.ui.docking_collapsed.retain(|id| !update.clear_collapse.contains(id));
    app.ui.docking = Some(update.layout);
    app.ui.docking_hidden = update.hidden;
    sync_visibility(app);
}

struct PanelContent<'a> {
    app: &'a mut PhotocraftApp,
    collapsed: &'a std::collections::HashSet<String>,
    commands: &'a mut Vec<(&'static str, Value)>,
    targets: &'a mut Vec<craft_ui::docking::DropTarget<String>>,
}

fn panel_menu(app: &mut PhotocraftApp, ui: &mut egui::Ui, panel: &str) {
    if let Some(menu) = modules::get(panel).and_then(|module| module.menu) {
        menu(app, ui);
        ui.separator();
    }
}

impl craft_ui::docking::DockContent<String> for PanelContent<'_> {
    fn body(&mut self, ui: &mut egui::Ui, id: &String) {
        if let Some(target) = self.targets.iter_mut().find(|target| target.anchor == *id) {
            target.rect = target.rect.union(ui.max_rect());
            if let Some(strip) = target.tab_strip.as_mut() {
                strip.min.x = target.rect.left();
                strip.max.x = target.rect.right();
            }
            let members = self.app.ui.docking.as_ref().map(|layout| group_members(layout, id)).unwrap_or_default();
            target.tabs = members
                .into_iter()
                .filter_map(|panel| {
                    ui.ctx().read_response(egui::Id::new("photocraft-panel-docking").with(("tab", &panel))).map(|response| (panel, response.rect))
                })
                .collect();
        }
        if !self.collapsed.contains(id) {
            egui::Frame::NONE.inner_margin(egui::Margin::same(8)).show(ui, |ui| body(self.app, ui, id));
        }
    }
    fn panel_menu(&mut self, ui: &mut egui::Ui, id: &String) {
        panel_menu(self.app, ui, id);
    }
    fn group_header_width(&self, _panels: &[String], _active: &String, _floating: bool) -> f32 {
        28.0
    }
    fn group_header(&mut self, ui: &mut egui::Ui, panels: &[String], active: &String, floating: bool) {
        if !floating {
            let rect = ui.max_rect();
            self.targets.push(craft_ui::docking::DropTarget {
                anchor: active.clone(),
                rect,
                tab_strip: Some(rect),
                tabs: Vec::new(),
                allow_splits: true,
                accepts_tabs: !self.app.session.prefs().workspace_locked,
            });
        }
        let collapsed = panels.iter().all(|id| self.collapsed.contains(id));
        let locked = self.app.session.prefs().workspace_locked;
        let resp = crate::icons::button(ui, "ellipsis", ui.available_height().min(28.0), false, tl!("Panel menu"));
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(180.0);
            panel_menu(self.app, ui, active);
            if ui.add_enabled(!locked, egui::Button::new(if collapsed { tl!("Expand Panel Group") } else { tl!("Collapse Panel Group") })).clicked() {
                self.commands.push(("window.panel.group", json!({"panel":active,"operation":if collapsed {"expand"} else {"collapse"}})));
                ui.close();
            }
            if !floating {
                let groups = self.app.ui.docking.as_ref().map(root_groups).unwrap_or_default();
                let index = groups.iter().position(|g| g.contains(active));
                for (operation, label, enabled) in
                    [("up", tl!("Move Group Up"), index.is_some_and(|i| i > 0)), ("down", tl!("Move Group Down"), index.is_some_and(|i| i + 1 < groups.len()))]
                {
                    if ui.add_enabled(!locked && enabled, egui::Button::new(label)).clicked() {
                        self.commands.push(("window.panel.group", json!({"panel":active,"operation":operation})));
                        ui.close();
                    }
                }
            }
            if !locked {
                ui.menu_button(tl!("Add Tab"), |ui| {
                    let into = self.app.ui.dock.pane_of(active);
                    crate::dock::picker_list(self.app, ui, egui::Id::new(("shared-add-tab", active)), into);
                });
            }
            ui.separator();
            if ui.add_enabled(!locked, egui::Button::new(tl!("Close Tab Group"))).clicked() {
                self.commands.push(("window.panel.group", json!({"panel":active,"operation":"close"})));
                ui.close();
            }
        });
    }
}

pub(crate) fn sync_visibility(app: &mut PhotocraftApp) {
    let Some(layout) = app.ui.docking.as_ref().filter(|layout| valid(layout)) else { return };
    // Mirror group membership for the module picker, icon rail and legacy ui.set bridge.
    // The shared tree remains authoritative for splits, floating windows and their sizes.
    let collapsed = collapsed_members(app, layout);
    let mut groups = root_groups(layout);
    groups.extend(layout.floating.iter().map(|group| group.panels.clone()));
    let old = app.ui.dock.panes.clone();
    app.ui.dock.panes = groups
        .into_iter()
        .filter(|ids| !ids.is_empty())
        .map(|tabs| {
            let active = tabs.iter().find(|id| panel_active(layout, id)).cloned().unwrap_or_else(|| tabs.first().cloned().unwrap_or_default());
            let height = old.iter().find(|pane| pane.tabs == tabs).and_then(|pane| pane.height);
            let is_collapsed = tabs.iter().all(|id| collapsed.contains(id));
            crate::dock::Pane { tabs, active, height, collapsed: is_collapsed }
        })
        .collect();
}

fn panel_active(layout: &Layout<String>, panel: &str) -> bool {
    let mut pending: Vec<_> = layout.root.iter().collect();
    while let Some(node) = pending.pop() {
        match node {
            Node::Split { first, second, .. } => pending.extend([first.as_ref(), second.as_ref()]),
            Node::Tabs { panels, active } if panels.iter().any(|p| p == panel) => return panels.get(*active).is_some_and(|p| p == panel),
            Node::Stack { entries } if entries.iter().any(|e| e.panel == panel) => return true,
            _ => {}
        }
    }
    layout.floating.iter().any(|g| g.panels.get(g.active).is_some_and(|p| p == panel))
}

/// Extra legacy menu choices use the same commands as the customized renderer.
pub(crate) fn legacy_menu(app: &mut PhotocraftApp, ui: &mut egui::Ui, label: &str) {
    let Some(panel) = normalize(label) else { return };
    let locked = app.session.prefs().workspace_locked;
    ui.add_enabled_ui(!locked, |ui| {
        if ui.button(tl!("Float Panel")).clicked() {
            if let Err(error) = app.run("window.panel.float", json!({"panel":panel})) {
                app.ui.status = error;
            }
            ui.close();
        }
        ui.menu_button(tl!("Group with"), |ui| {
            for module in modules::ALL {
                let (anchor, label) = (module.id, module.title);
                if anchor != panel && ui.button(tl!(label)).clicked() {
                    let result = app
                        .run("window.panel.activate", json!({"panel":anchor}))
                        .and_then(|_| app.run("window.panel.move", json!({"panel":panel,"anchor":anchor})));
                    if let Err(error) = result {
                        app.ui.status = error;
                    }
                    ui.close();
                }
            }
        });
    });
}

// A closed group forms a bounded chain of hidden tab neighbors. Restore just the requested
// panel at the first surviving return position, then let the remaining tabs rejoin it.
fn restore_hidden(layout: &mut Layout<String>, hidden: &mut std::collections::BTreeMap<String, craft_ui::docking::Location<String>>, panel: &str) {
    let Some(mut at) = hidden.get(panel).cloned() else { return };
    let mut terminal = panel.to_owned();
    let mut seen = vec![terminal.clone()];
    while let Some(anchor) = at.anchor.as_ref().filter(|id| !layout.contains(id)).cloned() {
        if seen.len() >= modules::ALL.len() || seen.contains(&anchor) {
            return;
        }
        let Some(next) = hidden.get(&anchor).cloned() else { return };
        seen.push(anchor.clone());
        terminal = anchor;
        at = next;
    }
    if layout.restore(panel.to_owned(), &at).is_ok()
        && terminal != panel
        && let Some(last) = hidden.get_mut(&terminal)
    {
        last.anchor = Some(panel.to_owned());
        last.placement = Placement::Tab { before: None };
        last.size = None;
        last.sibling.clear();
    }
}

/// The module picker must update the authoritative shared tree once docking is customized.
pub(crate) fn picker_change(app: &mut PhotocraftApp, panel: &str, into: Option<&str>, close: bool) -> Result<(), String> {
    if close {
        change(app, "close", &json!({"panel":panel}))?;
        return Ok(());
    }
    let anchor = into.map(str::to_owned).or_else(|| app.ui.docking.as_ref().and_then(|layout| root_groups(layout).last().and_then(|ids| ids.first()).cloned()));
    change(app, "activate", &json!({"panel":panel}))?;
    if let Some(anchor) = anchor.filter(|anchor| anchor != panel) {
        change(app, "move", &json!({"panel":panel,"anchor":anchor,"zone":if into.is_some() {"tab"} else {"bottom"}}))?;
    }
    Ok(())
}
