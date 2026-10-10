//! `change_impact`: given a change (paths, a command id, a symbol, or an intent), return the
//! **must-change-together touch set**, the invariants that apply, and the artifacts to regenerate.
//!
//! The signal registry encodes PhotoCraft-specific couplings that generic search cannot see (the
//! `format` persistence fields, PSD `grdm` read/write, the command↔menu↔parity contract, corpus
//! routing). Gates come from the authoritative `cargo xtask plan --json` (reused, not re-encoded).
//!
//! Forge never edits source (D7); this returns a work order for the agent.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the caller wants impact for. At least one field should be set.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Query {
    /// Changed paths (repo-relative) to analyse directly.
    #[serde(default)]
    pub paths: Vec<String>,
    /// An engine command id, e.g. `filter.blur.gaussianBlur`.
    #[serde(default)]
    pub command: Option<String>,
    /// A symbol name to locate (best-effort grep).
    #[serde(default)]
    pub symbol: Option<String>,
    /// A natural-language intent (best-effort keyword match).
    #[serde(default)]
    pub intent: Option<String>,
    /// Consult the optional host MCPs (`jCodeMunch`, `rust-lsp`) for affected files (default false).
    #[serde(default)]
    pub enrich: bool,
}

/// One file that must change, and why.
#[derive(Debug, Clone, Serialize)]
pub struct Touch {
    pub path: String,
    pub why: String,
    pub signals: Vec<String>,
}

/// A generated artifact to regenerate.
#[derive(Debug, Clone, Serialize)]
pub struct Regenerate {
    pub artifact: String,
    pub command: String,
    pub why: String,
}

/// The impact analysis.
#[derive(Debug, Default, Serialize)]
pub struct Impact {
    pub touch: Vec<Touch>,
    pub invariants: Vec<String>,
    pub regenerate: Vec<Regenerate>,
}

/// A detected signal: which rule fired, why, and what it adds.
struct Signal {
    id: &'static str,
    why: &'static str,
    touch: &'static [&'static str],
    invariants: &'static [&'static str],
    regenerate: &'static [(&'static str, &'static str)],
}

const DOC: Signal = Signal {
    id: "doc-field",
    why: "a document-model field ripples through format (persistence) and io (PSD mapping)",
    touch: &["crates/format/src/manifest.rs", "crates/format/src/convert.rs", "crates/format/src/finite.rs", "crates/io/src/lib.rs"],
    invariants: &["additive serialized fields need #[serde(default)]; every f32/f64 must remain finite"],
    regenerate: &[("format round-trip", "cargo test -p photocraft-format")],
};

const ADJUST: Signal = Signal {
    id: "adjustment-arm",
    why: "an Adjustment arm/shape touches PSD grdm read+write, CPU compose, GPU plan, engine params, the editor and the compile-guard lists",
    touch: &[
        "crates/io/src/adjust_map.rs",
        "crates/compose/src/adjust.rs",
        "crates/gpu/src/plan.rs",
        "crates/engine/src/adjust_params.rs",
        "crates/engine/src/commands.rs",
        "crates/ui-egui/src/adjust_editors.rs",
        "crates/io/tests/adjust_blend_roundtrip.rs",
        "crates/gpu/tests/parity.rs",
    ],
    invariants: &["keep PSD read and write symmetric (v3 `grdm` is read/written, not baked)"],
    regenerate: &[("corpus", "cargo xtask test-corpus")],
};

const FORMAT: Signal = Signal {
    id: "codec-format",
    why: "a `codecs::Format` variant needs every enum touch point and both dispatch matches",
    touch: &[
        "crates/codecs/src/lib.rs",
        "crates/codecs/src/via_image.rs",
        "crates/io/src/flat.rs",
        "crates/codecs/tests/detect_caps.rs",
        "crates/codecs/tests/roundtrip.rs",
        "crates/codecs/tests/malformed.rs",
    ],
    invariants: &["bump `ALL`, add name/extensions/mime_type/caps/detect, both dispatch matches; codecs is a standalone crate (no workspace deps)"],
    regenerate: &[("corpus", "cargo xtask test-corpus")],
};

