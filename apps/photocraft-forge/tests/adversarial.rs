//! Graceful-failure tests: every Forge tool must reject bad input with an error or a benign
//! non-verdict — never a panic and never a dropped connection (AGENTS.md rule 9).
//!
//! Cheap and deterministic: no Cargo runs, no host MCP, no repo mutation. Only `forge_capabilities`
//! is called for a green input; the rest are adversarial.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_photocraft-forge"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn photocraft-forge");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut server = Self { child, stdin, stdout, next: 1 };
        let id = server.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}
        }));
        let _ = server.recv(id);
        let _ = server.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        server
    }

    /// Send a message; returns its request id (0 for a notification).
    fn send(&mut self, v: Value) -> u64 {
        writeln!(self.stdin, "{v}").expect("write request");
        self.stdin.flush().expect("flush");
        match v.get("id").and_then(Value::as_u64) {
            Some(id) => {
                self.next = id + 1;
                id
            }
            None => 0,
        }
    }

    fn recv(&mut self, id: u64) -> Value {
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).expect("read response") > 0, "server closed (crash?)");
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v["id"] == id {
                return v;
            }
        }
    }

    fn call(&mut self, tool: &str, args: Value) -> Value {
        let id = self.next;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool, "arguments": args}}));
        self.recv(id)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A dropped connection, a panic or a hung call all surface as a panic here.
fn assert_survives(tool: &str, args: Value) -> Value {
    let mut server = Server::start();
    let response = server.call(tool, args);
    // A fresh green call after the bad one proves the server is still healthy.
    let alive = server.call("forge_capabilities", json!({}));
    assert_ne!(alive["result"]["isError"], true, "{tool}: server unhealthy after bad input: {alive}");
    response
}

fn is_graceful(response: &Value) -> bool {
    response.get("error").is_some() || response["result"].is_object()
}

#[test]
fn every_tool_survives_adversarial_params() {
    let cases: &[(&str, Value)] = &[
        ("change_impact", json!({"nope": 1})),
        ("change_impact", json!({})),
        ("change_impact", json!({"paths": "not-a-list"})),
        ("checks_for_changes", json!({"paths": "not-a-list"})),
        ("checks_for_changes", json!({"paths": [], "base": 42})),
        ("validation_start", json!({"paths": ["../../../etc/passwd"]})),
        ("validation_start", json!({"paths": ["x"], "idle_timeout_secs": "soon"})),
        ("scenario_start", json!({"steps": "x"})),
        ("scenario_start", json!({"steps": [], "assertions": [{"image": 999999}]})),
        ("scenario_start", json!({"backend": "holographic"})),
        ("job_status", json!({"job": -1})),
        ("job_logs", json!({"job": 999999})),
        ("job_logs", json!({"job": 1, "cursor": -5})),
        ("job_cancel", json!({"job": 0})),
        ("forge_events", json!({"kind": 1})),
        ("forge_capabilities", json!({"extra": true})),
        ("development_context", json!({"nope": 1})),
    ];
    for (tool, args) in cases {
        let response = assert_survives(tool, args.clone());
        assert!(is_graceful(&response), "{tool} {args} -> not graceful: {response}");
    }
}

#[test]
fn an_unknown_field_is_rejected_not_ignored() {
    let response = assert_survives("change_impact", json!({"nope": 1}));
    let is_error = response.get("error").is_some() || response["result"]["isError"] == true;
    assert!(is_error, "unknown field should be an error: {response}");
}
