//! Native PhotoCraft entry point for Android (no WebView, no JS).
//!
//! Winit/eframe owns Activity lifecycle, rotation, touch and the Vulkan surface.
//! Platform-only services are separate from the desktop app to avoid X11,
//! Wayland, desktop file pickers and subprocess dependencies.

#![cfg(target_os = "android")]
// Exporting android_main and the JNI callback requires unsafe symbol attributes.
// The entrypoints and their bodies do not dereference raw pointers.

mod picker;
mod services;
mod spen;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Once;

use android_activity::{AndroidApp, input::Axis};
use photocraft_engine::Session;
use photocraft_ui_egui::theme::ThemeKind;
use photocraft_ui_egui::{PhotocraftApp, FileDialogReply, FileDialogRequest};

type PendingDialogs = Rc<RefCell<VecDeque<(FileDialogRequest, FileDialogReply)>>>;

/// Called by android-activity, not by the desktop main().
/// The AndroidApp is supplied by the OS for every Activity creation.
#[unsafe(no_mangle)]
fn android_main(android_app: AndroidApp) {
    static INIT_LOGGER: Once = Once::new();
    INIT_LOGGER.call_once(|| {
        android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));
    });

    // Non-position axes are opt-in in android-activity. Winit still owns the
    // input queue; we must never drain the same queue ourselves.
    android_app.enable_motion_axis(Axis::Pressure);
    android_app.enable_motion_axis(Axis::Tilt);
    android_app.enable_motion_axis(Axis::Orientation);

    let data_dir = android_app.internal_data_path();
    let documents_dir = data_dir.as_ref().map(|path| path.join("Documents"));
    let dialogs: PendingDialogs = Rc::default();

    let mut options = eframe::NativeOptions {
        android_app: Some(android_app),
        viewport: egui::ViewportBuilder::default()
            .with_title("PhotoCraft")
            .with_fullscreen(true)
            .with_decorations(false),
        ..Default::default()
    };

    // Keep the engine's GPU canvas limited to the real hardware adapter.
    photocraft_ui_egui::gpu_canvas::use_adapter_limits(&mut options.wgpu_options.wgpu_setup);
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        create.instance_descriptor.backends = eframe::wgpu::Backends::VULKAN;
    }

    let result = eframe::run_native("PhotoCraft", options, Box::new(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, ThemeKind::Pro);
        let mut app = PhotocraftApp::new(Session::new(), services::android_services(data_dir, dialogs.clone()));
        app.set_theme(&cc.egui_ctx, ThemeKind::Pro);
        spen::install(Some(app.stylus.feed.clone()));
        photocraft_ui_egui::android_touch::configure_touch_ui(&cc.egui_ctx);
        if let Some(state) = cc.wgpu_render_state.clone() {
            app.perf.gpu_info.set_adapter(&state.adapter.get_info());
            // Diagnostic workaround: the Android Vulkan compositor may terminate
            // the Activity when a new document exceeds 128 pixels on either axis.
            // Keep Vulkan for egui, but render document pixels on the tested CPU
            // path until we can identify the offending GPU operation.
            //
            // To compare the original behavior, build with
            // PHOTOCRAFT_ANDROID_GPU_CANVAS=1 in the Rust build environment.
            if option_env!("PHOTOCRAFT_ANDROID_GPU_CANVAS") == Some("1") {
                log::info!("Android document canvas: GPU (diagnostic override)");
                app.set_wgpu(state);
            } else {
                log::warn!("Android document canvas: CPU diagnostic fallback; egui still uses Vulkan");
            }
        } else {
            log::warn!("No wgpu render state: PhotoCraft uses the CPU canvas");
        }
        Ok(Box::new(AndroidShell {
            app,
            documents_dir,
            dialogs,
            picker: None,
            touch_recovery: Default::default(),
            java_pen_start: None,
            java_native_seen: false,
            diag_pending_input: false,
        }))
    }));

    spen::install(None);
    if let Err(err) = result {
        log::error!("PhotoCraft Android Activity terminated: {err}");
    }
}

/// The editor is exactly the same PhotoCraftApp used by desktop and web.
/// Only the file picker UI is supplied by the Android shell.
struct AndroidShell {
    app: PhotocraftApp,
    documents_dir: Option<PathBuf>,
    dialogs: PendingDialogs,
    picker: Option<picker::Picker>,
    touch_recovery: photocraft_ui_egui::stylus::AndroidTouchRecovery,
    java_pen_start: Option<spen::PenTouch>,
    java_native_seen: bool,
    diag_pending_input: bool,
}

