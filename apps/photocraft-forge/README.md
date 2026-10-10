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

M3. Tools:

| Tool | Purpose |
|---|---|
| `forge_capabilities` | Report the environment; recommend installs for missing companions. |
| `development_context` | Source fingerprint, workspace crate map, and the authoritative layering report. |
| `change_impact` | Given paths / a command / a symbol / an intent: the must-change-together touch set, invariants, artifacts to regenerate, and the gate plan. |
| `checks_for_changes` | Which finish gates a working diff requires, and why. |

Planned: `validation_start` + job tools, `scenario_start`.

## Run

Build with `cargo build -p photocraft-forge`, then point any MCP client at the `photocraft-forge`
binary. It takes **no arguments**. A generic client config (not tied to any one host):

```json
{
  "mcpServers": {
    "forge": { "command": "/path/to/photocraft-forge" }
  }
}
```

Run it from inside the PhotoCraft checkout so it can find the repo root (it also walks up from the
current directory).

## Capabilities and companions

Forge is self-sufficient, but it is *better* with these host MCPs, which it calls itself when present
and falls back from when absent:

| Companion | Role | Install |
|---|---|---|
| `rust-lsp` (rust-analyzer) | Rust semantics — the authority (references, types, macros) | `rustup component add rust-analyzer` (+ a stdio LSP bridge such as `mcp-language-server`) |
| `jCodeMunch` | symbol/impact graph — an accelerator (blast radius, importers) | `uv tool install jcodemunch-mcp` |

**Forge recommends installing a missing companion** rather than silently degrading: `forge_capabilities`
returns the exact install command and reason for anything absent. (The host remains responsible for
registering the companion with the AI client.)

## Test

```sh
cargo test -p photocraft-forge
```

The suite is self-contained and deterministic — `tests/mcp_stdio.rs` spawns the built binary and
speaks raw JSON-RPC over stdio; probe unit tests cover the recommendation logic without touching the
real environment.

## Design notes

Decisions are recorded in `plan/photocraft-dev-mcp-decisions.md` (D1–D9) in the authoring repo:
internal optional MCP client with fallback + install recommendation (D1), environment independence
(D4), never edits source (D7), and its own test suite (D9).
