//! The page: draws the frames on a canvas and sends the Pencil, finger and toolbar input back.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlElement, MessageEvent, PointerEvent, WebSocket};

use crate::cursor::{self, Mark, Outline};
use crate::frame::Frame;
use crate::gesture::{Gesture, Gestures};
use crate::smooth::{Sample, Smoother};

/// Screen modes the mode button switches between: the whole window, or just the canvas.
const MODES: [(&str, &str); 2] = [("view.screenMode.standard", "Full window"), ("view.screenMode.fullScreen", "Canvas only")];

struct Page {
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    /// Drawn over the frames: the brush outline under a hovering Pencil.
    overlay: HtmlCanvasElement,
    overlay_ctx: CanvasRenderingContext2d,
    /// The canvas cursor's outline, as the desktop last reported it.
    outline: Option<Outline>,
    /// Where the Pencil hovers (page pixels), or `None` when it's touching or out of range.
    hover: Option<(f64, f64)>,
    /// A held finger picking a colour: the ring under it.
    picking: Option<Picking>,
    /// The desktop's foreground colour (the ring's lower half shows it as the pick began).
    foreground: Option<[u8; 3]>,
    status: HtmlElement,
    mode_button: Option<HtmlElement>,
    ws: Option<WebSocket>,
    key: String,
    width: u32,
    height: u32,
    gestures: Gestures,
    /// The Pencil is touching the screen (a stroke is in progress).
    pen_down: bool,
    /// The Pencil arrives as touch events (`touchType == "stylus"`): Safari reports it there at a
    /// higher rate than as pointer events, whose pen strokes come out as straight segments. Once
    /// seen, the pen's pointer events only move the hover cursor.
    touch_stylus: bool,
    smoother: Smoother,
    /// Where the last pen sample sent put the stroke (window pixels): the lift goes there.
    pen_at: (f64, f64),
    mode: usize,
    /// The socket has opened before: a later open is PhotoCraft back after a restart, and may
    /// be a newer build serving a newer page.
    was_open: bool,
}

/// The colour-picking ring: the colour under the finger above, the colour before below.
#[derive(Clone, Copy, Debug)]
struct Picking {
    at: (f64, f64),
    new: [u8; 3],
    old: [u8; 3],
}

/// The ring's radius and width, in page pixels: wide enough to show around a fingertip.
const RING_RADIUS: f64 = 52.0;
const RING_WIDTH: f64 = 18.0;

type Shared = Rc<RefCell<Page>>;

pub fn log(msg: &str) {
    web_sys::console::log_1(&msg.into());
}

fn now() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map_or(0.0, |p| p.now())
}

pub fn start() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let canvas: HtmlCanvasElement = document.get_element_by_id("screen").ok_or("no #screen")?.dyn_into()?;
    let ctx: CanvasRenderingContext2d = canvas.get_context("2d")?.ok_or("no 2d context")?.dyn_into()?;
    let overlay: HtmlCanvasElement = document.get_element_by_id("cursor").ok_or("no #cursor")?.dyn_into()?;
    let overlay_ctx: CanvasRenderingContext2d = overlay.get_context("2d")?.ok_or("no 2d context")?.dyn_into()?;
    let status: HtmlElement = document.get_element_by_id("status").ok_or("no #status")?.dyn_into()?;
    let mode_button = document.get_element_by_id("mode").and_then(|e| e.dyn_into().ok());
    let search = window.location().search().unwrap_or_default();
    let key = search.trim_start_matches('?').split('&').find_map(|kv| kv.strip_prefix("k=")).unwrap_or("").to_string();
    if key.is_empty() {
        status.set_text_content(Some("Open the address PhotoCraft shows: it includes the pairing key."));
        return Ok(());
    }
    let page: Shared = Rc::new(RefCell::new(Page {
        canvas: canvas.clone(),
        ctx,
        overlay,
        overlay_ctx,
        outline: None,
        hover: None,
        picking: None,
        foreground: None,
        status,
        mode_button,
        ws: None,
        key,
        width: 0,
        height: 0,
        gestures: Gestures::default(),
        pen_down: false,
        touch_stylus: false,
        smoother: Smoother::default(),
        pen_at: (0.0, 0.0),
        mode: 0,
        was_open: false,
    }));
    connect(&page);
    pointer_listeners(&page, &canvas)?;
    stylus_touch_listeners(&page, &canvas)?;
    toolbar_listeners(&page, &document)?;
    block_browser_gestures(&document)?;
    keep_awake(&document)?;
    Ok(())
}

