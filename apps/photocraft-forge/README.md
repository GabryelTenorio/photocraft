# Forge — the PhotoCraft development MCP

**Forge** is an AI-first development control plane for PhotoCraft. It gives any MCP-capable AI the
**must-change-together touch set**, the **finish gates**, and **verified evidence** for a change —
without a human in the loop.

- **AI-first, not human-first:** machine-parseable JSON, deterministic results, exact command lines.
- **Never edits source:** Forge is the AI's *brain and verifier*, not its hands. The agent edits with
  its own tools, then asks Forge what to run and whether the result is right.
- **Environment-independent:** stdio only, model-neutral, no OpenCode coupling, no config files, no
  network. One binary, portable to any MCP client (OpenCode, Claude Code, Cursor, …).
- **Dev-only:** never shipped in PhotoCraft, never a dependency of it.

This is a workspace member (`apps/photocraft-forge`) and follows every repo rule (never-crash,
no `unsafe`), but it is **not** part of the product build.

## Status

**MVP complete.** The full tool set (decision D5) is implemented and tested. Design decisions are in
`plan/photocraft-dev-mcp-decisions.md` (D1–D9) in the authoring repo; the research is in
`plan/photocraft-dev-mcp-research.md` / `-plan.md` / `-core-spec.md`.

## Build & run

```sh
cargo build -p photocraft-forge     # or: cargo build --release -p photocraft-forge
```

Point any MCP client at the `photocraft-forge` binary. It takes **no arguments**:

```json
{ "mcpServers": { "photocraft-forge": { "command": "/path/to/photocraft-forge" } } }
```

Run it from inside the PhotoCraft checkout (it also walks up from the current directory to find the
repo root). First action for an agent: **`forge_capabilities`**.

**PATH note.** Forge finds `git`/`cargo` on `PATH` (it does not hardcode paths — see the
environment-independence principle). Some hosts spawn MCP servers with a minimal `PATH`; if `cargo`
lives in `~/.cargo/bin`, set an explicit environment, e.g. for OpenCode:

```jsonc
"photocraft-forge": {
  "type": "local",
  "command": ["/path/to/photocraft-forge"],
  "cwd": "/path/to/photocraft",
  "environment": { "PATH": "/home/you/.cargo/bin:/usr/local/bin:/usr/bin:/bin" }
}
```

## Tools

| Tool | Purpose |
|---|---|
| `forge_capabilities` | Report required tools (`git`, `cargo`), optional companions, the checkout root, and install commands for anything missing. |
| `development_context` | Source fingerprint (HEAD, branch, dirty, diff/Cargo.lock hashes, behind origin/main), Forge's own build identity (version, profile, target, hashed binary), the workspace crate map, and the authoritative `cargo xtask layers` report. |
| `change_impact` | The must-change-together touch set, invariants, artifacts to regenerate, related files, and the gate plan, for `paths` / a `command` / a `symbol` / an `intent`. |
| `checks_for_changes` | Which finish gates a working diff requires, and why. |
| `validation_start` | Run the required gates for a diff as serialized background jobs. |
| `job_status` / `job_logs` / `job_cancel` | Poll status, read cursor-based logs, cancel a job (and its process tree). |
| `scenario_start` | Exercise a change and assert the result: headless app-MCP calls (JSON paths, files, rendered images) or offscreen full-UI snapshots. |

### Contracts

`change_impact { paths?: [..], command?: "…", symbol?: "…", intent?: "…", enrich?: bool }`
→ `{ inputs, resolved_paths, touch:[{path,why,signals}], invariants:[..], regenerate:[{artifact,command,why}], related:[..], graph, gates:{required,recommended}, notes }`.
Gates come from the authoritative `cargo xtask plan --json`; `enrich:true` consults the optional host
MCPs for `related` files (see below).

`checks_for_changes { paths?: [..], base?: "…" }`
→ `{ base, changed:[..], required:[gate], recommended:[gate], fingerprint, notes }`. With no `paths`,
it diffs against `base` (default: merge-base with `origin/main`, else `HEAD`).

`validation_start { paths?: [..], base?: "…", include_recommended?: bool, idle_timeout_secs?: n, timeout_secs?: n }`
→ `{ jobs:[{job,gate,command}], skipped:[..], unavailable:[{gate,reason}], note }`. Jobs run **one at a time** (Cargo jobs sharing
`target/` and the caches must not contend). A `cargo test` that matched **zero tests** ends as
`notests`, never a pass. Each completed job persists an immutable manifest under `target/forge/jobs/`
(`<id>.json` + `<id>.log`) carrying the source fingerprint, the gate's reason, **parsed diagnostics**
(Cargo/clippy — human or `--message-format=json` — mapped to `file:line:col`),
matched/executed/filtered **test counts**, and **artifacts** (media type + `blake3` content hash) —
research §8. A job persisted while still queued or running loads as **`interrupted`** — never a silent
`failed` — and each manifest carries start, finish and duration. Every job is **time-bounded**: it is
terminated after no output for `idle_timeout_secs` (default 900) — a suspected hang — or after
`timeout_secs` overall (default 3600, a runaway). A timed-out job ends as `timedout` (distinct from
`failed`) and records which limit fired and whether a hang was suspected, so a slow-but-printing build
is not mistaken for a stuck test (research §8).

