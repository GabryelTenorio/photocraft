//! `scenario_start`: exercise a change against the **headless app MCP** (`photocraft-cli mcp`) and
//! assert the result.
//!
//! Forge is an MCP client here: it spawns the headless server, runs bounded `tools/call` steps, then
//! evaluates assertions over the last step's JSON, on warnings, or on files in the scenario work
//! root. It never edits source (D7); it drives the same commands the UI does.

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
    /// Setup + exercise steps, each an app-MCP tool call.
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Assertions evaluated after the steps. The `json` assertions read the **last** step's result.
    #[serde(default)]
    pub assertions: Vec<Assertion>,
}

/// One app-MCP tool call.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// App-MCP tool name, e.g. `doc_new`, `command_run`, `doc_inspect`.
    pub tool: String,
    /// Tool arguments.
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

/// Run a scenario against the headless app MCP.
pub fn run(root: &Path, params: Params) -> Result<Report, String> {
    let work = root.join("target/forge-scenarios/work");
    std::fs::create_dir_all(&work).map_err(|e| format!("create {}: {e}", work.display()))?;
    let mut app = AppMcp::spawn(root, &work)?;
    let run = run_inner(&mut app, &work, &params);
    app.kill();
    run
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
    fn exists_assertion() {
        let subject = json!({"a": 1});
        assert!(evaluate(&Assertion { json: Some("a".into()), exists: Some(true), ..Default::default() }, &subject, &[], Path::new(".")).passed);
        assert!(evaluate(&Assertion { json: Some("b".into()), exists: Some(false), ..Default::default() }, &subject, &[], Path::new(".")).passed);
    }
}
