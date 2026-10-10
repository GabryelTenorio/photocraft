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
    assert_eq!(init["result"]["serverInfo"]["name"], "forge", "{init}");

    send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let tools = recv(2);
    let names: Vec<&str> = tools["result"]["tools"].as_array().expect("tools array").iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"forge_capabilities"), "{names:?}");
    assert!(names.contains(&"development_context"), "{names:?}");

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
}
