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

use crate::{archive, checks, context, events, impact, jobs, ledger, maintenance, probe, scenario, toolset};

/// What an agent should know before using Forge.
const INSTRUCTIONS: &str = "Forge is the PhotoCraft development MCP: an AI-first control plane for \
building PhotoCraft. It reports the must-change-together touch set, the finish gates, and verified \
evidence for a change. It never edits source — you (the agent) edit with your own tools, then ask \
Forge what to run and whether the result is right. Call `forge_capabilities` first to see what is \
available in this environment. If a Forge tool or companion fails or looks stale, tell the human and \
record it in apps/photocraft-forge/MAINTENANCE.md — don't route around it quietly. Before building a \
new tool, check the retired-tool catalog (`forge_capabilities` → `retired`) and resurrect a match \
instead; retire an unused tool to archive/ (the `ledger` shows usage).";

/// The Forge MCP server.
#[derive(Clone)]
pub struct Forge {
    tool_router: ToolRouter<Self>,
    jobs: Arc<jobs::JobManager>,
    root: Option<std::path::PathBuf>,
    toolset: Arc<toolset::Toolset>,
}

impl Forge {
    pub fn new() -> Self {
        let root = probe::discover_repo_root();
        let evidence = root.as_ref().map(|root| root.join("target/forge/jobs"));
        let toolset = Arc::new(toolset::Toolset::new(root.clone()));
        Self { tool_router: Self::tool_router(), jobs: Arc::new(jobs::JobManager::new(evidence)), root, toolset }
    }

