//! The `forge` MCP server (rmcp, stdio).
//!
//! Tools are added across milestones. M1 exposes `forge_capabilities`, which proves the transport
//! end-to-end and reports the environment (required tools, optional companions, install commands).

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::transport::io::stdio;
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};

use crate::{checks, context, impact, probe};

/// What an agent should know before using Forge.
const INSTRUCTIONS: &str = "Forge is the PhotoCraft development MCP: an AI-first control plane for \
building PhotoCraft. It reports the must-change-together touch set, the finish gates, and verified \
evidence for a change. It never edits source — you (the agent) edit with your own tools, then ask \
Forge what to run and whether the result is right. Call `forge_capabilities` first to see what is \
available in this environment.";

/// The Forge MCP server.
#[derive(Clone)]
pub struct Forge {
    #[expect(dead_code, reason = "the #[tool_handler] macro reads this router field")]
    tool_router: ToolRouter<Self>,
}

impl Forge {
    pub fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }
}

impl Default for Forge {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl Forge {
    /// Report the environment and how to fill any gaps.
    #[tool(
        description = "Report Forge's environment: which required tools (git, cargo) and optional companion MCPs (rust-lsp, jCodeMunch) are present, the PhotoCraft checkout root, and an install command for anything missing. Call this first."
    )]
    async fn forge_capabilities(&self) -> Result<CallToolResult, McpError> {
        let caps = probe::capabilities();
        let text = serde_json::to_string_pretty(&caps).map_err(|e| McpError::internal_error(format!("serialize capabilities: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// The orientation snapshot.
    #[tool(
        description = "Snapshot for orienting on the repository: the source fingerprint (HEAD, branch, dirty state, worktree/staged diff hashes, Cargo.lock hash, commits behind origin/main), the workspace crate map with workspace deps, and the authoritative `cargo xtask layers` report."
    )]
    async fn development_context(&self) -> Result<CallToolResult, McpError> {
        let root = probe::discover_repo_root().ok_or_else(|| {
            McpError::internal_error("no PhotoCraft checkout found — run forge from inside the repo (a directory containing crates/, apps/ and xtask/)", None)
        })?;
        let ctx = tokio::task::spawn_blocking(move || context::build(&root))
            .await
            .map_err(|e| McpError::internal_error(format!("context task: {e}"), None))?
            .map_err(|e| McpError::internal_error(e, None))?;
        let text = serde_json::to_string_pretty(&ctx).map_err(|e| McpError::internal_error(format!("serialize context: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// The must-change-together touch set.
    #[tool(
        description = "Given changed `paths`, an engine `command` id, a `symbol`, or an `intent`, return the must-change-together touch set, the invariants that apply, the artifacts to regenerate, and the authoritative gate plan. Prefer explicit `paths` (most accurate); `command`/`symbol`/`intent` are best-effort lookups."
    )]
    async fn change_impact(&self, Parameters(query): Parameters<impact::Query>) -> Result<CallToolResult, McpError> {
        let root = probe::discover_repo_root().ok_or_else(|| {
            McpError::internal_error("no PhotoCraft checkout found — run forge from inside the repo (a directory containing crates/, apps/ and xtask/)", None)
        })?;
        let result = tokio::task::spawn_blocking(move || impact::run(&root, query))
            .await
            .map_err(|e| McpError::internal_error(format!("impact task: {e}"), None))?
            .map_err(|e| McpError::internal_error(e, None))?;
        let text = serde_json::to_string_pretty(&result).map_err(|e| McpError::internal_error(format!("serialize impact: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// The finish gates for a working diff.
    #[tool(
        description = "Which finish gates a working diff requires, and why. Pass `paths` (most accurate) or omit them to diff against `base` (default: the merge-base with origin/main). Returns the required/recommended gates, the changed paths, and a source fingerprint."
    )]
    async fn checks_for_changes(&self, Parameters(query): Parameters<checks::Query>) -> Result<CallToolResult, McpError> {
        let root = probe::discover_repo_root().ok_or_else(|| {
            McpError::internal_error("no PhotoCraft checkout found — run forge from inside the repo (a directory containing crates/, apps/ and xtask/)", None)
        })?;
        let result = tokio::task::spawn_blocking(move || checks::run(&root, query))
            .await
            .map_err(|e| McpError::internal_error(format!("checks task: {e}"), None))?
            .map_err(|e| McpError::internal_error(e, None))?;
        let text = serde_json::to_string_pretty(&result).map_err(|e| McpError::internal_error(format!("serialize checks: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for Forge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("forge", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Serve Forge over stdio until the client disconnects.
pub async fn serve_stdio() -> Result<(), String> {
    let running = Forge::new().serve(stdio()).await.map_err(|e| format!("MCP init: {e}"))?;
    running.waiting().await.map_err(|e| e.to_string())?;
    Ok(())
}
