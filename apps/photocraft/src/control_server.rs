//! Localhost JSON-lines control server (one request per line, one reply per line).
//! Loopback only. This is the transport the MCP server will wrap.

use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Instant;

use photocraft_automation::budgets::write_reply;
use photocraft_automation::security::{
    ConnectionLimiter, LineRead, MAX_CONNECTIONS, MAX_REQUEST_BYTES, authentication_reply, configure_stream, read_bounded_line,
};
use photocraft_ui_egui::{ControlRequest, ControlResponse};
use photocraft_ui_egui::control::{
    ControlWatch, REPLY_DEADLINE, STALL_CHECK, Stalled, not_drawing_frames_error, stalled_action, timeout_error,
};
use serde_json::{Value, json};

pub fn start(port: u16, token: String, ctx: egui::Context) -> (Receiver<ControlRequest>, Arc<ControlWatch>) {
    let (tx, rx) = channel::<ControlRequest>();
    let watch = Arc::new(ControlWatch::new());
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("photocraft: control server failed to bind 127.0.0.1:{port}: {e}");
            return (rx, watch);
        }
    };
    eprintln!("photocraft: control server listening on 127.0.0.1:{port}");
    std::thread::spawn(move || {
        let limiter = ConnectionLimiter::new(MAX_CONNECTIONS);
        let token = Arc::new(token);
        for mut stream in listener.incoming().flatten() {
            let Some(permit) = limiter.try_acquire() else {
                let _ = configure_stream(&stream);
                let _ = writeln!(stream, "{}", json!({"id": null, "ok": false, "error": "connection limit reached"}));
                continue;
            };
            let tx = tx.clone();
            let ctx = ctx.clone();
            let token = Arc::clone(&token);
            let watch = Arc::clone(&watch);
            std::thread::spawn(move || {
                let _permit = permit;
                serve(stream, &token, tx, ctx, &watch);
            });
        }
    });
    (rx, watch)
}

fn serve(stream: TcpStream, token: &str, tx: Sender<ControlRequest>, ctx: egui::Context, watch: &ControlWatch) {
    if configure_stream(&stream).is_err() {
        return;
    }
    let Ok(read) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read);
    let mut out = stream;
    let mut line = String::new();
    let mut authenticated = false;
    loop {
        match read_bounded_line(&mut reader, &mut line) {
            Ok(LineRead::Eof) | Err(_) => break,
            Ok(LineRead::TooLong) => {
                let reply = json!({
                    "id": null,
                    "ok": false,
                    "error": format!("request exceeds {MAX_REQUEST_BYTES} bytes"),
                });
                let _ = write_reply(&mut out, &reply);
                let _ = out.flush();
                break;
            }
            Ok(LineRead::Line) if line.trim().is_empty() => continue,
            Ok(LineRead::Line) => {}
        }
        if !authenticated {
            let (reply, ok) = authentication_reply(&line, token);
            authenticated = ok;
            if write_reply(&mut out, &reply).is_err() || out.flush().is_err() || !authenticated {
                break;
            }
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => {
                let id = msg.get("id").cloned().unwrap_or(Value::Null);
                let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
                let params = msg.get("params").cloned().unwrap_or(json!({}));
                // `health` is answered from the watch alone, before the queue: it must not
                // need a frame (#3667), which is the state it exists to report on.
                let mut r = if method == "health" {
                    json!({"ok": true, "result": watch.health()})
                } else {
                    let deadline = Instant::now() + REPLY_DEADLINE;
                    let (mut req, rrx) = ControlRequest::new(method.clone(), params.clone());
                    req.deadline = Some(deadline);
                    // Read before the send: a later read could see a logic pass the request
                    // itself woke, and wrongly serve a snapshot for a live loop.
                    let logic_before = watch.logic_count();
                    if tx.send(req).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                    wait_reply(&rrx, watch, &method, &params, logic_before, deadline)
                };
                if let Some(o) = r.as_object_mut() {
                    o.insert("id".into(), id);
                }
                r
            }
            Err(e) => json!({"id": null, "ok": false, "error": format!("bad JSON: {e}")}),
        };
        if write_reply(&mut out, &reply).is_err() {
            break;
        }
        if out.flush().is_err() {
            break;
        }
    }
}

