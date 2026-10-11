//! View extras: rulers (⌘R), grid (⌘'), guides (⌘;). Guides are dragged out of the rulers and
//! moved with the Move tool (drag one off the canvas to delete it), all through undoable engine
//! commands (`view.newGuide` / `view.moveGuide` / `view.deleteGuide`).

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use photocraft_doc::Document;
use photocraft_engine::prefs::{GuidesGridAndSlices, Unit};
use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::ViewXform;
use crate::theme::Tokens;

pub const RULER: f32 = 16.0;

/// A `#rrggbb` preference colour.
fn pref_color(s: &str, fallback: Color32) -> Color32 {
    photocraft_engine::prefs::parse_hex(s).map_or(fallback, |c| Color32::from_rgb(c[0], c[1], c[2]))
}

/// Draw a line in a Guides/Grid preference style (lines, dashed lines, dots).
fn styled_line(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke, style: photocraft_engine::prefs::LineStyle) {
    use photocraft_engine::prefs::LineStyle;
    match style {
        LineStyle::Lines => {
            painter.line_segment([a, b], stroke);
        }
        LineStyle::Dashed => {
            painter.extend(egui::Shape::dashed_line(&[a, b], stroke, 4.0, 3.0));
        }
        LineStyle::Dots => {
            painter.extend(egui::Shape::dotted_line(&[a, b], stroke.color, 3.0, 0.6));
        }
    }
}

/// Ruler step in units so labelled ticks are at least `min` units apart: …0.1, 0.25, 0.5, 1, 2, 5,
/// 10… (whole steps only for pixels).
fn nice_step(min: f64, whole: bool) -> f64 {
    let mut step = if whole { 1.0 } else { 0.001 };
    loop {
        for m in [1.0, 2.0, 2.5, 5.0] {
            if (m != 2.5 || !whole) && step * m >= min {
                return step * m;
            }
        }
        step *= 10.0;
    }
}

/// English display name for a length unit, as shown in Preferences › Units & Rulers.
fn unit_label(u: Unit) -> &'static str {
    match u {
        Unit::Pixels => "Pixels",
        Unit::Inches => "Inches",
        Unit::Centimeters => "Centimeters",
        Unit::Millimeters => "Millimeters",
        Unit::Points => "Points",
        Unit::Picas => "Picas",
        Unit::Percent => "Percent",
    }
}

/// Every length unit, in the Preferences › Units & Rulers order.
const UNITS: [Unit; 7] = [Unit::Pixels, Unit::Inches, Unit::Centimeters, Unit::Millimeters, Unit::Points, Unit::Picas, Unit::Percent];

/// The ruler right-click menu: every length unit with its (translated) label and whether it's the
/// one in effect. Picking one changes the ruler unit exactly as the preference pane does.
fn unit_menu_items(current: Unit) -> Vec<(String, &'static str, bool)> {
    UNITS.iter().map(|&u| (tl!(unit_label(u)).to_string(), u.name(), u == current)).collect()
}

/// Right-clicking a ruler offers the length units; the chosen one's name is returned (`None` if the
/// menu is dismissed without a pick).
fn ruler_unit_menu(ui: &mut egui::Ui, current: Unit) -> Option<&'static str> {
    crate::widgets::menu_scroll(ui, |ui| {
        ui.set_min_width(150.0);
        let mut chosen = None;
        for (label, name, checked) in unit_menu_items(current) {
            let label = if checked { format!("✓ {label}") } else { label };
            if ui.button(label).clicked() {
                ui.close();
                chosen = Some(name);
            }
        }
        chosen
    })
}

/// A guide being dragged: from a ruler (new) or an existing one (index).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuideDrag {
    pub vertical: bool,
    pub index: Option<usize>,
    pub pos: f64,
}

/// Canvas area left after the rulers.
pub fn content_rect(app: &PhotocraftApp, rect: Rect) -> Rect {
    if app.ui.extras.rulers { Rect::from_min_max(rect.min + vec2(RULER, RULER), rect.max) } else { rect }
}

/// Tick step (document px) so major ticks are at least ~60 screen px apart: 1, 2, 5, 10, 20, 50…
#[cfg(test)]
fn tick_step(zoom: f32) -> f64 {
    nice_step(60.0 / zoom.max(1e-4) as f64, true)
}

/// Grid spacing in document pixels. Percentages use each axis's own extent.
pub(crate) fn grid_major_px(g: &GuidesGridAndSlices, dpi: f64, size: [f64; 2], ppi: f64) -> [f64; 2] {
    size.map(|extent| g.major_px(dpi, extent, ppi))
}

