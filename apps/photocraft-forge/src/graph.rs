//! Optional impact-graph enrichment from **host** MCPs (`jCodeMunch`, `rust-lsp`).
//!
//! Nothing here is required (D1/D4): Forge works without these companions. When present, `change_impact`
//! can consult them for importers/affected files; every call is bounded by a timeout and any failure
//! degrades to a note. Forge never bundles or installs them.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Which optional companions are present on `PATH`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Companions {
    pub jcodemunch: bool,
    pub rust_lsp: bool,
}

/// Detect the optional companions.
pub fn detect() -> Companions {
    Companions {
        jcodemunch: crate::probe::on_path("jcodemunch-mcp"),
        rust_lsp: crate::probe::on_path("mcp-language-server") && crate::probe::on_path("rust-analyzer"),
    }
}

/// What enrichment produced.
#[derive(Debug, Default)]
pub struct Enrichment {
    /// Repo-relative files the graph says are affected (advisory, not must-change).
    pub related: Vec<String>,
    pub used: Vec<String>,
    pub notes: Vec<String>,
}

const TIMEOUT: Duration = Duration::from_secs(20);

/// Consult the available companions for affected files.
pub fn enrich(root: &Path, changed: &[String], symbol: Option<&str>, companions: Companions) -> Enrichment {
    let mut e = Enrichment::default();

    // Companion graph (jCodeMunch), when present. It needs the *repo id* for every call, so resolve
    // it once from the checkout path first; a tool-level failure is reported, never counted as used.
    if companions.jcodemunch && !changed.is_empty() {
        match resolve_repo_id(root) {
            Ok(repo) => match mcp_call("jcodemunch-mcp", &["serve"], root, "find_importers", json!({ "repo": repo, "file_paths": changed }), TIMEOUT) {
                Ok(text) => {
                    let found = parse_importers(&text, changed);
                    if found.is_empty() {
                        e.notes.push("jCodeMunch returned no importers (its Rust index may be empty)".to_string());
                    } else {
                        e.used.push("jcodemunch:find_importers".to_string());
                        e.related.extend(found);
                    }
                }
                Err(err) => e.notes.push(format!("jCodeMunch enrichment skipped: {err}")),
            },
            Err(err) => e.notes.push(format!("jCodeMunch enrichment skipped: {err}")),
        }
    } else if companions.jcodemunch {
        e.notes.push("jCodeMunch present but no paths to check importers for".to_string());
    } else {
        e.notes.push("jCodeMunch not found (optional) — `uv tool install jcodemunch-mcp` enables impact-graph enrichment".to_string());
    }

    // Local module references: cheap, deterministic, and works with no companion at all. For each
    // changed Rust module, find the files that declare or `use` it.
    if !changed.is_empty() {
        let local = local_related(root, changed);
        if !local.is_empty() {
            e.used.push("local:module-refs".to_string());
            e.related.extend(local);
        }
    }

    if let Some(symbol) = symbol {
        if companions.rust_lsp {
            let workspace = root.display().to_string();
            match mcp_call(
                "mcp-language-server",
                &["-workspace", &workspace, "-lsp", "rust-analyzer"],
                root,
                "references",
                json!({ "symbolName": symbol }),
                TIMEOUT,
            ) {
                Ok(text) => {
                    e.used.push("rust-lsp:references".to_string());
                    e.related.extend(extract_repo_paths(&text));
                }
                Err(err) => e.notes.push(format!("rust-lsp enrichment skipped: {err}")),
            }
        } else {
            e.notes.push("rust-lsp (mcp-language-server + rust-analyzer) not found (optional)".to_string());
        }
    }
    e.related.sort();
    e.related.dedup();
    e
}

