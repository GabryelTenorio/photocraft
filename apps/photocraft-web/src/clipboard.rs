//! Browser-owned image clipboard. No polling or permission prompts outside an explicit paste.
use photocraft_ui_egui::{PhotocraftApp, Services};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};

type Pixels = (u32, u32, Vec<u8>);
type ReadResult = Result<Option<Pixels>, String>;
struct Arrival {
    command: String,
    params: serde_json::Value,
    target: Option<u64>,
    result: ReadResult,
}
#[derive(Default)]
struct State {
    image: Option<Pixels>,
    text: VecDeque<(Option<u64>, photocraft_ui_egui::state::TextEdit, Result<String, String>)>,
    arrivals: VecDeque<Arrival>,
    errors: VecDeque<String>,
    target: Option<u64>,
    typing: bool,
    pending: bool,
}
#[derive(Clone)]
pub struct Bridge {
    state: Rc<RefCell<State>>,
    ctx: egui::Context,
}
const MAX_BYTES: usize = 64 * 1024 * 1024;
fn error(e: JsValue) -> String {
    format!("Browser clipboard: {e:?}. Use Ctrl/Cmd+V to paste, or allow clipboard access for this site.")
}
impl Bridge {
    pub fn new(ctx: egui::Context) -> Self {
        let this = Self { state: Rc::default(), ctx };
        this.listen();
        this
    }
    pub fn set_target(&self, app: &PhotocraftApp, ctx: &egui::Context) {
        let mut state = self.state.borrow_mut();
        state.target = app.session.active().map(|d| d.doc.id.0);
        state.typing = ctx.egui_wants_keyboard_input() || app.ui.text_edit.is_some();
    }
    pub fn install(&self, services: &mut Services) {
        let bridge = self.clone();
        services.clipboard_get_image = Some(Box::new(move || bridge.state.borrow().image.clone()));
        let bridge = self.clone();
        services.clipboard_set_image = Some(Box::new(move |w, h, rgba| {
            let result = bridge.write(w, h, rgba);
            if let Err(e) = &result {
                bridge.state.borrow_mut().errors.push_back(format!("Copied inside PhotoCraft, but system clipboard write failed: {e}"));
                bridge.ctx.request_repaint();
            }
            result
        }));
        let bridge = self.clone();
        services.clipboard_text = Some(Box::new(move |target, edit| {
            let promise = browser_clipboard("readText")?.read_text();
            let bridge = bridge.clone();
            spawn_local(async move {
                let result = JsFuture::from(promise).await.map_err(error).and_then(|v| v.as_string().ok_or_else(|| "Clipboard returned no text".into()));
                bridge.state.borrow_mut().text.push_back((target, edit, result));
                bridge.ctx.request_repaint();
            });
            Ok(())
        }));
        let bridge = self.clone();
        services.clipboard_paste = Some(Box::new(move |command, params, target| {
            if bridge.state.borrow().pending {
                return Err("A clipboard read is already pending".into());
            }
            let promise = browser_clipboard("read")?.read();
            bridge.read(command.into(), params, target, async move {
                let items = JsFuture::from(promise).await.map_err(error)?;
                for item in js_sys::Array::from(&items).iter() {
                    let item: web_sys::ClipboardItem = item.dyn_into().map_err(error)?;
                    if item.types().iter().any(|t| t.as_string().as_deref() == Some("image/png")) {
                        let blob = JsFuture::from(item.get_type("image/png")).await.map_err(error)?;
                        return decode_blob(blob.dyn_into().map_err(error)?).await.map(Some);
                    }
                }
                Ok(None)
            });
            Ok(())
        }));
    }
    fn read(&self, command: String, params: serde_json::Value, target: Option<u64>, work: impl std::future::Future<Output = ReadResult> + 'static) {
        self.state.borrow_mut().pending = true;
        let bridge = self.clone();
        spawn_local(async move {
            let result = work.await;
            let mut state = bridge.state.borrow_mut();
            state.arrivals.push_back(Arrival { command, params, target, result });
            state.pending = false;
            bridge.ctx.request_repaint();
        });
    }
    fn listen(&self) {
        let Some(window) = web_sys::window() else { return };
        let bridge = self.clone();
        let listener = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(move |event: web_sys::ClipboardEvent| {
            if event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .is_some_and(|el| el.closest("input,textarea,[contenteditable='true'],[contenteditable='']").ok().flatten().is_some())
            {
                return;
            }
            let state = bridge.state.borrow();
            if state.typing || !bridge.ctx.input(|i| i.focused) {
                return;
            }
            let target = state.target;
            let pending = state.pending;
            drop(state);
            let Some(data) = event.clipboard_data() else { return };
            // Canvas paste is ours, including an empty/text clipboard (invalidate stale images).
            event.prevent_default();
            event.stop_immediate_propagation();
            if pending {
                return;
            }
            let image = data.files().and_then(|files| (0..files.length()).filter_map(|i| files.get(i)).find(|f| f.type_().starts_with("image/")));
            let command = if bridge.ctx.input(|i| i.modifiers.shift) { "edit.pasteSpecial.pasteInPlace" } else { "edit.paste" };
            bridge.read(command.into(), serde_json::json!({}), target, async move {
                match image {
                    Some(file) => decode_blob(file.into()).await.map(Some),
                    None => Ok(None),
                }
            });
        });
        if window.add_event_listener_with_callback_and_bool("paste", listener.as_ref().unchecked_ref(), true).is_ok() {
            listener.forget();
        }
    }
    fn write(&self, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
        let bytes = u64::from(w).checked_mul(u64::from(h)).and_then(|n| n.checked_mul(4)).ok_or("Image dimensions overflow")?;
        if bytes > MAX_BYTES as u64 || bytes != rgba.len() as u64 {
            return Err("Clipboard image exceeds 64 MiB or has invalid dimensions".into());
        }
        let image = photocraft_codecs::Image::from_u8(w, h, photocraft_codecs::ChannelLayout::Rgba, rgba.to_vec()).map_err(|e| e.to_string())?;
        let png = photocraft_codecs::encode(&image, photocraft_codecs::Format::Png, &Default::default()).map_err(|e| e.to_string())?;
        self.state.borrow_mut().image = Some((w, h, rgba.to_vec()));
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(png.as_slice()));
        let opts = web_sys::BlobPropertyBag::new();
        opts.set_type("image/png");
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(error)?;
        let data = js_sys::Object::new();
        js_sys::Reflect::set(&data, &JsValue::from_str("image/png"), &blob).map_err(error)?;
        let item = web_sys::ClipboardItem::new_with_record_from_str_to_blob_promise(&data).map_err(error)?;
        let clipboard = browser_clipboard("write")?;
        // Call write NOW, while the copy/cut event still carries browser user activation.
        let promise = clipboard.write(&js_sys::Array::of1(&item));
        let bridge = self.clone();
        spawn_local(async move {
            if let Err(e) = JsFuture::from(promise).await {
                bridge.state.borrow_mut().errors.push_back(format!("Copied inside PhotoCraft, but system clipboard write failed. {}", error(e)));
                bridge.ctx.request_repaint();
            }
        });
        Ok(())
    }
    pub fn drain(&self, app: &mut PhotocraftApp) {
        let text = std::mem::take(&mut self.state.borrow_mut().text);
        for (target, edit, result) in text {
            let same = app.session.active().map(|d| d.doc.id.0) == target
                && app.ui.text_edit.as_ref().is_some_and(|current| {
                    current.layer == edit.layer && current.session == edit.session && current.caret == edit.caret && current.anchor == edit.anchor
                });
            if !same {
                app.ui.status = "Text paste cancelled because the editing target changed".into();
                continue;
            }
            match result {
                Ok(text) => self.ctx.input_mut(|i| i.events.push(egui::Event::Paste(text))),
                Err(e) => {
                    app.ui.status = e;
                    app.ui.status_error = true;
                }
            }
        }
        let arrivals = std::mem::take(&mut self.state.borrow_mut().arrivals);
        for arrival in arrivals {
            let result = match arrival.result {
                Ok(image) => {
                    self.state.borrow_mut().image = image;
                    app.finish_clipboard_paste(&arrival.command, arrival.params, arrival.target).map(|_| ())
                }
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                app.ui.status = e;
                app.ui.status_error = true;
            }
        }
        if let Some(e) = self.state.borrow_mut().errors.pop_front() {
            app.ui.status = e;
            app.ui.status_error = true;
        }
    }
}
async fn decode_blob(blob: web_sys::Blob) -> Result<Pixels, String> {
    if blob.size() > MAX_BYTES as f64 {
        return Err("Clipboard image exceeds 64 MiB".into());
    }
    let buffer = JsFuture::from(blob.array_buffer()).await.map_err(error)?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    crate::clipboard_pixels::decode(&bytes)
}

fn browser_clipboard(method: &str) -> Result<web_sys::Clipboard, String> {
    let unavailable = "This browser does not allow clipboard access here. Use Ctrl/Cmd+V to paste, or use a browser with Clipboard API support over HTTPS.";
    let navigator = web_sys::window().ok_or(unavailable)?.navigator();
    let value = js_sys::Reflect::get(&navigator, &wasm_bindgen::JsValue::from_str("clipboard")).map_err(|_| unavailable)?;
    if value.is_null() || value.is_undefined() {
        return Err(unavailable.into());
    }
    let action = js_sys::Reflect::get(&value, &wasm_bindgen::JsValue::from_str(method)).map_err(|_| unavailable)?;
    if !action.is_function() {
        return Err(unavailable.into());
    }
    value.dyn_into::<web_sys::Clipboard>().map_err(|_| unavailable.into())
}