/// View › Show › Grid, from Preferences › Guides, Grid & Slices (spacing, subdivisions, colour,
/// style). Photoshop's default is a gridline every inch with 4 subdivisions.
pub fn draw_grid(app: &PhotocraftApp, painter: &egui::Painter, xf: &ViewXform, doc: &Document) {
    let g = &app.session.prefs().guides_grid_and_slices;
    let ppi = app.session.prefs().units_and_rulers.point_size.per_inch();
    let (w, h) = (doc.size.width as f64, doc.size.height as f64);
    let major = grid_major_px(g, doc.resolution_dpi.max(1.0) as f64, [w, h], ppi);
    let minor = major.map(|step| step / g.subdivisions.max(1) as f64);
    let base = pref_color(&g.grid_color, Color32::from_gray(140));
    for (steps, alpha) in [(minor, 0.45f32), (major, 1.0)] {
        if alpha < 1.0 && g.subdivisions <= 1 {
            continue;
        }
        let st = Stroke::new(1.0, base.gamma_multiply(alpha));
        let [step_x, step_y] = steps;
        if step_x * (xf.zoom_x() as f64) >= 6.0 {
            let mut x = step_x;
            while x < w {
                styled_line(painter, xf.to_screen(x as f32, 0.0), xf.to_screen(x as f32, h as f32), st, g.grid_style);
                x += step_x;
            }
        }
        if step_y * (xf.zoom as f64) >= 6.0 {
            let mut y = step_y;
            while y < h {
                styled_line(painter, xf.to_screen(0.0, y as f32), xf.to_screen(w as f32, y as f32), st, g.grid_style);
                y += step_y;
            }
        }
    }
}

pub(crate) fn parse_guide_color(value: &str) -> Option<Color32> {
    let value = value.strip_prefix('#')?;
    if value.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(value.get(0..2)?, 16).ok()?;
    let g = u8::from_str_radix(value.get(2..4)?, 16).ok()?;
    let b = u8::from_str_radix(value.get(4..6)?, 16).ok()?;
    Some(Color32::from_rgb(r, g, b))
}

pub fn draw_guides(app: &PhotocraftApp, painter: &egui::Painter, xf: &ViewXform, doc: &Document) {
    let clip = painter.clip_rect();
    let drag = app.guide_drag;
    let g = &app.session.prefs().guides_grid_and_slices;
    let preview = crate::guide_layout_ui::preview(app, doc);
    let guides = preview.as_ref().map(|(guides, _)| guides).unwrap_or(&doc.guides);
    let (guide, style) =
        (preview.as_ref().map(|(_, color)| *color).unwrap_or_else(|| pref_color(&g.guide_color, Color32::from_rgb(74, 255, 255))), g.guide_style);
    let line = |vertical: bool, pos: f64, color: Color32| {
        if vertical {
            let x = xf.to_screen(pos as f32, 0.0).x;
            styled_line(painter, pos2(x, clip.top()), pos2(x, clip.bottom()), Stroke::new(1.0, color), style);
        } else {
            let y = xf.to_screen(0.0, pos as f32).y;
            styled_line(painter, pos2(clip.left(), y), pos2(clip.right(), y), Stroke::new(1.0, color), style);
        }
    };
    if app.ui.extras.guides || preview.is_some() {
        for (vertical, list) in [(true, &guides.vertical), (false, &guides.horizontal)] {
            for (i, p) in list.iter().enumerate() {
                if drag.is_some_and(|d| d.vertical == vertical && d.index == Some(i)) {
                    continue;
                }
                line(vertical, *p, guide);
            }
        }
    }
    if let Some(d) = drag {
        line(d.vertical, d.pos, guide);
    }
}

/// Existing guide under a document point (within 4 screen px).
pub fn guide_at(app: &PhotocraftApp, x: f64, y: f64) -> Option<(bool, usize)> {
    // Hidden canvas guides are not interactive: the Move tool must reach the layer below.
    // Keep hit testing consistent with the visibility gate in canvas::draw.
    if !app.ui.extras.guides || !app.ui.view.shows(app.ui.view.show.canvas_guides) || app.ui.extras.lock_guides {
        return None;
    }
    let doc = &app.session.active()?.doc;
    let tol = 4.0 / app.point_zoom().max(0.01) as f64;
    for (i, g) in doc.guides.vertical.iter().enumerate() {
        if (*g - x).abs() <= tol {
            return Some((true, i));
        }
    }
    for (i, g) in doc.guides.horizontal.iter().enumerate() {
        if (*g - y).abs() <= tol {
            return Some((false, i));
        }
    }
    None
}

/// Finish a guide drag: create, move, or delete (dropped outside the canvas).
pub fn finish_drag(app: &mut PhotocraftApp, d: GuideDrag) {
    let Some(size) = app.session.active().map(|s| s.doc.size) else { return };
    let extent = if d.vertical { size.width } else { size.height } as f64;
    let orientation = if d.vertical { "vertical" } else { "horizontal" };
    let inside = d.pos >= 0.0 && d.pos <= extent;
    let pos = (d.pos * 1000.0).round() / 1000.0;
    let _ = match (d.index, inside) {
        (None, true) => app.run("view.newGuide", json!({"orientation": orientation, "position": pos})),
        (Some(i), true) => app.run("view.moveGuide", json!({"orientation": orientation, "index": i, "position": pos})),
        (Some(i), false) => app.run("view.deleteGuide", json!({"orientation": orientation, "index": i})),
        (None, false) => Ok(serde_json::Value::Null),
    };
    if d.index.is_none() && inside {
        app.ui.extras.guides = true;
    }
}