fn connect(page: &Shared) {
    let Some(window) = web_sys::window() else { return };
    let host = window.location().host().unwrap_or_default();
    let url = format!("ws://{host}/ws?k={}", page.borrow().key);
    let ws = match WebSocket::new(&url) {
        Ok(ws) => ws,
        Err(e) => {
            log(&format!("{e:?}"));
            return reconnect_later(page);
        }
    };
    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
    let p = page.clone();
    let on_open = Closure::<dyn FnMut()>::new(move || {
        let mut pg = p.borrow_mut();
        if pg.was_open {
            // Reconnected: reload so this page is the one the running PhotoCraft serves, not the
            // code from before it restarted (an updated build would otherwise keep the old page).
            if let Some(w) = web_sys::window() {
                let _ = w.location().reload();
            }
            return;
        }
        pg.was_open = true;
        pg.status.set_text_content(Some(""));
    });
    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    on_open.forget();
    let p = page.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        let data = e.data();
        if let Some(text) = data.as_string() {
            let Ok(v) = serde_json::from_str::<Value>(&text) else { return };
            let mut pg = p.borrow_mut();
            if let Some(outline) = cursor::parse(&v) {
                pg.outline = outline;
                pg.draw_cursor();
            } else if v.get("t").and_then(Value::as_str) == Some("fg")
                && let Some(rgb) = v.get("rgb").and_then(|c| serde_json::from_value::<[u8; 3]>(c.clone()).ok())
            {
                pg.foreground = Some(rgb);
            }
        } else if let Ok(buf) = data.dyn_into::<js_sys::ArrayBuffer>() {
            let mut pg = p.borrow_mut();
            pg.draw(&js_sys::Uint8Array::new(&buf).to_vec());
            // The window may have changed size, moving where the outline belongs.
            pg.draw_cursor();
        }
    });
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();
    let p = page.clone();
    let on_close = Closure::<dyn FnMut()>::new(move || {
        p.borrow().status.set_text_content(Some("Connecting to PhotoCraft…"));
        p.borrow_mut().ws = None;
        reconnect_later(&p);
    });
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();
    page.borrow_mut().ws = Some(ws);
}

fn reconnect_later(page: &Shared) {
    let p = page.clone();
    let retry = Closure::once_into_js(move || connect(&p));
    if let Some(w) = web_sys::window() {
        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(retry.unchecked_ref(), 1000);
    }
}

impl Page {
    fn draw(&mut self, msg: &[u8]) {
        let frame = match Frame::decode(msg) {
            Ok(f) => f,
            Err(e) => return log(&e),
        };
        if (frame.width, frame.height) != (self.width, self.height) {
            self.width = frame.width;
            self.height = frame.height;
            self.canvas.set_width(frame.width);
            self.canvas.set_height(frame.height);
        }
        for t in frame.tiles() {
            match web_sys::ImageData::new_with_u8_clamped_array_and_sh(Clamped(t.rgba), t.w, t.h) {
                Ok(img) => {
                    let _ = self.ctx.put_image_data(&img, f64::from(t.x), f64::from(t.y));
                }
                Err(e) => log(&format!("{e:?}")),
            }
        }
    }

    fn send(&self, v: Value) {
        if let Some(ws) = self.ws.as_ref().filter(|ws| ws.ready_state() == WebSocket::OPEN) {
            let _ = ws.send_with_str(&v.to_string());
        }
    }

