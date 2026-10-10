//! MCP stdio handshake for `photocraft-forge`: initialize, list tools, and call tools.
//!
//! Self-contained and deterministic: spawns the built binary, speaks raw JSON-RPC over stdio, and
//! asserts the discovery contract. No network, no host MCP, no repo mutation.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// Spawn Forge, initialize, call one tool, and return its parsed text content.
fn forge_tool_call(name: &str, arguments: Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_photocraft-forge"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn photocraft-forge");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").expect("write request");
        stdin.flush().expect("flush");
    };
    let mut recv = |id: u64| -> Value {
        loop {
            let mut line = String::new();
            assert!(stdout.read_line(&mut line).expect("read response") > 0, "server closed");
            let v: Value = serde_json::from_str(&line).expect("json line");
            if v["id"] == id {
                return v;
            }
        }
    };
    send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}));
    let _ = recv(1);
    send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
    let call = recv(3);
    assert_ne!(call["result"]["isError"], true, "{call}");
    let text = call["result"]["content"][0]["text"].as_str().expect("text content").to_string();
    let _ = child.kill();
    let _ = child.wait();
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

#[test]
fn stdio_handshake_lists_and_calls_capabilities() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_photocraft-forge"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn photocraft-forge");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").expect("write request");
        stdin.flush().expect("flush");
    };
    let mut recv = |id: u64| -> Value {
        loop {
            let mut line = String::new();
            assert!(stdout.read_line(&mut line).expect("read response") > 0, "server closed");
            let v: Value = serde_json::from_str(&line).expect("json line");
            if v["id"] == id {
                return v;
            }
        }
    };

    send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}));
    let init = recv(1);
    assert_eq!(init["result"]["serverInfo"]["name"], "photocraft-forge", "{init}");

    send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let tools = recv(2);
    let names: Vec<&str> = tools["result"]["tools"].as_array().expect("tools array").iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"forge_capabilities"), "{names:?}");
    assert!(names.contains(&"development_context"), "{names:?}");
    assert!(names.contains(&"change_impact"), "{names:?}");
    assert!(names.contains(&"checks_for_changes"), "{names:?}");
    assert!(names.contains(&"validation_start"), "{names:?}");
    assert!(names.contains(&"job_status"), "{names:?}");
    assert!(names.contains(&"job_logs"), "{names:?}");
    assert!(names.contains(&"job_cancel"), "{names:?}");
    assert!(names.contains(&"scenario_start"), "{names:?}");

    send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"forge_capabilities","arguments":{}}}));
    let call = recv(3);
    assert_ne!(call["result"]["isError"], true, "{call}");
    let text = call["result"]["content"][0]["text"].as_str().expect("text content");
    let caps: Value = serde_json::from_str(text).expect("capabilities JSON");
    assert!(caps["tools"].as_array().is_some_and(|a| !a.is_empty()), "{caps}");
    assert!(caps["tools"].as_array().is_some_and(|a| a.iter().any(|t| t["name"] == "git")), "{caps}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn development_context_reports_fingerprint_crates_and_layers() {
    let ctx = forge_tool_call("development_context", json!({}));
    assert!(ctx["repo_root"].as_str().is_some_and(|s| !s.is_empty()), "{ctx}");
    assert!(ctx["fingerprint"]["head"].as_str().is_some(), "{ctx}");
    let crates = ctx["crates"].as_array().expect("crates array");
    assert!(crates.iter().any(|c| c["name"] == "photocraft-forge"), "{ctx}");
    assert!(ctx["layers"]["ok"].as_bool() == Some(true), "layers: {ctx}");
    // Forge's own build identity (research §6/§14 Phase 0).
    assert!(ctx["forge"]["version"].as_str().is_some(), "{ctx}");
    assert!(ctx["forge"]["target"].as_str().is_some_and(|s| !s.is_empty()), "{ctx}");
    assert!(ctx["forge"]["build"]["hash"].as_str().is_some_and(|s| s.starts_with("blake3:")), "{ctx}");
}

