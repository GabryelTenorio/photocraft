//! Capability probe: what Forge found, and what to install when something is missing.
//!
//! Forge is environment-independent and never depends on a host MCP, but it is *better* with the
//! companions (`rust-lsp`, `jCodeMunch`). Rather than silently degrade, it reports what is present
//! and **recommends installing** what is missing, with the exact command (decision D1).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

/// Whether a capability is needed for Forge to function, or merely an accelerator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Required,
    Optional,
}

/// One tool or companion MCP Forge looks for.
#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    pub name: &'static str,
    pub kind: Kind,
    pub present: bool,
    pub summary: &'static str,
    /// Install command when absent and installable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<&'static str>,
}

/// The probe result.
#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    /// The PhotoCraft checkout Forge is running against, if one was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    pub tools: Vec<Capability>,
    /// Actionable suggestions: install a missing companion, or run from a checkout.
    pub recommendations: Vec<String>,
}

/// (tool/target, kind, summary, install command).
const TOOLS: &[(&str, Kind, &str, Option<&str>)] = &[
    ("git", Kind::Required, "local provenance: fingerprint, diff, change ancestry", None),
    ("cargo", Kind::Required, "builds and the xtask finish gates", None),
    ("wasm32-unknown-unknown", Kind::Required, "the `cargo xtask wasm` gate: L0-L6 must build for the web", Some("rustup target add wasm32-unknown-unknown")),
    ("jcodemunch-mcp", Kind::Optional, "symbol/impact graph accelerator (blast radius, importers)", Some("uv tool install jcodemunch-mcp")),
    ("rust-analyzer", Kind::Optional, "Rust semantic authority (via the rust-lsp MCP): references, types, macros", Some("rustup component add rust-analyzer")),
    (
        "mcp-language-server",
        Kind::Optional,
        "generic stdio LSP bridge that exposes rust-analyzer as `rust-lsp`",
        Some("go install github.com/isaacphi/mcp-language-server@latest"),
    ),
];

/// Probe the live environment.
pub fn capabilities() -> Capabilities {
    assemble(discover_repo_root(), present)
}

/// Is a capability present? Most are binaries on `PATH`; the wasm target is a rustup target.
fn present(name: &str) -> bool {
    if name == "wasm32-unknown-unknown" { wasm_target_installed() } else { on_path(name) }
}

/// Whether the `wasm32-unknown-unknown` rustup target is installed. A `cargo xtask wasm` gate fails
/// without it, and it is not a binary on `PATH` (no `rust-toolchain.toml` pins it, so rustup does
/// not install it automatically).
fn wasm_target_installed() -> bool {
    let Ok(out) = Command::new("rustup").args(["target", "list", "--installed"]).output() else {
        return false;
    };
    out.status.success() && String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == "wasm32-unknown-unknown")
}

/// Pure assembly, so it can be tested without touching the real environment.
pub fn assemble(repo_root: Option<PathBuf>, found: impl Fn(&str) -> bool) -> Capabilities {
    let mut recommendations = Vec::new();
    if repo_root.is_none() {
        recommendations.push("run forge from inside a PhotoCraft checkout (a directory containing crates/, apps/ and xtask/)".to_string());
    }
    let tools: Vec<Capability> = TOOLS
        .iter()
        .map(|(name, kind, summary, install)| {
            let present = found(name);
            if !present {
                match (kind, install) {
                    (Kind::Optional, Some(cmd)) => {
                        recommendations.push(format!("`{name}` not found — for {summary}: install with `{cmd}`"));
                    }
                    (Kind::Optional, None) => {
                        recommendations.push(format!("`{name}` not found — {summary}"));
                    }
                    (Kind::Required, Some(cmd)) => {
                        recommendations.push(format!("required `{name}` not installed — {summary}: install with `{cmd}`"));
                    }
                    (Kind::Required, None) => {
                        recommendations.push(format!("required `{name}` not found on PATH — {summary} will not work"));
                    }
                }
            }
            Capability { name, kind: *kind, present, summary, install: *install }
        })
        .collect();
    Capabilities { repo_root: repo_root.map(|p| p.display().to_string()), tools, recommendations }
}

/// Is `bin` an executable on `PATH`? (Shared with the optional graph enrichment.)
pub(crate) fn on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| is_executable(&dir.join(bin)))
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Walk up from the current directory to the PhotoCraft checkout root (one containing `crates/`,
/// `apps/` and `xtask/`).
pub fn discover_repo_root() -> Option<PathBuf> {
    let start = std::env::current_dir().ok()?;
    find_root(&start)
}

fn find_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join("crates").is_dir() && d.join("apps").is_dir() && d.join("xtask").is_dir() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_optional_companion_is_recommended_with_install_command() {
        let caps = assemble(Some(PathBuf::from("/repo")), |_| false);
        assert!(caps.recommendations.iter().any(|r| r.contains("uv tool install jcodemunch-mcp")));
        assert!(caps.recommendations.iter().any(|r| r.contains("rustup component add rust-analyzer")));
        assert!(caps.tools.iter().any(|t| t.name == "jcodemunch-mcp" && !t.present));
    }

    #[test]
    fn missing_required_tool_is_flagged() {
        let caps = assemble(Some(PathBuf::from("/repo")), |name| name != "git");
        assert!(caps.recommendations.iter().any(|r| r.contains("required `git`")));
    }

    #[test]
    fn missing_wasm_target_is_recommended_with_install_command() {
        let caps = assemble(Some(PathBuf::from("/repo")), |name| name != "wasm32-unknown-unknown");
        assert!(caps.recommendations.iter().any(|r| r.contains("rustup target add wasm32-unknown-unknown")), "{:?}", caps.recommendations);
        assert!(caps.tools.iter().any(|t| t.name == "wasm32-unknown-unknown" && !t.present));
    }

    #[test]
    fn all_present_and_in_repo_means_no_recommendations() {
        let caps = assemble(Some(PathBuf::from("/repo")), |_| true);
        assert!(caps.recommendations.is_empty(), "{:?}", caps.recommendations);
        assert!(caps.tools.iter().all(|t| t.present));
    }

    #[test]
    fn no_repo_root_is_recommended() {
        let caps = assemble(None, |_| true);
        assert!(caps.recommendations.iter().any(|r| r.contains("PhotoCraft checkout")));
        assert!(caps.repo_root.is_none());
    }

    #[test]
    fn find_root_walks_up() {
        let tmp = std::env::temp_dir().join(format!("forge-root-{}", std::process::id()));
        let nested = tmp.join("crates/x/src");
        std::fs::create_dir_all(&nested).unwrap_or_default();
        std::fs::create_dir_all(tmp.join("apps")).unwrap_or_default();
        std::fs::create_dir_all(tmp.join("xtask")).unwrap_or_default();
        assert_eq!(find_root(&nested), Some(tmp.clone()));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