    /// Page pixels → window pixels (the canvas is letterboxed with `object-fit: contain`), and
    /// window pixels per page pixel.
    fn mapping(&self) -> Option<(f64, f64, f64)> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let r = self.canvas.get_bounding_client_rect();
        let (w, h) = (f64::from(self.width), f64::from(self.height));
        let scale = (r.width() / w).min(r.height() / h);
        if !(scale.is_finite() && scale > 0.0) {
            return None;
        }
        let ox = r.left() + (r.width() - w * scale) / 2.0;
        let oy = r.top() + (r.height() - h * scale) / 2.0;
        Some((ox, oy, scale))
    }

    fn to_window(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (ox, oy, s) = self.mapping()?;
        Some(((x - ox) / s, (y - oy) / s))
    }

    /// A pen pointer event as a sample in window pixels.
    fn pointer_sample(&self, e: &PointerEvent) -> Option<Sample> {
        let (x, y) = client(e);
        let (x, y) = self.to_window(x, y)?;
        Some(Sample { x, y, pressure: f64::from(e.pressure()), tilt_x: f64::from(e.tilt_x()), tilt_y: f64::from(e.tilt_y()), twist: f64::from(e.twist()) })
    }

    /// A stylus `Touch` as a sample in window pixels: `force` is the pressure, and the Pencil's
    /// altitude and azimuth become Pointer Events tilt (degrees).
    fn touch_sample(&self, t: &JsValue) -> Option<Sample> {
        let num = |k: &str| js_sys::Reflect::get(t, &k.into()).ok().and_then(|v| v.as_f64()).filter(|v| v.is_finite());
        let (x, y) = self.to_window(num("clientX")?, num("clientY")?)?;
        let (tilt_x, tilt_y) = match (num("altitudeAngle"), num("azimuthAngle")) {
            (Some(alt), Some(az)) => {
                let tan = alt.clamp(1e-3, std::f64::consts::FRAC_PI_2).tan();
                (az.cos().atan2(tan).to_degrees(), az.sin().atan2(tan).to_degrees())
            }
            _ => (0.0, 0.0),
        };
        Some(Sample { x, y, pressure: num("force").unwrap_or(0.5).clamp(0.0, 1.0), tilt_x, tilt_y, twist: 0.0 })
    }

    fn pen_begin(&mut self, s: Sample) {
        if self.pen_down {
            return;
        }
        self.pen_down = true;
        self.set_hover(None);
        self.gestures.pen(true, now());
        let out = self.smoother.down(s);
        self.send_pen(&out, "d");
    }

    /// The Pencil hovers at `at` (page pixels), or stopped hovering.
    fn set_hover(&mut self, at: Option<(f64, f64)>) {
        if self.hover != at {
            self.hover = at;
            self.draw_cursor();
        }
    }

    /// Redraw the overlay: the outline under the hovering Pencil, or nothing.
    fn draw_cursor(&self) {
        let Some(w) = web_sys::window() else { return };
        let dpr = w.device_pixel_ratio();
        let dpr = if dpr.is_finite() && dpr > 0.0 { dpr.min(8.0) } else { 1.0 };
        // The overlay covers the page from its top-left corner, so its pixels are page pixels.
        let r = self.overlay.get_bounding_client_rect();
        let size = |v: f64| if v.is_finite() && v > 0.0 { v } else { 1.0 };
        let (cw, ch) = (size(r.width()), size(r.height()));
        let (pw, ph) = ((cw * dpr).round().min(16_384.0) as u32, (ch * dpr).round().min(16_384.0) as u32);
        if self.overlay.width() != pw || self.overlay.height() != ph {
            self.overlay.set_width(pw);
            self.overlay.set_height(ph);
        }
        let ctx = &self.overlay_ctx;
        let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        ctx.clear_rect(0.0, 0.0, f64::from(pw), f64::from(ph));
        if let Some(p) = self.picking {
            let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
            draw_ring(ctx, p);
            return;
        }
        let (Some(outline), Some((x, y)), Some((_, _, scale))) = (self.outline.as_ref(), self.hover, self.mapping()) else { return };
        let marks = cursor::marks(outline, x, y, scale);
        let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
        // A light line edged in dark, visible on any colour (as the desktop cursor).
        for (width, colour) in [(3.0, "rgba(0, 0, 0, 0.6)"), (1.0, "rgba(255, 255, 255, 0.95)")] {
            ctx.set_line_width(width);
            ctx.set_stroke_style_str(colour);
            ctx.begin_path();
            for m in &marks {
                match m {
                    Mark::Ring { x, y, r } => {
                        ctx.move_to(x + r, *y);
                        let _ = ctx.arc(*x, *y, *r, 0.0, std::f64::consts::TAU);
                    }
                    Mark::Line { x0, y0, x1, y1 } => {
                        ctx.move_to(*x0, *y0);
                        ctx.line_to(*x1, *y1);
                    }
                    Mark::Poly { points } => {
                        if let Some(((x0, y0), rest)) = points.split_first() {
                            ctx.move_to(*x0, *y0);
                            for (x, y) in rest {
                                ctx.line_to(*x, *y);
                            }
                            ctx.close_path();
                        }
                    }
                }
            }
            ctx.stroke();
        }
    }

    fn pen_move(&mut self, samples: &[Sample]) {
        if !self.pen_down {
            return;
        }
        let out: Vec<Sample> = samples.iter().flat_map(|s| self.smoother.push(*s)).collect();
        self.send_pen(&out, "m");
    }

    fn pen_end(&mut self) {
        if !self.pen_down {
            return;
        }
        self.pen_down = false;
        self.gestures.pen(false, now());
        let out = self.smoother.up();
        self.send_pen(&out, "m");
        // The lift, where the stroke ended (the desktop adds no point for it).
        let (x, y) = self.pen_at;
        self.send(json!({"t": "pen", "p": "u", "x": x, "y": y, "f": 0.0}));
    }

    /// Send smoothed samples: the first with `first_phase` (`d` starts the stroke), the rest as moves.
    fn send_pen(&mut self, out: &[Sample], first_phase: &str) {
        if let Some(last) = out.last() {
            self.pen_at = (last.x, last.y);
        }
        let msgs: Vec<Value> = out
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let phase = if i == 0 { first_phase } else { "m" };
                json!({"t": "pen", "p": phase, "x": s.x, "y": s.y, "f": s.pressure, "tx": s.tilt_x, "ty": s.tilt_y, "r": s.twist})
            })
            .collect();
        if !msgs.is_empty() {
            self.send(Value::Array(msgs));
        }
    }

    /// The colour the page shows at `x, y` (page pixels): the ring's instant preview, before
    /// the desktop's own pick (with the Eyedropper's settings) arrives in the next frame.
    fn screen_color(&self, x: f64, y: f64) -> Option<[u8; 3]> {
        let (wx, wy) = self.to_window(x, y)?;
        if !(wx >= 0.0 && wy >= 0.0 && wx < f64::from(self.width) && wy < f64::from(self.height)) {
            return None;
        }
        let data = self.ctx.get_image_data(wx.floor(), wy.floor(), 1.0, 1.0).ok()?.data();
        Some([*data.first()?, *data.get(1)?, *data.get(2)?])
    }

    /// One finger is down: check for a hold once it has lasted long enough.
    fn watch_hold(page: &Shared) {
        let p = page.clone();
        let check = Closure::once_into_js(move || {
            let mut pg = p.borrow_mut();
            let out = pg.gestures.tick(now());
            pg.gestures(out);
        });
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(check.unchecked_ref(), crate::gesture::HOLD_MS as i32 + 20);
        }
    }

    fn gestures(&mut self, out: Vec<Gesture>) {
        let Some((_, _, s)) = self.mapping() else { return };
        let mut msgs = Vec::new();
        for g in out {
            match g {
                Gesture::Scroll { x, y, dx, dy } => {
                    if let Some((x, y)) = self.to_window(x, y) {
                        msgs.push(json!({"t": "scroll", "x": x, "y": y, "dx": dx / s, "dy": dy / s}));
                    }
                }
                Gesture::Zoom { x, y, factor } => {
                    if let Some((x, y)) = self.to_window(x, y) {
                        msgs.push(json!({"t": "zoom", "x": x, "y": y, "f": factor}));
                    }
                }
                Gesture::Tap { x, y } => {
                    if let Some((x, y)) = self.to_window(x, y) {
                        msgs.push(json!({"t": "ptr", "p": "d", "x": x, "y": y}));
                        msgs.push(json!({"t": "ptr", "p": "u", "x": x, "y": y}));
                    }
                }
                Gesture::Rotate { x, y, degrees } => {
                    if let Some((x, y)) = self.to_window(x, y) {
                        msgs.push(json!({"t": "rotate", "x": x, "y": y, "d": degrees}));
                    }
                }
                Gesture::RotateEnd { x, y } => {
                    if let Some((x, y)) = self.to_window(x, y) {
                        msgs.push(json!({"t": "rotateEnd", "x": x, "y": y}));
                    }
                }
                Gesture::Pick { x, y } => {
                    let phase = if self.picking.is_some() { "m" } else { "d" };
                    let previous = self.picking.map(|p| p.new);
                    let new = self.screen_color(x, y).or(previous).or(self.foreground).unwrap_or([0, 0, 0]);
                    let old = self.picking.map(|p| p.old).or(self.foreground).unwrap_or(new);
                    self.picking = Some(Picking { at: (x, y), new, old });
                    self.draw_cursor();
                    if let Some((x, y)) = self.to_window(x, y) {
                        msgs.push(json!({"t": "pick", "p": phase, "x": x, "y": y}));
                    }
                }
                Gesture::PickEnd { x, y } => {
                    self.picking = None;
                    self.draw_cursor();
                    if let Some((x, y)) = self.to_window(x, y) {
                        msgs.push(json!({"t": "pick", "p": "u", "x": x, "y": y}));
                    }
                }
                Gesture::Undo => msgs.push(json!({"t": "menu", "id": "edit.undo"})),
                Gesture::Redo => msgs.push(json!({"t": "menu", "id": "edit.redo"})),
            }
        }
        if !msgs.is_empty() {
            self.send(Value::Array(msgs));
        }
    }
}

