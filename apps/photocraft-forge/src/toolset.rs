//! Runtime selection of dormant tools — Forge's just-in-time surface.
//!
//! A dormant tool is **compiled but not registered**, so the model never sees it until it is
//! enabled. Enabling registers it and the server sends `tools/list_changed`; disabling removes it.
//! No rebuild, no host restart, no `unsafe`. The active set is durable at `target/forge/toolset.json`
//! so a resurrected tool stays live across sessions.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const FILE: &str = "toolset.json";

/// Which dormant tools are currently live.
pub struct Toolset {
    active: Mutex<BTreeSet<String>>,
    root: Option<PathBuf>,
    generation: AtomicU64,
}

impl Toolset {
    /// Load the persisted active set for this checkout (empty when there is none).
    pub fn new(root: Option<PathBuf>) -> Self {
        let active = load(root.as_deref());
        Self { active: Mutex::new(active), root, generation: AtomicU64::new(0) }
    }

    /// Bumped whenever the active set changes, so a caller can tell whether to notify the client.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.active.lock().map(|s| s.contains(name)).unwrap_or(false)
    }

    pub fn enabled(&self) -> BTreeSet<String> {
        self.active.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Enable or disable a dormant tool. Returns whether the set actually changed.
    pub fn set(&self, name: &str, on: bool) -> bool {
        let changed = match self.active.lock() {
            Ok(mut set) => {
                if on {
                    set.insert(name.to_string())
                } else {
                    set.remove(name)
                }
            }
            Err(_) => false,
        };
        if changed {
            self.generation.fetch_add(1, Ordering::SeqCst);
            if let Some(root) = &self.root {
                persist(root, &self.enabled());
            }
        }
        changed
    }
}

fn file(root: &Path) -> PathBuf {
    root.join("target/forge").join(FILE)
}

fn load(root: Option<&Path>) -> BTreeSet<String> {
    let Some(root) = root else { return BTreeSet::new() };
    let Ok(text) = std::fs::read_to_string(file(root)) else { return BTreeSet::new() };
    serde_json::from_str::<Vec<String>>(&text).map(|names| names.into_iter().collect()).unwrap_or_default()
}

fn persist(root: &Path, active: &BTreeSet<String>) {
    let names: Vec<&String> = active.iter().collect();
    let Ok(text) = serde_json::to_string_pretty(&names) else { return };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_disable_persists_and_bumps_generation() {
        let dir = std::env::temp_dir().join(format!("forge-toolset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let set = Toolset::new(Some(dir.clone()));
        assert!(!set.is_enabled("x") && set.generation() == 0);
        assert!(set.set("x", true), "first enable changes the set");
        assert!(set.is_enabled("x") && set.generation() == 1);
        assert!(!set.set("x", true), "re-enabling changes nothing");
        // A fresh load (new session) sees the persisted active set.
        let reloaded = Toolset::new(Some(dir.clone()));
        assert!(reloaded.is_enabled("x"));
        assert!(set.set("x", false));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
