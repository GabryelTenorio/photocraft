//! The Windows Ink pen reader: the pen path when the driver serves Windows Ink (the default for
//! Wacom, Huion, XP-Pen, Surface and every other HID pen).
//!
//! winit 0.30 turns a `WM_POINTER` pen frame into a touch with a normalised force and drops the
//! rest of `POINTER_PEN_INFO`: tilt, barrel rotation, the eraser end, hover, and every frame the
//! system coalesced between two messages. This module subclasses the window and reads all of it
//! with `GetPointerPenInfoHistory` before passing each message on, so winit and egui keep
//! emulating the mouse exactly as before. It synthesizes no input and applies no curve; it only
//! reports each frame, oldest first, through the same callback as the other platforms — a pen in
//! range (hovering or touching) is `Some`, back to the mouse is `None`.
//!
//! As in `appkit`, `xi` and `wintab`, all decoding (message ids, the wParam pointer id, flag and
//! mask bits, ranges, hover vs contact, the eraser end, history order and its cap) is pure and
//! unit-tested on every platform; the Windows glue is the gated `win` module.

use crate::{Sample, Update};

/// `winuser.h` message ids of the pointer messages this reader looks at.
pub mod msg {
    pub const WM_POINTERUPDATE: u32 = 0x0245;
    pub const WM_POINTERDOWN: u32 = 0x0246;
    pub const WM_POINTERUP: u32 = 0x0247;
    pub const WM_POINTERENTER: u32 = 0x0249;
    pub const WM_POINTERLEAVE: u32 = 0x024A;
}

/// `POINTER_FLAG_*` (`winuser.h`): the frame's pointer state.
pub mod pointer_flag {
    pub const INRANGE: u32 = 0x0000_0002;
    pub const INCONTACT: u32 = 0x0000_0004;
    pub const CANCELED: u32 = 0x0000_8000;
}

/// `PEN_FLAG_*` (`winuser.h`): which end and buttons of the pen are in use.
pub mod pen_flag {
    pub const BARREL: u32 = 0x0000_0001;
    pub const INVERTED: u32 = 0x0000_0002;
    pub const ERASER: u32 = 0x0000_0004;
}

/// `PEN_MASK_*` (`winuser.h`): which optional `POINTER_PEN_INFO` fields the device reports.
pub mod pen_mask {
    pub const PRESSURE: u32 = 0x0000_0001;
    pub const ROTATION: u32 = 0x0000_0002;
    pub const TILT_X: u32 = 0x0000_0004;
    pub const TILT_Y: u32 = 0x0000_0008;
}

/// `POINTER_PEN_INFO::pressure` full scale: the API normalises every pen to 0..1024.
pub const PRESSURE_MAX: f32 = 1024.0;

/// Most coalesced frames read per message. Windows keeps a bounded history itself; this caps the
/// buffer a hostile or broken `historyCount` could ask for.
pub const HISTORY_CAP: u32 = 256;

/// The pointer id of a `WM_POINTER*` message: wParam's low word (`GET_POINTERID_WPARAM`).
pub fn pointer_id(wparam: usize) -> u32 {
    wparam as u32 & 0xFFFF
}

/// How many history entries to read for a frame whose `historyCount` is `count`: at least the
/// frame itself, at most [`HISTORY_CAP`].
pub fn history_len(count: u32) -> u32 {
    count.clamp(1, HISTORY_CAP)
}

/// One `POINTER_PEN_INFO`, the fields this reader decodes (the Windows glue copies them out).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// `pointerInfo.pointerFlags` ([`pointer_flag`]).
    pub pointer_flags: u32,
    /// `penFlags` ([`pen_flag`]).
    pub pen_flags: u32,
    /// `penMask` ([`pen_mask`]).
    pub pen_mask: u32,
    /// 0..1024 when [`pen_mask::PRESSURE`] is set.
    pub pressure: u32,
    /// Degrees clockwise 0..359 when [`pen_mask::ROTATION`] is set.
    pub rotation: u32,
    /// Degrees -90..90, positive to the right, when [`pen_mask::TILT_X`] is set.
    pub tilt_x: i32,
    /// Degrees -90..90, positive towards the user, when [`pen_mask::TILT_Y`] is set.
    pub tilt_y: i32,
}

