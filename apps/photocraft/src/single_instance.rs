//! Single instance (opt-in): a launch that finds PhotoCraft already running hands its files to
//! that window over a loopback socket and exits, instead of opening a second window.
//!
//! Turned on by Preferences › General › Single Instance or `--single-instance`; `--new-instance`
//! always starts a separate process. The running instance listens on `127.0.0.1` at a random port
//! and records it in `instance.json` in the settings directory, next to `instance.token` (a fresh
//! 256-bit token, mode 0600 on Unix). A second launch reads both, authenticates like a control
//! client and sends `app.openFiles {paths, wait}`. The socket accepts only
//! [`crate::control_server::INSTANCE_METHODS`]. With `--wait` the second launch keeps the
//! connection open until the running window reports `wait.done` for its files and exits with
//! that status, so external tools see the same behaviour with or without single instance.
//!
//! Platforms: plain loopback TCP and files in the settings directory, so Linux, Windows and macOS
//! behave alike (macOS Finder opens already reach the running app as Apple events).

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

const INFO_FILE: &str = "instance.json";
const TOKEN_FILE: &str = "instance.token";
/// How long a second launch tries to reach a running instance before starting its own.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether this launch uses single instance: `--new-instance` wins, then `--single-instance`,
/// then the preference (read from the saved preferences; missing or unreadable means off).
pub fn enabled(flag: Option<bool>, prefs_file: Option<&Path>) -> bool {
    flag.unwrap_or_else(|| {
        let prefs: photocraft_engine::prefs::Preferences =
            prefs_file.and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        prefs.general.single_instance
    })
}

/// The running instance's address and token, from the settings directory.
fn running(dir: &Path) -> Option<(SocketAddr, String)> {
    let info: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(INFO_FILE)).ok()?).ok()?;
    let port = u16::try_from(info.get("port")?.as_u64()?).ok().filter(|p| *p != 0)?;
    let token = photocraft_automation::security::client_token(None, Some(&dir.join(TOKEN_FILE))).ok()?;
    Some((SocketAddr::from(([127, 0, 0, 1], port)), token))
}

fn read_json(reader: &mut impl BufRead) -> Option<Value> {
    let mut line = String::new();
    match photocraft_automation::security::read_bounded_line(reader, &mut line) {
        Ok(photocraft_automation::security::LineRead::Line) => serde_json::from_str(&line).ok(),
        _ => None,
    }
}

/// What a forward did.
#[derive(Debug, PartialEq, Eq)]
pub enum Forwarded {
    /// No instance answered: start normally (and become the instance).
    NoInstance,
    /// The running window took the files; exit with this status.
    Exit(i32),
}

/// Hands `files` (absolute) to the running instance. With `wait`, blocks until it reports
/// `wait.done` and returns its exit status (1 if the instance went away first).
pub fn forward(dir: &Path, files: &[String], wait: bool) -> Forwarded {
    let Some((addr, token)) = running(dir) else { return Forwarded::NoInstance };
    let Ok(stream) = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) else { return Forwarded::NoInstance };
    let _ = stream.set_read_timeout(Some(REPLY_TIMEOUT));
    let _ = stream.set_write_timeout(Some(REPLY_TIMEOUT));
    let Ok(read) = stream.try_clone() else { return Forwarded::NoInstance };
    let mut reader = BufReader::new(read);
    let mut out = stream;
    if writeln!(out, "{}", json!({"id": "auth", "method": "auth", "params": {"token": token}})).is_err() {
        return Forwarded::NoInstance;
    }
    // A stale file (another program now owns the port, or an old token) is no instance.
    if read_json(&mut reader).and_then(|r| r.get("ok").and_then(Value::as_bool)) != Some(true) {
        return Forwarded::NoInstance;
    }
    // No files: just bring the running window forward, as a second click on the app icon does.
    let request = if files.is_empty() {
        json!({"id": 1, "method": "ui.focus", "params": {}})
    } else {
        json!({"id": 1, "method": "app.openFiles", "params": {"paths": files, "wait": wait}})
    };
    if writeln!(out, "{request}").is_err() {
        return Forwarded::NoInstance;
    }
    let Some(reply) = read_json(&mut reader) else { return Forwarded::NoInstance };
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        let error = reply.get("error").and_then(Value::as_str).unwrap_or("refused");
        eprintln!("photocraft: the running instance refused the files: {error}");
        return Forwarded::Exit(2);
    }
    if !wait || files.is_empty() {
        return Forwarded::Exit(0);
    }
    // The user may keep the files open for hours.
    let _ = out.set_read_timeout(None);
    loop {
        let Some(line) = read_json(&mut reader) else { return Forwarded::Exit(1) };
        if line.get("event").and_then(Value::as_str) == Some("wait.done") {
            let code = line.pointer("/params/exitCode").and_then(Value::as_i64).unwrap_or(1);
            return Forwarded::Exit(i32::try_from(code).unwrap_or(1));
        }
    }
}

