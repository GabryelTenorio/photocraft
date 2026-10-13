//! Canvas multi-touch ownership, centroid zoom/pan, and stroke suppression.
//! Keep this module independent of Android: Windows touch and offscreen tests use it too.
use crate::{PhotocraftApp, canvas::ViewXform, state::View};
use egui::{Context, Pos2, Vec2};

/// Returns true through the last finger release, so callers suppress every tool input path.
pub(crate) fn navigate(app: &mut PhotocraftApp, ctx: &Context, idx: usize, xf: &ViewXform, view: &mut View, primary: bool, ppp: f32) -> bool {
    let touch_id = egui::Id::new(("pc-canvas-touch", idx));
    let (touch, any_touches) = ctx.input(|i| (i.multi_touch(), i.any_touches()));
    let was_touch = ctx.data(|d| d.get_temp::<bool>(touch_id)).unwrap_or(false);
    let owns_touch = was_touch || touch.is_some_and(|t| xf.rect.contains(t.start_pos));
    // Keep tools suppressed until all fingers lift, including the final pointer release.
    let touch_navigation = owns_touch && (was_touch || touch.is_some());
    ctx.data_mut(|d| d.insert_temp(touch_id, owns_touch && any_touches));
    if touch_navigation {
        if let Some(touch) = touch {
            touch_navigate(view, xf, touch.center_pos, touch.translation_delta, touch.zoom_delta, ppp);
        }
        if primary {
            // Live brush pixels are a preview: dropping it cannot commit a stray stroke.
            app.drag = None;
            app.live_stroke = None;
            app.press_stroke = false;
        }
        ctx.request_repaint();
    }
    touch_navigation
}

/// Move the document point beneath the previous touch centroid to the new centroid.
pub(crate) fn touch_navigate(view: &mut View, xf: &ViewXform, center: Pos2, translation: Vec2, factor: f32, ppp: f32) {
    if !center.is_finite() || !translation.is_finite() || !factor.is_finite() || factor <= 0.0 {
        return;
    }
    let new_zoom = crate::zoom_levels::clamp(view.zoom * factor, view.doc_size);
    crate::canvas::zoom_about(view, xf, center - translation, new_zoom, false, ppp);
    let delta = xf.unmap_vec(translation) / (view.zoom / ppp);
    view.center[0] -= delta.x;
    view.center[1] -= delta.y;
}