/// Visible ruler range in ruler units, normalised to `(min, max)` so the tick loop runs for
/// flipped or rotated views too (#1814). Shared by the renderer and its tests.
fn ruler_range(xf: &ViewXform, a: Pos2, b: Pos2, axis: usize, px_per_unit: f64) -> (f64, f64) {
    let d0 = xf.to_doc(a)[axis];
    let d1 = xf.to_doc(b)[axis];
    let (u0, u1) = (d0 / px_per_unit, d1 / px_per_unit);
    (u0.min(u1), u0.max(u1))
}

/// Snap a document position to the nearest ruler tick, measured from the zero point `origin`
/// (`tick` px apart). Shift-dragging the zero point lands it on a tick, as in Photoshop.
fn snap_to_tick(px: f64, origin: f64, tick: f64) -> f64 {
    if !(tick.is_finite() && tick > 0.0) {
        return px;
    }
    origin + ((px - origin) / tick).round() * tick
}

/// Where a zero-point drag from the ruler corner lands for the pointer at document `d`: on the
/// nearest ruler tick (`ticks`, px per axis) with Shift, else View › Snap To targets.
fn origin_drop(app: &mut PhotocraftApp, d: [f64; 2], shift: bool, origin: [f64; 2], ticks: [f64; 2]) -> [f64; 2] {
    if shift {
        [snap_to_tick(d[0], origin[0], ticks[0]), snap_to_tick(d[1], origin[1], ticks[1])]
    } else {
        [crate::snap_ui::snap_guide(app, true, d[0]), crate::snap_ui::snap_guide(app, false, d[1])]
    }
}

