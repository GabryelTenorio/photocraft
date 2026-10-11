//! `cargo xtask plan [--changed | <paths>...] [--json]`: map changed paths to the finish gates.
//!
//! This is the machine-readable bridge for the PhotoCraft `forge` development MCP's
//! `checks_for_changes` tool (and for humans). It reuses the authoritative policy — `corpus::CRITICAL`,
//! `layers::TABLE`, `scorecard::SOURCES` — rather than re-encoding it, so the two never drift.
//!
//! Inputs:
//! - `--changed`  take the path set from git (merge-base with `origin/main` when available, plus
//!   staged/unstaged and untracked files).
//! - `<paths>...` explicit paths, used verbatim.
//! - `--json`     emit `{changed, required, recommended}` instead of a table. Each gate is
//!   `{id, command, reason}`; `required` must pass, `recommended` is advisory (hot paths, platform
//!   CI, docs).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde::Serialize;

use crate::{corpus, layers, scorecard};

/// A finish gate: what to run and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Gate {
    pub id: &'static str,
    pub command: &'static str,
    pub reason: &'static str,
}

const CI: Gate = Gate { id: "ci", command: "cargo xtask ci", reason: "fmt, clippy -D warnings and the workspace tests" };
const LAYERS: Gate = Gate { id: "layers", command: "cargo xtask layers", reason: "manifests/dependencies moved; enforce the crate layer table" };
const WASM: Gate = Gate { id: "wasm", command: "cargo xtask wasm", reason: "a wasm-safe crate (L0-L5 / standalone / ui-egui) changed" };
const PARITY: Gate = Gate {
    id: "parity",
    command: "cargo xtask parity --check",
    reason: "engine menu/command coverage; if stale, run `cargo xtask parity` and commit docs/parity-checklist.md",
};
const PANIC_HUNT: Gate = Gate {
    id: "panic_hunt",
    command: "cargo test -p photocraft-engine --test panic_hunt -- --ignored",
    reason: "engine commands must reject adversarial params without panicking",
};
const CORPUS: Gate = Gate {
    id: "test-corpus",
    command: "cargo xtask test-corpus",
    reason: "a corpus-routed crate (psd/io/codecs/compose/gpu/text/format/affinity/heif/raw) changed",
};
const SCORECARD: Gate = Gate {
    id: "scorecard",
    command: "cargo xtask scorecard --check",
    reason: "a scorecard source changed (budgets, corpus floors, prefs, parity, lib/main roots); if stale, run `cargo xtask scorecard` and commit docs/scorecard.md",
};
const PERF: Gate = Gate { id: "perf", command: "cargo xtask perf --quick", reason: "a hot path (raster/paint/algo/compose/gpu) changed" };
const REMOTE: Gate = Gate {
    id: "remote-platform",
    command: "(platform CI jobs)",
    reason: "packaging/workflow change; local gates cannot prove macOS/Windows/FreeBSD/packaging",
};
const MDBOOK: Gate = Gate { id: "mdbook", command: "mdbook build book", reason: "docs changed" };

/// The corpus-routed crates, from the one authoritative list.
fn is_critical(p: &str) -> bool {
    corpus::CRITICAL.iter().any(|c| p.starts_with(c))
}

/// A scorecard source: a directory prefix (entries ending `/`) or an exact file.
fn is_scorecard_source(p: &str) -> bool {
    scorecard::SOURCES.iter().any(|s| if s.ends_with('/') { p.starts_with(s) } else { p == *s })
}

/// Is a crate short name in the wasm set (L0-L5, standalone, or ui-egui)?
fn is_wasm_crate(short: &str) -> bool {
    match layers::classify(short) {
        Some(layers::Class::Layer(l)) => l <= 5 || short == "ui-egui",
        Some(layers::Class::Standalone) => true,
        _ => false,
    }
}

/// The crate short name of a `crates/<name>/...` path.
fn crate_of(p: &str) -> Option<&str> {
    p.strip_prefix("crates/").and_then(|r| r.split('/').next()).filter(|n| !n.is_empty())
}