/// The colour-picking ring at the finger: the picked colour on the upper half, the colour before
/// on the lower, each edged in dark so they read on any image.
fn draw_ring(ctx: &CanvasRenderingContext2d, p: Picking) {
    use std::f64::consts::PI;
    let (x, y) = p.at;
    let css = |c: [u8; 3]| format!("rgb({}, {}, {})", c[0], c[1], c[2]);
    for (a0, a1, colour) in [(PI, 2.0 * PI, p.new), (0.0, PI, p.old)] {
        ctx.begin_path();
        let _ = ctx.arc(x, y, RING_RADIUS, a0, a1);
        ctx.set_line_width(RING_WIDTH + 3.0);
        ctx.set_stroke_style_str("rgba(0, 0, 0, 0.55)");
        ctx.stroke();
        ctx.set_line_width(RING_WIDTH);
        ctx.set_stroke_style_str(&css(colour));
        ctx.stroke();
    }
}

/// The event's position in page pixels, with the fraction: Pointer Events report sub-pixel
/// positions, which web-sys's `client_x()` rounds to an integer (jagged strokes).
fn client(e: &web_sys::MouseEvent) -> (f64, f64) {
    let get = |k: &str, fallback: i32| js_sys::Reflect::get(e, &k.into()).ok().and_then(|v| v.as_f64()).unwrap_or(f64::from(fallback));
    (get("clientX", e.client_x()), get("clientY", e.client_y()))
}

