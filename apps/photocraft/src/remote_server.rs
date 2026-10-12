//! Remote Canvas server: serves the tablet page (`apps/photocraft-remote`) on the local network
//! and streams this window to it over a WebSocket, taking its pen and touch input back (see
//! `photocraft_ui_egui::remote`).
//!
//! Started with `--remote [port]` or `PHOTOCRAFT_REMOTE=<port>`. The page's files are public; the
//! WebSocket that sees and drives the window needs the pairing key (`?k=`) from the URL the app
//! prints and shows, so anyone else on the network gets a 403. The key is kept in the settings
//! directory, so a bookmark or Home Screen icon on the tablet keeps working.
//!
//! Wire format, server → page (binary messages): a 16-byte header `"PCRF"`, flags (`u32`, bit 0 =
//! full frame), width and height (`u32`, physical pixels), then a deflate stream of tiles:
//! `x, y, w, h` (`u16` each) and `w * h` RGBA pixels. Page → server: JSON text, one input object
//! or an array of them (see [`parse_input`]). Numbers are little-endian.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use photocraft_ui_egui::remote::{Phase, RemoteCursor, RemoteFrame, RemoteInput, RemoteLink};
use photocraft_ui_egui::stylus::PenSample;
use serde_json::Value;

/// The default port (`--remote` without one).
pub const DEFAULT_PORT: u16 = 9876;
/// Tile edge in pixels: only tiles that changed since the last frame are sent.
const TILE: usize = 64;
/// Unchanged frames in a row before capturing slows down.
const IDLE_AFTER: u32 = 15;
/// Largest request head and input message accepted.
const MAX_HEAD: usize = 8 * 1024;
const MAX_MESSAGE: usize = 256 * 1024;

mod assets {
    include!(concat!(env!("OUT_DIR"), "/remote_assets.rs"));
}

/// A running server: where the page is reachable, and how to stop it.
pub struct Started {
    pub urls: Vec<String>,
    /// Stop listening (freeing the port) and drop the connected page.
    pub stop: Box<dyn FnOnce() + Send>,
}

/// How often the accept loop checks whether it was stopped.
const ACCEPT_POLL: Duration = Duration::from_millis(50);

/// Bind the server and start accepting pages. Only one page drives the window at a time: a new
/// connection replaces the previous one.
pub fn start(port: u16, key: String, link: RemoteLink) -> Result<Started, String> {
    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("Remote Canvas could not listen on port {port}: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| format!("Remote Canvas: {e}"))?;
    let urls = lan_addresses(port).into_iter().map(|a| format!("http://{a}/?k={key}")).collect();
    let key = Arc::new(key);
    // Bumped by every new page and by stop: a page's loop ends once it isn't the latest.
    let session = Arc::new(AtomicU64::new(0));
    let stopped = Arc::new(AtomicBool::new(false));
    let (session_in, stopped_in) = (session.clone(), stopped.clone());
    std::thread::Builder::new()
        .name("remote-accept".into())
        .spawn(move || {
            while !stopped_in.load(Ordering::SeqCst) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(ACCEPT_POLL);
                        continue;
                    }
                    Err(e) => {
                        log::debug!("remote: accept: {e}");
                        std::thread::sleep(ACCEPT_POLL);
                        continue;
                    }
                };
                // Accepted sockets inherit non-blocking on some platforms: each connection blocks.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let (key, link, session) = (key.clone(), link.clone(), session_in.clone());
                let _ = std::thread::Builder::new().name("remote-client".into()).spawn(move || {
                    if let Err(e) = serve(stream, &key, &link, &session) {
                        log::debug!("remote: {e}");
                    }
                });
            }
        })
        .map_err(|e| format!("Remote Canvas: {e}"))?;
    let stop = Box::new(move || {
        stopped.store(true, Ordering::SeqCst);
        session.fetch_add(1, Ordering::SeqCst);
    });
    Ok(Started { urls, stop })
}

