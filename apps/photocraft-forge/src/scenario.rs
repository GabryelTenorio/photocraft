//! `scenario_start`: exercise a change headlessly and assert the result.
//!
//! Three backends: `headless` (the app MCP, `photocraft-cli mcp`), `offscreen` (the full UI rendered
//! to a PNG), and `cli` (`photocraft-cli run`, for engine commands that touch files — the app MCP
//! blocks ambient-path commands). Forge never edits source (D7); it drives the same commands the UI
//! and CLI do, then evaluates assertions over the last step's JSON or files in the work root.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use base64::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::artifact::{self, Artifact};

/// A scenario to run.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Params {
    /// Which backend exercises the change (research §5). Default `headless`.
    #[serde(default)]
    pub backend: Backend,
    /// Headless: setup + exercise steps, each an app-MCP tool call. Offscreen: control-protocol
    /// calls (`[method, params]`) run hands-off, then the UI is saved to `ui.png`.
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Assertions evaluated after the steps. For `headless`, `json` reads the **last** step's result.
    #[serde(default)]
    pub assertions: Vec<Assertion>,
    /// Offscreen: window size as `WxH` in points (default `1440x900`).
    #[serde(default)]
    pub size: Option<String>,
    /// Offscreen: pixels per point; the PNG is `size × scale` (default 1, so it matches `size`).
    #[serde(default)]
    pub scale: Option<f32>,
    /// Offscreen: a file to open before the script (`--open`).
    #[serde(default)]
    pub open: Option<String>,
    /// CLI: a file (relative to the work root) to open before the commands.
    #[serde(default)]
    pub input: Option<String>,
    /// CLI: a `file.new` document spec, instead of an `input` file.
    #[serde(default)]
    pub new: Option<Value>,
    /// CLI: where to save the result (relative to the work root).
    #[serde(default)]
    pub out: Option<String>,
    /// CLI: setup files written into the work root before running (name → text content).
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

/// Which runtime a scenario exercises.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// The headless app MCP (`photocraft-cli mcp`).
    #[default]
    Headless,
    /// The offscreen UI (`photocraft-ui-egui --example snapshot`) — renders the full UI to a PNG
    /// with no window and no focus stealing.
    Offscreen,
    /// The headless CLI (`photocraft-cli run`) — engine commands with filesystem access scoped to
    /// the scenario work root. Use this when the app MCP blocks a command that takes a path.
    Cli,
}

/// One step.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Headless: an app-MCP tool (`doc_new`, `command_run`, …). Offscreen: a control-protocol method.
    /// CLI: an engine command id (its `arguments` are that command's params).
    pub tool: String,
    /// Tool arguments / command params.
    #[serde(default)]
    pub arguments: Value,
}

/// One assertion.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    /// Dot-path into the last **structured** (object/array) step result, e.g. `width` or `layers.0.name`.
    #[serde(default)]
    pub json: Option<String>,
    /// The path must equal this value.
    #[serde(default)]
    pub equals: Option<Value>,
    /// The path's string value must contain this substring.
    #[serde(default)]
    pub contains: Option<String>,
    /// A path (with `json`) or file (with `file`) must exist (`true`, default) or not (`false`).
    #[serde(default)]
    pub exists: Option<bool>,
    /// A file, relative to the scenario work root.
    #[serde(default)]
    pub file: Option<String>,
    /// A step index whose rendered image should be checked.
    #[serde(default)]
    pub image: Option<usize>,
    /// The image width in pixels (for an `image`, or a `file` that is a PNG).
    #[serde(default)]
    pub width: Option<u32>,
    /// The image height in pixels (for an `image`, or a `file` that is a PNG).
    #[serde(default)]
    pub height: Option<u32>,
    /// The payload must be at least this many bytes (for an `image` or a `file`).
    #[serde(default)]
    pub min_bytes: Option<usize>,
    /// The content hash must equal this `blake3:<hex>` digest (for an `image` or a `file`).
    #[serde(default)]
    pub hash: Option<String>,
}

/// A scenario run: whether every assertion passed, plus the evidence.
#[derive(Debug, Serialize)]
pub struct Report {
    pub passed: bool,
    /// The evidence tier this scenario reached (research §4).
    pub evidence_tier: &'static str,
    /// Outputs produced by the scenario (rendered images, asserted files) with their media types
    /// and content hashes (research §8).
    pub artifacts: Vec<Artifact>,
    pub steps: Vec<StepResult>,
    pub assertions: Vec<AssertionResult>,
}

