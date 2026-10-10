//! Invoke the authoritative `xtask` policy (read-only). Forge never re-encodes the gate table.

use std::path::Path;
use std::process::Command;

/// `cargo xtask plan --json <paths>` — the authoritative gate plan, or `None` for no paths.
pub fn plan_json(root: &Path, paths: &[String]) -> Result<Option<serde_json::Value>, String> {
    if paths.is_empty() {
        return Ok(None);
    }
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["run", "-q", "-p", "xtask", "--", "plan", "--json"])
        .args(paths)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo xtask plan: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo xtask plan failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo xtask plan: bad JSON: {e}"))?;
    Ok(Some(value))
}
