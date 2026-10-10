//! The `forge` MCP server (rmcp, stdio).
//!
//! Tools are added across milestones. M1 exposes `forge_capabilities`, which proves the transport
//! end-to-end and reports the environment (required tools, optional companions, install commands).

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::transport::io::stdio;
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

use crate::{checks, context, impact, jobs, probe, scenario};

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
    jobs: Arc<jobs::JobManager>,
}

impl Forge {
    pub fn new() -> Self {
        let evidence = probe::discover_repo_root().map(|root| root.join("target/forge/jobs"));
        Self { tool_router: Self::tool_router(), jobs: Arc::new(jobs::JobManager::new(evidence)) }
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

    /// Start the finish gates for a diff as serialized jobs.
    #[tool(
        description = "Resolve the finish gates for a change (explicit `paths`, or the diff against `base`) and run them as serialized background jobs. Returns the job ids; poll with `job_status` and read `job_logs`. A `cargo test` that matched zero tests is reported as `notests`, not success."
    )]
    async fn validation_start(&self, Parameters(params): Parameters<jobs::ValidationParams>) -> Result<CallToolResult, McpError> {
        let root = probe::discover_repo_root().ok_or_else(checkout_error)?;
        let jobs::ValidationParams { paths, base, include_recommended } = params;
        let query = checks::Query { paths, base };
        let probe_root = root.clone();
        let report = tokio::task::spawn_blocking(move || checks::run(&probe_root, query))
            .await
            .map_err(|e| McpError::internal_error(format!("validation task: {e}"), None))?
            .map_err(|e| McpError::internal_error(e, None))?;

        let mut gates = report.required;
        if include_recommended {
            gates.extend(report.recommended);
        }
        let mut started = Vec::new();
        let mut skipped = Vec::new();
        for gate in gates {
            let id = gate.get("id").and_then(serde_json::Value::as_str).unwrap_or("gate").to_string();
            let command = gate.get("command").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
            let reason = gate.get("reason").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
            let mut parts = command.split_whitespace();
            match parts.next() {
                // Remote-only gates like "(platform CI jobs)" are not runnable locally.
                Some(program) if !program.starts_with('(') => {
                    let args: Vec<String> = parts.map(str::to_string).collect();
                    let tier = jobs::tier_for_gate(&id);
                    let spec =
                        jobs::JobSpec { name: id.clone(), kind: "validation".to_string(), reason, tier, program: program.to_string(), args, cwd: root.clone() };
                    let job = self.jobs.start(spec);
                    started.push(serde_json::json!({"job": job, "gate": id, "command": command}));
                }
                _ => skipped.push(id),
            }
        }
        let out = serde_json::json!({"jobs": started, "skipped": skipped, "note": "a zero-test match is reported as `notests`, not a pass"});
        let text = serde_json::to_string_pretty(&out).map_err(|e| McpError::internal_error(format!("serialize jobs: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// List jobs, or one job's status.
    #[tool(description = "List jobs (running first, then most recent finished), or a single job when `job` is given.")]
    async fn job_status(&self, Parameters(query): Parameters<JobQuery>) -> Result<CallToolResult, McpError> {
        let out = serde_json::json!({ "jobs": self.jobs.views(query.job) });
        let text = serde_json::to_string_pretty(&out).map_err(|e| McpError::internal_error(format!("serialize status: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Read a job's captured output from a cursor.
    #[tool(
        description = "Read a job's captured output starting at `cursor` (byte offset returned by the previous call). Returns the slice and the next cursor."
    )]
    async fn job_logs(&self, Parameters(query): Parameters<LogsQuery>) -> Result<CallToolResult, McpError> {
        let (status, log, next_cursor) =
            self.jobs.logs(query.job, query.cursor.unwrap_or(0)).ok_or_else(|| McpError::invalid_params(format!("unknown job {}", query.job), None))?;
        let out = serde_json::json!({ "job": query.job, "status": status, "log": log, "next_cursor": next_cursor });
        let text = serde_json::to_string_pretty(&out).map_err(|e| McpError::internal_error(format!("serialize logs: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Cancel a job and its process tree.
    #[tool(description = "Cancel a job (and its process tree). A running job is signalled; a queued job is dropped before it starts.")]
    async fn job_cancel(&self, Parameters(query): Parameters<CancelQuery>) -> Result<CallToolResult, McpError> {
        let requested = self.jobs.cancel(query.job);
        let out = serde_json::json!({ "job": query.job, "requested": requested });
        let text = serde_json::to_string_pretty(&out).map_err(|e| McpError::internal_error(format!("serialize cancel: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Exercise a change headlessly.
    #[tool(
        description = "Exercise a change against the headless app MCP (`photocraft-cli mcp`): run bounded `steps` (app-MCP tool calls) then evaluate `assertions` over the last step's JSON or files in the scenario work root. Returns the evidence and whether every assertion passed."
    )]
    async fn scenario_start(&self, Parameters(params): Parameters<scenario::Params>) -> Result<CallToolResult, McpError> {
        let root = probe::discover_repo_root().ok_or_else(checkout_error)?;
        let report = tokio::task::spawn_blocking(move || scenario::run(&root, params))
            .await
            .map_err(|e| McpError::internal_error(format!("scenario task: {e}"), None))?
            .map_err(|e| McpError::internal_error(e, None))?;
        let text = serde_json::to_string_pretty(&report).map_err(|e| McpError::internal_error(format!("serialize report: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

/// A status query (omit `job` to list all).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct JobQuery {
    /// Job id (omit to list all jobs).
    #[serde(default)]
    job: Option<u64>,
}

/// A logs query.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogsQuery {
    /// Job id.
    job: u64,
    /// Byte offset to read from (default 0).
    #[serde(default)]
    cursor: Option<usize>,
}

/// A cancel query.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CancelQuery {
    /// Job id.
    job: u64,
}

fn checkout_error() -> McpError {
    McpError::internal_error("no PhotoCraft checkout found — run forge from inside the repo (a directory containing crates/, apps/ and xtask/)", None)
}

#[tool_handler]
impl ServerHandler for Forge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("photocraft-forge", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Serve Forge over stdio until the client disconnects.
pub async fn serve_stdio() -> Result<(), String> {
    let running = Forge::new().serve(stdio()).await.map_err(|e| format!("MCP init: {e}"))?;
    running.waiting().await.map_err(|e| e.to_string())?;
    Ok(())
}