/// An engine command source: a `*_cmds.rs` module (or a file inside a `*_cmds/` module), or the
/// command registry `commands.rs`. Command coverage (`parity`) and the adversarial fuzz
/// (`panic_hunt`) matter here; ordinary engine modules (prefs, diagnostics) do not need them.
fn is_engine_command(p: &str) -> bool {
    p.starts_with("crates/engine/src/") && (p.ends_with("_cmds.rs") || p.contains("_cmds/") || p.ends_with("/commands.rs"))
}

/// A plan: the paths considered, and the gates they require/recommend.
#[derive(Debug, Serialize)]
pub struct Plan {
    pub changed: Vec<String>,
    pub required: Vec<Gate>,
    pub recommended: Vec<Gate>,
}

fn insert(map: &mut BTreeMap<&'static str, Gate>, gate: Gate) {
    map.entry(gate.id).or_insert(gate);
}

/// Map a path set to its finish gates.
pub fn classify(paths: &[String]) -> Plan {
    let mut required: BTreeMap<&'static str, Gate> = BTreeMap::new();
    let mut recommended: BTreeMap<&'static str, Gate> = BTreeMap::new();
    for p in paths {
        if p == "Cargo.toml" || p == "Cargo.lock" || p.ends_with("/Cargo.toml") {
            insert(&mut required, CI);
            insert(&mut required, LAYERS);
        }
        if p.starts_with("crates/") || p.starts_with("apps/") || p.starts_with("xtask/") {
            insert(&mut required, CI);
        }
        if let Some(name) = crate_of(p) {
            if is_wasm_crate(name) {
                insert(&mut required, WASM);
            }
            if is_engine_command(p) {
                insert(&mut required, PARITY);
                insert(&mut required, PANIC_HUNT);
            }
        }
        if is_critical(p) {
            insert(&mut required, CORPUS);
        }
        // A crate/app lib or bin root can change the scorecard's never-crash coverage count.
        let crate_root = (p.starts_with("crates/") || p.starts_with("apps/")) && (p.ends_with("/src/lib.rs") || p.ends_with("/src/main.rs"));
        if is_scorecard_source(p) || crate_root {
            insert(&mut required, SCORECARD);
        }
        if ["crates/raster/", "crates/paint/", "crates/algo/", "crates/compose/", "crates/gpu/"].iter().any(|c| p.starts_with(c)) {
            insert(&mut recommended, PERF);
        }
        if p.starts_with(".github/workflows/") || p.starts_with("packaging/") {
            insert(&mut recommended, REMOTE);
        }
        if p.starts_with("docs/") || p.starts_with("book/") {
            insert(&mut recommended, MDBOOK);
        }
    }
    Plan { changed: paths.to_vec(), required: required.into_values().collect(), recommended: recommended.into_values().collect() }
}

/// `git -C <root> <args>`, trimmed stdout, error on failure.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!("git {}: exited {}", args.join(" "), out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn lines(s: &str) -> impl Iterator<Item = String> + '_ {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string)
}

