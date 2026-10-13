//! Native Android shell. SAF documents remain capabilities, never filesystem paths.
// NativeActivity's C entry point and the two borrowed handles are the existing platform boundary.
#![allow(unsafe_code)]
use crate::android_files::{Bridge, err};
use android_activity::AndroidApp;
use jni::{
    JavaVM,
    objects::{JObject, JString},
};
use photocraft_ui_egui::PhotocraftApp;
use std::sync::{Arc, Mutex};

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));
    if let Err(e) = run(app) {
        log::error!("PhotoCraft: {e}");
    }
}
fn run(app: AndroidApp) -> Result<(), String> {
    // SAFETY: NativeActivity owns both handles for the entire run_native call. The object is
    // immediately promoted to a JVM global reference before any worker receives it.
    let vm = Arc::new(unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.map_err(err)?);
    let activity = {
        let env = vm.attach_current_thread().map_err(err)?;
        let borrowed = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
        env.new_global_ref(&borrowed).map_err(err)?
    };
    let bridge = Bridge { vm, activity };
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
    photocraft_ui_egui::gpu_canvas::use_adapter_limits(&mut options.wgpu_options.wgpu_setup);
    eframe::run_native(
        "PhotoCraft",
        options,
        Box::new(move |cc| {
            PhotocraftApp::setup_context(&cc.egui_ctx, photocraft_ui_egui::theme::ThemeKind::Pro);
            let mut services = crate::services::services();
            crate::android_files::install(&mut services, &bridge, &cc.egui_ctx, &errors);
            let mut editor = PhotocraftApp::new(photocraft_engine::Session::new(), services);
            editor.ui.touch.enabled = true;
            if let Some(rs) = cc.wgpu_render_state.clone() {
                editor.set_wgpu(rs);
            }
            Ok(Box::new(AndroidEditor { editor, bridge, errors }))
        }),
    )
    .map_err(|e| e.to_string())
}
struct AndroidEditor {
    editor: PhotocraftApp,
    bridge: Bridge,
    errors: Arc<Mutex<Vec<String>>>,
}
impl eframe::App for AndroidEditor {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.editor.ui(ui, frame);
    }
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.editor.raw_input_hook(ctx, raw);
    }
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Drain VIEW/SEND intents on every resume, including when the app is already running.
        let files = self.bridge.with(|env, act| {
            let obj = env.call_method(act, "takeIncoming", "()Ljava/lang/String;", &[]).map_err(err)?.l().map_err(err)?;
            let text: String = env.get_string(&JString::from(obj)).map_err(err)?.into();
            serde_json::from_str::<Vec<serde_json::Value>>(&text).map_err(|e| e.to_string())
        });
        match files {
            Ok(files) => {
                for file in files {
                    if let (Some(name), Some(uri)) = (file.get("name").and_then(|v| v.as_str()), file.get("uri").and_then(|v| v.as_str())) {
                        let result = self.bridge.read(uri).and_then(|bytes| self.editor.open_bytes(name, &bytes));
                        if let Err(e) = result {
                            self.editor.ui.status = e;
                            self.editor.ui.status_error = true;
                        }
                    }
                }
            }
            Err(e) => {
                self.editor.ui.status = e;
                self.editor.ui.status_error = true;
            }
        }
        self.editor.logic(ctx, frame);
        for error in self.errors.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drain(..) {
            self.editor.ui.status = error;
            self.editor.ui.status_error = true;
        }
    }
}
