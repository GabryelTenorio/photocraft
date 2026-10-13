//! Standard Pen edits use the same raw-pointer hit for the action and its cursor.

use egui::Modifiers;
use photocraft_doc::{Knot, Path, Subpath};
use photocraft_geom::Point;
use photocraft_vector::edit::{self, Handle, Hit};

use super::{PenPath, pen_close_at};
use crate::PhotocraftApp;
use crate::canvas::ToolEvent;
use crate::direct_select::PathRef;
use crate::state::Tool;
use crate::tool_cursor::PenCursor;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DraftDrag {
    base: Path,
    hit: Hit,
    start: [f64; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Undo {
    base: Path,
    applied_len: usize,
}

enum Action {
    Cursor(PenCursor),
    Draft(Path, Hit, PenCursor),
    Stored(PathRef, Hit, PenCursor),
}

impl Action {
    fn cursor(&self) -> PenCursor {
        match self {
            Self::Cursor(cursor) | Self::Draft(_, _, cursor) | Self::Stored(_, _, cursor) => *cursor,
        }
    }
}

pub(super) fn path(pen: &PenPath) -> Path {
    Path::new(vec![Subpath {
        closed: false,
        knots: pen
            .knots
            .iter()
            .enumerate()
            .map(|(i, k)| Knot {
                anchor: Point::new(k[0][0], k[0][1]),
                in_ctrl: Point::new(k[1][0], k[1][1]),
                out_ctrl: Point::new(k[2][0], k[2][1]),
                smooth: !pen.unlinked.contains(&i) && (k[1] != k[0] || k[2] != k[0]),
            })
            .collect(),
        ..Default::default()
    }])
}

pub(super) fn remember(pen: &mut PenPath, base: Path) {
    pen.edit_undo.push(Undo { base, applied_len: pen.knots.len() });
}

pub(super) fn undo(pen: &mut PenPath) -> bool {
    if !pen.edit_undo.last().is_some_and(|step| step.applied_len == pen.knots.len()) {
        return false;
    }
    let Some(step) = pen.edit_undo.pop() else { return false };
    restore(pen, step.base);
    true
}

pub(super) fn restore(pen: &mut PenPath, path: Path) {
    if let Some(subpath) = path.subpaths.first() {
        pen.knots = subpath.knots.iter().map(|k| [[k.anchor.x, k.anchor.y], [k.in_ctrl.x, k.in_ctrl.y], [k.out_ctrl.x, k.out_ctrl.y]]).collect();
        pen.unlinked = subpath.knots.iter().enumerate().filter_map(|(i, k)| (!k.smooth).then_some(i)).collect();
    }
    pen.selected_anchor = pen.selected_anchor.filter(|&k| k < pen.knots.len());
}

pub(super) fn cancel_drag(pen: &mut PenPath) {
    if let Some(drag) = pen.edit.take() {
        restore(pen, drag.base);
    }
}

fn draft_handles(pen: &PenPath) -> Vec<([usize; 2], Handle)> {
    pen.knots
        .iter()
        .enumerate()
        .filter(|(i, _)| pen.selected_anchor == Some(*i) || i.saturating_add(1) == pen.knots.len())
        .flat_map(|(i, k)| [(1, Handle::In), (2, Handle::Out)].into_iter().filter_map(move |(j, h)| (k[j] != k[0]).then_some(([0, i], h))))
        .collect()
}

fn intent(hit: Hit, convert: bool, auto: bool) -> Option<PenCursor> {
    match hit {
        Hit::Anchor(_) | Hit::Handle(..) if convert => Some(PenCursor::Convert),
        Hit::Anchor(_) if auto => Some(PenCursor::Delete),
        Hit::Segment(_, t) if auto && t > 0.0 && t < 1.0 => Some(PenCursor::Add),
        _ => None,
    }
}

fn resolve(app: &PhotocraftApp, point: [f64; 2], mods: Modifiers) -> Action {
    let auto = app.ui.tool_options.pen_auto_add_delete && !mods.alt && !mods.shift;
    let tol = 6.0 / f64::from(app.point_zoom().max(0.01));
    if let Some(pen) = &app.ui.pen {
        if pen.curvature || pen.knots.is_empty() || pen.owner.as_ref().is_none_or(|owner| !owner.valid(app)) {
            return Action::Cursor(PenCursor::New);
        }
        if pen.edit.is_some() {
            return Action::Cursor(PenCursor::Convert);
        }
        if pen.dragging {
            return Action::Cursor(PenCursor::Draw);
        }
        if !mods.alt && pen_close_at(pen, point, tol) {
            return Action::Cursor(PenCursor::Close);
        }
        let path = path(pen);
        let handles = if mods.alt { draft_handles(pen) } else { Vec::new() };
        if let Some(hit) = edit::hit(&path, Point::new(point[0], point[1]), tol, &handles) {
            // Re-clicking the final point adjusts its outgoing handle, as before.
            if !mods.alt && matches!(hit, Hit::Anchor([0, k]) if Some(k) == pen.knots.len().checked_sub(1)) {
                return Action::Cursor(PenCursor::Draw);
            }
            if let Some(cursor) = intent(hit, mods.alt, auto) {
                return Action::Draft(path, hit, cursor);
            }
        }
        return Action::Cursor(PenCursor::Draw);
    }
    if let Some((target, path, hit)) = crate::direct_select::hit_target_with(app, point, mods.alt, |target| {
        mods.alt || !matches!(target, PathRef::Work) || app.ui.selected_path.as_deref() == Some("work")
    }) && let Some(cursor) = intent(hit, mods.alt, auto)
    {
        // Open end points extend the path rather than disappearing under Auto Add/Delete.
        if cursor == PenCursor::Delete
            && let Hit::Anchor([subpath, knot]) = hit
            && path.subpaths.get(subpath).is_some_and(|sp| !sp.closed && (knot == 0 || knot.checked_add(1) == Some(sp.knots.len())))
        {
            return Action::Cursor(PenCursor::New);
        }
        return Action::Stored(target, hit, cursor);
    }
    Action::Cursor(PenCursor::New)
}

pub(crate) fn cursor(app: &PhotocraftApp, point: [f64; 2], mods: Modifiers) -> PenCursor {
    if converting(app) {
        return PenCursor::Convert;
    }
    match app.ui.tool {
        Tool::AddAnchorPoint | Tool::DeleteAnchorPoint => {
            if crate::anchor_tools::effective(app.ui.tool, mods) == Some(Tool::AddAnchorPoint) {
                PenCursor::Add
            } else {
                PenCursor::Delete
            }
        }
        Tool::ConvertPoint => PenCursor::Convert,
        _ => resolve(app, point, mods).cursor(),
    }
}

pub(crate) fn converting(app: &PhotocraftApp) -> bool {
    crate::direct_select::conversion_drag(app)
        || (app.ui.tool == Tool::Pen && app.ui.pen.as_ref().is_some_and(|pen| pen.edit.is_some() && pen.owner.as_ref().is_some_and(|owner| owner.valid(app))))
}

fn convert(base: &Path, hit: Hit, start: [f64; 2], end: [f64; 2], moved: bool) -> Result<Path, String> {
    if !end.iter().all(|c| c.is_finite() && c.abs() <= 1_000_000.0) {
        return Err("The Pen's direction point must have finite coordinates between -1000000 and 1000000".into());
    }
    let mut edited = base.clone();
    match hit {
        Hit::Anchor(k) => edit::convert_point(&mut edited, k, moved.then_some(Point::new(end[0], end[1])))?,
        Hit::Handle(k, handle) if moved => {
            let knot =
                base.subpaths.get(k[0]).and_then(|sub| sub.knots.get(k[1])).ok_or_else(|| "The Pen's direction point is no longer available".to_string())?;
            let from = if handle == Handle::In { knot.in_ctrl } else { knot.out_ctrl };
            edit::move_handle(&mut edited, k, handle, Point::new(from.x + end[0] - start[0], from.y + end[1] - start[1]), true)?;
        }
        _ => {}
    }
    Ok(edited)
}

/// Draft edits stay transient; finished-path edits use the existing undoable engine commands.
pub(crate) fn pointer(app: &mut PhotocraftApp, raw: ToolEvent, mods: Modifiers) -> bool {
    if app.ui.tool != Tool::Pen {
        return false;
    }
    if super::cancel_stale_pen(app) {
        return !matches!(raw, ToolEvent::Down { .. });
    }
    if let Some(drag) = app.ui.pen.as_ref().and_then(|pen| pen.edit.clone()) {
        // Conversion uses the same unsnapped coordinates from press through release. A
        // stationary grid-snapped click must not manufacture direction handles.
        let end = match raw {
            ToolEvent::Move { x, y, .. } | ToolEvent::Up { x, y } => [x, y],
            ToolEvent::Down { .. } => return true,
        };
        app.prefs_rt.snap = None;
        app.prefs_rt.snap_lines.clear();
        let moved = (end[0] - drag.start[0]).hypot(end[1] - drag.start[1]) * f64::from(app.point_zoom()) >= 1.0;
        match convert(&drag.base, drag.hit, drag.start, end, moved) {
            Ok(edited) => {
                if let Some(pen) = &mut app.ui.pen {
                    restore(pen, edited.clone());
                    if matches!(raw, ToolEvent::Up { .. }) {
                        if edited != drag.base {
                            remember(pen, drag.base);
                        }
                        pen.edit = None;
                    }
                }
            }
            Err(error) => {
                app.ui.status = error;
                app.ui.status_error = true;
                if let Some(pen) = &mut app.ui.pen {
                    restore(pen, drag.base);
                    pen.edit = None;
                }
            }
        }
        return true;
    }
    if mods.command || mods.ctrl || app.ui.direct_selection.drag.is_some() {
        return false;
    }
    let ToolEvent::Down { x, y, .. } = raw else { return false };
    match resolve(app, [x, y], mods) {
        Action::Draft(base, hit, PenCursor::Convert) => {
            if let Some(pen) = &mut app.ui.pen {
                pen.selected_anchor = match hit {
                    Hit::Anchor([_, k]) | Hit::Handle([_, k], _) => Some(k),
                    _ => None,
                };
                pen.edit = Some(DraftDrag { base, hit, start: [x, y] });
            }
        }
        Action::Draft(base, hit, _) => {
            let mut edited = base.clone();
            let result = match hit {
                Hit::Segment(k, t) => edit::add_anchor(&mut edited, k, t).map(|k| Some(k[1])),
                Hit::Anchor(k) => edit::delete_anchor(&mut edited, k).map(|_| None),
                _ => return false,
            };
            match result {
                Ok(selected) => {
                    if let Some(pen) = &mut app.ui.pen {
                        restore(pen, edited);
                        remember(pen, base);
                        pen.selected_anchor = selected;
                    }
                }
                Err(error) => {
                    app.ui.status = error;
                    app.ui.status_error = true;
                }
            }
        }
        Action::Stored(target, hit, PenCursor::Add | PenCursor::Delete) => crate::anchor_tools::apply(app, target, hit),
        _ => return false,
    }
    app.prefs_rt.snap = None;
    app.prefs_rt.snap_lines.clear();
    true
}

#[cfg(test)]
#[path = "pen_edit_tests.rs"]
mod tests;
