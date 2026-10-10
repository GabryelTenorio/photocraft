//! `checks_for_changes`: which finish gates a working diff requires, and why.
//!
//! The gate policy itself is owned by `xtask` and reused via `cargo xtask plan --json` (D2); this
//! module only enumerates the changed paths (explicit, or via git against a base) and attaches the
//! source fingerprint for provenance.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::git;

/// A request for the finish gates of a change.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Query {
    /// Changed paths (repo-relative). When empty, the diff against `base` is used.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Git ref to diff against. Default: the merge-base with `origin/main`, else `HEAD`.
    #[serde(default)]
    pub base: Option<String>,
}

/// The gate report for a change.
#[derive(Debug, Serialize)]
pub struct Checks {
    pub base: String,
    pub changed: Vec<String>,
    pub required: Vec<serde_json::Value>,
    pub recommended: Vec<serde_json::Value>,
    pub fingerprint: Fingerprint,
    pub notes: Vec<String>,
}

/// Just enough provenance to tie the report to a source state.
#[derive(Debug, Serialize)]
pub struct Fingerprint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    pub dirty: bool,
}

/// Enumerate changed paths (tracked diff vs `base` + untracked), sorted and de-duplicated.
pub fn changed_vs(root: &Path, base: &str) -> Vec<String> {
    let mut paths: Vec<String> = git::try_run(root, &["diff", "--name-only", base]).map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default();
    if let Some(untracked) = git::try_run(root, &["ls-files", "--others", "--exclude-standard"]) {
        paths.extend(untracked.lines().map(str::to_string));
    }
    paths.sort();
    paths.dedup();
    paths
}

/// The default base: the merge-base with `origin/main`, else `HEAD`.
pub fn default_base(root: &Path) -> String {
    git::try_run(root, &["merge-base", "HEAD", "origin/main"]).unwrap_or_else(|| "HEAD".to_string())
}

/// Build the gate report.
pub fn run(root: &Path, query: Query) -> Result<Checks, String> {
    let base = query.base.clone().unwrap_or_else(|| default_base(root));
    let changed = if query.paths.is_empty() { changed_vs(root, &base) } else { query.paths.clone() };

    let plan = crate::xtask::plan_json(root, &changed)?;
    let (required, recommended) = match plan {
        Some(v) => (
            v.get("required").and_then(serde_json::Value::as_array).cloned().unwrap_or_default(),
            v.get("recommended").and_then(serde_json::Value::as_array).cloned().unwrap_or_default(),
        ),
        None => (Vec::new(), Vec::new()),
    };

    let mut notes = Vec::new();
    if changed.is_empty() {
        notes.push("no changed paths — pass `paths` or a `base` with changes".to_string());
    }
    notes.push("run each gate and confirm it actually executed tests; a zero-test run is not a pass".to_string());

    let fp = crate::context::fingerprint(root);
    Ok(Checks { base, changed, required, recommended, fingerprint: Fingerprint { head: fp.head, dirty: fp.dirty }, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn temp_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-checks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create");
        let git = |args: &[&str]| {
            Command::new("git").arg("-C").arg(&dir).args(args).output().expect("git");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "forge@example.invalid"]);
        git(&["config", "user.name", "Forge Test"]);
        std::fs::write(dir.join("tracked.rs"), "a\n").expect("write");
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn changed_paths_include_modified_and_untracked() {
        let dir = temp_repo();
        std::fs::write(dir.join("tracked.rs"), "b\n").expect("modify");
        std::fs::write(dir.join("untracked.rs"), "new\n").expect("new");
        let changed = changed_vs(&dir, "HEAD");
        assert!(changed.contains(&"tracked.rs".to_string()), "{changed:?}");
        assert!(changed.contains(&"untracked.rs".to_string()), "{changed:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