const COMMAND: Signal = Signal {
    id: "engine-command",
    why: "a new/changed engine command must be registered and its menu id resolved; parity is regenerated",
    touch: &["crates/engine/src/commands.rs", "crates/ui-egui/src/menu_catalog.rs"],
    invariants: &[
        "every command has id/label/menu/shortcut/params doc/enabled/run + tests, and records exactly one history step",
        "engine command `run` must return Err (never panic) for any params",
    ],
    regenerate: &[("docs/parity.md", "cargo xtask parity"), ("panic_hunt", "cargo test -p photocraft-engine --test panic_hunt -- --ignored")],
};

const UI_I18N: Signal = Signal {
    id: "ui-strings",
    why: "new user-visible strings must be added to every localization catalog",
    touch: &["crates/ui-egui/src/i18n/mod.rs"],
    invariants: &[
        "new user-visible strings must be added to every localization catalog (*.tsv + i18n/mod.rs)",
        "UI uses theme::Tokens and shared widgets; verify visually (snapshot or control channel)",
    ],
    regenerate: &[],
};

const PREFERENCE: Signal = Signal {
    id: "preference",
    why: "a preference is read by the shell and shown in the schema-driven Preferences dialog, so its UI, its label's localizations and the scorecard prefs audit all move together",
    touch: &["crates/ui-egui/src/prefs_ui.rs", "crates/ui-egui/src/i18n/mod.rs", "docs/scorecard.md"],
    invariants: &[
        "the Preferences dialog renders the field from the struct; a field nothing reads must be wired or listed in HIDDEN_UNTIL_IMPLEMENTED",
        "a new user-visible preference label must be translated in every locale that claims complete menus, or `preference_labels_are_translated` fails",
    ],
    regenerate: &[("docs/scorecard.md", "cargo xtask scorecard")],
};

/// Rules, in the order they are probed.
const RULES: &[(&str, Signal)] = &[("crates/doc/", DOC), ("crates/doc/src/adjust", ADJUST), ("crates/codecs/src/format.rs", FORMAT)];

/// Signals for one path.
fn signals_for(path: &str) -> Vec<&'static Signal> {
    let mut out = Vec::new();
    for (prefix, sig) in RULES {
        if path.starts_with(*prefix) {
            out.push(sig);
        }
    }
    // The adjustment file is under `crates/doc/` too; de-duplicate by id.
    if path.starts_with("crates/engine/src/") && path.ends_with("_cmds.rs") {
        out.push(&COMMAND);
    }
    if path == "crates/engine/src/prefs.rs" {
        out.push(&PREFERENCE);
    }
    if path.starts_with("crates/ui-egui/src/") && path.ends_with(".rs") && !path.contains("/tests") {
        out.push(&UI_I18N);
    }
    out
}

/// Universal invariants that apply to any change.
const UNIVERSAL: &[&str] = &[
    "Never crash: no unwrap/expect/panic/unsafe in non-test code; validate all input.",
    "Honour bit depth and colour model (no u8-only public paths); test at several depths.",
    "Tests are required; format code needs round-trips and pixels at 8/16/32-bit.",
];

/// Analyse a set of changed paths.
pub fn analyze(paths: &[String]) -> Impact {
    let mut touch: BTreeMap<String, Touch> = BTreeMap::new();
    let mut regen: BTreeMap<String, Regenerate> = BTreeMap::new();
    let mut invariants: BTreeSet<String> = UNIVERSAL.iter().map(|s| (*s).to_string()).collect();
    for path in paths {
        for sig in signals_for(path) {
            for t in sig.touch {
                touch
                    .entry((*t).to_string())
                    .and_modify(|e| {
                        if !e.signals.iter().any(|s| s == sig.id) {
                            e.signals.push(sig.id.to_string())
                        }
                    })
                    .or_insert_with(|| Touch { path: (*t).to_string(), why: sig.why.to_string(), signals: vec![sig.id.to_string()] });
            }
            for inv in sig.invariants {
                invariants.insert((*inv).to_string());
            }
            for (artifact, command) in sig.regenerate {
                regen.entry((*artifact).to_string()).or_insert_with(|| Regenerate {
                    artifact: (*artifact).to_string(),
                    command: (*command).to_string(),
                    why: sig.why.to_string(),
                });
            }
        }
    }
    Impact { touch: touch.into_values().collect(), invariants: invariants.into_iter().collect(), regenerate: regen.into_values().collect() }
}

