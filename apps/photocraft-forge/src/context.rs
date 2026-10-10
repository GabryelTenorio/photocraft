//! `development_context`: the snapshot an agent needs to orient — the source fingerprint, the
//! workspace crate map, and the authoritative layering report.

use std::path::Path;
use std::process::Command;

use serde::Serialize;

use crate::git;

/// The full orientation snapshot.
#[derive(Debug, Serialize)]
pub struct Context {
    pub repo_root: String,
    pub fingerprint: Fingerprint,
    /// Forge's own build identity (research §6/§14 Phase 0): which binary produced this result.
    pub forge: ForgeIdentity,
    pub crates: Vec<CrateInfo>,
    pub layers: LayersReport,
}

/// The coordinator's own build identity — so a result says *which* Forge produced it.
#[derive(Debug, Serialize)]
pub struct ForgeIdentity {
    /// Forge's crate version.
    pub version: String,
    /// Cargo profile (`debug`, `release`, …).
    pub profile: String,
    /// Target triple this binary was built for.
    pub target: String,
    /// Absolute path to the running binary, when the OS reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    /// The running binary itself, hashed (the build-artifact identity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<crate::artifact::Artifact>,
}

/// Source fingerprint — identifies *exactly* which code is on disk.
#[derive(Debug, Serialize)]
pub struct Fingerprint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub dirty: bool,
    /// `git status --porcelain` lines for tracked/untracked changes.
    pub changed: Vec<String>,
    /// `git diff | git hash-object --stdin` — a cheap content hash of the worktree diff.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_diff_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staged_diff_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cargo_lock_hash: Option<String>,
    /// How many commits `origin/main` is ahead of HEAD (a release/main-ancestry hint).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind_origin_main: Option<u64>,
}

/// A workspace crate.
#[derive(Debug, Serialize)]
pub struct CrateInfo {
    pub name: String,
    pub version: String,
    /// Manifest path, relative to the repo root.
    pub path: String,
    /// Names of workspace crates this one depends on.
    pub workspace_deps: Vec<String>,
}

/// The `cargo xtask layers` report (the authority; reused, not re-encoded).
#[derive(Debug, Serialize)]
pub struct LayersReport {
    pub ok: bool,
    pub output: String,
}

/// Build the context for a checkout.
pub fn build(root: &Path) -> Result<Context, String> {
    Ok(Context { repo_root: root.display().to_string(), fingerprint: fingerprint(root), forge: forge_identity(), crates: crates(root)?, layers: layers(root) })
}

/// Forge's build identity. Never fails: a missing binary just leaves the artifact fields absent.
fn forge_identity() -> ForgeIdentity {
    let exe = std::env::current_exe().ok();
    let build = exe.as_deref().and_then(|p| {
        let name = p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
        crate::artifact::Artifact::from_file(name, crate::artifact::media_type(p), p)
    });
    ForgeIdentity {
        version: env!("CARGO_PKG_VERSION").to_string(),
        profile: env!("FORGE_BUILD_PROFILE").to_string(),
        target: env!("FORGE_BUILD_TARGET").to_string(),
        artifact: exe.map(|p| p.display().to_string()),
        build,
    }
}

/// Never fails: missing pieces are simply absent.
pub fn fingerprint(root: &Path) -> Fingerprint {
    let changed: Vec<String> = git::try_run(root, &["status", "--porcelain=v1"]).map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default();
    let diff_hash = |args: &[&str]| git::run_bytes(root, args).ok().and_then(|b| git::hash_bytes(root, &b).ok());
    Fingerprint {
        head: git::try_run(root, &["rev-parse", "HEAD"]),
        branch: git::try_run(root, &["rev-parse", "--abbrev-ref", "HEAD"]),
        dirty: !changed.is_empty(),
        changed,
        worktree_diff_hash: diff_hash(&["diff"]),
        staged_diff_hash: diff_hash(&["diff", "--cached"]),
        cargo_lock_hash: git::try_run(root, &["hash-object", "Cargo.lock"]),
        behind_origin_main: git::try_run(root, &["rev-list", "--count", "HEAD..origin/main"]).and_then(|s| s.parse().ok()),
    }
}

fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

fn crates(root: &Path) -> Result<Vec<CrateInfo>, String> {
    let out = cargo().args(["metadata", "--no-deps", "--format-version", "1"]).current_dir(root).output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    parse_packages(root, &String::from_utf8_lossy(&out.stdout))
}

/// Parse `cargo metadata --no-deps` into the crate map (workspace members only).
pub fn parse_packages(root: &Path, json: &str) -> Result<Vec<CrateInfo>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("cargo metadata: bad JSON: {e}"))?;
    let packages = v.get("packages").and_then(serde_json::Value::as_array).ok_or("cargo metadata: no packages")?;
    let mut crates = Vec::with_capacity(packages.len());
    for p in packages {
        let manifest = p.get("manifest_path").and_then(serde_json::Value::as_str).unwrap_or_default();
        let path = Path::new(manifest).strip_prefix(root).map(|r| r.display().to_string()).unwrap_or_else(|_| manifest.to_string());
        let workspace_deps = p
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .map(|deps| {
                deps.iter()
                    .filter(|d| d.get("path").is_some_and(|p| !p.is_null()))
                    .filter_map(|d| d.get("name").and_then(serde_json::Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        crates.push(CrateInfo {
            name: p.get("name").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
            version: p.get("version").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
            path,
            workspace_deps,
        });
    }
    crates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(crates)
}

fn layers(root: &Path) -> LayersReport {
    match cargo().args(["run", "-q", "-p", "xtask", "--", "layers"]).current_dir(root).output() {
        Ok(o) => LayersReport { ok: o.status.success(), output: String::from_utf8_lossy(&o.stdout).trim().to_string() },
        Err(e) => LayersReport { ok: false, output: format!("failed to run `cargo xtask layers`: {e}") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn temp_repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-ctx-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp repo");
        let git = |args: &[&str]| {
            Command::new("git").arg("-C").arg(&dir).args(args).output().expect("run git");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "forge@example.invalid"]);
        git(&["config", "user.name", "Forge Test"]);
        std::fs::write(dir.join("README.md"), "hello\n").expect("write");
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn fingerprint_reports_a_clean_commit() {
        let dir = temp_repo("clean");
        let fp = fingerprint(&dir);
        assert!(fp.head.is_some(), "{fp:?}");
        assert!(fp.branch.is_some());
        assert!(!fp.dirty, "{fp:?}");
        assert!(fp.changed.is_empty());
        assert!(fp.worktree_diff_hash.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fingerprint_sees_an_uncommitted_change() {
        let dir = temp_repo("dirty");
        std::fs::write(dir.join("README.md"), "changed\n").expect("write");
        let fp = fingerprint(&dir);
        assert!(fp.dirty);
        assert!(fp.changed.iter().any(|l| l.contains("README.md")), "{:?}", fp.changed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_packages_keeps_workspace_deps_only() {
        let json = r#"{"packages":[
            {"name":"photocraft-engine","version":"0.5.0","manifest_path":"/repo/crates/engine/Cargo.toml",
             "dependencies":[
                {"name":"photocraft-doc","path":"/repo/crates/doc"},
                {"name":"serde","path":null}
             ]},
            {"name":"serde","version":"1.0.0","manifest_path":"/repo/x/Cargo.toml","dependencies":[]}
        ]}"#;
        let crates = parse_packages(Path::new("/repo"), json).expect("parse");
        let engine = crates.iter().find(|c| c.name == "photocraft-engine").expect("engine");
        assert_eq!(engine.path, "crates/engine/Cargo.toml");
        assert_eq!(engine.workspace_deps, vec!["photocraft-doc".to_string()]);
    }
}