/// Pull `crates/…` / `apps/…` paths out of an arbitrary MCP text response.
pub fn extract_repo_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for anchor in ["crates/", "apps/"] {
        let mut start = 0;
        while let Some(i) = text[start..].find(anchor) {
            let abs = start + i;
            let rest = &text[abs..];
            let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '.' | '-'))).unwrap_or(rest.len());
            let path = &rest[..end];
            if path.ends_with(".rs") || path.ends_with(".toml") {
                out.push(path.to_string());
            }
            start = abs + end.max(1);
            if out.len() > 500 {
                break;
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The importer files in a `find_importers` response, never the queried files themselves (the tool
/// echoes each queried `file_path` back). Falls back to plain path extraction only when the response
/// is not the expected structured JSON.
pub fn parse_importers(text: &str, changed: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_str::<Value>(text)
        && let Some(results) = v.get("results").and_then(Value::as_array)
    {
        for r in results {
            for imp in r.get("importers").and_then(Value::as_array).into_iter().flatten() {
                for key in ["file", "file_path", "path"] {
                    if let Some(p) = imp.get(key).and_then(Value::as_str) {
                        out.push(p.to_string());
                    }
                }
            }
        }
    }
    if out.is_empty() {
        out = extract_repo_paths(text);
    }
    out.retain(|p| !changed.iter().any(|c| c == p));
    out.sort();
    out.dedup();
    out
}

/// Resolve the jCodeMunch repo id for a checkout (`resolve_repo` needs an absolute path).
fn resolve_repo_id(root: &Path) -> Result<String, String> {
    let text = mcp_call("jcodemunch-mcp", &["serve"], root, "resolve_repo", json!({ "path": root.display().to_string() }), TIMEOUT)?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("resolve_repo: {e}"))?;
    v.get("repo").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "resolve_repo: no repo id".to_string())
}

/// Files that reference a changed Rust module: its `mod X;` declaration or a `::X` path. Dependency
/// free, so path enrichment still works when no graph companion is present or its index is empty.
pub fn local_related(root: &Path, changed: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for path in changed {
        if !path.ends_with(".rs") {
            continue;
        }
        let Some(stem) = Path::new(path).file_stem().and_then(|s| s.to_str()) else { continue };
        if matches!(stem, "lib" | "main" | "mod") {
            continue;
        }
        let mod_needle = format!("mod {stem};");
        let use_needle = format!("::{stem}");
        let self_path = path.clone();
        let hits = crate::impact::grep(root, &["crates", "apps"], &|line| line.contains(&mod_needle) || line.contains(&use_needle), 40);
        out.extend(hits.into_iter().filter(|h| *h != self_path));
    }
    out.sort();
    out.dedup();
    out
}

/// A bounded MCP stdio call: spawn, initialize, call one tool, return its first text content.
fn mcp_call(program: &str, args: &[&str], cwd: &Path, tool: &str, arguments: Value, timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {program}: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(line.clone()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let result = (|| -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        write_line(
            &mut stdin,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"forge","version":env!("CARGO_PKG_VERSION")}}}),
        )?;
        wait_for(&rx, 1, deadline)?;
        write_line(&mut stdin, json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
        write_line(&mut stdin, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name": tool, "arguments": arguments}}))?;
        let response = wait_for(&rx, 2, deadline)?;
        let result = response.get("result").ok_or_else(|| format!("{tool}: no result"))?;
        // A tool-level `isError` (e.g. a bad argument) is a failure, not empty data.
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            let msg = result
                .get("content")
                .and_then(Value::as_array)
                .and_then(|items| items.iter().find_map(|c| c.get("text")).and_then(Value::as_str))
                .unwrap_or("tool error");
            return Err(format!("{tool}: {}", msg.lines().next().unwrap_or("").trim()));
        }
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|items| items.iter().find_map(|c| c.get("text")).and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        Ok(text)
    })();
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    result
}

fn write_line(stdin: &mut impl Write, value: Value) -> Result<(), String> {
    writeln!(stdin, "{value}").map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())
}