`job_status { job? }` → `{ jobs:[{id,name,kind,status,tier,blocking,unavailable?,command,zero_tests,tests,diagnostics,artifacts,head,dirty,evidence,finished_ms}], summary }`.
`summary` names what **blocks** the change (`failed`/`notests`/`timedout`), what never ran (`queued`), and where
evidence is **unavailable** (e.g. a gate needing another platform), so an agent never mistakes a
partial run for a green one.
`job_logs { job, cursor? }` → `{ job, status, log, next_cursor }`.
`job_cancel { job }` → `{ job, requested }`.

`scenario_start { backend?:"headless"|"offscreen", steps:[{tool,arguments}], assertions:[..], size?, scale?, open? }`
→ `{ passed, evidence_tier, artifacts, steps:[{tool,is_error,value,image?}], assertions:[{description,passed,detail}] }`. Every rendered image and asserted file is reported as an **artifact** with its media type and `blake3` hash.

- **`headless`** (default): spawns the headless app MCP (`photocraft-cli mcp`, roots under
  `target/forge-scenarios/work`), runs the steps (app-MCP tool calls), then checks assertions over the
  last **structured** (object/array) step result (`json` + `equals` / `contains` / `exists`), files in
  the work root (`file`), or a step's rendered **image** (`image` step index; PNG dimensions come from
  the IHDR header). `evidence_tier: "headless"`.
- **`offscreen`**: runs the steps as control-protocol calls (`photocraft-ui-egui --example snapshot`,
  no window, no focus stealing), writes `ui.png` (`size` × `scale`, default `1440x900` × 1), and
  asserts against it as a `file`. `evidence_tier: "offscreen"`.

Assertions accept `width` / `height` / `min_bytes` / `hash` on both an `image` and a `file` (a golden
PNG can be pinned by content, not just size).

## Optional companions

Forge is self-sufficient, but **better** with these host MCPs. They are used only when present, only
when relevant, and always bounded (20s) — any failure degrades to a note. Forge never bundles or
installs them.

| Companion | Role | Used by | Install |
|---|---|---|---|
| `jCodeMunch` | impact graph (importers, blast radius) | `change_impact` (when `enrich:true`) | `uv tool install jcodemunch-mcp` |
| `rust-lsp` (rust-analyzer + a bridge) | Rust semantics (references, types, macros) | `change_impact` (when `enrich:true` and a `symbol` is given) | `rustup component add rust-analyzer` (+ `mcp-language-server`) |

`forge_capabilities` reports which are present and how to install the rest.

## Architecture

| Module | Responsibility |
|---|---|
| `server.rs` | rmcp stdio server, tool definitions, params. |
| `probe.rs` | Capability probe (`on_path`, repo-root discovery, `forge_capabilities`). |
| `context.rs` | `development_context` (git fingerprint + `cargo metadata` + `xtask layers`). |
| `impact.rs` | `change_impact`: the signal registry, resolution, and output assembly. |
| `checks.rs` | `checks_for_changes`: changed-path enumeration + gate lookup. |
| `xtask.rs` | Read-only bridge to the authoritative `cargo xtask plan --json`. |
| `jobs.rs` | The serialized job manager + zero-test detection. |
| `scenario.rs` | `scenario_start`: a minimal MCP client to the headless app MCP, plus the offscreen snapshot backend. |
| `graph.rs` | Optional host-MCP graph enrichment (bounded, degrading). |
| `git.rs` | Read-only git helper. |

The **signal registry** in `impact.rs` encodes the couplings generic search cannot see: doc fields →
format/io; `Adjustment` arms → PSD `grdm` + compose + gpu + params + editor + compile guards;
`codecs::Format` variants → all enum touch points + dispatch + tests; engine commands → registry +
`menu_catalog` + parity; UI → localization. Gate policy is **never re-encoded** — it is read from
`cargo xtask plan --json`.

## Agent workflow

1. `forge_capabilities` — see what's available; install companions if you want them.
2. `development_context` — orient: fingerprint, crates, layers.
3. `change_impact` — get the precise work order; **edit the files yourself**.
4. `checks_for_changes` — the gates for your diff.
5. `validation_start` → poll `job_status` / `job_logs` — run and read the gates.
6. `scenario_start` — exercise and assert the new behaviour.

## Test

```sh
cargo test -p photocraft-forge
```

The suite is self-contained and deterministic: geometry/probe/impact/checks units, a job-manager test,
and integration tests that spawn the built binary over raw JSON-RPC stdio (including a live
`scenario_start`). No network; no host MCP required. The optional graph client is tested against a
stub stdio server.

## Adding a tool

1. Add the handler to the `#[tool_router] impl Forge` in `server.rs` (params struct with `Deserialize`
   + `JsonSchema`), keeping the output structured JSON.
2. Put logic in its own module; keep it deterministic and testable (`git`/`cargo`/`xtask` only — no
   new hard dependency).
3. Add unit tests and an integration test if the tool has an MCP surface.
4. Update the tool table here.
5. `cargo test -p photocraft-forge`, `cargo clippy -p photocraft-forge --all-targets -- -D warnings`,
   `cargo fmt`, and `cargo xtask layers` (Forge is registered `Exempt`).

## Not in scope (deferred, D5)

`runtime_*` / `application_call` (native desktop lifecycle and live desktop scenarios); pixel-*value*
assertions (dimensions and content hashes are checked, not individual pixels); the triage/evaluation
chain (`evidence_ingest`, `investigation_plan`, `feature_assess`).
