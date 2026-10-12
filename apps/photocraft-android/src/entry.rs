//! The Android entry point and the platform `Services`.

// ponytail: the one `unsafe` in this crate: `android_main` must be an unmangled symbol.
#![allow(unsafe_code)]

use eframe::egui_wgpu::WgpuSetup;
use photocraft_doc::Document;
use photocraft_engine::Session;
use photocraft_ui_egui::theme::ThemeKind;
use crate::browser::{Browser, Queue};
use photocraft_ui_egui::{PhotocraftApp, Services};
use android_activity::AndroidApp;


// SAFETY: the NativeActivity glue looks this symbol up by name; the signature is the one
// android-activity documents, and nothing else exports `android_main`.
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("photocraft"));
    let files_dir = app.internal_data_path();
    let activity = app.clone();
    let mut options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
    photocraft_ui_egui::gpu_canvas::use_adapter_limits(&mut options.wgpu_options.wgpu_setup);
    if let WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        create.instance_descriptor.backends = eframe::wgpu::Backends::VULKAN;
    }
    let result = eframe::run_native(
        "PhotoCraft",
        options,
        Box::new(move |cc| {
            PhotocraftApp::setup_context(&cc.egui_ctx, ThemeKind::Pro);
            let queue = Queue::default();
            let mut app = PhotocraftApp::new(Session::new(), services(files_dir, queue.clone()));
            app.set_theme(&cc.egui_ctx, ThemeKind::Pro);
            if let Some(rs) = cc.wgpu_render_state.clone() {
                app.set_wgpu(rs);
            }
            Ok(Box::new(Shell { app, queue, browser: None, activity }))
        }),
    );
    if let Err(e) = result {
        log::error!("PhotoCraft failed to start: {e}");
    }
}

struct Shell {
    app: PhotocraftApp,
    queue: Queue,
    browser: Option<Browser>,
    activity: AndroidApp,
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Keep the app out from under the status and navigation bars: the activity's content
        // rectangle (pixels) is the window minus the system bars.
        let ppp = ui.ctx().pixels_per_point();
        let r = self.activity.content_rect();
        let content = egui::Rect::from_min_max(egui::pos2(r.left as f32 / ppp, r.top as f32 / ppp), egui::pos2(r.right as f32 / ppp, r.bottom as f32 / ppp));
        let area = ui.max_rect();
        let inner = if content.width() > 1.0 && content.height() > 1.0 { area.intersect(content) } else { area };
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| self.app.ui(ui, frame));
        if self.browser.is_none()
            && let Some((request, reply)) = self.queue.lock().unwrap_or_else(|e| e.into_inner()).take()
        {
            self.browser = Some(Browser::new(request, reply));
        }
        if let Some(b) = &mut self.browser
            && !b.show(ui.ctx())
        {
            self.browser = None;
        }
    }
}

fn services(files_dir: Option<std::path::PathBuf>, queue: Queue) -> Services {
    let prefs = files_dir.map(|d| d.join("preferences.json"));
    let (load, save) = (prefs.clone(), prefs);
    Services {
        import: Some(Box::new(|name: &str, bytes: &[u8], depth: usize| {
            photocraft_io::import_with_svg_group_depth(name, bytes, depth).map(|r| (r.document, r.warnings)).map_err(|e| e.to_string())
        })),
        export: Some(Box::new(|doc: &Document, path: &str, _settings: &photocraft_ui_egui::ExportSettings| {
            photocraft_io::export(doc, path, &photocraft_io::ExportOptions::default()).map(|r| (r.bytes, r.warnings)).map_err(|e| e.to_string())
        })),
        file_dialog: Some(Box::new(move |request, _parent, reply| {
            *queue.lock().unwrap_or_else(|e| e.into_inner()) = Some((request, reply));
        })),
        write: Some(Box::new(|path: &str, bytes: &[u8]| std::fs::write(path, bytes).map_err(|e| e.to_string()))),
        load_prefs: Some(Box::new(move || load.as_ref().and_then(|p| std::fs::read_to_string(p).ok()))),
        save_prefs: Some(Box::new(move |text: &str| match &save {
            Some(p) => std::fs::write(p, text).map_err(|e| e.to_string()),
            None => Err("no files directory".into()),
        })),
        ..Default::default()
    }
}
