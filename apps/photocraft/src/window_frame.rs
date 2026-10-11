//! Keeps the custom (undecorated) title bar window truly borderless (#2246, #2850, #3144).
//!
//! winit 0.30 gives an undecorated window the `WS_CAPTION | WS_BORDER | WS_THICKFRAME` and
//! `WS_EX_WINDOWEDGE` styles and hides the non-client area by spreading the client rect over it
//! (`WM_NCCALCSIZE`). Whenever `WS_THICKFRAME` is set on Windows, the kernel also forces
//! `WS_EX_WINDOWEDGE` on and reports a 7 px sizing border in `WINDOWINFO` (`cxWindowBorders`,
//! `cyWindowBorders`) and `AdjustWindowRectEx`.
//!
//! On Windows 10 with Intel Haswell OpenGL (HD Graphics 4600 #2246, Iris Pro Graphics 5200 #2850),
//! that non-zero sizing border causes `DefWindowProcW` to paint the classic Windows Basic frame
//! and the OpenGL ICD (`ig75icd64.dll`) to offset the rendered surface down and right by `(7, 7)`
//! pixels while pointer input stays at the client origin. In addition, maximizing inflates the
//! outer window rect by the sizing border across the full monitor (covering the taskbar, #3144)
//! while `WM_NCCALCSIZE` constrains the client rect to the monitor's work area (`rcWork`).
//!
//! This module strips `WS_CAPTION`, `WS_BORDER`, `WS_THICKFRAME`, and `WS_EX_WINDOWEDGE` so the
//! window has zero non-client borders (`cxWindowBorders == 0, cyWindowBorders == 0`), and snaps
//! the outer window rect to `rcWork` when maximized (clear of the taskbar, #3144). It also sets
//! `ViewportCommand::Decorations(true)` on winit so `WM_NCCALCSIZE` forwards `SWP_FRAMECHANGED` to
//! `DefWindowProcW` once `WS_CAPTION` is removed — clearing the kernel's internal `WFCPRESENT` flag
//! (`win32kfull.sys` `xxxCalcClientRect`, which otherwise keeps painting the classic caption bar on
//! `WM_NCACTIVATE` and `SetWindowTextW`) and avoiding winit's 1 px `MARKER_UNDECORATED_SHADOW`
//! client offset. When [`photocraft_ui_egui::titlebar::resize_zones`] hands off an edge resize
//! (`titlebar::resizing`), `WS_THICKFRAME` is temporarily re-enabled without `SWP_FRAMECHANGED`
//! before `ViewportCommand::BeginResize` posts `WM_NCLBUTTONDOWN`, then stripped again once the
//! mouse button is released.
//!
//! The Windows calls go through safe `winsafe` wrappers; the workspace forbids `unsafe`, and
//! winsafe can't wrap a raw handle without it, so the window is found by its handle instead.

pub use platform::service;

#[cfg(target_os = "windows")]
mod platform {
    use eframe::wgpu::rwh::{HasWindowHandle, RawWindowHandle};
    use photocraft_ui_egui::{WindowFrameFn, titlebar};
    use winsafe::{HWND, HwndPlace, POINT, RECT, SIZE, co};

