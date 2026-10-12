//! Remote Canvas: draw in the running app from a tablet's browser (an iPad with Apple Pencil).
//!
//! The desktop app serves a small page on the local network (`apps/photocraft-remote`, served by
//! `apps/photocraft/src/remote_server.rs`). The page shows this window and sends the pen, touch
//! and gesture input back. This module is the platform-free half: a [`RemoteLink`] the server
//! fills with [`RemoteInput`] and drains [`RemoteFrame`]s from, and the shell code that turns the
//! input into ordinary egui events plus [`StylusFeed`](crate::stylus::StylusFeed) samples, so the
//! remote pen goes through exactly the path a local tablet does (menus, panels, every tool).
//!
//! Frames are window captures (`ViewportCommand::Screenshot`) taken while a client is connected:
//! about 30 per second while input arrives or the picture changes, slower when idle.

use crate::stylus::PenSample;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

/// Most input events kept between two frames (a stalled UI drops the oldest).
const INBOX_LIMIT: usize = 4096;
/// Capture interval while active / idle, in seconds.
const ACTIVE_INTERVAL: f64 = 1.0 / 30.0;
const IDLE_INTERVAL: f64 = 0.25;

/// The menu commands the remote page may run: its gesture and toolbar buttons. Anything else
/// (saving, closing, quitting) stays at the computer.
pub const MENU_ALLOWLIST: &[&str] = &[
    "edit.undo",
    "edit.redo",
    "view.zoomIn",
    "view.zoomOut",
    "view.fitOnScreen",
    "view.actualPixels",
    "view.resetView",
    "view.screenMode.standard",
    "view.screenMode.fullScreenWithMenuBar",
    "view.screenMode.fullScreen",
];

/// Contact phase of a pen or finger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
}

/// One input from the remote page. Positions are in the pixels of the last [`RemoteFrame`]
/// (physical window pixels).
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteInput {
    /// The stylus touching (or lifting from) the screen.
    Pen { phase: Phase, x: f32, y: f32, sample: PenSample },
    /// The stylus hovering above the screen (moves the brush cursor).
    Hover { x: f32, y: f32 },
    /// A finger used as the mouse (taps on menus and buttons).
    Pointer { phase: Phase, x: f32, y: f32 },
    /// Scroll (two-finger drag), in pixels; positive moves the content right/down.
    Scroll { x: f32, y: f32, dx: f32, dy: f32 },
    /// Pinch zoom factor this event (1 = none).
    Zoom { x: f32, y: f32, factor: f32 },
    /// A held finger picking the foreground colour at `x, y`, with the Eyedropper's Sample Size
    /// and Sample settings (as Procreate's touch-and-hold).
    Pick { phase: Phase, x: f32, y: f32 },
    /// Turn the canvas view by `degrees` (clockwise positive) about `x, y`.
    Rotate { x: f32, y: f32, degrees: f32 },
    /// The rotating fingers lifted at `x, y`: snap a view near a right angle onto it.
    RotateEnd { x: f32, y: f32 },
    /// Run a menu command by id (`edit.undo`, `view.screenMode.fullScreen`, …).
    Menu(String),
    /// The pointer left the page.
    Leave,
}

/// One captured window image for the remote page.
#[derive(Clone, Debug)]
pub struct RemoteFrame {
    pub image: Arc<egui::ColorImage>,
    pub pixels_per_point: f32,
}

/// The canvas cursor's outline: the page draws it under a hovering pen (an OS cursor never shows
/// in the window capture). A circle and a brush tip's rings are in window pixels, matching the
/// brush on the canvas; a crosshair is in UI points, a marker the page draws at its own size.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteCursor {
    Circle {
        radius: f32,
        centre: bool,
    },
    Crosshair {
        length: f32,
        gap: f32,
    },
    /// Closed rings of `[x, y]` offsets from the hotspot.
    Rings {
        rings: Vec<Vec<[f32; 2]>>,
        centre: bool,
    },
}

/// Most rings, and points per ring, sent for a brush tip outline (it is a hover hint; a detailed
/// tip is thinned to keep the message small).
const MAX_RINGS: usize = 16;
const MAX_RING_POINTS: usize = 128;

