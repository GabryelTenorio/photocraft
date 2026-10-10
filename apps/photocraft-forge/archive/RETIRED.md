# Retired Forge tools

The **catalog of tools Forge no longer serves**. The MCP advertises only its live tools; this file is
how the agent still knows a retired tool exists, so that "I need a capability that isn't there"
resolves to **check retired → resurrect** before **build new**. `forge_capabilities` returns this
table under `retired`.

Forge grows tools on demand, so some are one-shot. The pipeline (see `../README.md` → "Tool
lifecycle") is: **grow** on demand → the **ledger** shows usage → **retire** an unused tool here →
**resurrect** it when the need returns. Retiring is archiving, never deleting: the code stays in this
directory, so bringing it back is mechanical.

**To retire a tool:** move its handler (the `#[tool]` method + params) into this directory as
`<tool>.rs`, add a row below, and remove it from `src/server.rs` and the `README.md` tool table.

**To resurrect:** move the code back out of this directory into `src/`, re-add the `#[tool]` handler
(and `mod`), restore the `README.md` row, and delete the row below.

## Retired tools

| Tool | Why it existed | Retired | Code | Restore |
|---|---|---|---|---|
