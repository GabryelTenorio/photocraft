# Maintaining Forge

Forge's own maintenance record. **For the agent that maintains Forge**, and the human reviewing it:
read it when a session touches Forge, and update it in the same change that fixes something.

Forge is dev-only and AI-maintained, so context is lost between sessions. If a failure or a gap is not
written down here, it is silently rediscovered — or quietly routed around — forever. The point of this
file (and the `forge_events` log behind it) is to let the maintainer **fix or update** the tooling
across sessions, not merely to prune it.

## Standing rules

- **Prefer fixing to routing around.** A broken or stale tool/companion is a bug to fix. If you can't
  fix it now, add a row below and tell the human.
- **Tell the human immediately** when a failure blocks or degrades the task — plainly, with the
  symptom and the fallback you used.
- **Record it here** — one row per distinct failure, with a `status` — and keep the row current.
  Retire a row (or shorten it) once fixed so the list stays a work queue, not an archive.
- **Review periodically:** read this file, fix the open rows, and check that the tool table in
  `README.md` still matches the tools in `src/server.rs`.
- **Keep the inventory current:** adding, renaming, or removing a tool updates the `README.md` tool
  table in the same change.
- **Durable learnings** go into the right durable doc (`README.md`, this file, `docs/`), not into a
  code comment. Prefer correcting an existing instruction over adding a new one.
- **Grow on demand.** When a task repeats and no live tool covers it, build the tool — don't keep
  working around the gap. (That is why Forge has the tools it has.)
- **Retire by paging out; resurrect hot.** A tool that stops earning its place moves to
  `src/dormant.rs` (compiled but unregistered) with a row in `archive/RETIRED.md`; it leaves the live
  set without a rebuild. The signal is the ledger (`forge_ledger`, or `forge_capabilities.dormant`):
  a tool flagged `dormant` (no call in `RETIRE_AFTER_DAYS`, 30) is a candidate. Before building a new
  tool, check `forge_capabilities.dormant` / `retired` and **resurrect a match** with
  `forge_toolset { "enable": ["<tool>"] }` — instant, and it notifies the client
  (`notifications/tools/list_changed`). The MCP advertises only live tools, so the catalogs are how a
  retired tool is still found.

## Known-bad tools and limitations (with fallbacks)

| Thing | Symptom | Fallback |
|---|---|---|
| `jCodeMunch` (`enrich:true`) | Rust symbols were absent from the index, so `find_importers` returns nothing | Cause: its `~/.code-index/config.jsonc` `languages` allowlist omitted `rust`, so every `.rs` file was indexed for text search only (silent). Fixed 2026-10-10, and `jcm-sync` now derives the per-project `languages` from `git ls-files` (+ reindexes) so it cannot recur. Forge still adds local module references (`mod X;` / `::X`) itself; treat an empty graph as "no data", never "no coupling" |
| The host MCP process | The OpenCode-spawned server keeps the binary from session start | Rebuild **and** restart the host/session to pick up new Forge code |
| `rust-lsp` enrichment | Needs `mcp-language-server` + `rust-analyzer` on the MCP's `PATH` | Install them (see `README.md`), or accept the note that symbol enrichment was skipped |
| `scenario_start` offscreen backend | Does not render the canvas dot background (kittest layout) | Region/pixel assertions are only meaningful where the region is actually rendered; verify canvas-background changes natively |
| `change_impact` with explicit `paths` | An internal `*_cmds.rs` edit correctly yields no registry touch | Pass `command`/`symbol` to ask about a command without a diff; `base` shapes the diff |
| MCP client / agent wrapper | Ignores `notifications/tools/list_changed` (or withholds the `list_changed` capability) | The server swaps its tool set and emits the notification, but the model's schema is only rebuilt on the next `tools/list` — i.e. a session restart. The server side is already swapped; the agent-wrapper/terminal layer is the limit. |

## Failure / gap registry

One row per distinct item. `status`: `open` or `resolved`. Newest first.

| Date | Area | Symptom / gap | Diagnosis / fix | Status |
|---|---|---|---|---|
| 2026-10-10 | `validation_start` / `test-corpus` | A cold release build ended `timedout` (hang suspected) while still compiling | The idle timeout (default 900s) counts silence, but a first release build of a heavy crate can be quiet for far longer than 15 min — not a hang. Re-run with a larger `idle_timeout_secs`. | open — raise the default for release builds, or treat a live child process as progress |
| 2026-10-10 | jCodeMunch companion | The index held no Rust symbols repo-wide | `~/.code-index/config.jsonc` `languages` omitted `rust`, so every `.rs` file was indexed for text search only (no error). Added `rust`; full reindex → 20.6k Rust symbols. | resolved |
| 2026-10-10 | touch registry (`compose`) | `change_impact` on `crates/compose/src/adjust.rs` returned an empty touch set and missed `crates/gpu/src/compose.wgsl` + `crates/gpu/tests/parity.rs` | No signal for "adjustment maths changed → the GPU shader/plan that mirrors it". | open — add an adjustment→GPU-shader/parity coupling (or derive it from a local graph) |
| 2026-10-10 | `change_impact intent` | A Hue/Saturation intent matched no known area → empty touch set | The intent matcher has no adjustment vocabulary. | open — add adjustment terms (`hue`/`saturation`/`colorize`/`levels`/`curves`…) |
| 2026-10-10 | `change_impact enrich` | Path enrichment returned nothing | `find_importers` was called without `repo`, its `isError` counted as "used", and the echoed input read as "related". Fixed: pass `repo`, treat `isError` as failure, exclude inputs, add the local module-ref fallback. | resolved |
| 2026-10-10 | touch registry | A rule named a file that no longer existed | `crates/codecs/src/via_image.rs` had moved under `codecs/`. Fixed; `registry_paths_exist` now guards every path. | resolved |
| 2026-10-10 | touch registry | Rules are hand-maintained, so they can drift | No derivation from the repo or from real changes | open — derive couplings from a local code graph / real change history |
| 2026-10-10 | `scenario_start` offscreen | Toggling a canvas background changed no pixels | The snapshot harness does not draw the canvas dots; a whole-image hash passes vacuously | open — make the render faithful or fail loudly; region assertions expose it |
| 2026-10-10 | maintenance loop | The event log was a machine record with no queue to work from | `forge_capabilities` now lists this file's open rows every session | resolved |

## Review procedure

1. Read this file and the `README.md` tool table.
2. `cargo test -p photocraft-forge` and `cargo clippy -p photocraft-forge --no-deps --all-targets -- -D warnings`.
3. Fix or update the open rows; update their status; shorten/retire the resolved ones.
4. If Forge's rules changed a companion's assumptions, refresh that companion's index.