impl RemoteCursor {
    fn from_outline(o: crate::tool_cursor::Outline, ppp: f32) -> Option<Self> {
        use crate::tool_cursor::Outline;
        let c = match o {
            Outline::Circle { radius, centre } => Self::Circle { radius: radius * ppp, centre },
            Outline::Crosshair { length, gap } => Self::Crosshair { length, gap },
            Outline::Rings { rings, centre } => {
                let rings: Vec<Vec<[f32; 2]>> = rings
                    .iter()
                    .filter(|r| r.len() >= 2)
                    .take(MAX_RINGS)
                    .map(|r| {
                        let step = r.len().div_ceil(MAX_RING_POINTS);
                        r.iter().step_by(step).map(|p| [p.x * ppp, p.y * ppp]).collect()
                    })
                    .collect();
                Self::Rings { rings, centre }
            }
        };
        let ok = match &c {
            Self::Circle { radius, .. } => radius.is_finite() && *radius > 0.0,
            Self::Crosshair { length, gap } => length.is_finite() && gap.is_finite() && *gap >= 0.0 && length >= gap,
            Self::Rings { rings, .. } => !rings.is_empty() && rings.iter().flatten().flatten().all(|v| v.is_finite()),
        };
        ok.then_some(c)
    }
}

#[derive(Default)]
struct Shared {
    inbox: Mutex<Vec<RemoteInput>>,
    connected: AtomicBool,
    idle: AtomicBool,
    /// The newest capture not yet taken by the server (an older one is replaced).
    frame: Mutex<Option<RemoteFrame>>,
    frame_ready: Condvar,
    cursor: Mutex<Option<RemoteCursor>>,
    foreground: Mutex<Option<[u8; 3]>>,
    wake: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

/// The connection between the shell and the remote server. Cloning shares it.
#[derive(Clone, Default)]
pub struct RemoteLink(Arc<Shared>);

impl std::fmt::Debug for RemoteLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteLink").field("connected", &self.connected()).finish()
    }
}

/// Marks the screenshots taken for the remote page (automation's are `u64` tokens).
#[derive(Clone, Copy, Debug)]
struct RemoteShot;

