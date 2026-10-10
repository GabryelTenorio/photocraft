//! `scenario_start`: exercise a change against the **headless app MCP** (`photocraft-cli mcp`) and
//! assert the result.
//!
//! Forge is an MCP client here: it spawns the headless server, runs bounded `tools/call` steps, then
//! evaluates assertions over the last step's JSON, on warnings, or on files in the scenario work
//! root. It never edits source (D7); it drives the same commands the UI does.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
    /// Dot-path into the last step's JSON result, e.g. `width` or `layers.0.name`.
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
}

/// A scenario run: whether every assertion passed, plus the evidence.
#[derive(Debug, Serialize)]
pub struct Report {
    pub passed: bool,
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
    let mut subject = Value::Null;
    for step in &params.steps {
        let result = app.call(&step.tool, step.arguments.clone())?;
        let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
        let value = parse_content(&result);
        if let Some(v) = &value {
            subject = v.clone();
        }
        steps.push(StepResult { tool: step.tool.clone(), is_error, value });
    }

    let mut assertions = Vec::new();
    for a in &params.assertions {
        assertions.push(evaluate(a, &subject, work));
    }
    let passed = assertions.iter().all(|a| a.passed) && steps.iter().all(|s| !s.is_error);
    Ok(Report { passed, steps, assertions })
}

/// Evaluate one assertion against the subject (last step's JSON) and the work root.
pub fn evaluate(a: &Assertion, subject: &Value, work: &Path) -> AssertionResult {
    if let Some(file) = &a.file {
        let want = a.exists.unwrap_or(true);
        let exists = work.join(file).exists();
        return AssertionResult {
            description: format!("file `{file}` exists={want}"),
            passed: exists == want,
            detail: if exists { "exists" } else { "missing" }.to_string(),
        };
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
        let ok = evaluate(&Assertion { json: Some("width".into()), equals: Some(json!(8)), ..Default::default() }, &subject, Path::new("."));
        assert!(ok.passed, "{ok:?}");
        let bad = evaluate(&Assertion { json: Some("width".into()), equals: Some(json!(9)), ..Default::default() }, &subject, Path::new("."));
        assert!(!bad.passed);
        let contains = evaluate(&Assertion { json: Some("name".into()), contains: Some("title".into()), ..Default::default() }, &subject, Path::new("."));
        assert!(contains.passed, "{contains:?}");
    }

    #[test]
    fn exists_assertion() {
        let subject = json!({"a": 1});
        assert!(evaluate(&Assertion { json: Some("a".into()), exists: Some(true), ..Default::default() }, &subject, Path::new(".")).passed);
        assert!(evaluate(&Assertion { json: Some("b".into()), exists: Some(false), ..Default::default() }, &subject, Path::new(".")).passed);
    }
}
