//! Read Forge's own maintenance record (`MAINTENANCE.md`) and surface its **open** rows, so the
//! agent maintaining Forge is reminded of known-broken or incomplete things at session start
//! instead of rediscovering — or quietly routing around — them.
//!
//! The registry is a Markdown table (`| Date | Area | Symptom / gap | Diagnosis / fix | Status |`)
//! under `## Failure / gap registry`; a row is open when its status cell begins with `open`.

use std::path::{Path, PathBuf};

use serde::Serialize;

const FILE: &str = "MAINTENANCE.md";
const SECTION: &str = "## Failure / gap registry";
const MAX_ITEMS: usize = 50;

/// One open maintenance row.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub area: String,
    pub gap: String,
    pub status: String,
}

/// The path to the record inside the Forge crate.
pub fn path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FILE)
}

/// The open rows, capped. Never fails: a missing or malformed file yields an empty list.
pub fn open_items() -> Vec<Item> {
    let Ok(text) = std::fs::read_to_string(path()) else {
        return Vec::new();
    };
    parse_open(&text)
}

/// Parse the registry table and return the rows whose status begins with `open`. Cells containing a
/// literal `|` are not supported (the record does not use them).
pub fn parse_open(text: &str) -> Vec<Item> {
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
        // Date | Area | Symptom / gap | Diagnosis / fix | Status
        if cells.len() < 5 {
            continue; // the header and its `|---|` separator land here or are filtered by status
        }
        let status = cells[4];
        if status.to_ascii_lowercase().starts_with("open") {
            out.push(Item { area: cells[1].to_string(), gap: cells[2].to_string(), status: status.to_string() });
        }
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
# doc

## Failure / gap registry

| Date | Area | Symptom / gap | Diagnosis / fix | Status |
|---|---|---|---|---|
| 2026-10-10 | touch registry | Rules are hand-maintained | derive from a graph | open — see MAINTENANCE |
| 2026-10-10 | enrich | path enrichment empty | fixed | resolved |

## Next
| not | this | table | x | open |
";

    #[test]
    fn only_open_rows_in_the_registry_section() {
        let items = parse_open(SAMPLE);
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].area, "touch registry");
        assert!(items[0].status.starts_with("open"));
    }
}
