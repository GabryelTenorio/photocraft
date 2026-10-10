//! Dormant tools: compiled but unregistered until enabled via `forge_toolset`.
//!
//! These are the "paged out" tools of Forge's just-in-time surface. They ship in the binary (so they
//! cannot rot and need no rebuild to return) but the model does not see them until enabled — which is
//! what keeps the live surface lean. The record of why each is dormant lives in
//! `archive/RETIRED.md`; the pipeline is in `README.md` → "Tool lifecycle".

use rmcp::model::*;
use rmcp::{ErrorData as McpError, tool, tool_router};

use crate::server::Forge;

#[tool_router(router = dormant_router, vis = "pub(crate)")]
impl Forge {
    /// Read the per-tool call ledger (dormant: enable with `forge_toolset`).
    #[tool(
        description = "Read Forge's per-tool call ledger (`target/forge/tool_calls.json`): calls, errors, last-used and a `dormant` flag per tool, so the maintainer can see which tools earn their place. Dormant by default — enable with `forge_toolset`."
    )]
    async fn forge_ledger(&self) -> Result<CallToolResult, McpError> {
        let out = serde_json::json!({
            "retire_after_days": crate::ledger::RETIRE_AFTER_DAYS,
            "tools": crate::ledger::rows(self.root()),
        });
        let text = serde_json::to_string_pretty(&out).map_err(|e| McpError::internal_error(format!("serialize ledger: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}