/// Rulers along the top and left of `full` (the canvas rect before `content_rect`), with the
/// pointer position marked; dragging out of a ruler creates a guide.
pub fn draw_rulers(app: &mut PhotocraftApp, ui: &mut egui::Ui, full: Rect, xf: &ViewXform) {
    let t = Tokens::get(ui.ctx());
    let top = Rect::from_min_max(pos2(full.left() + RULER, full.top()), pos2(full.right(), full.top() + RULER));
    let left = Rect::from_min_max(pos2(full.left(), full.top() + RULER), pos2(full.left() + RULER, full.bottom()));
    let corner = Rect::from_min_size(full.min, vec2(RULER, RULER));
    let p = ui.painter_at(full);
    let bg = t.chrome;
    for r in [top, left, corner] {
        p.rect_filled(r, 0.0, bg);
    }
    p.line_segment([top.left_bottom(), top.right_bottom()], Stroke::new(1.0, t.separator));
    p.line_segment([left.right_top(), left.right_bottom()], Stroke::new(1.0, t.separator));
    let font = egui::FontId::proportional(9.0);
    let tick = Stroke::new(1.0, t.text_faint);
    // Preferences › Units & Rulers: labels in the ruler unit (percent of the document's side).
    let ur = &app.session.prefs().units_and_rulers;
    let (unit, ppi) = (ur.rulers, ur.point_size.per_inch());
    let (dpi, size) =
        app.session.active().map_or((72.0, [1.0, 1.0]), |d| (d.doc.resolution_dpi.max(1.0) as f64, [d.doc.size.width as f64, d.doc.size.height as f64]));
    let whole = unit == photocraft_engine::prefs::Unit::Pixels;
    // The zero point (#2955): rulers read 0 there and negative left of / above it.
    let origin = app.session.active().map_or([0.0, 0.0], |d| d.doc.ruler_origin);
    let mut ticks = [1.0; 2];
    let label = |v: f64, step: f64| -> String { if step >= 1.0 || whole { format!("{}", v.round() as i64) } else { crate::widgets::fmt_num2(v) } };
    for (vertical, extent) in [(false, size[0]), (true, size[1])] {
        let p = p.with_clip_rect(if vertical { left } else { top });
        let px_per_unit = unit.to_px(1.0, dpi, extent, ppi).max(1e-9);
        // The top ruler runs along document x, which pixel aspect correction stretches.
        let zoom = if vertical { xf.zoom } else { xf.zoom_x() };
        let step = nice_step(60.0 / (zoom as f64 * px_per_unit).max(1e-6), whole);
        let o = origin[usize::from(vertical)];
        ticks[usize::from(vertical)] = step * px_per_unit / 10.0;
        let (u0, u1) = if vertical {
            ruler_range(xf, left.left_top(), left.left_bottom(), 1, px_per_unit)
        } else {
            ruler_range(xf, top.left_top(), top.right_top(), 0, px_per_unit)
        };
        let (u0, u1) = (u0 - o / px_per_unit, u1 - o / px_per_unit);
        let mut v = (u0 / step).floor() * step;
        while v <= u1 {
            let at = |u: f64| -> f32 {
                let px = (u * px_per_unit + o) as f32;
                if vertical { xf.to_screen(0.0, px).y } else { xf.to_screen(px, 0.0).x }
            };
            let c = at(v);
            if vertical {
                p.line_segment([pos2(left.right() - RULER, c), pos2(left.right(), c)], tick);
                // Labels stacked, as in Photoshop.
                for (i, ch) in label(v, step).chars().enumerate() {
                    p.text(pos2(left.left() + 3.0, c + 2.0 + i as f32 * 8.5), Align2::LEFT_TOP, ch, font.clone(), t.text_dim);
                }
            } else {
                p.line_segment([pos2(c, top.bottom() - RULER), pos2(c, top.bottom())], tick);
                p.text(pos2(c + 2.0, top.top() + 1.0), Align2::LEFT_TOP, label(v, step), font.clone(), t.text_dim);
            }
            for k in 1..10 {
                let m = at(v + step * k as f64 / 10.0);
                let len = if k == 5 { 6.0 } else { 3.0 };
                if vertical {
                    p.line_segment([pos2(left.right() - len, m), pos2(left.right(), m)], tick);
                } else {
                    p.line_segment([pos2(m, top.bottom() - len), pos2(m, top.bottom())], tick);
                }
            }
            v += step;
        }
    }
    // Pointer position markers.
    if let Some(h) = ui.ctx().pointer_hover_pos().filter(|h| full.contains(*h)) {
        let m = Stroke::new(1.0, t.text);
        p.with_clip_rect(top).line_segment([pos2(h.x, top.top()), pos2(h.x, top.bottom())], m);
        p.with_clip_rect(left).line_segment([pos2(left.left(), h.y), pos2(left.right(), h.y)], m);
    }
    // Drag a new guide out of a ruler; right-click to switch the ruler unit.
    let mut chosen_unit: Option<&'static str> = None;
    for (r, vertical, salt) in [(top, false, "ruler-top"), (left, true, "ruler-left"), (corner, false, "ruler-corner")] {
        // Guides drag out of the top/left rulers and the zero point out of the corner; all three
        // sense clicks so the right-click unit menu opens anywhere on the ruler.
        let resp = ui.interact(r, ui.id().with(salt), Sense::click_and_drag());
        if salt == "ruler-corner" {
            corner_origin(app, ui, &resp, &p, xf, origin, ticks);
        } else if resp.hovered() {
            ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
        }
        if let Some(pos) = resp.interact_pointer_pos().filter(|_| salt != "ruler-corner" && (resp.dragged() || resp.drag_started())) {
            let d = xf.to_doc(pos);
            let pos = crate::snap_ui::snap_guide(app, vertical, if vertical { d[0] } else { d[1] });
            app.guide_drag = Some(GuideDrag { vertical, index: None, pos });
        }
        if resp.drag_stopped()
            && let Some(d) = app.guide_drag.take()
        {
            finish_drag(app, d);
        }
        resp.context_menu(|ui| {
            if let Some(name) = ruler_unit_menu(ui, unit) {
                chosen_unit = Some(name);
            }
        });
    }
    if let Some(name) = chosen_unit {
        let _ = app.run("prefs.set", json!({"path": "unitsAndRulers.rulers", "value": name}));
    }
    let _ = Pos2::ZERO;
}

