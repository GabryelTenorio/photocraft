# Retired Forge tools

The **catalog of tools Forge no longer serves**. The MCP advertises only its live tools; this file is
how the agent still knows a retired tool exists, so that "I need a capability that isn't there"
resolves to **check retired → resurrect** before **build new**. `forge_capabilities` returns this
table under `retired`.

Forge grows tools on demand, so some are one-shot. The pipeline (see `../README.md` → "Tool
lifecycle") is: **grow** on demand → the **ledger** shows usage → **retire** an unused tool here →
**resurrect** it when the need returns. Retiring is archiving, never deleting: the code stays in this
directory, so bringing it back is mechanical.

**To retire a tool:** make it dormant — move its handler (the `#[tool]` method + params) from
`src/server.rs` into `src/dormant.rs` (it stays compiled but unregistered), add a row below, and
remove it from the `README.md` live-tool table. No rebuild is needed to *stop serving* it; the state
is the compiled-but-unregistered router.

**To resurrect:** `forge_toolset { "enable": ["<tool>"] }` — it becomes live immediately and the
client is notified (`tools/list_changed`), with no rebuild. Restore its `README.md` row and delete the
row below. (Fully removing a tool from the build — out of `src/` entirely — is a heavier step for
later; the dormant router is the hot path.)

## Retired tools

| Tool | Why it existed | Retired | Code | Restore |
|---|---|---|---|---|