/// Every sample the browser merged into this event (the Pencil reports 240 per second), or the
/// event itself.
fn coalesced(e: &PointerEvent) -> Vec<PointerEvent> {
    let list = js_sys::Reflect::get(e, &"getCoalescedEvents".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .and_then(|f| f.call0(e).ok())
        .and_then(|a| a.dyn_into::<js_sys::Array>().ok());
    let events: Vec<PointerEvent> = list.map(|a| a.iter().filter_map(|v| v.dyn_into().ok()).collect()).unwrap_or_default();
    if events.is_empty() { vec![e.clone()] } else { events }
}

fn listen(target: &web_sys::EventTarget, name: &str, passive: bool, f: impl FnMut(web_sys::Event) + 'static) -> Result<(), JsValue> {
    let cb = Closure::<dyn FnMut(web_sys::Event)>::new(f);
    let opts = web_sys::AddEventListenerOptions::new();
    opts.set_passive(passive);
    target.add_event_listener_with_callback_and_add_event_listener_options(name, cb.as_ref().unchecked_ref(), &opts)?;
    cb.forget();
    Ok(())
}

fn pointer_listeners(page: &Shared, canvas: &HtmlCanvasElement) -> Result<(), JsValue> {
    let p = page.clone();
    let c = canvas.clone();
    listen(canvas, "pointerdown", false, move |e| {
        let Ok(e) = e.dyn_into::<PointerEvent>() else { return };
        e.prevent_default();
        let mut pg = p.borrow_mut();
        let (x, y) = client(&e);
        match e.pointer_type().as_str() {
            "pen" => {
                let _ = c.set_pointer_capture(e.pointer_id());
                if !pg.touch_stylus
                    && let Some(s) = pg.pointer_sample(&e)
                {
                    pg.pen_begin(s);
                }
            }
            "touch" => {
                let out = pg.gestures.down(e.pointer_id(), x, y, now());
                pg.gestures(out);
                Page::watch_hold(&p);
            }
            _ => {
                if let Some((x, y)) = pg.to_window(x, y) {
                    pg.send(json!({"t": "ptr", "p": "d", "x": x, "y": y}));
                }
            }
        }
    })?;
    let p = page.clone();
    listen(canvas, "pointermove", false, move |e| {
        let Ok(e) = e.dyn_into::<PointerEvent>() else { return };
        e.prevent_default();
        let mut pg = p.borrow_mut();
        match e.pointer_type().as_str() {
            "pen" => {
                if pg.pen_down {
                    if !pg.touch_stylus {
                        let samples: Vec<Sample> = coalesced(&e).iter().filter_map(|s| pg.pointer_sample(s)).collect();
                        pg.pen_move(&samples);
                    }
                } else {
                    // Hovering (Pencil hover on M2 and later iPad Pros): the outline follows here
                    // at once, and the desktop pointer too (it decides the outline's shape).
                    let (cx, cy) = client(&e);
                    pg.set_hover(Some((cx, cy)));
                    if let Some((x, y)) = pg.to_window(cx, cy) {
                        pg.send(json!({"t": "hover", "x": x, "y": y}));
                    }
                }
            }
            "touch" => {
                let (x, y) = client(&e);
                let out = pg.gestures.moved(e.pointer_id(), x, y);
                pg.gestures(out);
            }
            _ => {
                if let Some((x, y)) = pg.to_window(client(&e).0, client(&e).1) {
                    let t = if e.buttons() & 1 != 0 { json!({"t": "ptr", "p": "m", "x": x, "y": y}) } else { json!({"t": "hover", "x": x, "y": y}) };
                    pg.send(t);
                }
            }
        }
    })?;
    for name in ["pointerup", "pointercancel"] {
        let p = page.clone();
        let cancel = name == "pointercancel";
        listen(canvas, name, false, move |e| {
            let Ok(e) = e.dyn_into::<PointerEvent>() else { return };
            e.prevent_default();
            let mut pg = p.borrow_mut();
            match e.pointer_type().as_str() {
                "pen" => {
                    if !pg.touch_stylus {
                        pg.pen_end();
                    }
                }
                "touch" => {
                    if cancel {
                        let out = pg.gestures.cancel(e.pointer_id());
                        pg.gestures(out);
                    } else {
                        let out = pg.gestures.up(e.pointer_id(), now());
                        pg.gestures(out);
                    }
                }
                _ => {
                    if let Some((x, y)) = pg.to_window(client(&e).0, client(&e).1) {
                        pg.send(json!({"t": "ptr", "p": "u", "x": x, "y": y}));
                    }
                }
            }
        })?;
    }
    let p = page.clone();
    listen(canvas, "pointerleave", true, move |e| {
        if e.dyn_ref::<PointerEvent>().is_some_and(|e| e.pointer_type() != "touch") && !p.borrow().pen_down {
            let mut pg = p.borrow_mut();
            pg.set_hover(None);
            pg.send(json!({"t": "leave"}));
        }
    })?;
    // Mouse wheel / trackpad when testing from a computer's browser.
    let p = page.clone();
    listen(canvas, "wheel", false, move |e| {
        let Ok(e) = e.dyn_into::<web_sys::WheelEvent>() else { return };
        e.prevent_default();
        let pg = p.borrow();
        let Some((_, _, s)) = pg.mapping() else { return };
        if let Some((x, y)) = pg.to_window(client(&e).0, client(&e).1) {
            pg.send(json!({"t": "scroll", "x": x, "y": y, "dx": -e.delta_x() / s, "dy": -e.delta_y() / s}));
        }
    })?;
    Ok(())
}

/// The Pencil as touch events (`touchType == "stylus"`), which Safari delivers at a higher rate
/// than its pen pointer events. Finger touches are left to the pointer events (gestures).
fn stylus_touch_listeners(page: &Shared, canvas: &HtmlCanvasElement) -> Result<(), JsValue> {
    /// The event's changed touches that are the Pencil.
    fn stylus_touches(e: &web_sys::Event) -> Vec<JsValue> {
        let Ok(list) = js_sys::Reflect::get(e, &"changedTouches".into()) else { return Vec::new() };
        let n = js_sys::Reflect::get(&list, &"length".into()).ok().and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 32.0) as u32;
        (0..n)
            .filter_map(|i| js_sys::Reflect::get_u32(&list, i).ok())
            .filter(|t| js_sys::Reflect::get(t, &"touchType".into()).ok().and_then(|v| v.as_string()).as_deref() == Some("stylus"))
            .collect()
    }
    for name in ["touchstart", "touchmove", "touchend", "touchcancel"] {
        let p = page.clone();
        listen(canvas, name, false, move |e| {
            let touches = stylus_touches(&e);
            if touches.is_empty() {
                return;
            }
            e.prevent_default();
            let mut pg = p.borrow_mut();
            pg.touch_stylus = true;
            match name {
                "touchstart" => {
                    // The pen pointer event may have started the stroke already: it continues.
                    if let Some(s) = touches.first().and_then(|t| pg.touch_sample(t)) {
                        if pg.pen_down {
                            pg.pen_move(&[s]);
                        } else {
                            pg.pen_begin(s);
                        }
                    }
                }
                "touchmove" => {
                    let samples: Vec<Sample> = touches.iter().filter_map(|t| pg.touch_sample(t)).collect();
                    pg.pen_move(&samples);
                }
                _ => pg.pen_end(),
            }
        })?;
    }
    Ok(())
}