/// `text` as a QR code (medium error correction), for the Remote Canvas window.
pub fn qr(text: &str) -> Option<photocraft_ui_egui::remote::Qr> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).ok()?;
    let size = code.width();
    let modules = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
    Some(photocraft_ui_egui::remote::Qr { size, modules })
}

/// The persistent pairing key (created on first use).
pub fn pairing_key(dir: Option<&std::path::Path>) -> Result<String, String> {
    let path = dir.map(|d| d.join("remote-canvas.key"));
    photocraft_automation::security::server_token(None, path.as_deref()).map_err(|e| format!("Remote Canvas key: {e}"))
}

/// This machine's addresses other devices on the network can use (the route to the internet
/// first), with the port. Nothing is sent: connecting a UDP socket only picks the route.
fn lan_addresses(port: u16) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    if let Ok(s) = UdpSocket::bind("0.0.0.0:0")
        && s.connect("192.0.2.1:9").is_ok()
        && let Ok(a) = s.local_addr()
        && !a.ip().is_loopback()
    {
        out.push(SocketAddr::new(a.ip(), port));
    }
    if out.is_empty() {
        out.push(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    out
}

/// One HTTP connection: a page asset, or the WebSocket.
fn serve(mut stream: TcpStream, key: &str, link: &RemoteLink, session: &AtomicU64) -> Result<(), String> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    let head = peek_head(&stream)?;
    let target = head.lines().next().and_then(|l| l.split(' ').nth(1)).unwrap_or("/").to_string();
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    if path == "/ws" {
        let supplied = query.split('&').find_map(|kv| kv.strip_prefix("k=")).unwrap_or("");
        if !photocraft_automation::security::token_matches(key, supplied) {
            return respond(&mut stream, "403 Forbidden", "text/plain", b"Open the address PhotoCraft shows (it includes the pairing key).");
        }
        return websocket(stream, link, session);
    }
    let path = if path == "/" { "/index.html" } else { path };
    match assets::ASSETS.iter().find(|(p, _, _)| *p == path) {
        Some((_, mime, body)) => respond(&mut stream, "200 OK", mime, body),
        None => respond(&mut stream, "404 Not Found", "text/plain", b"not found"),
    }
}

/// The request head without consuming it (the WebSocket handshake reads it again).
fn peek_head(stream: &TcpStream) -> Result<String, String> {
    let mut buf = vec![0u8; MAX_HEAD];
    for _ in 0..100 {
        let n = stream.peek(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed".into());
        }
        let got = buf.get(..n).unwrap_or_default();
        if let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            return Ok(String::from_utf8_lossy(got.get(..end).unwrap_or_default()).into_owned());
        }
        if n == MAX_HEAD {
            return Err("request head too large".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err("incomplete request".into())
}

fn respond(stream: &mut TcpStream, status: &str, mime: &str, body: &[u8]) -> Result<(), String> {
    // Drain the request we only peeked at, so closing doesn't reset the connection.
    let mut sink = [0u8; MAX_HEAD];
    let _ = stream.read(&mut sink);
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body)).map_err(|e| e.to_string())
}

