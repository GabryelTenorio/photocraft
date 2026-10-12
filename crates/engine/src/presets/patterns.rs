//! Window › Patterns: folders over the pattern library (`pattern.*` owns the patterns
//! themselves), the selected pattern, and Pattern Fill layers made from it.
//!
//! Groups hold pattern **ids**. Library patterns that are in no group (newly defined or imported)
//! are shown in a trailing "Patterns" group; ids whose pattern was deleted are dropped.

use photocraft_doc::{Fill, LayerContent};
use serde_json::{Value, json};

use super::{Group, always, bad, edit_groups, group_index, req_str, str_param};
use crate::commands::CommandSpec;
use crate::{Result, Session};

const UNGROUPED: &str = "Patterns";

/// Built-in folders for the built-in procedural patterns.
pub fn builtin_groups() -> Vec<Group<String>> {
    let lib = crate::pattern_cmds::builtin();
    let ids = |names: &[&str]| -> Vec<String> { names.iter().filter_map(|n| lib.iter().find(|p| p.name == *n).map(|p| p.id.clone())).collect() };
    vec![Group::new("Geometric", ids(&["Checkerboard", "Diagonal Lines", "Dots", "Grid"])), Group::new("Textures", ids(&["Bricks", "Weave", "Paper Noise"]))]
}

/// Bring the groups in line with the library (see the module docs).
pub fn sync(s: &mut Session) {
    let lib: Vec<String> = s.patterns.items.iter().map(|p| p.id.clone()).collect();
    let groups = &mut s.presets.pattern_groups;
    for g in groups.iter_mut() {
        g.items.retain(|id| lib.contains(id));
    }
    let loose: Vec<String> = lib.iter().filter(|id| !groups.iter().any(|g| g.items.contains(id))).cloned().collect();
    if !loose.is_empty() {
        let gi = group_index(groups, Some(UNGROUPED));
        groups[gi].items.extend(loose);
    }
}

/// The selected pattern, if it is still in the library.
pub fn current(s: &Session) -> Option<&photocraft_doc::Pattern> {
    let id = s.presets.pattern.as_deref()?;
    s.patterns.items.iter().find(|p| p.id == id)
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    sync(s);
    let groups: Vec<Value> = s
        .presets
        .pattern_groups
        .iter()
        .map(|g| {
            let pats: Vec<Value> = g
                .items
                .iter()
                .filter_map(|id| s.patterns.items.iter().find(|p| &p.id == id))
                .map(|p| json!({"id": p.id, "name": p.display_name(), "width": p.width, "height": p.height}))
                .collect();
            json!({"name": g.name, "patterns": pats})
        })
        .collect();
    Ok(json!({"groups": groups, "current": current(s).map(|p| p.id.clone())}))
}

fn pattern_key(s: &Session, p: &Value, cmd: &str) -> Result<photocraft_doc::Pattern> {
    let key = str_param(p, "pattern").unwrap_or("");
    crate::pattern_cmds::resolve(s, key).ok_or_else(|| bad(cmd, format!("no pattern \"{key}\" (see pattern.presets.list)")))
}

fn active_pattern_fill(s: &Session) -> Option<photocraft_doc::LayerId> {
    let d = s.active()?;
    let id = d.active_layer?;
    matches!(d.doc.layer(id)?.content, LayerContent::Fill(Fill::Pattern { .. })).then_some(id)
}

fn select(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "pattern.presets.select";
    let pat = pattern_key(s, p, CMD)?;
    s.presets.pattern = Some(pat.id.clone());
    s.presets.rev += 1;
    let mut out = json!({"pattern": pat.id, "name": pat.display_name()});
    if p.get("applyToLayer").and_then(Value::as_bool).unwrap_or(true)
        && let Some(id) = active_pattern_fill(s)
    {
        s.edit("Change Pattern Fill", |doc, _| {
            crate::pattern_cmds::ensure_in_doc(doc, &pat);
            if let Some(l) = doc.layer_mut(id)
                && let LayerContent::Fill(Fill::Pattern { name, id: pid, .. }) = &mut l.content
            {
                *name = pat.name.clone();
                *pid = pat.id.clone();
            }
            Ok(())
        })?;
        out["layer"] = json!(id.0);
    }
    Ok(out)
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let pat = pattern_key(s, p, "pattern.presets.apply")?;
    let mut q = p.clone();
    q["pattern"] = json!(pat.id);
    super::call(s, "layer.newFillLayer.pattern", q)
}

