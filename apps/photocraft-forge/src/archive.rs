//! The retired-tool catalog.
//!
//! Forge grows tools on demand and keeps its **live** surface lean by retiring tools that stop
//! being used (the ledger shows which). Retired tools are not advertised by the MCP, so this
//! catalog is how the agent still knows they exist: when a task needs a capability no live tool
//! provides, **check here first and resurrect a match** before building a new tool.
//!
//! The catalog is Markdown (`archive/RETIRED.md`) so the human and the agent read the same file.

use std::path::{Path, PathBuf};

use serde::Serialize;

const FILE: &str = "archive/RETIRED.md";
const SECTION: &str = "## Retired tools";
const MAX_ITEMS: usize = 200;

/// One retired tool.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub tool: String,
    /// The need it existed to serve (so a later agent can tell if the need is back).
    pub why: String,
    pub retired: String,
    /// Where the code lives in the archive.
    pub code: String,
    /// How to bring it back.
    pub restore: String,
}

/// The path to the catalog inside the Forge crate.
pub fn path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FILE)
}

/// The retired tools, capped. Never fails: a missing or malformed file yields an empty list.
pub fn retired() -> Vec<Item> {
    let Ok(text) = std::fs::read_to_string(path()) else {
        return Vec::new();
    };
    parse(&text)
}

/// Parse the `## Retired tools` table: `| Tool | Why it existed | Retired | Code | Restore |`.
/// Cells containing a literal `|` are not supported (the catalog does not use them).
pub fn parse(text: &str) -> Vec<Item> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            in_section = trimmed.eq_ignore_ascii_case(SECTION);
            continue;
        }
        if !in_section || !trimmed.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = trimmed.trim_matches('|').split('|').map(str::trim).collect();
        // Tool | Why it existed | Retired | Code | Restore
        if cells.len() < 5 {
            continue;
        }
        let tool = cells[0];
        // Skip the header row and the `|---|` separator.
        if tool.is_empty() || tool.eq_ignore_ascii_case("Tool") || tool.chars().all(|c| c == '-' || c == ':') {
            continue;
        }
        out.push(Item {
            tool: tool.to_string(),
            why: cells[1].to_string(),
            retired: cells[2].to_string(),
            code: cells[3].to_string(),
            restore: cells[4].to_string(),
        });
        if out.len() >= MAX_ITEMS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Retired tools

## Retired tools

| Tool | Why it existed | Retired | Code | Restore |
|---|---|---|---|---|
| old_picker | needed to pick a layer at a point (now in the app) | 2026-01-01 | archive/old_picker.rs | move back into src/, re-add `#[tool]` + the README row |

## Something else

| not | in | this | table | x |
";

    #[test]
    fn parses_only_the_retired_table() {
        let items = parse(SAMPLE);
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].tool, "old_picker");
        assert!(items[0].restore.contains("src/"));
    }

    #[test]
    fn an_empty_table_is_empty() {
        let items = parse("## Retired tools\n\n| Tool | Why it existed | Retired | Code | Restore |\n|---|---|---|---|---|\n");
        assert!(items.is_empty());
    }
}