/// This process is the instance: the socket it listens on and its token. Remove the record with
/// [`Instance::release`] when the app exits.
pub struct Instance {
    pub listener: TcpListener,
    pub token: String,
    info: PathBuf,
}

/// Binds a random loopback port and records it (with a fresh token) in the settings directory.
pub fn claim(dir: &Path) -> Result<Instance, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let token_path = dir.join(TOKEN_FILE);
    // A fresh token per instance: a token read by an earlier client is useless now.
    let _ = std::fs::remove_file(&token_path);
    let token = photocraft_automation::security::server_token(None, Some(&token_path)).map_err(|e| e.to_string())?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("single instance: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let info = dir.join(INFO_FILE);
    let text = json!({"port": port, "pid": std::process::id()}).to_string();
    photocraft_format::atomic_write(&info, text.as_bytes()).map_err(|e| format!("{}: {e}", info.display()))?;
    Ok(Instance { listener, token, info })
}

impl Instance {
    /// Forget the record if it is still this process's (a later instance may have replaced it).
    pub fn release(&self) {
        let ours =
            std::fs::read_to_string(&self.info).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.get("pid").and_then(Value::as_u64))
                == Some(u64::from(std::process::id()));
        if ours {
            let _ = std::fs::remove_file(&self.info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-instance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_flag_wins_over_the_preference_and_the_default_is_off() {
        let d = dir("enabled");
        let prefs = d.join("prefs.json");
        assert!(!enabled(None, None));
        assert!(!enabled(None, Some(&prefs)), "no file: off");
        std::fs::write(&prefs, "{broken").unwrap();
        assert!(!enabled(None, Some(&prefs)), "unreadable: off");
        std::fs::write(&prefs, r#"{"general":{"singleInstance":true}}"#).unwrap();
        assert!(enabled(None, Some(&prefs)));
        assert!(!enabled(Some(false), Some(&prefs)), "--new-instance");
        assert!(enabled(Some(true), None), "--single-instance");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn no_record_garbage_or_a_dead_port_means_no_instance() {
        let d = dir("stale");
        assert_eq!(forward(&d, &["/a.png".into()], false), Forwarded::NoInstance);
        std::fs::write(d.join(INFO_FILE), "not json").unwrap();
        assert_eq!(forward(&d, &["/a.png".into()], false), Forwarded::NoInstance);
        std::fs::write(d.join(INFO_FILE), r#"{"port": 99999}"#).unwrap();
        assert_eq!(forward(&d, &["/a.png".into()], false), Forwarded::NoInstance);
        // A port nobody listens on any more.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        std::fs::write(d.join(INFO_FILE), json!({"port": port}).to_string()).unwrap();
        std::fs::write(d.join(TOKEN_FILE), "0".repeat(64)).unwrap();
        assert_eq!(forward(&d, &["/a.png".into()], false), Forwarded::NoInstance);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A claimed instance serving the real instance socket: a forward opens the files, and a
    /// waiting forward exits with the status of `wait.done`; other methods are refused.
    #[test]
    fn a_second_launch_forwards_its_files_and_waits_for_them() {
        let d = dir("forward");
        let instance = claim(&d).unwrap();
        let (tx, rx) = channel::<photocraft_ui_egui::ControlRequest>();
        crate::control_server::listen(
            instance.listener.try_clone().unwrap(),
            instance.token.clone(),
            egui::Context::default(),
            tx,
            crate::control_server::Channel::Instance,
        );
        let ui = std::thread::spawn(move || {
            let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(first.method, "app.openFiles");
            assert!(first.instance && first.events.is_none());
            assert_eq!(first.params["paths"], json!(["/x/a.tif"]));
            first.reply.send(json!({"ok": true, "result": {"opened": 1}})).unwrap();
            let second = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(second.params["wait"], true);
            let events = second.events.clone().unwrap();
            second.reply.send(json!({"ok": true, "result": {"opened": 1}})).unwrap();
            events.send(json!({"event": "wait.done", "params": {"exitCode": 0, "saved": true}})).unwrap();
        });
        assert_eq!(forward(&d, &["/x/a.tif".into()], false), Forwarded::Exit(0));
        assert_eq!(forward(&d, &["/x/a.tif".into()], true), Forwarded::Exit(0));
        ui.join().unwrap();

        // The instance socket refuses everything but its few methods.
        let (addr, token) = running(&d).unwrap();
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut out = stream;
        writeln!(out, "{}", json!({"id": 1, "method": "auth", "params": {"token": token}})).unwrap();
        assert_eq!(read_json(&mut reader).unwrap()["ok"], true);
        writeln!(out, "{}", json!({"id": 2, "method": "engine.execute", "params": {"command": "file.new"}})).unwrap();
        let refused = read_json(&mut reader).unwrap();
        assert_eq!(refused["ok"], false);
        assert!(refused["error"].as_str().unwrap().contains("single-instance"));

        instance.release();
        assert!(!d.join(INFO_FILE).exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
