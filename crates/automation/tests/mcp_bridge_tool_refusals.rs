//! Bridge mode refuses the capability-scoped document tools up front: nothing is
//! sent to the app, and the error names the surfaces that do run them.

use photocraft_automation::PhotocraftMcp;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientConfig};
use rmcp::service::RunningService;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const CONTROL_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Clone, Default)]
struct Client;
impl ClientHandler for Client {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

async fn connect(server: PhotocraftMcp) -> RunningService<RoleClient, Client> {
    let (s, c) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(s).await {
            let _ = running.waiting().await;
        }
    });
    Client.serve(c).await.expect("client init")
}

async fn call(client: &RunningService<RoleClient, Client>, name: &str, args: Value) -> CallToolResult {
    let mut p = CallToolRequestParams::new(name.to_owned());
    if let Value::Object(m) = args {
        p = p.with_arguments(m);
    }
    client.call_tool(p).await.expect("call_tool transport")
}

fn text(r: &CallToolResult) -> String {
    r.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect::<Vec<_>>().join("\n")
}

fn json_of(r: &CallToolResult) -> Value {
    assert_ne!(r.is_error, Some(true), "tool error: {}", text(r));
    serde_json::from_str(&text(r)).unwrap_or_else(|e| panic!("not JSON ({e}): {}", text(r)))
}

/// `(tool, arguments, the serve method that runs the same operation)`.
fn refused_tools() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        ("doc_place", json!({"path": "logo.png"}), "doc.place"),
        ("doc_export_layers", json!({"dir": "layers"}), "doc.exportLayers"),
        ("doc_export_data_sets", json!({"dir": "sets"}), "doc.exportDataSets"),
        ("doc_import_data_sets", json!({"path": "sets.csv"}), "doc.importDataSets"),
    ]
}

/// A control server that answers any `engine.execute` and records every request it saw.
async fn fake_app() -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let h = tokio::spawn(async move {
        let mut seen = Vec::new();
        let (sock, _) = listener.accept().await.unwrap();
        let (read, mut write) = sock.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let request: Value = serde_json::from_str(&line).unwrap();
            let reply = match request["method"].as_str().unwrap_or("") {
                "auth" => json!({"id": request["id"], "ok": true, "result": {"authenticated": true}}),
                "engine.execute" => json!({"id": request["id"], "ok": true, "result": {"ran": request["params"]["command"]}}),
                _ => json!({"id": request["id"], "ok": false, "error": "unknown method"}),
            };
            write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            seen.push(request);
        }
        seen
    });
    (addr, h)
}

#[tokio::test(flavor = "multi_thread")]
async fn bridge_refuses_the_scoped_document_tools_before_connecting() {
    // Nothing listens: an up-front refusal never tries the connection.
    let client = connect(PhotocraftMcp::bridge("127.0.0.1:1", &"a".repeat(64)).unwrap()).await;
    for (tool, args, serve_method) in refused_tools() {
        let r = call(&client, tool, args).await;
        let error = text(&r);
        assert_eq!(r.is_error, Some(true), "{tool}: {error}");
        assert!(error.contains("not available in bridge mode"), "{tool}: {error}");
        assert!(error.contains("photocraft-cli mcp"), "{tool}: {error}");
        assert!(error.contains("photocraft-cli serve"), "{tool}: {error}");
        assert!(error.contains(serve_method), "{tool}: {error}");
        assert!(!error.contains("--control"), "{tool}: the refusal reached for the app: {error}");
    }
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn bridge_refusals_send_nothing_to_the_app() {
    let (addr, app) = fake_app().await;
    let client = connect(PhotocraftMcp::bridge(&addr, CONTROL_TOKEN).unwrap()).await;
    // The bridge itself works: a plain command reaches the app and runs.
    let ran = json_of(&call(&client, "command_run", json!({"id": "layer.new.layer"})).await);
    assert_eq!(ran["ran"], "layer.new.layer");
    for (tool, args, _) in refused_tools() {
        let r = call(&client, tool, args).await;
        let error = text(&r);
        assert_eq!(r.is_error, Some(true), "{tool}: {error}");
        assert!(error.contains("not available in bridge mode"), "{tool}: {error}");
    }
    client.cancel().await.unwrap();
    let seen = tokio::time::timeout(std::time::Duration::from_secs(5), app).await.expect("fake app finished").unwrap();
    let commands: Vec<&str> = seen
        .iter()
        .filter(|request| request["method"] == "engine.execute")
        .map(|request| request["params"]["command"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(commands, ["layer.new.layer"], "the refused tools must not reach the app: {seen:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_document_tools_advertise_the_bridge_caveat() {
    let client = connect(PhotocraftMcp::headless()).await;
    let tools = client.list_all_tools().await.unwrap();
    for (tool, _, _) in refused_tools() {
        let found = tools.iter().find(|t| t.name == tool).unwrap_or_else(|| panic!("missing tool {tool}"));
        let description = found.description.as_deref().unwrap_or_else(|| panic!("{tool} has no description"));
        assert!(description.contains("(bridge mode: not available)"), "{tool}: {description}");
    }
    client.cancel().await.unwrap();
}
