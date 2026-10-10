//! An append-only log of what Forge was asked and answered (`target/forge/events.jsonl`).
//!
//! This is the seed of a queryable development record: an agent (or a later session) can read back
//! what was planned and why, instead of re-running it. Local, plain JSON lines, no dependency — it
//! can be deleted and rebuilt at any time.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Keep the log bounded: past this size, only the newest [`KEEP_LINES`] are kept.
const MAX_BYTES: u64 = 4 << 20;
const KEEP_LINES: usize = 2000;

/// Where the log lives for a checkout.
pub fn log_path(root: &Path) -> PathBuf {
    root.join("target/forge/events.jsonl")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Append one event. Best effort: logging never fails a tool call.
pub fn append(root: &Path, kind: &str, data: Value) {
    let path = log_path(root);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Rare: trim to the newest lines once the file grows past the cap.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES)
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        let kept: Vec<&str> = text.lines().rev().take(KEEP_LINES).collect();
        let _ = std::fs::write(&path, kept.into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = writeln!(file, "{}", json!({"ts": now_ms(), "kind": kind, "data": data}));
}

/// The most recent events, newest first, optionally filtered by `kind` and a minimum timestamp
/// (ms since the epoch); capped at 500.
pub fn read(root: &Path, kind: Option<&str>, since: Option<u64>, limit: usize) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(log_path(root)) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
    if let Some(kind) = kind {
        out.retain(|e| e.get("kind").and_then(Value::as_str) == Some(kind));
    }
    if let Some(since) = since {
        out.retain(|e| e.get("ts").and_then(Value::as_u64).is_some_and(|ts| ts >= since));
    }
    out.reverse();
    out.truncate(limit.min(500));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_then_read_filtered_newest_first() {
        let dir = std::env::temp_dir().join(format!("forge-events-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create");
        append(&dir, "a", json!({"x": 1}));
        append(&dir, "b", json!({"x": 2}));
        append(&dir, "a", json!({"x": 3}));
        let all = read(&dir, None, None, 10);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0]["data"]["x"], 3, "newest first");
        let only_a = read(&dir, Some("a"), None, 10);
        assert_eq!(only_a.len(), 2);
        assert!(only_a.iter().all(|e| e["kind"] == "a"));
        // `since` after every recorded event returns nothing.
        let future = now_ms() + 60_000;
        assert!(read(&dir, None, Some(future), 10).is_empty());
        assert_eq!(read(&dir, None, Some(0), 10).len(), 3);
        assert!(read(&dir.join("missing"), None, None, 10).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