#[test]
fn change_impact_returns_touch_set_and_gates() {
    let r = forge_tool_call("change_impact", json!({"paths": ["crates/engine/src/retouch_cmds.rs"]}));
    let touch = r["touch"].as_array().expect("touch array");
    assert!(touch.iter().any(|t| t["path"] == "crates/engine/src/commands.rs"), "{r}");
    let required = r["gates"]["required"].as_array().expect("required gates");
    assert!(required.iter().any(|g| g["id"] == "parity"), "{r}");
    assert!(required.iter().any(|g| g["id"] == "panic_hunt"), "{r}");
}

#[test]
fn checks_for_changes_selects_gates_for_a_path() {
    let r = forge_tool_call("checks_for_changes", json!({"paths": ["crates/engine/src/retouch_cmds.rs"]}));
    let required = r["required"].as_array().expect("required gates");
    assert!(required.iter().any(|g| g["id"] == "parity"), "{r}");
    assert!(required.iter().any(|g| g["id"] == "panic_hunt"), "{r}");
    assert!(r["fingerprint"]["head"].as_str().is_some(), "{r}");
}

#[test]
fn job_tools_are_queryable() {
    let status = forge_tool_call("job_status", json!({}));
    assert!(status["jobs"].as_array().is_some(), "{status}");
    let cancel = forge_tool_call("job_cancel", json!({"job": 999999}));
    assert_eq!(cancel["requested"], false, "{cancel}");
}

#[test]
fn validation_start_with_only_recommended_gates_starts_nothing() {
    // A docs-only change has no required gate (mdbook is recommended), so no job is launched.
    let r = forge_tool_call("validation_start", json!({"paths": ["docs/development.md"]}));
    assert_eq!(r["jobs"].as_array().map(Vec::len), Some(0), "{r}");
}

#[test]
#[ignore = "offscreen snapshot needs a render backend (llvmpipe/GPU); run with --ignored"]
fn scenario_start_offscreen_renders_a_png() {
    let r = forge_tool_call(
        "scenario_start",
        json!({
            "backend": "offscreen",
            "size": "64x48",
            "scale": 1,
            "steps": [],
            "assertions": [{"file": "ui.png", "width": 64, "height": 48, "min_bytes": 1}]
        }),
    );
    assert_eq!(r["passed"], true, "{r}");
    assert_eq!(r["evidence_tier"], "offscreen", "{r}");
    assert!(r["artifacts"].as_array().is_some_and(|a| a.iter().any(|x| x["media_type"] == "image/png")), "{r}");
}

#[test]
#[ignore = "runs the headless CLI (photocraft-cli run); run with --ignored"]
fn scenario_start_cli_runs_file_commands() {
    // A CSV with a CRLF inside a quoted field (issue #2239): the app MCP disables this command, so
    // only the `cli` backend can exercise it.
    let csv = "DataSet,caption\r\nOwned One,\"Owned caption\r\nSecond line\"\r\nOwned Two,Owned healthy second\r\n";
    let r = forge_tool_call(
        "scenario_start",
        json!({
            "backend": "cli",
            "new": {"width": 64, "height": 48},
            "files": {"owned.csv": csv},
            "steps": [
                {"tool": "file.import.variableDataSets", "arguments": {"path": "owned.csv"}},
                {"tool": "variables.list", "arguments": {}}
            ],
            "assertions": [
                {"json": "dataSets.0.name", "equals": "Owned One"},
                {"json": "dataSets.1.name", "equals": "Owned Two"},
                {"json": "dataSets.2", "exists": false}
            ]
        }),
    );
    assert_eq!(r["passed"], true, "{r}");
}

#[test]
fn scenario_start_exercises_doc_new_and_asserts() {
    let r = forge_tool_call(
        "scenario_start",
        json!({
            "steps": [
                {"tool": "doc_new", "arguments": {"width": 8, "height": 8}},
                {"tool": "doc_inspect", "arguments": {}},
                {"tool": "doc_render_preview", "arguments": {}}
            ],
            "assertions": [
                {"json": "width", "equals": 8},
                {"json": "height", "equals": 8},
                {"image": 2, "width": 8, "height": 8, "min_bytes": 1}
            ]
        }),
    );
    assert_eq!(r["passed"], true, "{r}");
}