/// Wait for the frame thread's reply, falling back when the frame loop is wedged (#2493):
/// after [`STALL_CHECK`] without a reply, a live loop keeps waiting while a wedged one serves
/// the getter's last snapshot or fails fast. A dropped reply sender (the request's receiver
/// gone) can never be answered; it gets the same deadline error as before.
fn wait_reply(
    rrx: &Receiver<ControlResponse>,
    watch: &ControlWatch,
    method: &str,
    params: &Value,
    logic_before: u64,
    deadline: Instant,
) -> Value {
    let stall_at = std::cmp::min(deadline, Instant::now() + STALL_CHECK);
    match rrx.recv_timeout(stall_at) {
        Ok(r) => return r,
        Err(RecvTimeoutError::Disconnected) => return json!({"ok": false, "error": timeout_error()}),
        Err(RecvTimeoutError::Timeout) => {}
    }
    if Instant::now() >= deadline {
        return json!({"ok": false, "error": timeout_error()});
    }
    match stalled_action(watch, method, params, logic_before) {
        Stalled::KeepWaiting => match rrx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(r) => r,
            Err(_) => json!({"ok": false, "error": timeout_error()}),
        },
        Stalled::Snapshot { age_ms, mut value } => {
            // Stamp into `result` when it is an object; a non-object result still gets the
            // flags, into the reply itself.
            if let Some(obj) = value.get_mut("result").and_then(Value::as_object_mut) {
                obj.insert("frameStalled".into(), json!(true));
                obj.insert("snapshotAgeMs".into(), json!(age_ms));
            } else if let Some(obj) = value.as_object_mut() {
                obj.insert("frameStalled".into(), json!(true));
                obj.insert("snapshotAgeMs".into(), json!(age_ms));
            }
            value
        }
        Stalled::NotDrawingFrames => json!({"ok": false, "error": not_drawing_frames_error()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::time::Duration;

    #[test]
    fn oversized_reply_preserves_framing_id_and_the_next_control_request() {
        use photocraft_automation::budgets::MAX_RESPONSE_BYTES;
        const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = channel::<ControlRequest>();
        let watch = ControlWatch::new();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve(stream, TOKEN, tx, egui::Context::default(), &watch);
        });
        let handler = std::thread::spawn(move || {
            let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(first.method, "test.large");
            assert!(first.deadline.is_some_and(|deadline| deadline > std::time::Instant::now()), "native requests carry their reply deadline");
            first.reply.send(json!({"ok": true, "result": "x".repeat(MAX_RESPONSE_BYTES)})).unwrap();
            let second = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(second.method, "test.small");
            second.reply.send(json!({"ok": true, "result": "still serving"})).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        writeln!(stream, "{}", json!({"id": 1, "method": "auth", "params": {"token": TOKEN}})).unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["ok"], true);
        writeln!(stream, "{}", json!({"id": 2, "method": "test.large"})).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let rejected: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(rejected["id"], 2);
        assert_eq!(rejected["ok"], false);
        assert!(rejected["error"].as_str().unwrap().contains("operation may have completed"));
        writeln!(stream, "{}", json!({"id": 3, "method": "test.small"})).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let accepted: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(accepted["id"], 3);
        assert_eq!(accepted["result"], "still serving");
        drop(reader);
        drop(stream);
        handler.join().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn bad_json_reply_carries_a_null_id_and_the_connection_keeps_serving() {
        const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = channel::<ControlRequest>();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve(stream, TOKEN, tx, egui::Context::default());
        });
        let handler = std::thread::spawn(move || {
            let req = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(req.method, "test.after");
            req.reply.send(json!({"ok": true, "result": "still serving"})).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        writeln!(stream, "{}", json!({"id": "auth", "method": "auth", "params": {"token": TOKEN}})).unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["ok"], true);
        writeln!(stream, "{{not json}}").unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let bad: Value = serde_json::from_str(&line).unwrap();
        let bad = bad.as_object().unwrap();
        assert_eq!(bad.get("id"), Some(&Value::Null), "bad JSON reply must carry `id: null`: {line}");
        assert_eq!(bad.get("ok"), Some(&Value::Bool(false)));
        assert!(bad.get("error").and_then(Value::as_str).unwrap().starts_with("bad JSON: "));
        writeln!(stream, "{}", json!({"id": 7, "method": "test.after"})).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let after: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(after["id"], 7);
        assert_eq!(after["result"], "still serving");
        drop(reader);
        drop(stream);
        handler.join().unwrap();
        server.join().unwrap();
    }
}
