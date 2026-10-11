//! Samsung S Pen / Android stylus samples.
//!
//! Winit already forwards touch position and pressure through egui. This optional
//! bridge receives the extra tilt, orientation and eraser metadata from the Java
//! NativeActivity subclass. No JNI pointers are dereferenced in Rust.
//!
//! Android's AXIS_TILT is an inclination in radians from the surface normal,
//! and AXIS_ORIENTATION points clockwise from vertical. The conversion below
//! matches Android-to-W3C Pointer Events (Chromium) tiltX/tiltY conventions.

use std::sync::{Mutex, Once, OnceLock, PoisonError};
use std::collections::VecDeque;

use photocraft_ui_egui::stylus::{PenSample, StylusFeed};

static FEED: OnceLock<Mutex<Option<StylusFeed>>> = OnceLock::new();

fn current_feed() -> &'static Mutex<Option<StylusFeed>> {
    FEED.get_or_init(|| Mutex::new(None))
}

/// Associate the current Activity with its own pen feed; Android may recreate
/// the Activity, so never hold a stale feed across instances.
pub fn install(feed: Option<StylusFeed>) {
    *current_feed().lock().unwrap_or_else(PoisonError::into_inner) = feed;
}

/// Radians from Android motion axes to tilt angles in W3C-style degrees.
/// Per-axis atan2 avoids undefined tangent close to 90 degrees.
pub fn axes_to_tilt(tilt_radians: f32, orientation_radians: f32) -> (f32, f32) {
    if !tilt_radians.is_finite() || !orientation_radians.is_finite() {
        return (0.0, 0.0);
    }
    let r = tilt_radians.clamp(0.0, std::f32::consts::FRAC_PI_2).sin();
    let z = tilt_radians.clamp(0.0, std::f32::consts::FRAC_PI_2).cos();
    let x = (-orientation_radians).sin().mul_add(r, 0.0).atan2(z).to_degrees();
    let y = (-orientation_radians).cos().mul_add(r, 0.0).atan2(z).to_degrees();
    (x, y)
}

/// JNI callback for ai.storyteller.photocraft.PhotocraftActivity.
///
/// Java supplies samples on the UI thread; StylusFeed is thread-safe and
/// consumed by the eframe render thread. The last two arguments are JNI's
/// jboolean (unsigned 8-bit), not Rust bool.
/// 
/// Only the exported symbol is an unsafe-language feature. All operations
/// here are safe; opaque JNI pointers are deliberately ignored.
#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_storyteller_photocraft_PhotocraftActivity_nativeStylusSample(
    _env: *mut std::ffi::c_void,
    _activity: *mut std::ffi::c_void,
    pressure: f32,
    tilt_radians: f32,
    orientation_radians: f32,
    eraser: u8,
    contact: u8,
) {
    static FIRST_JAVA_PEN: Once = Once::new();
    if contact != 0 {
        FIRST_JAVA_PEN.call_once(|| log::info!("S Pen JNI metadata active (pressure/tilt/eraser)"));
    }
    let feed = current_feed().lock().unwrap_or_else(PoisonError::into_inner).clone();
    let Some(feed) = feed else { return };
    if contact == 0 {
        feed.set(None);
        return;
    }
    let (tilt_x, tilt_y) = axes_to_tilt(tilt_radians, orientation_radians);
    // W3C twist describes rotation around the pen axis. Android orientation
    // describes its direction on the screen, NOT barrel twist. Keep twist at
    // zero rather than passing a misleading angle to the paint engine.
    feed.set(Some(PenSample {
        pressure,
        tilt_x,
        tilt_y,
        rotation: 0.0,
        eraser: eraser != 0,
    }));
}

/// Native Android S Pen touch event. Coordinates are physical window pixels.
#[derive(Clone, Copy, Debug)]
pub struct PenTouch {
    pub action: i32,
    pub pointer_id: i32,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

static TOUCH_EVENTS: OnceLock<Mutex<VecDeque<PenTouch>>> = OnceLock::new();

fn touch_events() -> &'static Mutex<VecDeque<PenTouch>> {
    TOUCH_EVENTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Take the Java events once per egui frame. Native winit events remain authoritative.
pub fn drain_touch_events() -> Vec<PenTouch> {
    touch_events().lock().unwrap_or_else(PoisonError::into_inner).drain(..).collect()
}

/// JNI receives coordinates and Android ACTION_DOWN/UP/MOVE/CANCEL from Java.
/// Do not inject egui events here: the callback runs on the Android UI thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_storyteller_photocraft_PhotocraftActivity_nativeStylusTouch(
    _env: *mut std::ffi::c_void,
    _activity: *mut std::ffi::c_void,
    action: i32,
    pointer_id: i32,
    x: f32,
    y: f32,
    pressure: f32,
) {
    if !x.is_finite() || !y.is_finite() { return; }
    let mut q = touch_events().lock().unwrap_or_else(PoisonError::into_inner);
    if q.len() >= 1024 { q.pop_front(); }
    q.push_back(PenTouch { action, pointer_id, x, y, pressure: pressure.clamp(0.0, 1.0) });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upright_stylus_has_no_tilt() {
        assert_eq!(axes_to_tilt(0.0, 0.0), (0.0, 0.0));
    }

    #[test]
    fn tilting_right_produces_negative_x() {
        let (x, y) = axes_to_tilt(std::f32::consts::FRAC_PI_4, std::f32::consts::FRAC_PI_2);
        assert!((x + 45.0).abs() < 0.01, "x={x}");
        assert!(y.abs() < 0.01, "y={y}");
    }

    #[test]
    fn axes_are_sanitized() {
        assert_eq!(axes_to_tilt(f32::NAN, 0.0), (0.0, 0.0));
        let (x, y) = axes_to_tilt(1000.0, -100.0);
        assert!(x.is_finite() && y.is_finite());
    }
}