/// Resolve a request into paths, analyse it, and attach the authoritative gate plan.
pub fn run(root: &Path, q: Query) -> Result<serde_json::Value, String> {
    let mut paths: Vec<String> = q.paths.clone();
    let mut notes: Vec<String> = Vec::new();

    if let Some(command) = &q.command {
        match resolve_command(root, command)? {
            v if v.is_empty() => notes.push(format!("no engine module found defining command `{command}`")),
            v => paths.extend(v),
        }
    }
    if let Some(symbol) = &q.symbol {
        let v = resolve_symbol(root, symbol);
        if v.is_empty() {
            notes.push(format!("no definition found for symbol `{symbol}`"));
        }
        paths.extend(v);
    }
    if let Some(intent) = &q.intent {
        let v = resolve_intent(intent);
        if v.is_empty() {
            notes.push(format!("intent `{intent}` matched no known area; pass explicit paths"));
        }
        paths.extend(v);
    }

    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        notes.push("no paths resolved — pass `paths`, a `command`, a `symbol`, or a recognised `intent`".to_string());
    }

    let impact = analyze(&paths);
    // Gates cover the whole change: the input paths *and* everything they touch.
    let mut gate_paths: Vec<String> = paths.clone();
    gate_paths.extend(impact.touch.iter().map(|t| t.path.clone()));
    gate_paths.sort();
    gate_paths.dedup();
    let gates = crate::xtask::plan_json(root, &gate_paths).ok().flatten();
    if gates.is_none() && !gate_paths.is_empty() {
        notes.push("could not obtain the gate plan from `cargo xtask plan`".to_string());
    }

    // Optional graph enrichment from host MCPs — never required (D1/D4).
    let (related, graph) = if q.enrich {
        let companions = crate::graph::detect();
        let enrichment = crate::graph::enrich(root, &paths, q.symbol.as_deref(), companions);
        let graph = serde_json::json!({
            "jcodemunch": companions.jcodemunch,
            "rust_lsp": companions.rust_lsp,
            "used": enrichment.used,
            "notes": enrichment.notes,
        });
        (enrichment.related, graph)
    } else {
        (Vec::new(), serde_json::Value::Null)
    };

    Ok(serde_json::json!({
        "inputs": { "paths": q.paths, "command": q.command, "symbol": q.symbol, "intent": q.intent },
        "resolved_paths": paths,
        "touch": impact.touch,
        "invariants": impact.invariants,
        "regenerate": impact.regenerate,
        "related": related,
        "graph": graph,
        "gates": gates,
        "notes": notes,
    }))
}

/// Engine files that mention the command id (its `specs()` literal).
fn resolve_command(root: &Path, id: &str) -> Result<Vec<String>, String> {
    let needle = format!("\"{id}\"");
    Ok(grep(root, &["crates/engine/src"], &|line| line.contains(&needle), 20))
}

/// Best-effort symbol definition lookup.
fn resolve_symbol(root: &Path, name: &str) -> Vec<String> {
    let needles = [format!("fn {name}"), format!("struct {name}"), format!("enum {name}"), format!("trait {name}")];
    grep(root, &["crates"], &|line| needles.iter().any(|n| line.contains(n.as_str())), 10)
}