impl Frame {
    fn has(&self, mask: u32) -> bool {
        self.pen_mask & mask != 0
    }

    /// The frame as a [`Sample`]. Windows Ink already uses the sample's conventions (tilt in
    /// degrees with W3C signs, rotation clockwise in degrees), so only the pressure is scaled. A
    /// field the device doesn't report reads as the sample's neutral value: no tilt, no rotation,
    /// and full pressure in contact for a pen without a pressure sensor (it paints like a mouse,
    /// never at zero). Hovering, and a contact the system cancelled, report pressure 0.
    pub fn sample(&self) -> Sample {
        let contact = self.pointer_flags & pointer_flag::INCONTACT != 0 && self.pointer_flags & pointer_flag::CANCELED == 0;
        let pressure = match (contact, self.has(pen_mask::PRESSURE)) {
            (false, _) => 0.0,
            (true, true) => (self.pressure as f32 / PRESSURE_MAX).clamp(0.0, 1.0),
            (true, false) => 1.0,
        };
        Sample {
            pressure,
            tilt_x: if self.has(pen_mask::TILT_X) { self.tilt_x as f32 } else { 0.0 },
            tilt_y: if self.has(pen_mask::TILT_Y) { self.tilt_y as f32 } else { 0.0 },
            rotation: if self.has(pen_mask::ROTATION) { self.rotation as f32 } else { 0.0 },
            eraser: self.pen_flags & (pen_flag::INVERTED | pen_flag::ERASER) != 0,
        }
        .sanitized()
    }

    /// The pen has left hover range.
    fn out_of_range(&self) -> bool {
        self.pointer_flags & pointer_flag::INRANGE == 0
    }
}