    /// A guard that keeps the window borderless each frame, or `None` when the app uses the
    /// system title bar (the window is decorated) or its handle can't be found.
    pub fn service(cc: &eframe::CreationContext<'_>, custom_titlebar: bool) -> Option<WindowFrameFn> {
        if !custom_titlebar {
            return None;
        }
        let target = match cc.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => h.hwnd.get(),
            _ => return None,
        };
        let hwnd = find_window(target)?;
        cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        cc.egui_ctx.request_repaint();
        strip(&hwnd, false);
        let ctx = cc.egui_ctx.clone();
        let mut resizing = false;
        Some(Box::new(move || {
            if titlebar::resizing(&ctx) {
                resizing = true;
            } else if !primary_mouse_button_down() {
                resizing = false;
            }
            strip(&hwnd, resizing);
        }))
    }

    /// The top-level window with the given raw handle. `EnumWindows` hands out real `HWND`s, so
    /// this matches winit's own window without wrapping a raw handle (which needs `unsafe`).
    fn find_window(target: isize) -> Option<HWND> {
        let mut found = None;
        let _ = winsafe::EnumWindows(|hwnd| {
            if hwnd.ptr() as isize == target {
                found = Some(hwnd);
                false
            } else {
                true
            }
        });
        found
    }

    /// Whether the OS primary mouse button is currently held down.
    fn primary_mouse_button_down() -> bool {
        let swapped = winsafe::GetSystemMetrics(co::SM::SWAPBUTTON) != 0;
        let vk = if swapped { co::VK::RBUTTON } else { co::VK::LBUTTON };
        let (down, _) = winsafe::GetKeyState(vk);
        down
    }

    /// Removes `WS_CAPTION`, `WS_BORDER`, `WS_THICKFRAME` (unless `allow_resize` is active), and
    /// `WS_EX_WINDOWEDGE`, and has Windows recalculate the frame. When maximized, snaps the outer
    /// window rect to the monitor's work area (`rcWork`) so the sizing-border inflation doesn't
    /// leak past the screen edges. A no-op once the styles are clean and the rect matches, so it
    /// is cheap to call every frame.
    fn strip(hwnd: &HWND, allow_resize: bool) {
        let style = hwnd.style();
        let maximized = style.has(co::WS::MAXIMIZE);
        let resize = allow_resize && !maximized;
        let target_style = borderless(style, resize);
        let ex = hwnd.style_ex();
        let target_ex = borderless_ex(ex);

        // Windows forces `WS_EX_WINDOWEDGE` back on whenever `WS_THICKFRAME` is set. Only treat
        // `THICKFRAME` or `ex` as dirty when we are not temporarily keeping `WS_THICKFRAME` for an
        // active edge-resize drag.
        let caption_or_border = (style.raw() & (co::WS::CAPTION | co::WS::BORDER).raw()) != 0;
        let style_dirty = caption_or_border || (!resize && (style.has(co::WS::THICKFRAME) || target_ex.raw() != ex.raw()));

        let cur_rect = hwnd.GetWindowRect().ok();
        let maximized_work = if maximized { work_area(hwnd) } else { None };
        let rect_dirty = match (maximized_work, cur_rect) {
            (Some(work), Some(cur)) => !same_rect(cur, work),
            (Some(_), None) => true,
            (None, _) => false,
        };

        if !style_dirty && !rect_dirty {
            if resize && !style.has(co::WS::THICKFRAME) {
                hwnd.set_style(target_style);
            }
            return;
        }

        hwnd.set_style(target_style);
        hwnd.set_style_ex(target_ex);

        let flags = co::SWP::FRAMECHANGED | co::SWP::NOZORDER | co::SWP::NOACTIVATE;
        if let Some(work) = maximized_work {
            let (x, y, w, h) = rect_xywh(work);
            let _ = hwnd.SetWindowPos(HwndPlace::None, POINT::with(x, y), SIZE::with(w, h), flags);
        } else {
            let _ = hwnd.SetWindowPos(HwndPlace::None, POINT::new(), SIZE::new(), flags | co::SWP::NOMOVE | co::SWP::NOSIZE);
        }
    }

    /// Work area (`rcWork`, excluding the taskbar) of the monitor nearest `hwnd`.
    fn work_area(hwnd: &HWND) -> Option<RECT> {
        let monitor = hwnd.MonitorFromWindow(co::MONITOR::DEFAULTTONEAREST);
        Some(monitor.GetMonitorInfo().ok()?.rcWork)
    }

    fn same_rect(a: RECT, b: RECT) -> bool {
        a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
    }

    fn rect_xywh(r: RECT) -> (i32, i32, i32, i32) {
        (r.left, r.top, r.right.saturating_sub(r.left), r.bottom.saturating_sub(r.top))
    }

    /// `style` without the OS title bar, border, and sizing border (`WS_CAPTION | WS_BORDER |
    /// WS_THICKFRAME`), keeping `WS_THICKFRAME` only while `allow_resize` is true so native edge
    /// resizing (`WM_NCLBUTTONDOWN` -> `SC_SIZE`) still works.
    fn borderless(style: co::WS, allow_resize: bool) -> co::WS {
        let stripped = style & !(co::WS::CAPTION | co::WS::BORDER | co::WS::THICKFRAME);
        if allow_resize { stripped | co::WS::THICKFRAME } else { stripped }
    }

    /// `ex_style` without 3D/raised window border edges (`WS_EX_WINDOWEDGE`, etc.). These
    /// contribute to the non-client area that the Intel WGL ICD interprets as an offset.
    fn borderless_ex(ex_style: co::WS_EX) -> co::WS_EX {
        ex_style & !(co::WS_EX::WINDOWEDGE | co::WS_EX::CLIENTEDGE | co::WS_EX::DLGMODALFRAME | co::WS_EX::STATICEDGE)
    }

    #[cfg(test)]
    mod tests {
        use super::{borderless, borderless_ex, rect_xywh, same_rect};
        use winsafe::{RECT, co};

        #[test]
        fn the_borderless_style_drops_caption_border_and_thickframe_unless_resizing() {
            let decorated = co::WS::CAPTION | co::WS::BORDER | co::WS::SYSMENU | co::WS::THICKFRAME | co::WS::MAXIMIZEBOX | co::WS::MINIMIZEBOX;
            let bare = borderless(decorated, false);
            assert!(!bare.has(co::WS::CAPTION), "caption gone");
            assert!(!bare.has(co::WS::BORDER), "border gone");
            assert!(!bare.has(co::WS::THICKFRAME), "thickframe gone when idle");
            assert!(bare.has(co::WS::MAXIMIZEBOX) && bare.has(co::WS::SYSMENU), "window buttons kept");
            assert_eq!(bare.raw(), borderless(bare, false).raw());

            let resizable = borderless(decorated, true);
            assert!(!resizable.has(co::WS::CAPTION));
            assert!(!resizable.has(co::WS::BORDER));
            assert!(resizable.has(co::WS::THICKFRAME), "thickframe kept during edge resize");

            let ex = co::WS_EX::WINDOWEDGE | co::WS_EX::ACCEPTFILES | co::WS_EX::APPWINDOW;
            let bare_ex = borderless_ex(ex);
            assert!(!bare_ex.has(co::WS_EX::WINDOWEDGE), "window edge gone");
            assert!(bare_ex.has(co::WS_EX::ACCEPTFILES) && bare_ex.has(co::WS_EX::APPWINDOW));
            assert_eq!(bare_ex.raw(), borderless_ex(bare_ex).raw());
        }

        #[test]
        fn work_area_rect_comparison_and_dimensions() {
            let work = RECT { left: 0, top: 30, right: 1920, bottom: 1080 };
            let inflated = RECT { left: -7, top: -7, right: 1927, bottom: 1087 };
            assert!(same_rect(work, work));
            assert!(!same_rect(inflated, work));
            assert_eq!(rect_xywh(work), (0, 30, 1920, 1050));
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use photocraft_ui_egui::WindowFrameFn;

    pub fn service(_cc: &eframe::CreationContext<'_>, _custom_titlebar: bool) -> Option<WindowFrameFn> {
        None
    }
}