fn wait_for(rx: &mpsc::Receiver<String>, id: u64, deadline: Instant) -> Result<Value, String> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("timed out".to_string());
        }
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && value.get("id").and_then(Value::as_u64) == Some(id)
                {
                    if let Some(err) = value.get("error") {
                        return Err(format!("{err}"));
                    }
                    return Ok(value);
                }
            }
            Err(_) => return Err("timed out".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_repo_paths_from_free_text() {
        let text = r#"{"importers":[{"file":"crates/io/src/lib.rs"},{"file":"apps/photocraft/src/main.rs"},{"x":"not-a-path"}]}"#;
        let paths = extract_repo_paths(text);
        assert_eq!(paths, vec!["apps/photocraft/src/main.rs".to_string(), "crates/io/src/lib.rs".to_string()]);
    }

    #[test]
    fn absent_companions_only_note() {
        let e = enrich(Path::new("."), &["crates/doc/src/lib.rs".to_string()], None, Companions::default());
        assert!(e.related.is_empty());
        assert!(e.used.is_empty());
        assert!(e.notes.iter().any(|n| n.contains("jCodeMunch not found")), "{:?}", e.notes);
    }

    #[test]
    fn mcp_call_reads_a_stub_server() {
        // A tiny stdio server that answers initialize (id 1) and tools/call (id 2).
        let script = r#"while IFS= read -r line; do
          case "$line" in
            *'"method":"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{}}' ;;
            *'"method":"tools/call"'*) echo '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"crates/foo/bar.rs"}]}}' ;;
          esac
        done"#;
        let text = mcp_call("sh", &["-c", script], Path::new("."), "find_importers", json!({"file_paths":["crates/x/y.rs"]}), Duration::from_secs(10))
            .expect("stub call");
        assert!(text.contains("crates/foo/bar.rs"), "{text}");
        assert_eq!(extract_repo_paths(&text), vec!["crates/foo/bar.rs".to_string()]);
    }

    #[test]
    fn mcp_call_treats_a_tool_error_as_a_failure() {
        let script = r#"while IFS= read -r line; do
          case "$line" in
            *'"method":"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{}}' ;;
            *'"method":"tools/call"'*) echo '{"jsonrpc":"2.0","id":2,"result":{"isError":true,"content":[{"type":"text","text":"Input validation error: boom"}]}}' ;;
          esac
        done"#;
        let err = mcp_call("sh", &["-c", script], Path::new("."), "find_importers", json!({}), Duration::from_secs(10)).expect_err("should be an error");
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn parse_importers_reads_structured_results_and_excludes_the_queried_file() {
        let text = r#"{"results":[{"file_path":"crates/x/y.rs","importer_count":2,"importers":[{"file":"crates/a/b.rs"},{"file":"crates/x/y.rs"}]}]}"#;
        assert_eq!(parse_importers(text, &["crates/x/y.rs".to_string()]), vec!["crates/a/b.rs".to_string()]);
        // A response that only echoes the queried file yields nothing.
        let echoed = r#"{"results":[{"file_path":"crates/x/y.rs","importer_count":0,"importers":[]}]}"#;
        assert!(parse_importers(echoed, &["crates/x/y.rs".to_string()]).is_empty());
    }

    #[test]
    fn local_related_finds_the_module_declaration() {
        let dir = std::env::temp_dir().join(format!("forge-graph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("crates/x/src")).expect("create");
        std::fs::create_dir_all(dir.join("apps")).expect("apps");
        std::fs::write(dir.join("crates/x/src/lib.rs"), "mod foo;\n").expect("lib");
        std::fs::write(dir.join("crates/x/src/foo.rs"), "// foo\n").expect("foo");
        let related = local_related(&dir, &["crates/x/src/foo.rs".to_string()]);
        assert!(related.contains(&"crates/x/src/lib.rs".to_string()), "{related:?}");
        assert!(!related.contains(&"crates/x/src/foo.rs".to_string()), "the changed file itself is excluded");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