/// The outcome of one step.
#[derive(Debug, Serialize)]
pub struct StepResult {
    pub tool: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageInfo>,
}

/// Metadata about an image content block returned by a step.
#[derive(Debug, Serialize)]
pub struct ImageInfo {
    pub media_type: String,
    pub bytes: usize,
    /// `blake3:<hex>` content hash of the decoded image bytes.
    pub hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// The outcome of one assertion.
#[derive(Debug, Serialize)]
pub struct AssertionResult {
    pub description: String,
    pub passed: bool,
    pub detail: String,
}

/// Run a scenario against the chosen backend.
pub fn run(root: &Path, params: Params) -> Result<Report, String> {
    let work = root.join("target/forge-scenarios/work");
    std::fs::create_dir_all(&work).map_err(|e| format!("create {}: {e}", work.display()))?;
    match params.backend {
        Backend::Headless => run_headless(root, &work, &params),
        Backend::Offscreen => run_offscreen(root, &work, &params),
        Backend::Cli => run_cli(&work, &params),
    }
}

/// Run against the headless app MCP.
fn run_headless(root: &Path, work: &Path, params: &Params) -> Result<Report, String> {
    let mut app = AppMcp::spawn(root, work)?;
    let run = run_inner(&mut app, work, params);
    app.kill();
    run
}

/// Render the full UI offscreen (`--example snapshot`) after running the control-protocol script,
/// then assert on the produced `ui.png` and any other files in the work root.
fn run_offscreen(root: &Path, work: &Path, params: &Params) -> Result<Report, String> {
    let out = work.join("ui.png");
    let size = params.size.clone().unwrap_or_else(|| "1440x900".to_string());
    let scale = params.scale.unwrap_or(1.0);
    let script = script_json(&params.steps)?;
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    command.args(["run", "-q", "-p", "photocraft-ui-egui", "--example", "snapshot", "--", "--out"]).arg(&out).args([
        "--size",
        &size,
        "--scale",
        &scale.to_string(),
        "--script",
        &script,
    ]);
    if let Some(open) = &params.open {
        command.arg("--open").arg(open);
    }
    let output = command.current_dir(root).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("spawn offscreen snapshot: {e}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let ran = output.status.success();

    let mut artifacts = Vec::new();
    if let Some(art) = Artifact::from_file("ui.png".to_string(), artifact::media_type(&out), &out) {
        artifacts.push(art);
    }
    let mut assertions = Vec::new();
    for a in &params.assertions {
        assertions.push(evaluate(a, &Value::Null, &[], work));
    }
    let passed = ran && assertions.iter().all(|a| a.passed);
    let mut value = json!({ "backend": "offscreen", "size": size, "scale": scale, "exit": output.status.code() });
    if !ran && let Some(obj) = value.as_object_mut() {
        obj.insert("stderr".to_string(), Value::String(tail(&stderr, 2000)));
    }
    let steps = vec![StepResult { tool: "snapshot".to_string(), is_error: !ran, value: Some(value), image: None }];
    Ok(Report { passed, evidence_tier: "offscreen", artifacts, steps, assertions })
}

/// Run engine commands through the headless CLI (`photocraft-cli run`), which has filesystem access
/// scoped to the work root. The app MCP disables commands that use ambient paths, so this is how a
/// scenario exercises them (for example a CSV import).
fn run_cli(work: &Path, params: &Params) -> Result<Report, String> {
    if params.input.is_some() && params.new.is_some() {
        return Err("scenario `cli`: give `input` or `new`, not both".to_string());
    }
    for (name, content) in &params.files {
        if !safe_rel(name) {
            return Err(format!("scenario `cli` files: `{name}` must be relative to the work root"));
        }
        let path = work.join(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        std::fs::write(&path, content).map_err(|e| format!("write `{name}`: {e}"))?;
    }
    for (label, name) in [("input", &params.input), ("out", &params.out)] {
        if let Some(name) = name
            && !safe_rel(name)
        {
            return Err(format!("scenario `cli` {label}: `{name}` must be relative to the work root"));
        }
    }

    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    command.args(["run", "-q", "-p", "photocraft-cli", "--", "run"]);
    if let Some(input) = &params.input {
        command.arg(input);
    } else if let Some(new) = &params.new {
        command.arg("--new").arg(serde_json::to_string(new).map_err(|e| format!("encode `new`: {e}"))?);
    } else {
        return Err("scenario `cli`: give `input` or `new`".to_string());
    }
    for step in &params.steps {
        command.arg("--cmd").arg(&step.tool);
        command.arg("--params").arg(serde_json::to_string(&step.arguments).map_err(|e| format!("encode params for `{}`: {e}", step.tool))?);
    }
    if let Some(out) = &params.out {
        command.arg("--out").arg(out);
    }
    let output = command.current_dir(work).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("spawn photocraft-cli: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let ran = output.status.success();

    // `photocraft-cli run` prints one `{command, result}` JSON line per command; the last structured
    // result is the subject for `json` assertions.
    let mut subject = Value::Null;
    let mut steps = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line).unwrap_or_else(|_| Value::String(line.to_string()));
        let tool = v.get("command").and_then(Value::as_str).unwrap_or("command").to_string();
        let result = v.get("result").cloned().unwrap_or_else(|| v.clone());
        if result.is_object() || result.is_array() {
            subject = result.clone();
        }
        steps.push(StepResult { tool, is_error: false, value: Some(result), image: None });
    }
    if !ran {
        steps.push(StepResult {
            tool: "photocraft-cli".to_string(),
            is_error: true,
            value: Some(json!({ "exit": output.status.code(), "stderr": tail(&stderr, 2000) })),
            image: None,
        });
    }

    let mut artifacts = Vec::new();
    if let Some(out) = &params.out
        && let Some(art) = Artifact::from_file(out.clone(), artifact::media_type(&work.join(out)), &work.join(out))
    {
        artifacts.push(art);
    }
    collect_file_artifacts(work, &params.assertions, &mut artifacts);

    let mut assertions = Vec::new();
    for a in &params.assertions {
        assertions.push(evaluate(a, &subject, &steps, work));
    }
    let passed = ran && assertions.iter().all(|a| a.passed) && steps.iter().all(|s| !s.is_error);
    Ok(Report { passed, evidence_tier: "headless", artifacts, steps, assertions })
}

/// A scenario path that stays inside the work root: relative and with no `..`.
fn safe_rel(name: &str) -> bool {
    !name.is_empty() && !Path::new(name).is_absolute() && !Path::new(name).components().any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Build the `--script` JSON: a list of `[method, params]` control-protocol calls.
fn script_json(steps: &[Step]) -> Result<String, String> {
    let script: Vec<(&str, &Value)> = steps.iter().map(|s| (s.tool.as_str(), &s.arguments)).collect();
    serde_json::to_string(&script).map_err(|e| format!("encode offscreen script: {e}"))
}

/// The last `max` characters of `text`, for a bounded failure detail.
fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let start = text.char_indices().map(|(i, _)| i).find(|i| text.len() - i <= max).unwrap_or(text.len());
    text[start..].to_string()
}

fn run_inner(app: &mut AppMcp, work: &Path, params: &Params) -> Result<Report, String> {
    let mut steps = Vec::new();
    let mut artifacts = Vec::new();
    let mut subject = Value::Null;
    for (i, step) in params.steps.iter().enumerate() {
        let result = app.call(&step.tool, step.arguments.clone())?;
        let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
        let value = parse_content(&result);
        let image = parse_image(&result);
        if let Some(img) = &image {
            artifacts.push(Artifact { name: format!("step[{i}]"), media_type: img.media_type.clone(), bytes: img.bytes, hash: img.hash.clone() });
        }
        // The subject for `json` assertions is the last *structured* (object/array) step result.
        if let Some(v) = &value
            && (v.is_object() || v.is_array())
        {
            subject = v.clone();
        }
        steps.push(StepResult { tool: step.tool.clone(), is_error, value, image });
    }

    let mut assertions = Vec::new();
    for a in &params.assertions {
        assertions.push(evaluate(a, &subject, &steps, work));
    }
    collect_file_artifacts(work, &params.assertions, &mut artifacts);
    let passed = assertions.iter().all(|a| a.passed) && steps.iter().all(|s| !s.is_error);
    Ok(Report { passed, evidence_tier: "headless", artifacts, steps, assertions })
}

/// Add an artifact for every `file` assertion that exists in the work root.
fn collect_file_artifacts(work: &Path, assertions: &[Assertion], out: &mut Vec<Artifact>) {
    for a in assertions {
        let Some(file) = &a.file else { continue };
        let path = work.join(file);
        if let Some(art) = Artifact::from_file(file.clone(), artifact::media_type(&path), &path) {
            out.push(art);
        }
    }
}

/// Evaluate one assertion against the subject (last step's JSON) and the work root.
pub fn evaluate(a: &Assertion, subject: &Value, steps: &[StepResult], work: &Path) -> AssertionResult {
    if let Some(index) = a.image {
        return evaluate_image(a, steps.get(index), index);
    }
    if let Some(file) = &a.file {
        return evaluate_file(a, work, file);
    }
    let Some(path) = &a.json else {
        return AssertionResult { description: "assertion has neither `json` nor `file`".to_string(), passed: false, detail: String::new() };
    };
    let found = dot_get(subject, path);
    if let Some(expect) = a.exists {
        let exists = found.is_some();
        return AssertionResult {
            description: format!("json `{path}` exists={expect}"),
            passed: exists == expect,
            detail: found.map_or_else(|| "absent".to_string(), |v| v.to_string()),
        };
    }
    if let Some(expect) = &a.equals {
        let pass = found == Some(expect);
        return AssertionResult {
            description: format!("json `{path}` == {expect}"),
            passed: pass,
            detail: found.map_or_else(|| "absent".to_string(), |v| v.to_string()),
        };
    }
    if let Some(sub) = &a.contains {
        let pass = found.and_then(Value::as_str).is_some_and(|s| s.contains(sub.as_str()));
        return AssertionResult {
            description: format!("json `{path}` contains `{sub}`"),
            passed: pass,
            detail: found.map_or_else(|| "absent".to_string(), |v| v.to_string()),
        };
    }
    AssertionResult { description: format!("json `{path}` has no check (equals/contains/exists)"), passed: false, detail: String::new() }
}

/// Evaluate a `file` assertion: existence, and (when given) size, hash and PNG dimensions. An
/// existing file is also recorded as an artifact by [`collect_file_artifacts`].
fn evaluate_file(a: &Assertion, work: &Path, file: &str) -> AssertionResult {
    let path = work.join(file);
    let want_exists = a.exists.unwrap_or(true);
    let Some(artifact) = Artifact::from_file(file, artifact::media_type(&path), &path) else {
        let passed = !want_exists;
        return AssertionResult {
            description: format!("file `{file}` exists={want_exists}"),
            passed,
            detail: if passed { "absent (as required)".to_string() } else { "missing".to_string() },
        };
    };
    if a.exists == Some(false) {
        return AssertionResult { description: format!("file `{file}` exists=false"), passed: false, detail: "exists (expected absent)".to_string() };
    }
    let mut checks: Vec<(bool, String)> = vec![(true, "exists".to_string())];
    if let Some(min) = a.min_bytes {
        checks.push((artifact.bytes >= min, format!("bytes {} >= {min}", artifact.bytes)));
    }
    if let Some(expect) = &a.hash {
        checks.push((artifact.hash == *expect, format!("hash {} == {expect}", artifact.hash)));
    }
    if a.width.is_some() || a.height.is_some() {
        let bytes = std::fs::read(&path).unwrap_or_default();
        let (w, h) = png_dims(&bytes);
        if let Some(want) = a.width {
            checks.push((w == Some(want), format!("width {} == {want}", show(w))));
        }
        if let Some(want) = a.height {
            checks.push((h == Some(want), format!("height {} == {want}", show(h))));
        }
    }
    let passed = checks.iter().all(|(ok, _)| *ok);
    AssertionResult { description: format!("file `{file}`"), passed, detail: join_checks(&checks) }
}

/// Evaluate an image assertion against a step's rendered image.
fn evaluate_image(a: &Assertion, step: Option<&StepResult>, index: usize) -> AssertionResult {
    let Some(image) = step.and_then(|s| s.image.as_ref()) else {
        return AssertionResult { description: format!("image[{index}]"), passed: false, detail: "step has no image".to_string() };
    };
    let mut checks: Vec<(bool, String)> = Vec::new();
    if let Some(w) = a.width {
        checks.push((image.width == Some(w), format!("width {} == {w}", show(image.width))));
    }
    if let Some(h) = a.height {
        checks.push((image.height == Some(h), format!("height {} == {h}", show(image.height))));
    }
    if let Some(min) = a.min_bytes {
        checks.push((image.bytes >= min, format!("bytes {} >= {min}", image.bytes)));
    }
    if let Some(expect) = &a.hash {
        checks.push((image.hash == *expect, format!("hash {} == {expect}", image.hash)));
    }
    let passed = !checks.is_empty() && checks.iter().all(|(ok, _)| *ok);
    let detail = if checks.is_empty() { "no image check (width/height/min_bytes/hash)".to_string() } else { join_checks(&checks) };
    AssertionResult { description: format!("image[{index}] {}", image.media_type), passed, detail }
}

fn show(v: Option<u32>) -> String {
    v.map_or_else(|| "?".to_string(), |v| v.to_string())
}

fn join_checks(checks: &[(bool, String)]) -> String {
    checks.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>().join("; ")
}

/// Dot-path lookup: `.` separates keys; a numeric segment indexes an array.
pub fn dot_get<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = value;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cur = match cur {
            Value::Object(map) => map.get(seg)?,
            Value::Array(items) => items.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// The first text content block, parsed as JSON when possible.
fn parse_content(result: &Value) -> Option<Value> {
    let text = result.get("content")?.as_array()?.iter().find(|c| c.get("type").and_then(Value::as_str) == Some("text"))?.get("text")?.as_str()?;
    Some(serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())))
}

/// The first image content block, with its PNG dimensions when decodable.
fn parse_image(result: &Value) -> Option<ImageInfo> {
    let block = result.get("content")?.as_array()?.iter().find(|c| c.get("type").and_then(Value::as_str) == Some("image"))?;
    let media_type = block.get("mimeType").and_then(Value::as_str).unwrap_or("image/png").to_string();
    let data = block.get("data").and_then(Value::as_str)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
    let (width, height) = if media_type.contains("png") { png_dims(&bytes) } else { (None, None) };
    Some(ImageInfo { media_type, bytes: bytes.len(), hash: artifact::hash_bytes(&bytes), width, height })
}

/// PNG width/height from the IHDR header (no decoder needed).
pub fn png_dims(bytes: &[u8]) -> (Option<u32>, Option<u32>) {
    let signature = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || bytes.get(..8) != Some(&signature[..]) || bytes.get(12..16) != Some(&b"IHDR"[..]) {
        return (None, None);
    }
    let width = bytes.get(16..20).and_then(|s| s.try_into().ok()).map(u32::from_be_bytes);
    let height = bytes.get(20..24).and_then(|s| s.try_into().ok()).map(u32::from_be_bytes);
    (width, height)
}

// ---------------------------------------------------------------------------
// A minimal MCP client over the child's stdio.
// ---------------------------------------------------------------------------

struct AppMcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl AppMcp {
    fn spawn(root: &Path, work: &Path) -> Result<Self, String> {
        let work = work.display().to_string();
        let mut child = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["run", "-q", "-p", "photocraft-cli", "--", "mcp", "--automation-read-root", &work, "--automation-write-root", &work])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn photocraft-cli mcp: {e}"))?;
        let stdin = child.stdin.take().ok_or("app MCP: no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("app MCP: no stdout")?);
        let mut app = AppMcp { child, stdin, stdout, next: 1 };
        app.initialize()?;
        Ok(app)
    }