impl RemoteLink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called (from any thread) when input arrives: the shell wakes the UI here.
    pub fn set_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.0.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(wake));
    }

    fn wake(&self) {
        if let Some(w) = self.0.wake.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            w();
        }
    }

    /// Queue input from the page (server thread).
    pub fn push(&self, input: RemoteInput) {
        {
            let mut q = self.0.inbox.lock().unwrap_or_else(PoisonError::into_inner);
            if q.len() >= INBOX_LIMIT {
                q.remove(0);
            }
            q.push(input);
        }
        self.0.idle.store(false, Ordering::Relaxed);
        self.wake();
    }

    pub fn take_inputs(&self) -> Vec<RemoteInput> {
        std::mem::take(&mut *self.0.inbox.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// A page is connected (server thread sets it); frames are captured only then.
    pub fn set_connected(&self, on: bool) {
        self.0.connected.store(on, Ordering::Relaxed);
        self.0.idle.store(false, Ordering::Relaxed);
        self.wake();
    }

    /// The canvas cursor's outline now (set by the shell each frame, sent to the page by the server).
    pub fn set_cursor(&self, cursor: Option<RemoteCursor>) {
        *self.0.cursor.lock().unwrap_or_else(PoisonError::into_inner) = cursor;
    }

    pub fn cursor(&self) -> Option<RemoteCursor> {
        self.0.cursor.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The foreground colour (8-bit display RGB), for the page's colour-picking ring.
    pub fn set_foreground(&self, rgb: Option<[u8; 3]>) {
        *self.0.foreground.lock().unwrap_or_else(PoisonError::into_inner) = rgb;
    }

    pub fn foreground(&self) -> Option<[u8; 3]> {
        *self.0.foreground.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn connected(&self) -> bool {
        self.0.connected.load(Ordering::Relaxed)
    }

    /// The server saw several unchanged frames in a row: capture less often until input or a
    /// change arrives.
    pub fn set_idle(&self, idle: bool) {
        self.0.idle.store(idle, Ordering::Relaxed);
    }

    fn interval(&self) -> f64 {
        if self.0.idle.load(Ordering::Relaxed) { IDLE_INTERVAL } else { ACTIVE_INTERVAL }
    }

    fn send_frame(&self, frame: RemoteFrame) {
        *self.0.frame.lock().unwrap_or_else(PoisonError::into_inner) = Some(frame);
        self.0.frame_ready.notify_all();
    }

    /// The newest capture, waiting up to `timeout` for one (server thread). Only the newest
    /// waits: a slow connection skips frames rather than falling behind.
    pub fn wait_frame(&self, timeout: Duration) -> Option<RemoteFrame> {
        let mut slot = self.0.frame.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            slot = self.0.frame_ready.wait_timeout(slot, timeout).unwrap_or_else(PoisonError::into_inner).0;
        }
        slot.take()
    }
}

/// A QR code: `size × size` modules, row by row, `true` = dark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qr {
    pub size: usize,
    pub modules: Vec<bool>,
}

/// A running Remote Canvas server, from [`RemoteServerFn`].
pub struct Served {
    /// Addresses the tablet can open (the pairing key included).
    pub urls: Vec<String>,
    /// The first address as a QR code, for the tablet's camera.
    pub qr: Option<Qr>,
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl Served {
    pub fn new(urls: Vec<String>, qr: Option<Qr>, stop: impl FnOnce() + Send + 'static) -> Self {
        Self { urls, qr, stop: Some(Box::new(stop)) }
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

impl std::fmt::Debug for Served {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Served").field("urls", &self.urls).finish_non_exhaustive()
    }
}

/// Start the Remote Canvas server for a link (the desktop app's; `None` on the web, where a page
/// can't listen for connections).
pub type RemoteServerFn = Box<dyn FnMut(&RemoteLink) -> Result<Served, String>>;

/// Shell state for the remote link (see `PhotocraftApp::remote`).
#[derive(Debug)]
pub struct RemoteState {
    /// Set while Remote Canvas is on.
    pub link: Option<RemoteLink>,
    /// The running server (dropping it stops it).
    pub served: Option<Served>,
    /// Window › Remote Canvas is showing.
    pub window: bool,
    /// Why the last attempt to turn it on failed.
    pub error: Option<String>,
    /// `ctx.input(|i| i.time)` of the last capture request.
    last_capture: f64,
    /// A capture was requested and its image hasn't arrived yet.
    in_flight: bool,
}

impl Default for RemoteState {
    fn default() -> Self {
        Self { link: None, served: None, window: false, error: None, last_capture: f64::NEG_INFINITY, in_flight: false }
    }
}

/// The Window menu item.
pub const MENU_ID: &str = "window.remoteCanvas";

/// Turn Remote Canvas on (no-op when it already is). The error is also kept for the window.
pub fn turn_on(app: &mut crate::PhotocraftApp, ctx: &egui::Context) -> Result<(), String> {
    if app.remote.served.is_some() {
        return Ok(());
    }
    let Some(start) = app.services.remote_server.as_mut() else {
        let e = "Remote Canvas needs the desktop app.".to_string();
        app.remote.error = Some(e.clone());
        return Err(e);
    };
    let link = RemoteLink::new();
    let wake = ctx.clone();
    link.set_wake(move || wake.request_repaint());
    match start(&link) {
        Ok(served) => {
            app.remote.link = Some(link);
            app.remote.served = Some(served);
            app.remote.error = None;
            app.remote.in_flight = false;
            Ok(())
        }
        Err(e) => {
            app.remote.error = Some(e.clone());
            Err(e)
        }
    }
}

/// Turn Remote Canvas off: the server stops and a connected tablet is let go.
pub fn turn_off(app: &mut crate::PhotocraftApp) {
    if let Some(link) = app.remote.link.take() {
        link.set_connected(false);
    }
    app.remote.served = None;
    app.remote.in_flight = false;
}

/// Window › Remote Canvas…: show the window (turning Remote Canvas on, so the QR code is there).
pub(crate) fn menu(app: &mut crate::PhotocraftApp, ctx: &egui::Context, id: &str) -> Option<Result<serde_json::Value, String>> {
    if id != MENU_ID {
        return None;
    }
    app.remote.window = true;
    let _ = turn_on(app, ctx);
    Some(Ok(serde_json::json!({"on": app.remote.served.is_some(), "urls": app.remote.served.as_ref().map(|s| s.urls.clone())})))
}

/// The Remote Canvas window: on/off, the QR code and address, and whether a tablet is connected.
pub(crate) fn window(app: &mut crate::PhotocraftApp, ctx: &egui::Context) {
    if !app.remote.window {
        return;
    }
    let t = crate::theme::Tokens::get(ctx);
    let mut open = true;
    let mut switch = None;
    egui::Window::new("Remote Canvas").id(egui::Id::new("remote-canvas")).open(&mut open).collapsible(false).resizable(false).default_width(300.0).show(
        ctx,
        |ui| {
            ui.set_width(300.0);
            match (&app.remote.served, &app.remote.link) {
                (Some(served), Some(link)) => {
                    let (status, color) = if link.connected() { ("● Tablet connected", t.accent) } else { ("Waiting for your tablet or phone…", t.text_dim) };
                    ui.label(egui::RichText::new(status).color(color).strong());
                    ui.add_space(6.0);
                    if let Some(qr) = &served.qr {
                        ui.vertical_centered(|ui| draw_qr(ui, qr, 220.0));
                        ui.add_space(6.0);
                    }
                    ui.add(egui::Label::new("Scan this with the tablet's or phone's camera, or open the address in its browser. Then add it to the home screen (Safari: Share › Add to Home Screen; Chrome: menu › Add to Home screen).").wrap());
                    ui.add_space(4.0);
                    for url in &served.urls {
                        ui.horizontal(|ui| {
                            if ui.small_button("Copy").on_hover_text("Copy the address").clicked() {
                                ui.ctx().copy_text(url.clone());
                            }
                            ui.add(egui::Label::new(egui::RichText::new(url).monospace().small().color(t.text_dim)).truncate());
                        });
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("Not connecting? Both need the same Wi-Fi, and the firewall must let PhotoCraft in (Windows asks the first time; allow Private networks).")
                                .small()
                                .color(t.text_faint),
                        )
                        .wrap(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Pen draws (the page's finger button lets a finger draw too) • one finger pans • pinch zooms and turns • hold a finger to pick a colour • two-finger tap undoes")
                            .small()
                            .color(t.text_faint),
                    );
                    ui.add_space(8.0);
                    if ui.button("Turn Off").clicked() {
                        switch = Some(false);
                    }
                }
                _ => {
                    ui.add(egui::Label::new("Draw in PhotoCraft from a tablet or phone (iPad, Android, Windows) with a pen or a finger, over your Wi-Fi.").wrap());
                    if let Some(e) = &app.remote.error {
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(egui::RichText::new(e).color(t.warning)).wrap());
                    }
                    ui.add_space(8.0);
                    if ui.button("Turn On").clicked() {
                        switch = Some(true);
                    }
                }
            }
        },
    );
    match switch {
        Some(true) => {
            let _ = turn_on(app, ctx);
        }
        Some(false) => turn_off(app),
        None => {}
    }
    app.remote.window = open;
    // The connected status follows the tablet without other input.
    ctx.request_repaint_after(Duration::from_millis(500));
}

/// Paint `qr` as a `side`-point square with its quiet zone (dark modules on white, as scanners
/// expect in any theme).
fn draw_qr(ui: &mut egui::Ui, qr: &Qr, side: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::WHITE);
    if qr.size == 0 || qr.modules.len() != qr.size.saturating_mul(qr.size) {
        return;
    }
    let quiet = 4.0;
    let cell = side / (qr.size as f32 + 2.0 * quiet);
    let origin = rect.min + egui::vec2(cell * quiet, cell * quiet);
    for (i, dark) in qr.modules.iter().enumerate() {
        if *dark {
            let (x, y) = ((i % qr.size) as f32, (i / qr.size) as f32);
            // Each module rounds out to whole pixels so neighbours meet without seams.
            let min = origin + egui::vec2(x * cell, y * cell);
            let r = egui::Rect::from_min_size(min, egui::vec2(cell, cell)).expand(0.25);
            painter.rect_filled(r, 0.0, egui::Color32::BLACK);
        }
    }
}

/// Convert queued remote input into this frame's raw input (from `raw_input_hook`). Returns
/// the menu commands to run.
pub(crate) fn inject(app: &mut crate::PhotocraftApp, ctx: &egui::Context, raw: &mut egui::RawInput) -> Vec<String> {
    let Some(link) = app.remote.link.clone() else { return Vec::new() };
    let inputs = link.take_inputs();
    if inputs.is_empty() {
        return Vec::new();
    }
    let ppp = raw.viewports.get(&raw.viewport_id).and_then(|v| v.native_pixels_per_point).unwrap_or_else(|| ctx.pixels_per_point());
    let ppp = if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    for op in view_ops(&inputs, ppp) {
        apply_view_op(app, op);
    }
    if let Some(p) = last_pick(&inputs, ppp) {
        pick(app, p);
    }
    let (events, menus) = to_events(inputs, ppp, ctx.input(|i| i.modifiers), |s| app.stylus.feed.set(s));
    raw.events.extend(events);
    menus
}

/// A rotation lifted within this many degrees of a right angle snaps onto it, as in Procreate.
pub const SNAP_DEG: f32 = 10.0;

/// A change to the active view's angle about a pivot (window points).
#[derive(Clone, Copy, Debug, PartialEq)]
enum ViewOp {
    Rotate { pivot: egui::Pos2, degrees: f32 },
    Snap { pivot: egui::Pos2 },
}

/// The rotation inputs as view changes, pivots in points.
fn view_ops(inputs: &[RemoteInput], ppp: f32) -> Vec<ViewOp> {
    let pos = |x: f32, y: f32| egui::pos2(if x.is_finite() { x / ppp } else { 0.0 }, if y.is_finite() { y / ppp } else { 0.0 });
    inputs
        .iter()
        .filter_map(|i| match *i {
            RemoteInput::Rotate { x, y, degrees } if degrees.is_finite() && degrees != 0.0 => {
                // A single step past a half turn is no finger gesture.
                Some(ViewOp::Rotate { pivot: pos(x, y), degrees: degrees.clamp(-180.0, 180.0) })
            }
            RemoteInput::RotateEnd { x, y } => Some(ViewOp::Snap { pivot: pos(x, y) }),
            _ => None,
        })
        .collect()
}

/// Where this batch's colour picking ends up (points): only the last sample matters, as each
/// one replaces the foreground colour.
fn last_pick(inputs: &[RemoteInput], ppp: f32) -> Option<egui::Pos2> {
    inputs.iter().rev().find_map(|i| match *i {
        RemoteInput::Pick { x, y, .. } if x.is_finite() && y.is_finite() => Some(egui::pos2(x / ppp, y / ppp)),
        _ => None,
    })
}

/// Set the foreground colour to what the Eyedropper would pick at `p` (points) on the canvas.
/// Off the canvas, off the image or over transparency, it stays.
fn pick(app: &mut crate::PhotocraftApp, p: egui::Pos2) {
    let Some(xf) = crate::canvas::ViewXform::active(app) else { return };
    if !xf.rect.contains(p) {
        return;
    }
    let [x, y] = xf.to_doc(p);
    if let Some([r, g, b]) = crate::canvas::eyedropper_color(app, x, y) {
        let _ = app.run("tools.setColors", serde_json::json!({"foreground": [r, g, b, 1.0]}));
    }
}

/// The angle a lifted rotation settles at: the nearest right angle when within [`SNAP_DEG`].
pub fn snapped(rotation: f32) -> f32 {
    let target = (rotation / 90.0).round() * 90.0;
    let r = if (rotation - target).abs() <= SNAP_DEG { target } else { rotation };
    crate::rotate_view::wrap_deg(r)
}

/// Turn the active document's view to a new angle about `pivot`, keeping the document point
/// under the pivot where it is on screen (the canvas turns under the fingers like paper).
fn apply_view_op(app: &mut crate::PhotocraftApp, op: ViewOp) {
    let Some(xf) = crate::canvas::ViewXform::active(app) else { return };
    let (pivot, rotation) = match op {
        ViewOp::Rotate { pivot, degrees } => (pivot, crate::rotate_view::wrap_deg(xf.rotation + degrees)),
        ViewOp::Snap { pivot } => (pivot, snapped(xf.rotation)),
    };
    if rotation == xf.rotation || !xf.zoom.is_finite() || xf.zoom <= 0.0 {
        return;
    }
    let Some(center) = recentre(xf, pivot, rotation) else { return };
    let Some(view) = app.session.active_index().and_then(|i| app.ui.views.get_mut(i)) else { return };
    view.rotation = rotation;
    view.center = center;
}

/// The view centre that keeps the document point under `pivot` in place when `xf` turns to
/// `rotation` (`None` if the numbers stop being finite).
fn recentre(xf: crate::canvas::ViewXform, pivot: egui::Pos2, rotation: f32) -> Option<[f32; 2]> {
    let [px, py] = xf.to_doc(pivot);
    let turned = crate::canvas::ViewXform { rotation, ..xf };
    let u = turned.unmap_vec((pivot - turned.rect.center()) / turned.zoom);
    let c = [px as f32 - u.x, py as f32 - u.y];
    c.iter().all(|v| v.is_finite()).then_some(c)
}

/// [`inject`]'s conversion, separate for tests: egui events (positions in points) and menu ids.
/// `feed` receives the pen samples in order (`None` = no pen down).
fn to_events(inputs: Vec<RemoteInput>, ppp: f32, modifiers: egui::Modifiers, mut feed: impl FnMut(Option<PenSample>)) -> (Vec<egui::Event>, Vec<String>) {
    let pos = |x: f32, y: f32| {
        let f = |v: f32| if v.is_finite() { v / ppp } else { 0.0 };
        egui::pos2(f(x), f(y))
    };
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers };
    let mut events = Vec::new();
    let mut menus = Vec::new();
    for input in inputs {
        match input {
            RemoteInput::Pen { phase, x, y, sample } => {
                let p = pos(x, y);
                match phase {
                    Phase::Down | Phase::Move => {
                        feed(Some(sample));
                        events.push(egui::Event::PointerMoved(p));
                        if phase == Phase::Down {
                            events.push(button(p, true));
                        }
                    }
                    // The lift is no new point: the stroke ends where the last move put it. The
                    // pen stays "a pen at pressure 0" (as tablets report a lift), never "no pen":
                    // a release arriving alone in a frame would otherwise read as a mouse and
                    // end the stroke with a full-pressure blob.
                    Phase::Up => {
                        feed(Some(PenSample { pressure: 0.0, ..sample }));
                        events.push(button(p, false));
                    }
                }
            }
            RemoteInput::Hover { x, y } => events.push(egui::Event::PointerMoved(pos(x, y))),
            RemoteInput::Pointer { phase, x, y } => {
                let p = pos(x, y);
                feed(None);
                events.push(egui::Event::PointerMoved(p));
                match phase {
                    Phase::Down => events.push(button(p, true)),
                    Phase::Move => {}
                    Phase::Up => events.push(button(p, false)),
                }
            }
            RemoteInput::Scroll { x, y, dx, dy } => {
                events.push(egui::Event::PointerMoved(pos(x, y)));
                let d = |v: f32| if v.is_finite() { (v / ppp).clamp(-10_000.0, 10_000.0) } else { 0.0 };
                events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(d(dx), d(dy)),
                    phase: egui::TouchPhase::Move,
                    modifiers,
                });
            }
            RemoteInput::Zoom { x, y, factor } => {
                if factor.is_finite() && factor > 0.0 {
                    events.push(egui::Event::PointerMoved(pos(x, y)));
                    events.push(egui::Event::Zoom(factor.clamp(0.1, 10.0)));
                }
            }
            // View changes and colour picks: applied by `inject` (`view_ops`, `pick`).
            RemoteInput::Rotate { .. } | RemoteInput::RotateEnd { .. } | RemoteInput::Pick { .. } => {}
            RemoteInput::Menu(id) => {
                if MENU_ALLOWLIST.contains(&id.as_str()) {
                    menus.push(id);
                }
            }
            RemoteInput::Leave => events.push(egui::Event::PointerGone),
        }
    }
    (events, menus)
}

