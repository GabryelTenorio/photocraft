//! Thin, **read-only** git helper. Forge never mutates the repository.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn base(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(root);
    c
}

/// Run git and return trimmed UTF-8 stdout; `Err` on spawn failure or a non-zero exit.
pub fn run(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = base(root).args(args).output().map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git {}: {}", args.join(" "), err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Raw stdout bytes (no trimming) — used when feeding a diff to `git hash-object`.
pub fn run_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = base(root).args(args).output().map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

/// Like [`run`] but returns `None` instead of failing (e.g. a ref that does not exist).
pub fn try_run(root: &Path, args: &[&str]) -> Option<String> {
    run(root, args).ok().filter(|s| !s.is_empty())
}

/// Run git with `input` on stdin and return trimmed stdout.
pub fn run_with_stdin(root: &Path, args: &[&str], input: &[u8]) -> Result<String, String> {
    let mut child = base(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input).map_err(|e| format!("git {}: stdin: {e}", args.join(" ")))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git hash-object --stdin` for the given bytes (a cheap content fingerprint).
pub fn hash_bytes(root: &Path, bytes: &[u8]) -> Result<String, String> {
    run_with_stdin(root, &["hash-object", "--stdin"], bytes)
}