fn new_preset(s: &mut Session, p: &Value) -> Result<Value> {
    let r = super::call(s, "edit.definePattern", p.clone())?;
    let id = r.get("pattern").and_then(Value::as_str).unwrap_or_default().to_string();
    sync(s);
    if let Some(g) = str_param(p, "group") {
        for gr in &mut s.presets.pattern_groups {
            gr.items.retain(|x| *x != id);
        }
        let gi = group_index(&mut s.presets.pattern_groups, Some(g));
        s.presets.pattern_groups[gi].items.push(id.clone());
    }
    s.presets_changed();
    Ok(json!({"pattern": id}))
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "pattern.presets.edit";
    let action = req_str(p, "action", CMD)?.to_string();
    // A refused deletion must leave the folders unchanged too.
    if action != "delete" {
        sync(s);
    }
    // Rename/delete act on the library pattern (by id or name).
    let r = match action.as_str() {
        "rename" => super::call(s, "pattern.rename", json!({"pattern": req_str(p, "preset", CMD)?, "name": req_str(p, "name", CMD)?}))?,
        "delete" => {
            let keys: Vec<&str> = match p.get("preset") {
                Some(Value::Array(a)) => {
                    a.iter().map(|v| v.as_str().ok_or_else(|| bad(CMD, "`preset` must contain only pattern ids or names"))).collect::<Result<_>>()?
                }
                _ => vec![req_str(p, "preset", CMD)?],
            };
            // Resolve the whole batch before removing anything. A name and an id can refer
            // to the same tile, which must be deleted only once.
            let mut ids = std::collections::BTreeSet::new();
            for key in keys {
                let pat = photocraft_doc::pattern::find(&s.patterns.items, key, key).ok_or_else(|| bad(CMD, format!("no library pattern \"{key}\"")))?;
                ids.insert(pat.id.clone());
            }
            s.patterns.items.retain(|pat| !ids.contains(&pat.id));
            json!({"deleted": ids.len()})
        }
        "deleteGroup" => {
            // Deleting a folder deletes its patterns, as in Photoshop.
            let g = req_str(p, "group", CMD)?;
            let i = s.presets.pattern_groups.iter().position(|x| x.name == g).ok_or_else(|| bad(CMD, format!("no group \"{g}\"")))?;
            let gone = s.presets.pattern_groups.remove(i);
            s.patterns.items.retain(|q| !gone.items.contains(&q.id));
            json!({"group": g, "deleted": gone.items.len()})
        }
        "move" => {
            let key = req_str(p, "preset", CMD)?;
            let id = crate::pattern_cmds::resolve(s, key).map(|q| q.id).ok_or_else(|| bad(CMD, format!("no pattern \"{key}\"")))?;
            let mut q = p.clone();
            q["preset"] = json!(id);
            edit_groups(&mut s.presets.pattern_groups, "move", &q, CMD)?
        }
        a => edit_groups(&mut s.presets.pattern_groups, a, p, CMD)?,
    };
    sync(s);
    s.presets_changed();
    Ok(r)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "pattern.presets.list",
            label: "Pattern Presets",
            menu: &[],
            shortcut: None,
            params: "{} → {groups:[{name,patterns:[{id,name,width,height}]}],current}",
            enabled: always,
            run: list,
            journal: false,
        },
        CommandSpec {
            id: "pattern.presets.select",
            label: "Select Pattern",
            menu: &[],
            shortcut: None,
            params: r##"{"pattern":id|name,"applyToLayer":bool=true (also changes a selected Pattern Fill layer)} (Fill and new Pattern Fill layers default to it)"##,
            enabled: always,
            run: select,
            journal: true,
        },
        CommandSpec {
            id: "pattern.presets.apply",
            label: "New Pattern Fill Layer from Preset",
            menu: &[],
            shortcut: None,
            params: r##"{"pattern":id|name?=selected,"scale":1..1000=100,"angle":deg=0} → {layer}"##,
            enabled: super::has_doc,
            run: apply,
            journal: true,
        },
        CommandSpec {
            id: "pattern.presets.new",
            label: "New Pattern Preset",
            menu: &[],
            shortcut: None,
            params: r##"{"name":str?,"group":name?,"rect":[x0,y0,x1,y1]?} (Define Pattern from the selection/canvas into a group)"##,
            enabled: super::has_doc,
            run: new_preset,
            journal: true,
        },
        CommandSpec {
            id: "pattern.presets.edit",
            label: "Edit Pattern Presets",
            menu: &[],
            shortcut: None,
            params: super::GROUP_EDIT_PARAMS,
            enabled: always,
            run: edit,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_patterns(depth: u32) -> (Session, String) {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 4, "height": 4, "depth": depth, "background": "white"})).unwrap();
        let first = s.execute("pattern.presets.new", json!({"name": "First", "group": "Custom"})).unwrap();
        s.execute("pattern.presets.new", json!({"name": "Second", "group": "Custom"})).unwrap();
        let id = first["pattern"].as_str().unwrap().to_string();
        s.execute("pattern.presets.select", json!({"pattern": id, "applyToLayer": false})).unwrap();
        (s, id)
    }

    fn assert_failed_delete_preserves_state(s: &mut Session, keys: Value) {
        let patterns = s.patterns.items.clone();
        let groups = s.presets.pattern_groups.clone();
        let selected = s.presets.pattern.clone();
        let prefs = s.prefs_to_json();
        let revisions = (s.presets.rev, s.prefs.rev());
        let result = s.execute("pattern.presets.edit", json!({"action": "delete", "preset": keys}));
        assert!(result.is_err(), "malformed or unresolved deletion must fail: {result:?}");
        assert_eq!(s.patterns.items, patterns, "a refused batch must preserve every pattern and its pixels");
        assert_eq!(s.presets.pattern_groups, groups);
        assert_eq!(s.presets.pattern, selected);
        assert_eq!(s.prefs_to_json(), prefs, "persisted preferences must remain unchanged");
        assert_eq!((s.presets.rev, s.prefs.rev()), revisions);
    }

    #[test]
    fn pattern_batch_delete_missing_name_is_atomic() {
        for depth in [8, 16, 32] {
            let (mut s, _) = custom_patterns(depth);
            assert_failed_delete_preserves_state(&mut s, json!(["First", "missing"]));
        }
    }

    #[test]
    fn pattern_batch_delete_failure_preserves_unsynced_groups() {
        let (mut s, _) = custom_patterns(8);
        // Define Pattern adds a library tile; the panel has not reconciled its folders yet.
        s.execute("edit.definePattern", json!({"name": "Not Yet Grouped"})).unwrap();
        assert_failed_delete_preserves_state(&mut s, json!(["missing"]));
    }

    #[test]
    fn pattern_batch_delete_rejects_document_only_pattern() {
        let (mut s, _) = custom_patterns(8);
        let added = s.execute("edit.definePattern", json!({"name": "Document Only"})).unwrap();
        let id = added["pattern"].as_str().unwrap();
        let pattern = s.patterns.items.iter().find(|p| p.id == id).unwrap().clone();
        s.edit("Embed pattern fixture", |doc, _| {
            doc.patterns.push(pattern.clone());
            Ok(())
        })
        .unwrap();
        s.execute("pattern.delete", json!({"pattern": id})).unwrap();
        let embedded = s.active().unwrap().doc.patterns.clone();
        assert_failed_delete_preserves_state(&mut s, json!(["First", id]));
        assert_eq!(s.active().unwrap().doc.patterns, embedded);
    }

    #[test]
    fn pattern_batch_delete_rejects_non_string_members() {
        for depth in [8, 16, 32] {
            for invalid in [json!(null), json!(7), json!({})] {
                let (mut s, _) = custom_patterns(depth);
                assert_failed_delete_preserves_state(&mut s, json!(["First", invalid]));
            }
        }
    }

    #[test]
    fn pattern_batch_delete_deduplicates_names_and_ids() {
        for depth in [8, 16, 32] {
            for by_id in [false, true] {
                let (mut s, id) = custom_patterns(depth);
                let remaining: Vec<_> = s.patterns.items.iter().filter(|p| p.id != id).cloned().collect();
                let alias = if by_id { id.as_str() } else { "First" };
                let result = s.execute("pattern.presets.edit", json!({"action": "delete", "preset": ["First", alias]})).unwrap();
                assert_eq!(result["deleted"], 1, "aliases name one pattern");
                assert_eq!(s.patterns.items, remaining);
                assert!(s.presets.pattern_groups.iter().all(|g| !g.items.contains(&id)));
                let saved: Value = serde_json::from_str(&s.prefs_to_json()).unwrap();
                assert!(!saved["presets"]["pattern_groups"].to_string().contains(&id));
            }
        }
    }
}