/// Request a capture when one is due, and hand finished captures to the server (from `logic`).
pub(crate) fn tick(app: &mut crate::PhotocraftApp, ctx: &egui::Context) {
    let Some(link) = app.remote.link.clone() else { return };
    let ppp = ctx.pixels_per_point();
    let shots: Vec<Arc<egui::ColorImage>> = ctx.input(|i| {
        i.raw
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Screenshot { user_data, image, .. } if user_data.data.as_ref().is_some_and(|d| d.is::<RemoteShot>()) => Some(image.clone()),
                _ => None,
            })
            .collect()
    });
    if let Some(image) = shots.into_iter().last() {
        app.remote.in_flight = false;
        link.send_frame(RemoteFrame { image, pixels_per_point: ppp });
    }
    if !link.connected() {
        app.remote.in_flight = false;
        return;
    }
    link.set_cursor(crate::tool_cursor::last_outline(ctx).and_then(|o| RemoteCursor::from_outline(o, ppp)));
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let [r, g, b, _] = app.session.tools.foreground;
    link.set_foreground(Some([byte(r), byte(g), byte(b)]));
    let now = ctx.input(|i| i.time);
    let interval = link.interval();
    // A capture lost on the way (window hidden, device lost) is retried after a second.
    let stale = now - app.remote.last_capture > 1.0;
    if (!app.remote.in_flight || stale) && now - app.remote.last_capture >= interval {
        app.remote.last_capture = now;
        app.remote.in_flight = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(RemoteShot)));
    }
    ctx.request_repaint_after(Duration::from_secs_f64(interval));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pen(phase: Phase, x: f32, y: f32, pressure: f32) -> RemoteInput {
        RemoteInput::Pen { phase, x, y, sample: PenSample { pressure, ..Default::default() } }
    }

    #[test]
    fn a_held_finger_picks_the_foreground_from_the_canvas() {
        let mut app = crate::PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", serde_json::json!({"width": 40, "height": 20, "background": "white"})).unwrap();
        app.run("edit.fill", serde_json::json!({"color": "#ff0000"})).unwrap();
        app.last_canvas_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 200.0));
        app.run("tools.setColors", serde_json::json!({"foreground": [0.0, 0.0, 1.0, 1.0]})).unwrap();
        let centre = crate::canvas::ViewXform::active(&app).unwrap().to_screen(20.0, 10.0);
        // Only the batch's last sample is applied; positions arrive in window pixels.
        let inputs =
            vec![RemoteInput::Pick { phase: Phase::Down, x: -50.0, y: -50.0 }, RemoteInput::Pick { phase: Phase::Move, x: centre.x * 2.0, y: centre.y * 2.0 }];
        assert_eq!(last_pick(&inputs, 2.0), Some(centre));
        pick(&mut app, centre);
        let fg = app.session.tools.foreground;
        assert!(fg[0] > 0.99 && fg[1] < 0.01 && fg[2] < 0.01, "picked {fg:?}");
        // Off the canvas: unchanged.
        pick(&mut app, egui::pos2(-10.0, -10.0));
        assert_eq!(app.session.tools.foreground, fg);
        assert_eq!(last_pick(&[RemoteInput::Pick { phase: Phase::Up, x: f32::NAN, y: 0.0 }], 1.0), None);
    }

    #[test]
    fn the_menu_item_is_live_where_there_is_a_server() {
        let session = photocraft_engine::Session::new();
        let mut app = crate::PhotocraftApp::new(session, crate::Services::default());
        // The web build has no server to start: greyed.
        assert!(!crate::menus::is_enabled(&app, MENU_ID));
        app.services.remote_server = Some(Box::new(|_| Ok(Served::new(vec!["http://x".into()], None, || {}))));
        assert!(crate::menus::is_enabled(&app, MENU_ID));
    }

    #[test]
    fn cursor_outlines_reach_the_page_in_its_units() {
        use crate::tool_cursor::Outline;
        // The brush ring is in window pixels; the crosshair keeps its UI size.
        assert_eq!(RemoteCursor::from_outline(Outline::Circle { radius: 10.0, centre: true }, 2.0), Some(RemoteCursor::Circle { radius: 20.0, centre: true }));
        assert_eq!(RemoteCursor::from_outline(Outline::Crosshair { length: 6.0, gap: 0.0 }, 2.0), Some(RemoteCursor::Crosshair { length: 6.0, gap: 0.0 }));
        assert_eq!(RemoteCursor::from_outline(Outline::Circle { radius: f32::NAN, centre: false }, 2.0), None);
        assert_eq!(RemoteCursor::from_outline(Outline::Crosshair { length: 2.0, gap: 3.0 }, 1.0), None);
    }

    #[test]
    fn the_canvas_cursor_is_remembered_for_one_pass() {
        let ctx = egui::Context::default();
        let run = |show: bool| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                if show {
                    let _ = crate::tool_cursor::circle(ui.painter(), egui::pos2(5.0, 5.0), 8.0, false);
                }
            });
            out.textures_delta.clear();
        };
        run(true);
        assert_eq!(crate::tool_cursor::last_outline(&ctx), Some(crate::tool_cursor::Outline::Circle { radius: 8.0, centre: false }));
        run(false);
        run(false);
        assert_eq!(crate::tool_cursor::last_outline(&ctx), None, "a cursor no longer shown is forgotten");
    }

    #[test]
    fn pen_stroke_becomes_pointer_events_and_samples() {
        let inputs = vec![pen(Phase::Down, 20.0, 40.0, 0.2), pen(Phase::Move, 30.0, 40.0, 0.6), pen(Phase::Up, 40.0, 40.0, 0.7)];
        let mut samples = Vec::new();
        let (events, menus) = to_events(inputs, 2.0, egui::Modifiers::NONE, |s| samples.push(s.map(|s| s.pressure)));
        assert!(menus.is_empty());
        // The lift leaves a pen at pressure 0, not "no pen" (which would paint like a mouse).
        assert_eq!(samples, vec![Some(0.2), Some(0.6), Some(0.0)]);
        assert_eq!(events[0], egui::Event::PointerMoved(egui::pos2(10.0, 20.0)));
        assert!(matches!(events[1], egui::Event::PointerButton { pressed: true, .. }));
        assert_eq!(events[2], egui::Event::PointerMoved(egui::pos2(15.0, 20.0)));
        // The release adds no point of its own.
        assert_eq!(events.len(), 4);
        assert!(matches!(events[3], egui::Event::PointerButton { pressed: false, .. }));
    }

    /// The end-of-stroke blob: a lift arriving alone in a frame must not read as a mouse
    /// (pressure 1) when the canvas places the stroke's final point.
    #[test]
    fn a_lone_lift_paints_no_full_pressure_blob() {
        let mut stylus = crate::stylus::Stylus::default();
        let feed = stylus.feed.clone();
        let (events, _) = to_events(vec![pen(Phase::Down, 0.0, 0.0, 0.3), pen(Phase::Move, 5.0, 0.0, 0.4)], 1.0, egui::Modifiers::NONE, |s| feed.set(s));
        stylus.update(&events);
        assert!((stylus.pressure() - 0.4).abs() < 1e-6);
        // Next frame: only the lift (Safari reports pressure 0 on pointerup).
        let (events, _) = to_events(vec![pen(Phase::Up, 5.0, 0.0, 0.0)], 1.0, egui::Modifiers::NONE, |s| feed.set(s));
        stylus.update(&events);
        assert!(stylus.pressure() < 0.5, "final point pressure {} (a mouse's 1.0 is the blob)", stylus.pressure());
    }

    fn xf(rotation: f32, flip: bool) -> crate::canvas::ViewXform {
        crate::canvas::ViewXform {
            rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0)),
            zoom: 0.5,
            center: [1000.0, 700.0],
            flip,
            rotation,
            aspect: 1.0,
        }
    }

    #[test]
    fn rotating_keeps_the_point_under_the_fingers() {
        for flip in [false, true] {
            for (from, to) in [(0.0, 37.0), (37.0, 90.0), (-170.0, 175.0)] {
                let before = xf(from, flip);
                let pivot = egui::pos2(620.0, 140.0);
                let doc = before.to_doc(pivot);
                let center = recentre(before, pivot, to).unwrap();
                let after = crate::canvas::ViewXform { rotation: to, center, ..before };
                let back = after.to_screen(doc[0] as f32, doc[1] as f32);
                assert!((back - pivot).length() < 1e-2, "flip {flip} {from}→{to}: {back:?} vs {pivot:?}");
            }
        }
    }

    #[test]
    fn lifting_snaps_near_right_angles_only() {
        assert_eq!(snapped(4.0), 0.0);
        assert_eq!(snapped(-9.5), 0.0);
        assert_eq!(snapped(83.0), 90.0);
        assert_eq!(snapped(-95.0), -90.0);
        assert_eq!(snapped(176.0), 180.0);
        assert_eq!(snapped(-178.0), 180.0);
        assert_eq!(snapped(45.0), 45.0);
        assert_eq!(snapped(25.0), 25.0);
        assert_eq!(snapped(f32::NAN), 0.0);
    }

    #[test]
    fn rotation_inputs_become_view_ops() {
        let inputs = vec![
            RemoteInput::Rotate { x: 200.0, y: 100.0, degrees: 12.0 },
            RemoteInput::Rotate { x: 0.0, y: 0.0, degrees: f32::NAN },
            RemoteInput::Rotate { x: 0.0, y: 0.0, degrees: 9999.0 },
            RemoteInput::RotateEnd { x: 200.0, y: 100.0 },
        ];
        let ops = view_ops(&inputs, 2.0);
        assert_eq!(
            ops,
            vec![
                ViewOp::Rotate { pivot: egui::pos2(100.0, 50.0), degrees: 12.0 },
                ViewOp::Rotate { pivot: egui::pos2(0.0, 0.0), degrees: 180.0 },
                ViewOp::Snap { pivot: egui::pos2(100.0, 50.0) },
            ]
        );
        // They add no egui events.
        let (events, _) = to_events(inputs, 2.0, egui::Modifiers::NONE, |_| {});
        assert!(events.is_empty());
    }

    #[test]
    fn hostile_numbers_never_escape() {
        let inputs = vec![
            pen(Phase::Down, f32::NAN, f32::INFINITY, f32::NAN),
            RemoteInput::Zoom { x: 0.0, y: 0.0, factor: f32::NAN },
            RemoteInput::Zoom { x: 0.0, y: 0.0, factor: 1e30 },
            RemoteInput::Scroll { x: 0.0, y: 0.0, dx: f32::INFINITY, dy: -1e30 },
        ];
        let (events, _) = to_events(inputs, 1.0, egui::Modifiers::NONE, |_| {});
        for e in &events {
            match e {
                egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. } => assert!(p.x.is_finite() && p.y.is_finite()),
                egui::Event::Zoom(f) => assert!(f.is_finite() && *f <= 10.0),
                egui::Event::MouseWheel { delta, .. } => assert!(delta.x.is_finite() && delta.y.abs() <= 10_000.0),
                _ => {}
            }
        }
        assert_eq!(events.iter().filter(|e| matches!(e, egui::Event::Zoom(_))).count(), 1);
    }

    #[test]
    fn menus_are_returned_and_inbox_is_bounded() {
        let link = RemoteLink::new();
        for _ in 0..INBOX_LIMIT + 10 {
            link.push(RemoteInput::Leave);
        }
        link.push(RemoteInput::Menu("edit.undo".into()));
        link.push(RemoteInput::Menu("app.quit".into()));
        let inputs = link.take_inputs();
        assert_eq!(inputs.len(), INBOX_LIMIT);
        assert!(matches!(inputs.last(), Some(RemoteInput::Menu(id)) if id == "app.quit"));
        let (_, menus) = to_events(inputs, 1.0, egui::Modifiers::NONE, |_| {});
        assert_eq!(menus, vec!["edit.undo".to_string()]);
        assert!(link.take_inputs().is_empty());
    }

    #[test]
    fn only_the_newest_frame_waits() {
        let link = RemoteLink::new();
        let frame = |w| RemoteFrame { image: Arc::new(egui::ColorImage::filled([w, 1], egui::Color32::BLACK)), pixels_per_point: 1.0 };
        assert!(link.wait_frame(Duration::ZERO).is_none());
        link.send_frame(frame(1));
        link.send_frame(frame(2));
        assert_eq!(link.wait_frame(Duration::ZERO).map(|f| f.image.size), Some([2, 1]));
        assert!(link.wait_frame(Duration::from_millis(1)).is_none());
    }
}