fn websocket(stream: TcpStream, link: &RemoteLink, session: &AtomicU64) -> Result<(), String> {
    let mut ws = tungstenite::accept(stream).map_err(|e| format!("handshake: {e}"))?;
    ws.get_mut().set_nonblocking(true).map_err(|e| e.to_string())?;
    let me = session.fetch_add(1, Ordering::SeqCst) + 1;
    link.set_connected(true);
    log::info!("remote: page connected");
    let mut encoder = Encoder::default();
    let mut unflushed = false;
    // The cursor outline the page was last told about (`None` before the first).
    let mut told_cursor: Option<Option<RemoteCursor>> = None;
    let mut told_foreground: Option<Option<[u8; 3]>> = None;
    let result = loop {
        if session.load(Ordering::SeqCst) != me {
            let _ = ws.close(None);
            break Ok(()); // a newer page took over
        }
        // Input first: it is what the pen is waiting on.
        match read_inputs(&mut ws, link) {
            Ok(true) => {}
            Ok(false) => break Ok(()),
            Err(e) => break Err(e),
        }
        if unflushed {
            match ws.flush() {
                Ok(()) => unflushed = false,
                Err(e) if would_block(&e) => {}
                Err(e) => break Err(e.to_string()),
            }
            if unflushed {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
        }
        let cursor = link.cursor();
        if told_cursor.as_ref() != Some(&cursor) {
            let message = cursor_message(cursor.as_ref());
            told_cursor = Some(cursor);
            match ws.send(tungstenite::Message::text(message)) {
                Ok(()) => {}
                Err(e) if would_block(&e) => {
                    unflushed = true;
                    continue;
                }
                Err(e) => break Err(e.to_string()),
            }
        }
        let foreground = link.foreground();
        if told_foreground != Some(foreground)
            && let Some(rgb) = foreground
        {
            told_foreground = Some(foreground);
            match ws.send(tungstenite::Message::text(serde_json::json!({"t": "fg", "rgb": rgb}).to_string())) {
                Ok(()) => {}
                Err(e) if would_block(&e) => {
                    unflushed = true;
                    continue;
                }
                Err(e) => break Err(e.to_string()),
            }
        }
        let Some(frame) = link.wait_frame(Duration::from_millis(3)) else { continue };
        let Some(message) = encoder.encode(&frame) else {
            if encoder.unchanged == IDLE_AFTER {
                link.set_idle(true);
            }
            continue;
        };
        link.set_idle(false);
        match ws.send(tungstenite::Message::binary(message)) {
            Ok(()) => {}
            Err(e) if would_block(&e) => unflushed = true,
            Err(e) => break Err(e.to_string()),
        }
    };
    if session.load(Ordering::SeqCst) == me {
        link.set_connected(false);
        link.push(RemoteInput::Leave);
    }
    log::info!("remote: page disconnected");
    result
}

/// The page message for the canvas cursor's outline (window pixels):
/// `{"t": "cursor", "shape": "circle" | "cross" | "rings" | "none", …}`.
fn cursor_message(cursor: Option<&RemoteCursor>) -> String {
    let v = match cursor {
        Some(RemoteCursor::Rings { rings, centre }) => serde_json::json!({"t": "cursor", "shape": "rings", "rings": rings, "c": centre}),
        Some(RemoteCursor::Circle { radius, centre }) => serde_json::json!({"t": "cursor", "shape": "circle", "r": radius, "c": centre}),
        Some(RemoteCursor::Crosshair { length, gap }) => serde_json::json!({"t": "cursor", "shape": "cross", "l": length, "g": gap}),
        None => serde_json::json!({"t": "cursor", "shape": "none"}),
    };
    v.to_string()
}

fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// Read every waiting message. `Ok(false)`: the page closed.
fn read_inputs(ws: &mut tungstenite::WebSocket<TcpStream>, link: &RemoteLink) -> Result<bool, String> {
    loop {
        match ws.read() {
            Ok(tungstenite::Message::Text(text)) => {
                if text.len() > MAX_MESSAGE {
                    return Err("input message too large".into());
                }
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                for input in parse_input(&v) {
                    link.push(input);
                }
            }
            Ok(tungstenite::Message::Close(_)) => return Ok(false),
            Ok(_) => {}
            Err(e) if would_block(&e) => return Ok(true),
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return Ok(false),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Page input: `{"t": "pen" | "hover" | "ptr" | "scroll" | "zoom" | "rotate" | "rotateEnd" | "menu" | "leave", …}`
/// or an array of those. Unknown or malformed entries are skipped.
fn parse_input(v: &Value) -> Vec<RemoteInput> {
    match v {
        Value::Array(items) => items.iter().take(4096).filter_map(parse_one).collect(),
        other => parse_one(other).into_iter().collect(),
    }
}

fn parse_one(v: &Value) -> Option<RemoteInput> {
    let n = |k: &str| v.get(k).and_then(Value::as_f64).map(|f| f as f32).filter(|f| f.is_finite());
    let phase = || match v.get("p").and_then(Value::as_str)? {
        "d" => Some(Phase::Down),
        "m" => Some(Phase::Move),
        "u" => Some(Phase::Up),
        _ => None,
    };
    let (x, y) = (n("x").unwrap_or(0.0), n("y").unwrap_or(0.0));
    Some(match v.get("t")?.as_str()? {
        "pen" => RemoteInput::Pen {
            phase: phase()?,
            x,
            y,
            sample: PenSample {
                pressure: n("f").unwrap_or(1.0),
                tilt_x: n("tx").unwrap_or(0.0),
                tilt_y: n("ty").unwrap_or(0.0),
                rotation: n("r").unwrap_or(0.0),
                eraser: v.get("e").and_then(Value::as_bool).unwrap_or(false),
            }
            .sanitized(),
        },
        "hover" => RemoteInput::Hover { x, y },
        "ptr" => RemoteInput::Pointer { phase: phase()?, x, y },
        "scroll" => RemoteInput::Scroll { x, y, dx: n("dx")?, dy: n("dy")? },
        "zoom" => RemoteInput::Zoom { x, y, factor: n("f")? },
        "rotate" => RemoteInput::Rotate { x, y, degrees: n("d")? },
        "rotateEnd" => RemoteInput::RotateEnd { x, y },
        "pick" => RemoteInput::Pick { phase: phase()?, x, y },
        "menu" => RemoteInput::Menu(v.get("id")?.as_str()?.chars().take(128).collect()),
        "leave" => RemoteInput::Leave,
        _ => return None,
    })
}

/// Tile differ: remembers the last frame sent and encodes what changed.
#[derive(Default)]
struct Encoder {
    prev: Option<Arc<egui::ColorImage>>,
    /// Frames in a row with nothing to send.
    unchanged: u32,
}

impl Encoder {
    /// The message for `frame`, or `None` when nothing changed.
    fn encode(&mut self, frame: &RemoteFrame) -> Option<Vec<u8>> {
        let img = &frame.image;
        let [w, h] = img.size;
        if w == 0 || h == 0 || w > u16::MAX as usize || h > u16::MAX as usize || img.pixels.len() != w.checked_mul(h)? {
            return None;
        }
        let prev = self.prev.as_ref().filter(|p| p.size == img.size);
        let full = prev.is_none();
        let mut body = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tiles = 0usize;
        for ty in (0..h).step_by(TILE) {
            for tx in (0..w).step_by(TILE) {
                let (tw, th) = (TILE.min(w - tx), TILE.min(h - ty));
                fn row(p: &egui::ColorImage, w: usize, tx: usize, tw: usize, y: usize) -> &[egui::Color32] {
                    p.pixels.get(y * w + tx..y * w + tx + tw).unwrap_or_default()
                }
                let row = |p, y| row(p, w, tx, tw, ty + y);
                let changed = match prev {
                    None => true,
                    Some(p) => (0..th).any(|y| row(p, y) != row(img, y)),
                };
                if !changed {
                    continue;
                }
                tiles += 1;
                let mut out = Vec::with_capacity(8 + tw * th * 4);
                for v in [tx, ty, tw, th] {
                    out.extend_from_slice(&(v as u16).to_le_bytes());
                }
                for y in 0..th {
                    for c in row(img, y) {
                        out.extend_from_slice(&c.to_array());
                    }
                }
                body.write_all(&out).ok()?;
            }
        }
        self.prev = Some(img.clone());
        if tiles == 0 {
            self.unchanged = self.unchanged.saturating_add(1);
            return None;
        }
        self.unchanged = 0;
        let body = body.finish().ok()?;
        let mut msg = Vec::with_capacity(16 + body.len());
        msg.extend_from_slice(b"PCRF");
        msg.extend_from_slice(&u32::from(full).to_le_bytes());
        msg.extend_from_slice(&(w as u32).to_le_bytes());
        msg.extend_from_slice(&(h as u32).to_le_bytes());
        msg.extend_from_slice(&body);
        Some(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(w: usize, h: usize, f: impl Fn(usize, usize) -> egui::Color32) -> RemoteFrame {
        let pixels = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect();
        RemoteFrame { image: Arc::new(egui::ColorImage { size: [w, h], pixels, source_size: egui::vec2(w as f32, h as f32) }), pixels_per_point: 1.0 }
    }

    fn tiles(msg: &[u8]) -> (u32, Vec<[u16; 4]>) {
        let flags = u32::from_le_bytes(msg[4..8].try_into().unwrap());
        let mut body = Vec::new();
        flate2::read::DeflateDecoder::new(&msg[16..]).read_to_end(&mut body).unwrap();
        let mut out = Vec::new();
        let mut at = 0;
        while at < body.len() {
            let v = |i: usize| u16::from_le_bytes([body[at + i * 2], body[at + i * 2 + 1]]);
            let t = [v(0), v(1), v(2), v(3)];
            at += 8 + t[2] as usize * t[3] as usize * 4;
            out.push(t);
        }
        assert_eq!(at, body.len());
        (flags, out)
    }

    #[test]
    fn first_frame_is_full_then_only_changed_tiles() {
        let mut enc = Encoder::default();
        let a = frame(100, 70, |_, _| egui::Color32::WHITE);
        let (flags, t) = tiles(&enc.encode(&a).unwrap());
        assert_eq!(flags, 1);
        assert_eq!(t, vec![[0, 0, 64, 64], [64, 0, 36, 64], [0, 64, 64, 6], [64, 64, 36, 6]]);
        assert!(enc.encode(&a).is_none());
        assert_eq!(enc.unchanged, 1);
        let b = frame(100, 70, |x, y| if (x, y) == (80, 66) { egui::Color32::RED } else { egui::Color32::WHITE });
        let (flags, t) = tiles(&enc.encode(&b).unwrap());
        assert_eq!((flags, t), (0, vec![[64, 64, 36, 6]]));
        // A resized window is a full frame again.
        let (flags, _) = tiles(&enc.encode(&frame(50, 50, |_, _| egui::Color32::WHITE)).unwrap());
        assert_eq!(flags, 1);
    }

    #[test]
    fn malformed_frames_are_skipped() {
        let mut enc = Encoder::default();
        let mut f = frame(10, 10, |_, _| egui::Color32::WHITE);
        Arc::make_mut(&mut f.image).pixels.pop();
        assert!(enc.encode(&f).is_none());
        assert!(enc.encode(&frame(0, 10, |_, _| egui::Color32::WHITE)).is_none());
    }

    #[test]
    fn stop_frees_the_port_and_ends_the_page() {
        let link = RemoteLink::new();
        // An ephemeral port: bind one, free it, then serve on it.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let started = start(port, "a".repeat(64), link.clone()).unwrap();
        assert!(started.urls[0].ends_with(&format!(":{port}/?k={}", "a".repeat(64))));
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(c, "GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut page = String::new();
        c.read_to_string(&mut page).unwrap();
        assert!(page.starts_with("HTTP/1.1 200"));
        (started.stop)();
        // The accept loop notices within its poll interval and drops the listener.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while TcpListener::bind(("0.0.0.0", port)).is_err() {
            assert!(std::time::Instant::now() < deadline, "port {port} still taken after stop");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_page_hears_cursor_outline_changes() {
        let link = RemoteLink::new();
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let key = "b".repeat(64);
        let started = start(port, key.clone(), link.clone()).unwrap();
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (mut ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}/ws?k={key}"), stream).unwrap();
        let mut next_cursor = || loop {
            if let tungstenite::Message::Text(t) = ws.read().unwrap() {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["t"] == "cursor" {
                    return v;
                }
            }
        };
        assert_eq!(next_cursor()["shape"], "none");
        link.set_cursor(Some(RemoteCursor::Circle { radius: 24.0, centre: false }));
        let v = next_cursor();
        assert_eq!((v["shape"].as_str(), v["r"].as_f64(), v["c"].as_bool()), (Some("circle"), Some(24.0), Some(false)));
        link.set_cursor(Some(RemoteCursor::Crosshair { length: 6.0, gap: 0.0 }));
        let v = next_cursor();
        assert_eq!((v["shape"].as_str(), v["l"].as_f64()), (Some("cross"), Some(6.0)));
        link.set_cursor(Some(RemoteCursor::Rings { rings: vec![vec![[0.0, -4.0], [3.0, 2.0], [-3.0, 2.0]]], centre: true }));
        let v = next_cursor();
        assert_eq!((v["shape"].as_str(), v["rings"][0][1][0].as_f64(), v["c"].as_bool()), (Some("rings"), Some(3.0), Some(true)));
        (started.stop)();
    }

    #[test]
    fn qr_codes_encode_the_address() {
        let q = qr("http://192.168.1.79:9876/?k=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap();
        assert!(q.size >= 21 && q.modules.len() == q.size * q.size);
        // The three finder patterns' corners are dark.
        assert!(q.modules[0] && q.modules[q.size - 1] && q.modules[(q.size - 1) * q.size]);
    }

    #[test]
    fn input_parsing() {
        let v = json!([
            {"t": "pen", "p": "d", "x": 10, "y": 20, "f": 0.5, "tx": 30, "e": true},
            {"t": "pen", "p": "?", "x": 1},
            {"t": "zoom", "x": 1, "y": 2, "f": 1.1},
            {"t": "scroll", "x": 1},
            {"t": "menu", "id": "edit.undo"},
            {"t": "nope"},
            "junk",
        ]);
        let got = parse_input(&v);
        assert_eq!(got.len(), 3);
        assert!(
            matches!(&got[0], RemoteInput::Pen { phase: Phase::Down, x: 10.0, y: 20.0, sample } if sample.pressure == 0.5 && sample.tilt_x == 30.0 && sample.eraser)
        );
        assert!(matches!(&got[2], RemoteInput::Menu(id) if id == "edit.undo"));
        let got = parse_input(&json!([{"t": "pick", "p": "m", "x": 3, "y": 4}, {"t": "pick", "x": 3}]));
        assert_eq!(got, vec![RemoteInput::Pick { phase: Phase::Move, x: 3.0, y: 4.0 }], "a pick needs its phase");
        // Out-of-range pen values are clamped.
        let got = parse_input(&json!({"t": "pen", "p": "m", "f": 9.0, "tx": -500}));
        assert!(matches!(&got[0], RemoteInput::Pen { sample, .. } if sample.pressure == 1.0 && sample.tilt_x == -90.0));
    }

    #[test]
    fn the_socket_needs_the_key_and_the_page_is_served() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let get = |path: &str| {
            let mut c = TcpStream::connect(addr).unwrap();
            write!(c, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let (s, _) = listener.accept().unwrap();
            let _ = serve(s, &"a".repeat(64), &RemoteLink::new(), &AtomicU64::new(0));
            let mut out = String::new();
            let _ = c.read_to_string(&mut out);
            out
        };
        // The live connection needs the key …
        assert!(get("/ws").starts_with("HTTP/1.1 403"));
        assert!(get(&format!("/ws?k={}", "b".repeat(64))).starts_with("HTTP/1.1 403"));
        // … the page's files don't (the script and wasm load without it).
        let ok = get(&format!("/?k={}", "b".repeat(64)));
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("text/html"));
        assert!(get("/missing").starts_with("HTTP/1.1 404"));
    }
}