/// Paths changed vs the merge-base with `origin/main` (fallback: `HEAD`), plus untracked files.
fn changed_paths(root: &Path) -> Result<Vec<String>, String> {
    let base = git(root, &["merge-base", "HEAD", "origin/main"]).ok().filter(|s| !s.is_empty());
    let base = base.unwrap_or_else(|| "HEAD".to_string());
    let mut paths: Vec<String> = lines(&git(root, &["diff", "--name-only", &base])?).collect();
    if let Ok(untracked) = git(root, &["ls-files", "--others", "--exclude-standard"]) {
        paths.extend(lines(&untracked));
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub fn run(root: &Path, rest: &[&str]) -> Result<(), String> {
    if let Some(bad) = rest.iter().find(|a| a.starts_with("--") && **a != "--json" && **a != "--changed") {
        return Err(format!("plan: unknown option `{bad}`"));
    }
    let json = rest.contains(&"--json");
    let explicit: Vec<String> = rest.iter().filter(|a| !a.starts_with("--")).map(|s| (*s).to_string()).collect();
    let paths = if !explicit.is_empty() {
        explicit
    } else if rest.contains(&"--changed") {
        changed_paths(root)?
    } else {
        return Err("plan: pass one or more paths, or --changed".to_string());
    };
    let plan = classify(&paths);
    if json {
        println!("{}", serde_json::to_string(&plan).map_err(|e| format!("plan: serialize: {e}"))?);
        return Ok(());
    }
    println!("Changed paths ({}):", plan.changed.len());
    for p in &plan.changed {
        println!("  {p}");
    }
    let show = |label: &str, gates: &[Gate]| {
        println!("\n{label} ({}):", gates.len());
        for g in gates {
            println!("  [{}] {}\n        {}", g.id, g.command, g.reason);
        }
    };
    show("Required", &plan.required);
    show("Recommended", &plan.recommended);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(gates: &[Gate], id: &str) -> bool {
        gates.iter().any(|g| g.id == id)
    }

    #[test]
    fn engine_change_requires_parity_panic_hunt_and_wasm() {
        let plan = classify(&["crates/engine/src/foo_cmds.rs".to_string()]);
        assert!(has(&plan.required, "ci"));
        assert!(has(&plan.required, "wasm"));
        assert!(has(&plan.required, "parity"));
        assert!(has(&plan.required, "panic_hunt"));
    }

    #[test]
    fn non_command_engine_file_needs_neither_parity_nor_panic_hunt() {
        let plan = classify(&["crates/engine/src/prefs.rs".to_string()]);
        assert!(has(&plan.required, "ci"));
        assert!(has(&plan.required, "wasm"));
        assert!(has(&plan.required, "scorecard"));
        assert!(!has(&plan.required, "parity"), "{:?}", plan.required);
        assert!(!has(&plan.required, "panic_hunt"), "{:?}", plan.required);
    }

    #[test]
    fn corpus_crate_requires_corpus_and_wasm() {
        let plan = classify(&["crates/codecs/src/format.rs".to_string()]);
        assert!(has(&plan.required, "test-corpus"));
        assert!(has(&plan.required, "wasm"));
    }

    #[test]
    fn manifest_requires_layers_and_ci() {
        let plan = classify(&["Cargo.toml".to_string(), "crates/doc/Cargo.toml".to_string()]);
        assert!(has(&plan.required, "layers"));
        assert!(has(&plan.required, "ci"));
    }

    #[test]
    fn scorecard_source_requires_scorecard_check() {
        let plan = classify(&["scorecard/tools.toml".to_string()]);
        assert!(has(&plan.required, "scorecard"));
    }

    #[test]
    fn lib_root_requires_scorecard_check() {
        let plan = classify(&["crates/doc/src/lib.rs".to_string()]);
        assert!(has(&plan.required, "scorecard"));
    }

    #[test]
    fn xtask_root_requires_ci_but_not_scorecard() {
        let plan = classify(&["xtask/src/main.rs".to_string()]);
        assert!(has(&plan.required, "ci"));
        assert!(!has(&plan.required, "scorecard"));
    }

    #[test]
    fn hot_path_recommends_perf() {
        let plan = classify(&["crates/compose/src/lib.rs".to_string()]);
        assert!(has(&plan.recommended, "perf"));
    }

    #[test]
    fn docs_recommend_mdbook() {
        let plan = classify(&["docs/development.md".to_string()]);
        assert!(has(&plan.recommended, "mdbook"));
    }

    #[test]
    fn gates_deduplicate() {
        let plan = classify(&["crates/io/src/a.rs".to_string(), "crates/io/src/b.rs".to_string()]);
        assert_eq!(plan.required.iter().filter(|g| g.id == "ci").count(), 1);
    }

    #[test]
    fn json_round_trips() {
        let plan = classify(&["crates/engine/src/foo_cmds.rs".to_string()]);
        let s = serde_json::to_string(&plan);
        assert!(s.is_ok());
        let s = s.unwrap_or_default();
        assert!(s.contains("\"required\""));
        assert!(s.contains("\"panic_hunt\""));
    }
}
