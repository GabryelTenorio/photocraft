//! #2672: Z is a normal Zoom tool tap and a spring-loaded Zoom while held.
//! The selected tool stays intact during the hold, preserving pending Pen and Crop state.
use crate::{PhotocraftApp, state::Tool};
use egui::{Context, Id, Key};

// Photoshop's default spring-loaded timing; short taps select the tool permanently.
const HOLD_SECONDS: f64 = 0.2;

#[derive(Clone, Copy)]
struct Hold {
    previous: Tool,
    document: Option<photocraft_doc::DocId>,
    started: f64,
    used: bool,
}

fn id() -> Id {
    Id::new("spring-loaded-zoom")
}

/// Poll before shortcut focus/modal early returns so a release or focus loss cannot stick.
pub fn poll(app: &mut PhotocraftApp, ctx: &Context) {
    let Some(hold) = ctx.data(|d| d.get_temp::<Hold>(id())) else { return };
    let (down, now, focused) = ctx.input(|i| (i.key_down(Key::Z), i.time, i.focused));
    let same = app.ui.tool == hold.previous && app.session.active().map(|s| s.doc.id) == hold.document;
    let blocked = ctx.text_edit_focused() || app.ui.text_edit.is_some() || !app.ui.dialogs.is_empty() || app.camera_raw.is_some() || app.discard.is_some();
    if !same || !focused || blocked {
        ctx.data_mut(|d| d.remove::<Hold>(id()));
        return;
    }
    if !down {
        ctx.data_mut(|d| d.remove::<Hold>(id()));
        // The temporary-tool latch owns an ongoing zoom drag through pointer release.
        // A short tap with no canvas interaction is the ordinary tool shortcut.
        if !hold.used && now - hold.started < HOLD_SECONDS {
            app.ui.tool = Tool::Zoom;
        }
    }
}

/// Called only after application shortcuts and text input have had first refusal of Z.
pub fn press(app: &mut PhotocraftApp, ctx: &Context) {
    if ctx.data(|d| d.get_temp::<Hold>(id())).is_some() {
        return; // Key repeat must not replace the original tool or start time.
    }
    // Explicit temporary-tool bindings retain priority over the letter shortcut.
    if crate::hold_keys::held_tool(app, ctx).is_some() {
        return;
    }
    if app.drag.is_some() {
        return; // A running stroke keeps its tool and its release.
    }
    let (down, now) = ctx.input(|i| (i.key_down(Key::Z), i.time));
    if !down {
        app.ui.tool = Tool::Zoom; // Press and release delivered in the same frame: a tap.
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id(), Hold { previous: app.ui.tool, document: app.session.active().map(|s| s.doc.id), started: now, used: false }));
}

/// The held Zoom can use Alt to zoom out; unlike Cmd+Space it does not force a direction.
pub fn held(app: &PhotocraftApp, ctx: &Context) -> bool {
    let Some(mut hold) = ctx.data(|d| d.get_temp::<Hold>(id())) else { return false };
    if app.ui.tool != hold.previous || app.session.active().map(|s| s.doc.id) != hold.document || ctx.text_edit_focused() {
        return false;
    }
    if ctx.input(|i| i.pointer.primary_down() && i.pointer.interact_pos().is_some_and(|p| app.last_canvas_rect.contains(p))) {
        hold.used = true;
        ctx.data_mut(|d| d.insert_temp(id(), hold));
    }
    true
}

#[cfg(test)]
mod tests;