    /// The PhotoCraft checkout root, when Forge found one.
    pub(crate) fn root(&self) -> Option<&std::path::Path> {
        self.root.as_deref()
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
        let mut value = serde_json::to_value(&caps).map_err(|e| McpError::internal_error(format!("serialize capabilities: {e}"), None))?;
        // Surface Forge's own open maintenance rows, so the maintainer sees them every session.
        if let Some(obj) = value.as_object_mut() {
            obj.insert("maintenance".to_string(), serde_json::json!({ "file": "apps/photocraft-forge/MAINTENANCE.md", "open": maintenance::open_items() }));
            // The retired-tool catalog: tools the MCP no longer serves, to resurrect instead of rebuild.
            obj.insert(
                "retired".to_string(),
                serde_json::json!({
                    "file": "apps/photocraft-forge/archive/RETIRED.md",
                    "tools": archive::retired(),
                    "note": "the MCP advertises only live tools; if you need a capability none provides, check these retired tools first and resurrect a match before building a new one",
                }),
            );
            // The dormant (compiled but unregistered) tools, and which are currently live.
            let available: Vec<String> = Self::dormant_router().list_all().into_iter().map(|t| t.name.to_string()).collect();
            obj.insert(
                "dormant".to_string(),
                serde_json::json!({
                    "enabled": self.toolset.enabled(),
                    "available": available,
                    "note": "enable with forge_toolset; dormant tools are compiled but not advertised until then",
                }),
            );
        }
        let text = serde_json::to_string_pretty(&value).map_err(|e| McpError::internal_error(format!("serialize capabilities: {e}"), None))?;
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
        let log_root = root.clone();
        let result = tokio::task::spawn_blocking(move || impact::run(&root, query))
            .await
            .map_err(|e| McpError::internal_error(format!("impact task: {e}"), None))?
            .map_err(|e| McpError::internal_error(e, None))?;
        events::append(
            &log_root,
            "change_impact",
            serde_json::json!({
                "inputs": result.get("inputs"),
                "resolved": result.get("resolved_paths"),
                "touch": result.pointer("/touch").and_then(serde_json::Value::as_array).map(|a| a.iter().map(|t| serde_json::json!({"path": t.get("path"), "confidence": t.get("confidence")})).collect::<Vec<_>>()),
                "required": result.pointer("/gates/required").and_then(serde_json::Value::as_array).map(|a| a.iter().filter_map(|g| g.get("id").and_then(serde_json::Value::as_str)).collect::<Vec<_>>()),
            }),
        );
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
        let log_root = root.clone();
        let result = tokio::task::spawn_blocking(move || checks::run(&root, query))
            .await
            .map_err(|e| McpError::internal_error(format!("checks task: {e}"), None))?
            .map_err(|e| McpError::internal_error(e, None))?;
        events::append(
            &log_root,
            "checks_for_changes",
            serde_json::json!({
                "base": result.base,
                "changed": result.changed,
                "required": result.required.iter().filter_map(|g| g.get("id").and_then(serde_json::Value::as_str)).collect::<Vec<_>>(),
                "head": result.fingerprint.head,
            }),
        );
        let text = serde_json::to_string_pretty(&result).map_err(|e| McpError::internal_error(format!("serialize checks: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Start the finish gates for a diff as serialized jobs.
    #[tool(
        description = "Resolve the finish gates for a change (explicit `paths`, or the diff against `base`) and run them as serialized background jobs. Returns the job ids; poll with `job_status` and read `job_logs`. A `cargo test` that matched zero tests is reported as `notests`, not success."
    )]
    async fn validation_start(&self, Parameters(params): Parameters<jobs::ValidationParams>) -> Result<CallToolResult, McpError> {
        let root = probe::discover_repo_root().ok_or_else(checkout_error)?;
        let log_root = root.clone();
        let jobs::ValidationParams { paths, base, include_recommended, idle_timeout_secs, timeout_secs } = params;
        let timeout = jobs::Timeout::from_secs(idle_timeout_secs, timeout_secs);
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
        let run = self.jobs.begin_run();
        let mut started = Vec::new();
        let mut skipped = Vec::new();
        let mut unavailable = Vec::new();
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
                    let spec = jobs::JobSpec {
                        name: id.clone(),
                        kind: "validation".to_string(),
                        reason,
                        tier,
                        run,
                        program: program.to_string(),
                        args,
                        cwd: root.clone(),
                        timeout,
                    };
                    let job = self.jobs.start(spec);
                    started.push(serde_json::json!({"job": job, "gate": id, "command": command}));
                }
                _ => {
                    unavailable.push(serde_json::json!({"gate": id.clone(), "reason": "not runnable locally (remote-only gate)"}));
                    skipped.push(id);
                }
            }
        }
        let out = serde_json::json!({
            "run": run,
            "jobs": started,
            "skipped": skipped,
            "unavailable": unavailable,
            "note": "a zero-test match is reported as `notests`, not a pass; `unavailable` gates need another environment",
        });
        events::append(&log_root, "validation_start", out.clone());
        let text = serde_json::to_string_pretty(&out).map_err(|e| McpError::internal_error(format!("serialize jobs: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// List jobs, or one job's status.
    #[tool(description = "List jobs (running first, then most recent finished), or a single job when `job` is given.")]
    async fn job_status(&self, Parameters(query): Parameters<JobQuery>) -> Result<CallToolResult, McpError> {
        let views = self.jobs.views(query.job);
        let out = if query.job.is_none() {
            serde_json::json!({ "jobs": views, "summary": self.jobs.summary(query.run) })
        } else {
            serde_json::json!({ "jobs": views })
        };
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

    /// Read Forge's own event log.
    #[tool(
        description = "Read Forge's append-only event log (`target/forge/events.jsonl`): recent `change_impact` / `checks_for_changes` calls (inputs, touch, gates), newest first, optionally filtered by `kind`. A local, rebuildable record of what was planned, so a later session sees it without re-running."
    )]
    async fn forge_events(&self, Parameters(query): Parameters<EventsQuery>) -> Result<CallToolResult, McpError> {
        let root = probe::discover_repo_root().ok_or_else(checkout_error)?;
        let list = events::read(&root, query.kind.as_deref(), query.since, query.limit.unwrap_or(50));
        let out = serde_json::json!({ "events": list });
        let text = serde_json::to_string_pretty(&out).map_err(|e| McpError::internal_error(format!("serialize events: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// List or change the live tool set (the just-in-time surface).
    #[tool(
        description = "List or change Forge's live tool set. `enable`/`disable` the dormant (compiled but unregistered) tools; enabling one makes it live immediately and notifies the client (`tools/list_changed`), so a retired tool is resurrected without a rebuild. Omit both to just list."
    )]
    async fn forge_toolset(&self, Parameters(query): Parameters<ToolsetQuery>) -> Result<CallToolResult, McpError> {
        let available: Vec<String> = Self::dormant_router().list_all().into_iter().map(|t| t.name.to_string()).collect();
        let mut changed: Vec<String> = Vec::new();
        for name in query.enable.unwrap_or_default() {
            if !available.contains(&name) {
                return Err(McpError::invalid_params(format!("`{name}` is not a dormant tool"), None));
            }
            if self.toolset.set(&name, true) {
                changed.push(name);
            }
        }
        for name in query.disable.unwrap_or_default() {
            if self.toolset.set(&name, false) {
                changed.push(name);
            }
        }
        let out = serde_json::json!({ "enabled": self.toolset.enabled(), "available": available, "changed": changed });
        let text = serde_json::to_string_pretty(&out).map_err(|e| McpError::internal_error(format!("serialize toolset: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Exercise a change headlessly.
    #[tool(
        description = "Exercise a change and assert the result. `backend:\"headless\"` (default) runs `steps` as app-MCP tool calls against `photocraft-cli mcp`; `backend:\"offscreen\"` runs them as control-protocol calls and snapshots the full UI to `ui.png`; `backend:\"cli\"` runs `steps` as engine commands via `photocraft-cli run` with `input`/`new`, `out` and `files`, for commands the app MCP blocks (those that use filesystem paths). Evaluates `assertions` over the last structured step result, files, or rendered images (a `file` image can target a `region` and assert its `color`/`uniform`), and returns the evidence tier, artifacts (with hashes) and whether every assertion passed."
    )]
    async fn scenario_start(&self, Parameters(params): Parameters<scenario::Params>) -> Result<CallToolResult, McpError> {
        let root = probe::discover_repo_root().ok_or_else(checkout_error)?;
        let log_root = root.clone();
        let report = tokio::task::spawn_blocking(move || scenario::run(&root, params))
            .await
            .map_err(|e| McpError::internal_error(format!("scenario task: {e}"), None))?
            .map_err(|e| McpError::internal_error(e, None))?;
        events::append(
            &log_root,
            "scenario_start",
            serde_json::json!({
                "passed": report.passed,
                "evidence_tier": report.evidence_tier,
                "failed": report.assertions.iter().filter(|a| !a.passed).count(),
            }),
        );
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
    /// Scope the summary to one `validation_start` batch (default: the latest run).
    #[serde(default)]
    run: Option<u64>,
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

/// An events query.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EventsQuery {
    /// Only events of this kind (e.g. `change_impact`, `checks_for_changes`).
    #[serde(default)]
    kind: Option<String>,
    /// Only events at or after this timestamp (ms since the epoch).
    #[serde(default)]
    since: Option<u64>,
    /// Maximum events to return, newest first (default 50, max 500).
    #[serde(default)]
    limit: Option<usize>,
}

/// A toolset query (`enable`/`disable` dormant tools; omit both to list).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ToolsetQuery {
    /// Dormant tool names to enable (resurrect).
    #[serde(default)]
    enable: Option<Vec<String>>,
    /// Dormant tool names to disable (retire).
    #[serde(default)]
    disable: Option<Vec<String>>,
}

fn checkout_error() -> McpError {
    McpError::internal_error("no PhotoCraft checkout found — run forge from inside the repo (a directory containing crates/, apps/ and xtask/)", None)
}

#[tool_handler]
impl ServerHandler for Forge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_tool_list_changed().build())
            .with_server_info(Implementation::new("photocraft-forge", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    /// The live tool set: the compiled core plus any dormant tools that are enabled.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = self.tool_router.list_all();
        let enabled = self.toolset.enabled();
        for tool in Self::dormant_router().list_all() {
            if enabled.contains(tool.name.as_ref()) {
                tools.push(tool);
            }
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    /// Record the call, dispatch to the core or a dormant router, and notify on a tool-set change.
    async fn call_tool(&self, request: CallToolRequestParams, context: rmcp::service::RequestContext<rmcp::RoleServer>) -> Result<CallToolResponse, McpError> {
        let tool = request.name.to_string();
        let peer = context.peer.clone();
        let generation = self.toolset.generation();
        let dormant = self.toolset.is_enabled(&tool) && Self::dormant_router().list_all().iter().any(|t| t.name.as_ref() == tool.as_str());
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        let result = if dormant { Self::dormant_router().call(tcc).await } else { self.tool_router.call(tcc).await };
        ledger::record(self.root.as_deref(), &tool, result.is_err());
        if self.toolset.generation() != generation {
            let _ = peer.notify_tool_list_changed().await;
        }
        result
    }
}

/// Serve Forge over stdio until the client disconnects.
pub async fn serve_stdio() -> Result<(), String> {
    let running = Forge::new().serve(stdio()).await.map_err(|e| format!("MCP init: {e}"))?;
    running.waiting().await.map_err(|e| e.to_string())?;
    Ok(())
}