impl eframe::App for AndroidShell {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        static FIRST_NATIVE_TOUCH: Once = Once::new();
        if let Some(force) = raw.events.iter().find_map(|e| match e {
            egui::Event::Touch { phase: egui::TouchPhase::Start, force, .. } => Some(*force),
            _ => None,
        }) {
            FIRST_NATIVE_TOUCH.call_once(|| {
                log::info!("Android native touch events active (first force: {force:?})");
            });
        }
        // Native winit touch is authoritative. Track it for the *whole*
        // Java contact rather than choosing a source independently per frame.
        // Java is only a fallback for complete taps never seen by winit.
        let native_touch = raw.events.iter().any(|event| matches!(
            event, egui::Event::Touch { .. }
        ));
        if native_touch && self.java_pen_start.is_some() {
            self.java_native_seen = true;
        }
        // Diagnostic: compare the egui input stream against Java S Pen pixels.
        // Log transitions only to avoid flooding logcat on stylus movement.
        for event in &raw.events {
            match event {
                egui::Event::Touch { phase, pos, .. }
                    if matches!(phase, egui::TouchPhase::Start | egui::TouchPhase::End | egui::TouchPhase::Cancel) =>
                {
                    log::info!("SPEN_DIAG native touch phase={phase:?} egui=({:.1},{:.1}) scale={:.3}",
                        pos.x, pos.y, ctx.pixels_per_point());
                }
                egui::Event::PointerButton { pos, pressed, .. } => {
                    log::info!("SPEN_DIAG native pointer pressed={pressed} egui=({:.1},{:.1})",
                        pos.x, pos.y);
                }
                _ => {}
            }
        }
        for sample in spen::drain_touch_events() {
            if matches!(sample.action, 0 | 1 | 3) {
                log::info!("SPEN_DIAG java action={} id={} px=({:.1},{:.1}) egui=({:.1},{:.1}) scale={:.3}",
                    sample.action, sample.pointer_id, sample.x, sample.y,
                    sample.x / ctx.pixels_per_point().max(0.001),
                    sample.y / ctx.pixels_per_point().max(0.001), ctx.pixels_per_point());
            }
            match sample.action {
                0 => {
                    self.java_pen_start = Some(sample);
                    self.java_native_seen = native_touch;
                }
                1 | 3 => {
                    if let Some(start) = self.java_pen_start.take() {
                        if sample.action == 1 && !self.java_native_seen && !native_touch {
                            // Both transitions are injected together, so a
                            // missing UP cannot leave an egui button stuck.
                            let scale = ctx.pixels_per_point().max(0.001);
                            let down = egui::pos2(start.x / scale, start.y / scale);
                            let up = egui::pos2(sample.x / scale, sample.y / scale);
                            let modifiers = egui::Modifiers::NONE;
                            raw.events.push(egui::Event::PointerMoved(down));
                            raw.events.push(egui::Event::PointerButton {
                                pos: down, button: egui::PointerButton::Primary,
                                pressed: true, modifiers,
                            });
                            raw.events.push(egui::Event::PointerMoved(up));
                            raw.events.push(egui::Event::PointerButton {
                                pos: up, button: egui::PointerButton::Primary,
                                pressed: false, modifiers,
                            });
                            log::info!("S Pen JNI fallback: completed missing tap");
                        }
                    }
                    self.java_native_seen = false;
                }
                _ => {}
            }
        }
        // Forward the editor's normal input processing: AndroidShell is an
        // eframe::App wrapper, so it must not drop the inner app's hook.
        <PhotocraftApp as eframe::App>::raw_input_hook(&mut self.app, ctx, raw);
        // An Android touch cancellation only emits PointerGone in egui-winit.
        // Synthesize the *missing release*, not a second normal touch stream.
        self.touch_recovery.repair(raw);
        let touches = raw.events.iter().filter(|e| matches!(e, egui::Event::Touch { .. })).count();
        let buttons = raw.events.iter().filter(|e| matches!(e, egui::Event::PointerButton { .. })).count();
        if touches > 0 || buttons > 0 {
            self.diag_pending_input = true;
            log::info!("SPEN_DIAG raw_input final touches={touches} buttons={buttons} total={} ppp={:.3}", raw.events.len(), ctx.pixels_per_point());
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if self.diag_pending_input {
            let (pos, down, released, clicked, events) = ui.ctx().input(|i| {
                (i.pointer.interact_pos(), i.pointer.primary_down(),
                 i.pointer.any_released(), i.pointer.any_click(), i.events.len())
            });
            log::info!("SPEN_DIAG egui_frame pos={pos:?} down={down} released={released} clicked={clicked} events={events}");
            self.diag_pending_input = false;
        }
        self.app.ui(ui, frame);

        if self.picker.is_none() {
            if let Some((request, reply)) = self.dialogs.borrow_mut().pop_front() {
                self.picker = Some(picker::Picker::new(request, reply));
            }
        }

        if let Some(picker) = self.picker.as_mut() {
            if picker.show(ui.ctx(), self.documents_dir.as_deref()) {
                self.picker = None;
            }
        }
    }
}

impl Drop for AndroidShell {
    fn drop(&mut self) {
        // A re-created Activity must not write samples into the old session.
        spen::install(None);
    }
}
