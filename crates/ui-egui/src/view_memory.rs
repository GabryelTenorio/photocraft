//! View › Extras, the View › Show items, Snap and View › Snap To remembered between launches
//! (#3762), as Photoshop keeps them across sessions.
//!
//! `Preferences::view_extras` holds the flags that differ from their defaults, keyed by their
//! menu id without `view.` (`"extras"`, `"show.pixelGrid"`, `"snapTo.guides"`), so a default
//! changed in a later release still reaches users who never touched that item. Saved when one
//! changes, by a menu, a shortcut or the control channel; restored at launch. An unknown key or a
//! value that isn't a bool is ignored.

use serde_json::{Map, Value};

use crate::PhotocraftApp;
use crate::state::UiState;

/// The remembered flags: Extras, every View › Show item, Snap and every View › Snap To target.
const KEYS: [&str; 22] = [
    "extras",
    "show.layerEdges",
    "show.selectionEdges",
    "show.targetPath",
    "show.grid",
    "show.guides",
    "show.canvasGuides",
    "show.artboardGuides",
    "show.count",
    "show.smartGuides",
    "show.slices",
    "show.notes",
    "show.pixelGrid",
    "show.brushPreview",
    "show.mesh",
    "show.editPins",
    "snap",
    "snapTo.guides",
    "snapTo.grid",
    "snapTo.layers",
    "snapTo.slices",
    "snapTo.documentBounds",
];

/// The flag behind a key (Grid, Guides and Snap live in `UiState::extras`, the rest in
/// `UiState::view`).
fn slot<'a>(ui: &'a mut UiState, key: &str) -> Option<&'a mut bool> {
    match key {
        "extras" => Some(&mut ui.view.extras),
        "show.grid" => Some(&mut ui.extras.grid),
        "show.guides" => Some(&mut ui.extras.guides),
        "snap" => Some(&mut ui.extras.snap),
        _ => {
            if let Some(k) = key.strip_prefix("show.") {
                crate::view_cmds::show_slot(&mut ui.view.show, k)
            } else {
                crate::view_cmds::snap_slot(&mut ui.view.snap_to, key.strip_prefix("snapTo.")?)
            }
        }
    }
}

/// The flags in `KEYS` order.
fn flags(ui: &mut UiState) -> Vec<bool> {
    KEYS.into_iter().map(|k| slot(ui, k).is_some_and(|v| *v)).collect()
}

/// Remember the flags in the preferences when one changed. Cheap when nothing did: one compare
/// of a few bools per frame. Run every frame, so flags set by the control channel are remembered.
pub fn persist(app: &mut PhotocraftApp) {
    let now = flags(&mut app.ui);
    if app.prefs_rt.view_extras.as_ref() == Some(&now) {
        return;
    }
    let def = flags(&mut UiState::default());
    let changed: Map<String, Value> =
        KEYS.into_iter().zip(now.iter().zip(def)).filter(|(_, (v, d))| **v != *d).map(|(k, (v, _))| (k.to_string(), Value::Bool(*v))).collect();
    let changed = if changed.is_empty() { Value::Null } else { Value::Object(changed) };
    app.prefs_rt.view_extras = Some(now);
    if app.session.prefs().view_extras != changed {
        app.session.prefs.edit(|p| p.view_extras = changed);
    }
}

/// Restore the remembered flags at launch, over the defaults.
pub fn restore(app: &mut PhotocraftApp) {
    let saved = app.session.prefs().view_extras.clone();
    if let Some(saved) = saved.as_object() {
        for (k, v) in saved {
            match (slot(&mut app.ui, k), v.as_bool()) {
                (Some(flag), Some(v)) => *flag = v,
                _ => log::warn!("Preferences: ignoring the remembered View flag `{k}`: {v}"),
            }
        }
    } else if !saved.is_null() {
        log::warn!("Preferences: the remembered View flags are not an object; using the defaults");
    }
    app.prefs_rt.view_extras = Some(flags(&mut app.ui));
}