/// Best-effort intent → seed paths.
fn resolve_intent(intent: &str) -> Vec<String> {
    let i = intent.to_ascii_lowercase();
    let mut out = Vec::new();
    if i.contains("gradient") {
        out.push("crates/doc/src/adjust.rs".to_string());
    }
    if i.contains("codec") || i.contains("format") || i.contains("dds") || i.contains("tga") || i.contains("nef") {
        out.push("crates/codecs/src/format.rs".to_string());
    }
    if i.contains("mask") {
        out.push("crates/engine/src/layer_multi_cmds.rs".to_string());
    }
    if i.contains("brush") {
        out.push("crates/paint/src/lib.rs".to_string());
    }
    out
}

/// Recursive text grep over the given roots, returning repo-relative file paths (capped).
pub(crate) fn grep(root: &Path, roots: &[&str], predicate: &impl Fn(&str) -> bool, cap: usize) -> Vec<String> {
    let mut out = Vec::new();
    for r in roots {
        walk(&root.join(r), root, predicate, cap, &mut out);
        if out.len() >= cap {
            break;
        }
    }
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, root: &Path, predicate: &impl Fn(&str) -> bool, cap: usize, out: &mut Vec<String>) {
    if out.len() >= cap {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= cap {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            walk(&path, root, predicate, cap, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.lines().any(predicate)
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.push(rel.display().to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_touch(impact: &Impact, path: &str) -> bool {
        impact.touch.iter().any(|t| t.path == path)
    }

    #[test]
    fn doc_change_touches_format_and_io() {
        let impact = analyze(&["crates/doc/src/lib.rs".to_string()]);
        assert!(has_touch(&impact, "crates/format/src/manifest.rs"), "{:?}", impact.touch);
        assert!(has_touch(&impact, "crates/format/src/convert.rs"));
        assert!(impact.invariants.iter().any(|i| i.contains("serde(default)")));
    }

    #[test]
    fn adjustment_touches_psd_compose_gpu_and_editor() {
        let impact = analyze(&["crates/doc/src/adjust.rs".to_string()]);
        assert!(has_touch(&impact, "crates/io/src/adjust_map.rs"));
        assert!(has_touch(&impact, "crates/gpu/src/plan.rs"));
        assert!(has_touch(&impact, "crates/ui-egui/src/adjust_editors.rs"));
    }

    #[test]
    fn format_variant_touches_dispatch_and_tests() {
        let impact = analyze(&["crates/codecs/src/format.rs".to_string()]);
        assert!(has_touch(&impact, "crates/io/src/flat.rs"));
        assert!(impact.invariants.iter().any(|i| i.contains("standalone")));
    }

    #[test]
    fn engine_command_touches_registry_and_regenerates_parity() {
        let impact = analyze(&["crates/engine/src/retouch_cmds.rs".to_string()]);
        assert!(has_touch(&impact, "crates/engine/src/commands.rs"));
        assert!(has_touch(&impact, "crates/ui-egui/src/menu_catalog.rs"));
        assert!(impact.regenerate.iter().any(|r| r.artifact == "docs/parity.md"));
        assert!(impact.invariants.iter().any(|i| i.contains("never panic")));
    }

    #[test]
    fn preference_change_touches_dialog_i18n_and_scorecard() {
        let impact = analyze(&["crates/engine/src/prefs.rs".to_string()]);
        assert!(has_touch(&impact, "crates/ui-egui/src/prefs_ui.rs"), "{:?}", impact.touch);
        assert!(has_touch(&impact, "crates/ui-egui/src/i18n/mod.rs"));
        assert!(has_touch(&impact, "docs/scorecard.md"));
        assert!(impact.regenerate.iter().any(|r| r.artifact == "docs/scorecard.md"));
    }

    #[test]
    fn ui_change_notes_localization() {
        let impact = analyze(&["crates/ui-egui/src/panels.rs".to_string()]);
        assert!(impact.invariants.iter().any(|i| i.contains("localization")));
    }

    #[test]
    fn unrelated_path_still_gets_universal_invariants() {
        let impact = analyze(&["crates/geom/src/lib.rs".to_string()]);
        assert!(impact.touch.is_empty());
        assert!(impact.invariants.iter().any(|i| i.contains("Never crash")));
    }
}