    fn initialize(&mut self) -> Result<(), String> {
        self.request(
            "initialize",
            json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"forge","version":env!("CARGO_PKG_VERSION")}}),
        )?;
        // notifications/initialized has no response.
        writeln!(self.stdin, "{}", json!({"jsonrpc":"2.0","method":"notifications/initialized"})).map_err(|e| e.to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, String> {
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        writeln!(self.stdin, "{}", json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).map_err(|e| format!("{method}: {e}"))?;
        self.stdin.flush().map_err(|e| format!("{method}: {e}"))?;
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line).map_err(|e| format!("{method}: {e}"))? == 0 {
                return Err(format!("app MCP closed while waiting for `{method}`"));
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                return Err(format!("app MCP `{method}` error: {err}"));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_paths_must_stay_inside_the_work_root() {
        assert!(safe_rel("owned.csv"));
        assert!(safe_rel("sub/a.txt"));
        assert!(!safe_rel("../escape.txt"));
        assert!(!safe_rel("/etc/passwd"));
        assert!(!safe_rel(""));
    }

    #[test]
    fn dot_get_walks_objects_and_arrays() {
        let v = json!({"width": 8, "layers": [{"name": "Background"}]});
        assert_eq!(dot_get(&v, "width"), Some(&json!(8)));
        assert_eq!(dot_get(&v, "layers.0.name"), Some(&json!("Background")));
        assert_eq!(dot_get(&v, "layers.1.name"), None);
        assert_eq!(dot_get(&v, "missing"), None);
    }

    #[test]
    fn equals_and_contains_assertions() {
        let subject = json!({"width": 8, "name": "Untitled"});
        let ok = evaluate(&Assertion { json: Some("width".into()), equals: Some(json!(8)), ..Default::default() }, &subject, &[], Path::new("."));
        assert!(ok.passed, "{ok:?}");
        let bad = evaluate(&Assertion { json: Some("width".into()), equals: Some(json!(9)), ..Default::default() }, &subject, &[], Path::new("."));
        assert!(!bad.passed);
        let contains = evaluate(&Assertion { json: Some("name".into()), contains: Some("title".into()), ..Default::default() }, &subject, &[], Path::new("."));
        assert!(contains.passed, "{contains:?}");
    }

    #[test]
    fn png_dims_parses_ihdr() {
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        png.extend_from_slice(&[0, 0, 0, 13]); // IHDR length
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0, 0, 0, 64]); // width 64
        png.extend_from_slice(&[0, 0, 0, 48]); // height 48
        assert_eq!(png_dims(&png), (Some(64), Some(48)));
        assert_eq!(png_dims(b"not a png"), (None, None));
    }

    #[test]
    fn file_hash_and_size_assertions() {
        let dir = std::env::temp_dir().join(format!("forge-scenario-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create");
        std::fs::write(dir.join("out.txt"), b"hello").expect("write");
        let hash = artifact::hash_bytes(b"hello");
        let ok =
            evaluate(&Assertion { file: Some("out.txt".into()), min_bytes: Some(5), hash: Some(hash.clone()), ..Default::default() }, &Value::Null, &[], &dir);
        assert!(ok.passed, "{ok:?}");
        let bad = evaluate(&Assertion { file: Some("out.txt".into()), hash: Some("blake3:deadbeef".into()), ..Default::default() }, &Value::Null, &[], &dir);
        assert!(!bad.passed, "{bad:?}");
        let missing = evaluate(&Assertion { file: Some("nope.txt".into()), ..Default::default() }, &Value::Null, &[], &dir);
        assert!(!missing.passed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_block_records_a_hash_and_dimensions() {
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0, 0, 0, 32]); // width 32
        png.extend_from_slice(&[0, 0, 0, 16]); // height 16
        let data = base64::engine::general_purpose::STANDARD.encode(&png);
        let result = json!({"content":[{"type":"image","mimeType":"image/png","data":data}]});
        let info = parse_image(&result).expect("image");
        assert_eq!(info.width, Some(32));
        assert_eq!(info.height, Some(16));
        assert_eq!(info.hash, artifact::hash_bytes(&png));
    }

    #[test]
    fn backend_defaults_to_headless_and_parses_offscreen() {
        assert_eq!(Params::default().backend, Backend::Headless);
        let p: Params = serde_json::from_value(json!({"backend": "offscreen", "size": "800x600"})).expect("parse");
        assert_eq!(p.backend, Backend::Offscreen);
        assert_eq!(p.size.as_deref(), Some("800x600"));
    }

    #[test]
    fn offscreen_script_is_method_params_pairs() {
        let steps = vec![
            Step { tool: "ui.set".into(), arguments: json!({"tool": "type"}) },
            Step { tool: "ui.menu.invoke".into(), arguments: json!({"id": "image.imageSize"}) },
        ];
        let s = script_json(&steps).expect("script");
        assert_eq!(s, r#"[["ui.set",{"tool":"type"}],["ui.menu.invoke",{"id":"image.imageSize"}]]"#);
    }

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("abc", 10), "abc");
    }

    #[test]
    fn exists_assertion() {
        let subject = json!({"a": 1});
        assert!(evaluate(&Assertion { json: Some("a".into()), exists: Some(true), ..Default::default() }, &subject, &[], Path::new(".")).passed);
        assert!(evaluate(&Assertion { json: Some("b".into()), exists: Some(false), ..Default::default() }, &subject, &[], Path::new(".")).passed);
    }
}