/// The ruler corner: drag the zero point out of it (cross hairs mark where it lands; Shift snaps
/// to the ruler ticks), double-click it to put the zero point back at the image's top-left.
fn corner_origin(app: &mut PhotocraftApp, ui: &egui::Ui, resp: &egui::Response, p: &egui::Painter, xf: &ViewXform, origin: [f64; 2], ticks: [f64; 2]) {
    let id = resp.id.with("origin-drag");
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if resp.double_clicked() {
        let _ = app.run("view.resetRulerOrigin", json!({}));
        return;
    }
    // Dropped back on the corner (or a slow click), the zero point stays where it was.
    if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.drag_started()) {
        if resp.rect.contains(pos) {
            ui.data_mut(|d| d.remove_temp::<[f64; 2]>(id));
            return;
        }
        let shift = ui.input(|i| i.modifiers.shift);
        let at = origin_drop(app, xf.to_doc(pos), shift, origin, ticks);
        ui.data_mut(|d| d.insert_temp(id, at));
        // Cross hairs along the document axes through the new zero point.
        let t = Tokens::get(ui.ctx());
        let reach = (p.clip_rect().size().length() / xf.zoom.max(1e-4)) as f64;
        let (x, y) = (at[0], at[1]);
        let line = |a: [f64; 2], b: [f64; 2]| {
            p.line_segment([xf.to_screen(a[0] as f32, a[1] as f32), xf.to_screen(b[0] as f32, b[1] as f32)], Stroke::new(1.0, t.text));
        };
        line([x - reach, y], [x + reach, y]);
        line([x, y - reach], [x, y + reach]);
    }
    if resp.drag_stopped()
        && let Some(at) = ui.data_mut(|d| d.remove_temp::<[f64; 2]>(id))
    {
        let round = |v: f64| (v * 1000.0).round() / 1000.0;
        let _ = app.run("view.setRulerOrigin", json!({"x": round(at[0]), "y": round(at[1])}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_ticks_labels_and_pointer_stay_out_of_the_origin_corner() {
        let mut failures = Vec::new();
        for theme in crate::theme::ThemeKind::ALL {
            for (zoom, flip, rotation, width) in [(1.0, false, 0.0, 560.0), (1.0, false, 0.0, 800.0), (0.25, true, 0.0, 560.0), (2.0, false, 30.0, 560.0)] {
                let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
                app.run("file.new", json!({"width": 256, "height": 128})).unwrap();
                let full = Rect::from_min_size(pos2(32.0, 32.0), vec2(width - 80.0, 320.0));
                let corner = Rect::from_min_size(full.min, vec2(RULER, RULER));
                let builder = egui_kittest::Harness::builder().with_size(vec2(width, 400.0));
                let mut h = builder.build_ui_state(
                    move |ui, app| {
                        let rect = Rect::from_min_max(full.min + vec2(RULER, RULER), full.max);
                        let xf = ViewXform { rect, zoom, center: [(width - 80.0) / 2.0, 160.0], flip, rotation, aspect: 1.0 };
                        draw_rulers(app, ui, full, &xf);
                    },
                    app,
                );
                PhotocraftApp::setup_context(&h.ctx, theme);
                h.event(egui::Event::PointerMoved(corner.center()));
                h.run_steps(4);
                let mut ticks = 0;
                let mut labels = 0;
                for shape in &h.output().shapes {
                    if matches!(shape.shape, egui::Shape::LineSegment { .. } | egui::Shape::Text(_)) {
                        ticks += usize::from(matches!(shape.shape, egui::Shape::LineSegment { .. }));
                        labels += usize::from(matches!(shape.shape, egui::Shape::Text(_)));
                        let visible = shape.shape.visual_bounding_rect().intersect(shape.clip_rect);
                        if visible.intersect(corner.shrink(0.5)).is_positive() {
                            failures.push(format!("{theme:?}: zoom={zoom}, flip={flip}, rotation={rotation}, {visible:?}"));
                        }
                    }
                }
                assert!(ticks > 2 && labels > 0, "rulers still draw ticks and labels");
            }
        }
        assert!(failures.is_empty(), "ruler drawing overlaps the origin corner: {failures:?}");
    }

    #[test]
    fn shift_snaps_the_zero_point_to_ruler_ticks_from_the_current_origin() {
        assert_eq!(snap_to_tick(47.0, 0.0, 10.0), 50.0);
        assert_eq!(snap_to_tick(44.9, 0.0, 10.0), 40.0);
        // Ticks run from the current zero point, so they sit at 3 + 10k.
        assert_eq!(snap_to_tick(47.0, 3.0, 10.0), 43.0);
        assert_eq!(snap_to_tick(-8.0, 3.0, 10.0), -7.0);
        // A degenerate tick never panics or moves the point.
        for tick in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(snap_to_tick(47.5, 3.0, tick), 47.5);
        }
    }

    /// Rulers alone in a 400×300 frame at 100 %, centred on a 256×128 document.
    fn ruler_harness(app: PhotocraftApp) -> (egui_kittest::Harness<'static, PhotocraftApp>, Rect, ViewXform) {
        let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
        let rect = Rect::from_min_max(full.min + vec2(RULER, RULER), full.max);
        let xf = ViewXform { rect, zoom: 1.0, center: [128.0, 64.0], flip: false, rotation: 0.0, aspect: 1.0 };
        let h = egui_kittest::Harness::builder()
            .with_size(vec2(400.0, 300.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(move |ui, app| draw_rulers(app, ui, full, &xf), app);
        (h, full, xf)
    }

    fn press(h: &mut egui_kittest::Harness<'static, PhotocraftApp>, pos: Pos2, pressed: bool, modifiers: egui::Modifiers) {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers });
        h.run_steps(2);
    }

    fn drag_corner_to(h: &mut egui_kittest::Harness<'static, PhotocraftApp>, full: Rect, to: Pos2, modifiers: egui::Modifiers) {
        let corner = Rect::from_min_size(full.min, vec2(RULER, RULER)).center();
        h.event(egui::Event::ModifiersChanged(modifiers));
        h.event(egui::Event::PointerMoved(corner));
        h.run_steps(1);
        press(h, corner, true, modifiers);
        for p in [corner + vec2(10.0, 10.0), to] {
            h.event(egui::Event::PointerMoved(p));
            h.run_steps(2);
        }
        press(h, to, false, modifiers);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(1);
    }

    fn origin(h: &egui_kittest::Harness<'static, PhotocraftApp>) -> [f64; 2] {
        h.state().session.active().unwrap().doc.ruler_origin
    }

    /// #2955: dragging out of the ruler corner sets the zero point (cross hairs while dragging),
    /// the rulers then read 0 there, Shift snaps it to a tick, and double-clicking the corner
    /// resets it.
    #[test]
    fn dragging_the_ruler_corner_sets_the_zero_point_and_double_click_resets_it() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 256, "height": 128})).unwrap();
        app.ui.extras.snap = false;
        let (mut h, full, xf) = ruler_harness(app);
        h.run_steps(2);
        let target = xf.to_screen(100.0, 40.0);
        drag_corner_to(&mut h, full, target, egui::Modifiers::NONE);
        let o = origin(&h);
        assert!((o[0] - 100.0).abs() < 0.01 && (o[1] - 40.0).abs() < 0.01, "zero point lands under the pointer: {o:?}");
        // The top ruler's "0" now sits at the new zero point, "-100" at the image's left edge.
        h.run_steps(2);
        let labels: Vec<(String, f32)> = h
            .output()
            .shapes
            .iter()
            .filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some((t.galley.text().to_string(), t.pos.x)) } else { None })
            .collect();
        let near = |text: &str, x: f32| labels.iter().any(|(t, lx)| t == text && (lx - (x + 2.0)).abs() < 1.0);
        assert!(near("0", target.x), "a 0 label at the zero point: {labels:?}");
        assert!(near("-100", xf.to_screen(0.0, 0.0).x), "-100 at the image's left edge: {labels:?}");

        // Mid-drag, cross hairs run through the pointer across the canvas.
        let corner = Rect::from_min_size(full.min, vec2(RULER, RULER)).center();
        let mid = xf.to_screen(150.0, 90.0);
        press(&mut h, corner, true, egui::Modifiers::NONE);
        for p in [corner + vec2(10.0, 10.0), mid] {
            h.event(egui::Event::PointerMoved(p));
            h.run_steps(2);
        }
        let crosshair = h.output().shapes.iter().any(|s| {
            matches!(s.shape, egui::Shape::LineSegment { points: [a, b], .. } if (a.x - mid.x).abs() < 0.5 && (b.x - mid.x).abs() < 0.5 && (b.y - a.y).abs() > 200.0)
        });
        assert!(crosshair, "a vertical cross-hair line through the pointer while dragging");
        assert_eq!(origin(&h), o, "the zero point moves on release only");
        press(&mut h, mid, false, egui::Modifiers::NONE);
        assert_eq!(origin(&h), [150.0, 90.0]);
        h.state_mut().run("edit.undo", json!({})).unwrap();
        assert_eq!(origin(&h), o, "setting the zero point is undoable");

        // A drag dropped back on the corner leaves it alone.
        drag_corner_to(&mut h, full, full.min + vec2(4.0, 4.0), egui::Modifiers::NONE);
        assert_eq!(origin(&h), o);

        // Shift: ticks are 10 px apart at 100 %, measured from the current zero point (100, 40).
        drag_corner_to(&mut h, full, xf.to_screen(123.0, 57.0), egui::Modifiers::SHIFT);
        assert_eq!(origin(&h), [120.0, 60.0]);

        for _ in 0..40 {
            h.step();
        }
        let corner = Rect::from_min_size(full.min, vec2(RULER, RULER)).center();
        h.event(egui::Event::PointerMoved(corner));
        h.step();
        for _ in 0..2 {
            for pressed in [true, false] {
                h.event(egui::Event::PointerButton { pos: corner, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
                h.step();
            }
        }
        h.run_steps(2);
        assert_eq!(origin(&h), [0.0, 0.0], "double-clicking the corner resets the zero point");
    }

    /// #2955: the Info panel's X/Y are measured from the rulers' zero point.
    #[test]
    fn info_panel_position_reads_from_the_zero_point() {
        use egui_kittest::kittest::Queryable;
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 200, "height": 100})).unwrap();
        app.run("view.setRulerOrigin", json!({"x": 20, "y": 5})).unwrap();
        app.hover_doc = Some([50.5, 20.5]);
        let mut h = egui_kittest::Harness::builder()
            .with_size(vec2(260.0, 240.0))
            .build_ui_state(|ui, app| crate::panels::dock_body(app, ui, crate::dock::Group::Navigator, 2), app);
        h.run_steps(2);
        assert!(h.query_by_label("30").is_some() && h.query_by_label("15").is_some(), "X 30, Y 15 from the zero point at (20, 5)");
        assert!(h.query_by_label("50").is_none(), "not the raw image x");
    }

    #[test]
    fn percentage_grid_spacing_uses_both_canvas_dimensions() {
        let mut g = GuidesGridAndSlices { grid_unit: Unit::Percent, gridline_every: 20.0, subdivisions: 2, ..Default::default() };
        assert_eq!(grid_major_px(&g, 72.0, [1280.0, 720.0], 72.0), [256.0, 144.0]);
        // Absolute units remain equal on both axes, irrespective of aspect ratio.
        g.grid_unit = Unit::Pixels;
        g.gridline_every = 100.0;
        assert_eq!(grid_major_px(&g, 72.0, [1280.0, 720.0], 72.0), [100.0, 100.0]);
    }

    #[test]
    fn unit_steps_allow_fractions() {
        assert_eq!(nice_step(0.3, false), 0.5);
        assert_eq!(nice_step(0.2, false), 0.2);
        assert_eq!(nice_step(0.22, false), 0.25);
        assert_eq!(nice_step(0.3, true), 1.0);
        assert_eq!(nice_step(23.0, true), 50.0);
    }

    #[test]
    fn ticks_follow_1_2_5_series() {
        assert_eq!(tick_step(1.0), 100.0);
        assert_eq!(tick_step(0.1), 1000.0);
        assert_eq!(tick_step(4.0), 20.0);
        assert_eq!(tick_step(64.0), 1.0);
        assert_eq!(tick_step(0.25), 500.0);
    }

    #[test]
    fn hidden_guides_are_not_hit_tested() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.session.execute("file.new", json!({"width": 200, "height": 100})).unwrap();
        app.sync_views();
        app.session.execute("view.newGuide", json!({"orientation": "vertical", "position": 50})).unwrap();
        app.session.execute("view.newGuide", json!({"orientation": "horizontal", "position": 25})).unwrap();
        let positions = app.session.active().unwrap().doc.guides.clone();

        // Visible, unlocked guides remain interactive in either orientation.
        assert_eq!(guide_at(&app, 50.0, 10.0), Some((true, 0)));
        assert_eq!(guide_at(&app, 10.0, 25.0), Some((false, 0)));

        // All ways to hide the canvas guides must also disable hit testing.
        app.ui.extras.guides = false;
        assert_eq!(guide_at(&app, 50.0, 10.0), None);
        assert_eq!(guide_at(&app, 10.0, 25.0), None);
        app.ui.extras.guides = true;

        app.ui.view.show.canvas_guides = false;
        assert_eq!(guide_at(&app, 50.0, 10.0), None);
        assert_eq!(guide_at(&app, 10.0, 25.0), None);
        app.ui.view.show.canvas_guides = true;

        app.ui.view.extras = false;
        assert_eq!(guide_at(&app, 50.0, 10.0), None);
        assert_eq!(guide_at(&app, 10.0, 25.0), None);
        app.ui.view.extras = true;

        // Hiding/showing must not change the stored guide positions.
        assert_eq!(app.session.active().unwrap().doc.guides, positions);
        assert_eq!(guide_at(&app, 50.0, 10.0), Some((true, 0)));
        assert_eq!(guide_at(&app, 10.0, 25.0), Some((false, 0)));

        app.ui.extras.lock_guides = true;
        assert_eq!(guide_at(&app, 50.0, 10.0), None);
        assert_eq!(guide_at(&app, 10.0, 25.0), None);
    }

    #[test]
    fn unit_menu_lists_every_unit_with_the_current_one_checked() {
        let items = unit_menu_items(Unit::Inches);
        assert_eq!(items.len(), Unit::NAMES.len());
        // Every unit is offered, labelled, and in the preference's order.
        assert_eq!(items[0], ("Pixels".to_string(), "pixels", false));
        assert_eq!(items.iter().find(|(_, name, _)| *name == "cm").unwrap().0, "Centimeters");
        // Exactly the current unit is checked.
        let checked: Vec<_> = items.iter().filter(|(_, _, c)| *c).map(|(_, name, _)| *name).collect();
        assert_eq!(checked, vec!["inches"]);
    }

    #[test]
    fn picking_a_ruler_unit_changes_the_preference() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        assert_eq!(app.session.prefs().units_and_rulers.rulers, Unit::Pixels);
        // The context menu applies its pick exactly as the preference pane does.
        app.run("prefs.set", json!({"path": "unitsAndRulers.rulers", "value": "cm"})).unwrap();
        assert_eq!(app.session.prefs().units_and_rulers.rulers, Unit::Centimeters);
    }

    /// View › Rulers is remembered: the next launch, and every document created or opened in it,
    /// shows the rulers as last set, on or off (#2918).
    #[test]
    fn rulers_open_as_last_set_after_a_restart() {
        use std::{cell::RefCell, rc::Rc};
        let saved = Rc::new(RefCell::new(None::<String>));
        let launch = |saved: &Rc<RefCell<Option<String>>>| {
            let (load, store) = (saved.clone(), saved.clone());
            PhotocraftApp::new(
                photocraft_engine::Session::new(),
                crate::Services {
                    load_prefs: Some(Box::new(move || load.borrow().clone())),
                    save_prefs: Some(Box::new(move |text| {
                        *store.borrow_mut() = Some(text.to_string());
                        Ok(())
                    })),
                    ..Default::default()
                },
            )
        };
        for on in [true, false] {
            let mut app = launch(&saved);
            assert_eq!(crate::menus::invoke(&mut app, &egui::Context::default(), "view.rulers", json!({})).unwrap(), json!(on));
            crate::prefs_ui::save_preferences(&mut app).unwrap();
            let mut next = launch(&saved);
            next.session.execute("file.new", json!({"width": 20, "height": 10})).unwrap();
            next.sync_views();
            assert_eq!(next.ui.extras.rulers, on, "a new document after a restart shows the rulers as last set");
        }
    }

    #[test]
    fn guide_drags_create_move_and_delete() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.session.execute("file.new", json!({"width": 200, "height": 100})).unwrap();
        app.sync_views();
        finish_drag(&mut app, GuideDrag { vertical: true, index: None, pos: 50.0 });
        finish_drag(&mut app, GuideDrag { vertical: false, index: None, pos: 500.0 }); // off canvas: ignored
        assert_eq!(app.session.active().unwrap().doc.guides.vertical, vec![50.0]);
        assert!(app.session.active().unwrap().doc.guides.horizontal.is_empty());
        assert_eq!(guide_at(&app, 51.0, 10.0), Some((true, 0)));
        finish_drag(&mut app, GuideDrag { vertical: true, index: Some(0), pos: 120.0 });
        assert_eq!(app.session.active().unwrap().doc.guides.vertical, vec![120.0]);
        finish_drag(&mut app, GuideDrag { vertical: true, index: Some(0), pos: -30.0 });
        assert!(app.session.active().unwrap().doc.guides.vertical.is_empty());
        app.ui.extras.lock_guides = true;
        assert_eq!(guide_at(&app, 0.0, 0.0), None);
    }

    /// ⌘ is the Move tool with any tool, so a ⌘-drag over a guide moves the guide, never the layer
    /// (#2690); with Lock Guides a ⌘-drag with a marquee moves the layer as before.
    #[test]
    fn command_drag_over_a_guide_moves_the_guide_with_any_tool() {
        use crate::canvas::{ToolEvent, tool_event};
        use crate::state::Tool;
        for (tool, locked) in [(Tool::Brush, false), (Tool::RectMarquee, false), (Tool::Lasso, false), (Tool::MagicWand, false), (Tool::RectMarquee, true)] {
            let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
            app.run("file.new", json!({"width": 80, "height": 60, "background": "transparent"})).unwrap();
            app.sync_views();
            app.ui.tool = tool;
            app.ui.extras.lock_guides = locked;
            let layer = app.session.active().unwrap().active_layer.unwrap();
            app.session
                .edit("paint", |doc, _| {
                    doc.layer_mut(layer).unwrap().surface_mut().unwrap().fill_rect(photocraft_geom::Rect::new(10, 10, 30, 30), &[1.0, 0.0, 0.0, 1.0]);
                    Ok(())
                })
                .unwrap();
            app.run("view.newGuide", json!({"orientation": "vertical", "position": 50})).unwrap();
            let cmd = egui::Modifiers::COMMAND;
            tool_event(&mut app, ToolEvent::Down { x: 50.0, y: 40.0, pressure: 1.0 }, cmd);
            tool_event(&mut app, ToolEvent::Move { x: 65.0, y: 40.0, pressure: 1.0 }, cmd);
            tool_event(&mut app, ToolEvent::Up { x: 65.0, y: 40.0 }, cmd);
            let doc = &app.session.active().unwrap().doc;
            let moved = doc.layer(layer).unwrap().surface().unwrap().rgba(12, 12)[3] == 0.0;
            if locked {
                assert_eq!((doc.guides.vertical.clone(), moved), (vec![50.0], true), "{tool:?}, guides locked: the layer moves");
            } else {
                assert_eq!((doc.guides.vertical.clone(), moved), (vec![65.0], false), "{tool:?}: the guide moves, the layer stays");
            }
        }
    }

    #[test]
    fn ruler_range_is_normalised_when_flipped() {
        // #1814: with View > Flip Horizontal, `to_doc` negates x, so the raw
        // (d0, d1) comes out reversed. ruler_range must normalise to (min, max)
        // so the tick loop still runs. Exercises the production helper.
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let top = Rect::from_min_size(rect.min, vec2(rect.width(), RULER));
        for flip in [false, true] {
            let xf = ViewXform { rect, zoom: 1.0, center: [400.0, 300.0], flip, rotation: 0.0, aspect: 1.0 };
            let (u0, u1) = ruler_range(&xf, top.left_top(), top.right_top(), 0, 1.0);
            assert!(u0 <= u1, "flip={flip}: u0 ({u0}) must be <= u1 ({u1})");
            assert!(u1 - u0 > 0.0, "flip={flip}: range is empty");
        }
    }
}