/// What one `WM_POINTER*` message reports for a pen. Pure, so every message is unit-tested on
/// every platform.
///
/// - `WM_POINTERENTER`/`UPDATE`/`DOWN`/`UP`: one sample per frame, oldest first (`frames` is the
///   message's coalesced history in the order the system returns it, newest first). A frame out
///   of hover range ends the pen.
/// - `WM_POINTERLEAVE`: the pen left the window or hover range; input is the mouse's again.
/// - Anything else, or a pointer that isn't a pen (mouse, finger), reports nothing: those keep
///   winit's own path.
pub fn message(msg: u32, is_pen: bool, frames_newest_first: &[Frame]) -> Vec<Update> {
    if !is_pen {
        return Vec::new();
    }
    match msg {
        msg::WM_POINTERLEAVE => vec![Update::Set(None)],
        msg::WM_POINTERENTER | msg::WM_POINTERUPDATE | msg::WM_POINTERDOWN | msg::WM_POINTERUP => {
            frames_newest_first.iter().rev().map(|f| Update::Set(if f.out_of_range() { None } else { Some(f.sample()) })).collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(target_os = "windows")]
pub use win::Monitor;

/// The Windows glue: subclass the window and read each pen message's `POINTER_PEN_INFO` history.
/// Real Win32 calls, so `unsafe`, allowed on this module alone (the crate denies it everywhere
/// else).
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod win {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::Pointer::{GetPointerPenInfo, GetPointerPenInfoHistory, GetPointerType, POINTER_PEN_INFO};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, POINTER_INPUT_TYPE, PT_PEN};

    use super::*;
    use crate::{Error, deliver};

    /// The subclass id (`'pcik'`), distinct from the Wintab reader's, and `WM_NCDESTROY`, the last
    /// message a window receives.
    const SUBCLASS_ID: usize = 0x7063_696B;
    const WM_NCDESTROY: u32 = 0x0082;

    /// The proc's state. Only the window's thread touches it.
    struct Data {
        callback: Box<dyn Fn(Option<Sample>)>,
        /// 1 once the proc freed `self` (`WM_NCDESTROY`); `Monitor::drop` must then not touch it.
        freed: *mut AtomicUsize,
    }

    /// Keeps the subclass alive; dropping it removes the subclass. The proc runs on the window's
    /// thread, so install and drop on that thread.
    pub struct Monitor {
        hwnd: HWND,
        /// The `Box<Data>` the proc reads; freed in `Drop` after the subclass is removed.
        data: *mut Data,
        /// 1 once the proc freed `data`; outside `Data` so `Drop` can read it without touching
        /// freed memory.
        freed: *mut AtomicUsize,
    }

    impl Monitor {
        /// Subclass the window (the eframe/winit window's `HWND`) and report Windows Ink pen
        /// samples through `callback`. Every message still reaches winit unchanged.
        pub fn install(hwnd: *mut c_void, callback: impl Fn(Option<Sample>) + 'static) -> Result<Self, Error> {
            let hwnd = HWND(hwnd);
            if hwnd.is_invalid() {
                return Err(Error::Unsupported("no window handle".into()));
            }
            let freed = Box::into_raw(Box::new(AtomicUsize::new(0)));
            let data = Box::into_raw(Box::new(Data { callback: Box::new(callback), freed }));
            // SAFETY: `hwnd` is the caller's live window; `data` is a `Box<Data>` the proc reads
            // until `Drop` removes the subclass or the proc frees it at `WM_NCDESTROY`;
            // `subclass_proc` is a plain `extern "system"` fn.
            if !unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, data as usize) }.as_bool() {
                // SAFETY: the subclass was not installed, so the proc never runs: free both boxes.
                unsafe {
                    drop(Box::from_raw(data));
                    drop(Box::from_raw(freed));
                }
                return Err(Error::Platform("SetWindowSubclass failed".into()));
            }
            Ok(Monitor { hwnd, data, freed })
        }
    }

    impl Drop for Monitor {
        fn drop(&mut self) {
            // SAFETY: `self.freed` is a live `AtomicUsize` until this function frees it exactly
            // once; `Acquire` pairs with the proc's `Release` store at `WM_NCDESTROY`, and a
            // nonzero flag means the proc already freed `data`.
            unsafe {
                if (*self.freed).load(Ordering::Acquire) != 0 {
                    drop(Box::from_raw(self.freed));
                    return;
                }
                // Removing the subclass on the installing thread ends the callbacks; a destroyed
                // window (`!IsWindow`) can't deliver any either. Only then is `data` safe to free.
                // If removal failed while the window lives, the proc may still run at
                // `WM_NCDESTROY`, so leak both rather than free memory a message could read.
                if RemoveWindowSubclass(self.hwnd, Some(subclass_proc), SUBCLASS_ID).as_bool() || !IsWindow(Some(self.hwnd)).as_bool() {
                    drop(Box::from_raw(self.data));
                    drop(Box::from_raw(self.freed));
                }
            }
        }
    }

    /// Is this pointer a pen? A failed query (the pointer is already gone) is no pen.
    fn is_pen(id: u32) -> bool {
        let mut kind = POINTER_INPUT_TYPE::default();
        // SAFETY: `kind` lives for the call, which writes one `POINTER_INPUT_TYPE`.
        unsafe { GetPointerType(id, &mut kind) }.is_ok() && kind == PT_PEN
    }

    /// The message's frames, newest first: the current frame's `historyCount` says how many the
    /// system coalesced into it (capped by [`history_len`]). Empty when the pointer is gone.
    fn frames(id: u32) -> Vec<Frame> {
        let mut info = POINTER_PEN_INFO::default();
        // SAFETY: `info` lives for the call, which writes one `POINTER_PEN_INFO`.
        if unsafe { GetPointerPenInfo(id, &mut info) }.is_err() {
            return Vec::new();
        }
        let mut count = history_len(info.pointerInfo.historyCount);
        let mut history = vec![POINTER_PEN_INFO::default(); count as usize];
        if count > 1 {
            // SAFETY: `history` holds `count` entries and `count` says so; the call writes at most
            // that many and stores how many it wrote back into `count`.
            if unsafe { GetPointerPenInfoHistory(id, &mut count, Some(history.as_mut_ptr())) }.is_err() {
                history = vec![info];
            } else {
                history.truncate(count.min(HISTORY_CAP) as usize);
            }
        } else {
            history = vec![info];
        }
        history
            .iter()
            .map(|p| Frame {
                pointer_flags: p.pointerInfo.pointerFlags.0,
                pen_flags: p.penFlags,
                pen_mask: p.penMask,
                pressure: p.pressure,
                rotation: p.rotation,
                tilt_x: p.tiltX,
                tilt_y: p.tiltY,
            })
            .collect()
    }

    /// The subclass proc: reports Windows Ink pen frames through the callback and passes every
    /// message on to winit.
    unsafe extern "system" fn subclass_proc(hwnd: HWND, m: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, dwref: usize) -> LRESULT {
        if m == WM_NCDESTROY {
            // SAFETY: Microsoft's pattern for a subclassed window being destroyed: release the
            // subclass from inside the `WM_NCDESTROY` handler, while the window still exists.
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            }
            if dwref != 0 {
                // SAFETY: `dwref` is the `Box<Data>` `install` leaked; `WM_NCDESTROY` is the last
                // message, so it is freed exactly once here, after storing the flag
                // `Monitor::drop` reads.
                unsafe {
                    let data = Box::from_raw(dwref as *mut Data);
                    (*data.freed).store(1, Ordering::Release);
                }
            }
            // SAFETY: plain pass-through; the chain must still see the destruction.
            return unsafe { DefSubclassProc(hwnd, m, wparam, lparam) };
        }
        if dwref != 0 && (msg::WM_POINTERUPDATE..=msg::WM_POINTERLEAVE).contains(&m) {
            // The body is panic-contained: an unwind out of `extern "system"` aborts the process,
            // and these frames carry driver data.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // SAFETY: `dwref` is the `Box<Data>` `install` leaked; it stays alive until this
                // proc frees it at `WM_NCDESTROY` or `Drop` removes the subclass, and no message
                // arrives after either.
                let data = unsafe { &*(dwref as *const Data) };
                let id = pointer_id(wparam.0);
                let pen = is_pen(id);
                let frames = if pen && m != msg::WM_POINTERLEAVE { frames(id) } else { Vec::new() };
                for update in message(m, pen, &frames) {
                    if let Update::Set(sample) = update {
                        deliver(&*data.callback, sample);
                    }
                }
            }));
        }
        // SAFETY: plain pass-through: winit handles every pointer message exactly as before.
        unsafe { DefSubclassProc(hwnd, m, wparam, lparam) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(pressure: u32) -> Frame {
        Frame { pointer_flags: pointer_flag::INRANGE | pointer_flag::INCONTACT, pen_mask: pen_mask::PRESSURE, pressure, ..Default::default() }
    }

    fn samples(updates: &[Update]) -> Vec<Option<Sample>> {
        updates
            .iter()
            .map(|u| match u {
                Update::Set(s) => *s,
                Update::Keep => panic!("ink never keeps"),
            })
            .collect()
    }

    #[test]
    fn the_pointer_id_is_wparams_low_word() {
        assert_eq!(pointer_id(0x0006_0001), 1);
        assert_eq!(pointer_id(0xFFFF_FFFF_1234_ABCD), 0xABCD);
    }

    #[test]
    fn history_length_is_at_least_one_and_capped() {
        assert_eq!(history_len(0), 1);
        assert_eq!(history_len(7), 7);
        assert_eq!(history_len(u32::MAX), HISTORY_CAP);
    }

    #[test]
    fn a_contact_frame_maps_every_axis() {
        let f = Frame {
            pointer_flags: pointer_flag::INRANGE | pointer_flag::INCONTACT,
            pen_mask: pen_mask::PRESSURE | pen_mask::ROTATION | pen_mask::TILT_X | pen_mask::TILT_Y,
            pressure: 512,
            rotation: 270,
            tilt_x: 30,
            tilt_y: -45,
            ..Default::default()
        };
        assert_eq!(f.sample(), Sample { pressure: 0.5, tilt_x: 30.0, tilt_y: -45.0, rotation: 270.0, eraser: false });
    }

    #[test]
    fn unreported_axes_are_neutral_and_a_pressureless_pen_paints_full() {
        // A pen that reports only position: full pressure in contact (like a mouse), no tilt or
        // rotation even if the fields hold garbage.
        let f =
            Frame { pointer_flags: pointer_flag::INRANGE | pointer_flag::INCONTACT, pressure: 3, rotation: 90, tilt_x: 60, tilt_y: 60, ..Default::default() };
        assert_eq!(f.sample(), Sample { pressure: 1.0, tilt_x: 0.0, tilt_y: 0.0, rotation: 0.0, eraser: false });
    }

    #[test]
    fn hover_and_cancelled_contacts_report_zero_pressure() {
        let hover =
            Frame { pointer_flags: pointer_flag::INRANGE, pen_mask: pen_mask::PRESSURE | pen_mask::TILT_X, pressure: 700, tilt_x: 20, ..Default::default() };
        assert_eq!(hover.sample(), Sample { pressure: 0.0, tilt_x: 20.0, tilt_y: 0.0, rotation: 0.0, eraser: false }, "hover keeps tilt for the cursor");
        let cancelled = Frame { pointer_flags: pointer_flag::INRANGE | pointer_flag::INCONTACT | pointer_flag::CANCELED, ..contact(900) };
        assert_eq!(cancelled.sample().pressure, 0.0);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let f = Frame {
            pen_mask: pen_mask::PRESSURE | pen_mask::ROTATION | pen_mask::TILT_X | pen_mask::TILT_Y,
            rotation: 725,
            tilt_x: -500,
            tilt_y: i32::MAX,
            ..contact(u32::MAX)
        };
        assert_eq!(f.sample(), Sample { pressure: 1.0, tilt_x: -90.0, tilt_y: 90.0, rotation: 5.0, eraser: false });
    }

    #[test]
    fn the_inverted_pen_and_the_eraser_button_are_the_eraser_end() {
        for flags in [pen_flag::INVERTED, pen_flag::ERASER, pen_flag::INVERTED | pen_flag::ERASER] {
            assert!(Frame { pen_flags: flags, ..contact(100) }.sample().eraser, "{flags:#x}");
        }
        assert!(!Frame { pen_flags: pen_flag::BARREL, ..contact(100) }.sample().eraser, "the barrel button is no eraser");
    }

    #[test]
    fn coalesced_history_is_reported_oldest_first() {
        // The system returns the newest frame first; the feed needs them in time order so the
        // stroke interpolates pressure across the frame instead of jumping (#2046).
        let newest_first = [contact(900), contact(600), contact(300)];
        let got = samples(&message(msg::WM_POINTERUPDATE, true, &newest_first));
        let pressures: Vec<f32> = got.iter().map(|s| s.map_or(-1.0, |s| s.pressure)).collect();
        assert_eq!(pressures, [300.0 / 1024.0, 600.0 / 1024.0, 900.0 / 1024.0]);
    }

    #[test]
    fn a_pen_session_enters_touches_lifts_and_leaves() {
        let hover = Frame { pointer_flags: pointer_flag::INRANGE, pen_mask: pen_mask::PRESSURE, ..Default::default() };
        assert_eq!(samples(&message(msg::WM_POINTERENTER, true, &[hover])), [Some(Sample { pressure: 0.0, ..Sample::default() })]);
        assert_eq!(samples(&message(msg::WM_POINTERDOWN, true, &[contact(256)])), [Some(Sample { pressure: 0.25, ..Sample::default() })]);
        // WM_POINTERUP's frame has left contact: the lift reads as hover.
        assert_eq!(samples(&message(msg::WM_POINTERUP, true, &[hover])), [Some(Sample { pressure: 0.0, ..Sample::default() })]);
        assert_eq!(samples(&message(msg::WM_POINTERLEAVE, true, &[])), [None]);
        // A frame that left hover range ends the pen even without a leave message.
        let gone = Frame { pointer_flags: 0, ..hover };
        assert_eq!(samples(&message(msg::WM_POINTERUPDATE, true, &[gone])), [None]);
    }

    #[test]
    fn mice_fingers_and_other_messages_report_nothing() {
        assert!(message(msg::WM_POINTERUPDATE, false, &[contact(512)]).is_empty(), "not a pen: winit's own path");
        assert!(message(msg::WM_POINTERLEAVE, false, &[]).is_empty());
        assert!(message(0x0248 /* WM_POINTERACTIVATE */, true, &[contact(512)]).is_empty());
        assert!(message(msg::WM_POINTERUPDATE, true, &[]).is_empty(), "a vanished pointer reports nothing");
    }
}