fn toolbar_listeners(page: &Shared, document: &web_sys::Document) -> Result<(), JsValue> {
    let buttons = document.query_selector_all("[data-menu]")?;
    for i in 0..buttons.length() {
        let Some(b) = buttons.item(i).and_then(|n| n.dyn_into::<HtmlElement>().ok()) else { continue };
        let Some(id) = b.get_attribute("data-menu") else { continue };
        let p = page.clone();
        listen(&b, "click", true, move |_| p.borrow().send(json!({"t": "menu", "id": id})))?;
    }
    if let Some(mode) = document.get_element_by_id("mode") {
        let p = page.clone();
        listen(&mode, "click", true, move |_| {
            let mut pg = p.borrow_mut();
            pg.mode = (pg.mode + 1) % MODES.len();
            let Some((id, label)) = MODES.get(pg.mode) else { return };
            pg.send(json!({"t": "menu", "id": id}));
            if let Some(b) = &pg.mode_button {
                b.set_title(label);
                let _ = b.class_list().toggle_with_force("on", pg.mode == 1);
            }
        })?;
    }
    Ok(())
}

/// Stop Safari scrolling, zooming or selecting the page under the fingers.
fn block_browser_gestures(document: &web_sys::Document) -> Result<(), JsValue> {
    for name in ["touchstart", "touchmove", "gesturestart", "gesturechange", "dblclick", "contextmenu", "selectstart"] {
        listen(document, name, false, |e| {
            let on_button = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|el| el.closest("button").ok().flatten().is_some());
            if !on_button {
                e.prevent_default();
            }
        })?;
    }
    Ok(())
}

/// Keep the iPad's screen on while drawing (Screen Wake Lock, Safari 16.4+), again whenever the
/// page comes back to the front.
fn keep_awake(document: &web_sys::Document) -> Result<(), JsValue> {
    fn request() {
        let Some(nav) = web_sys::window().map(|w| w.navigator()) else { return };
        let lock = js_sys::Reflect::get(&nav, &"wakeLock".into()).ok().filter(|v| !v.is_undefined());
        let Some(lock) = lock else { return };
        if let Some(f) = js_sys::Reflect::get(&lock, &"request".into()).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok()) {
            let _ = f.call1(&lock, &"screen".into());
        }
    }
    request();
    listen(document, "visibilitychange", true, |_| request())?;
    // Safari grants the lock only after a user gesture: ask again on the first touch.
    listen(document, "pointerdown", true, |_| request())?;
    Ok(())
}
