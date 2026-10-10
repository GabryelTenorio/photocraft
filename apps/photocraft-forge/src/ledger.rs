//! Per-tool call ledger.
//!
//! Forge grows tools on demand: a tool is born when a task needs it, and may only be used at that
//! time. This records calls/errors/last-used per tool (durable in `target/forge/tool_calls.json`)
//! so the maintainer can see which tools still earn their place and retire — archive — the rest
//! without guessing. Recording is best-effort: it never fails or perturbs a call.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const FILE: &str = "tool_calls.json";

/// A tool not called for this many days is a retirement (archive) candidate.
pub const RETIRE_AFTER_DAYS: u64 = 30;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// One tool's usage.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Entry {
    pub calls: u64,
    pub errors: u64,
    /// ms since the epoch of the last call (0 = never).
    pub last_ms: u64,
}

/// A ledger row, for reporting.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub tool: String,
    #[serde(flatten)]
    pub entry: Entry,
    /// No call in [`RETIRE_AFTER_DAYS`]: retire it (archive) unless a live need is known.
    pub dormant: bool,
}

/// Whether a last-call timestamp is old enough to call the tool dormant.
pub fn is_dormant(last_ms: u64, now_ms: u64) -> bool {
    last_ms != 0 && now_ms.saturating_sub(last_ms) > RETIRE_AFTER_DAYS * DAY_MS
}

static LEDGER: Mutex<Option<BTreeMap<String, Entry>>> = Mutex::new(None);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn file(root: &Path) -> PathBuf {
    root.join("target/forge").join(FILE)
}

fn load(root: Option<&Path>) -> BTreeMap<String, Entry> {
    let Some(root) = root else { return BTreeMap::new() };
    let Ok(text) = std::fs::read_to_string(file(root)) else { return BTreeMap::new() };
    serde_json::from_str(&text).unwrap_or_default()
}

fn persist(root: &Path, map: &BTreeMap<String, Entry>) {
    let Ok(text) = serde_json::to_string_pretty(map) else { return };
    let path = file(root);
    if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_err()
    {
        return;
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Record one call. Best-effort; never panics and never fails the caller.
pub fn record(root: Option<&Path>, tool: &str, error: bool) {
    let Ok(mut guard) = LEDGER.lock() else { return };
    let map = guard.get_or_insert_with(|| load(root));
    let entry = map.entry(tool.to_string()).or_default();
    entry.calls = entry.calls.saturating_add(1);
    if error {
        entry.errors = entry.errors.saturating_add(1);
    }
    entry.last_ms = now_ms();
    if let Some(root) = root {
        persist(root, map);
    }
}

/// The ledger, most-used first (ties by tool name), for reporting.
pub fn rows(root: Option<&Path>) -> Vec<Row> {
    let map = match LEDGER.lock() {
        Ok(guard) => guard.clone().unwrap_or_else(|| load(root)),
        Err(_) => load(root),
    };
    let now = now_ms();
    let mut rows: Vec<Row> = map.into_iter().map(|(tool, entry)| Row { dormant: is_dormant(entry.last_ms, now), tool, entry }).collect();
    rows.sort_by(|a, b| b.entry.calls.cmp(&a.entry.calls).then_with(|| a.tool.cmp(&b.tool)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_persists_counts() {
        let dir = std::env::temp_dir().join(format!("forge-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Two calls, one error, then a fresh read from disk.
        record(Some(&dir), "some_tool", false);
        record(Some(&dir), "some_tool", true);
        let text = std::fs::read_to_string(file(&dir)).unwrap();
        let map: BTreeMap<String, Entry> = serde_json::from_str(&text).unwrap();
        assert_eq!(map["some_tool"].calls, 2);
        assert_eq!(map["some_tool"].errors, 1);
        assert!(map["some_tool"].last_ms > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dormancy_needs_an_old_call() {
        let now = 1_000_000_000_000u64;
        assert!(!is_dormant(0, now), "never-called is not 'dormant' here (it is simply absent)");
        assert!(!is_dormant(now - DAY_MS, now), "called yesterday");
        assert!(is_dormant(now - (RETIRE_AFTER_DAYS + 1) * DAY_MS, now), "older than the threshold");
    }
}
