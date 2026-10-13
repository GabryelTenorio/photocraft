//! Direct Selection tool (A, #790), the Convert Point tool (#3204), the Pens' ⌘/Ctrl (Direct
//! Selection: the Pen, Freeform Pen and Curvature Pen) and the Pen's ⌥/Alt (Convert Point)
//! modifiers, as in Photoshop. It edits the paths the canvas shows: the targeted vector mask or
//! the active shape layer's path, and the work or selected saved path.
//!
//! * Click an anchor to select it (⇧ adds or removes, ⌥ selects its whole subpath), drag on empty
//!   canvas to marquee-select anchors, click empty canvas to deselect.
//! * Drag selected anchors to move them, a direction handle to turn it (a smooth point keeps its
//!   handles collinear; ⌥ breaks them apart), or a segment to bend it (straight segments move).
//! * Convert Point (the tool, or Pen + ⌥): click an anchor to make it a corner, drag from it to
//!   pull out smooth handles, drag a handle to move it alone. The tool selects the anchor it
//!   converts, so its handles show; a press on empty canvas or a segment does nothing, and ⌘/Ctrl
//!   is Direct Selection, as with the Pen.
//!
//! A drag previews with `photocraft_vector::edit` and commits once on release through the engine
//! (`path.moveAnchors`, `path.moveHandle`, `path.bendSegment`, `path.convertPoint`): one history
//! step per drag, and the same commands drive it over MCP and the control channel.

use egui::{Color32, Modifiers, Pos2};
use photocraft_doc::vector::Path;
use photocraft_geom::Point;
use photocraft_vector::edit::{self, Handle, Hit};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::canvas::ToolEvent;
use crate::state::Tool;

/// A path Direct Selection edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathRef {
    Work,
    Saved(String),
    /// A layer's shape path, or its vector mask.
    Layer(u64),
}

impl PathRef {
    /// The engine's target params for this path.
    pub(crate) fn params(&self) -> Value {
        match self {
            PathRef::Work => json!({"name": "work"}),
            PathRef::Saved(name) => json!({"name": name}),
            PathRef::Layer(id) => json!({"name": "layer", "layer": id}),
        }
    }
}

/// Direct Selection state: which anchors are selected and the drag in progress.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectSelection {
    /// The path whose anchors are selected.
    pub target: Option<PathRef>,
    /// Selected anchors as `[subpath, knot]`.
    pub anchors: Vec<[usize; 2]>,
    /// A clicked segment (`[subpath, knot]` it leaves): its two handles show.
    pub segment: Option<[usize; 2]>,
    #[serde(skip)]
    pub drag: Option<Drag>,
    #[serde(skip)]
    pub(crate) owner: Option<Owner>,
    #[serde(skip)]
    pub(crate) cancelled_drag: bool,
}

/// Runtime selection ownership is never restored from settings onto another document.
#[derive(Clone, Debug)]
pub(crate) struct Owner {
    document: photocraft_doc::DocId,
    source: std::sync::Weak<photocraft_doc::Document>,
    revision: u64,
    layer: Option<photocraft_doc::LayerId>,
    selected_path: Option<String>,
    vector_mask: bool,
    tool: Tool,
}

impl PartialEq for Owner {
    fn eq(&self, other: &Self) -> bool {
        self.source.ptr_eq(&other.source)
            && (self.document, self.revision, self.layer, &self.selected_path, self.vector_mask, self.tool)
                == (other.document, other.revision, other.layer, &other.selected_path, other.vector_mask, other.tool)
    }
}

impl Owner {
    fn current(app: &PhotocraftApp) -> Option<Self> {
        let st = app.session.active()?;
        Some(Self {
            document: st.doc.id,
            source: std::sync::Arc::downgrade(&st.doc),
            revision: st.revision,
            layer: st.active_layer,
            selected_path: app.ui.selected_path.clone(),
            vector_mask: app.ui.vector_mask_target || st.channel_view.target == photocraft_engine::channel_cmds::ChannelTarget::VectorMask,
            tool: app.ui.tool,
        })
    }

    fn same_target(&self, app: &PhotocraftApp) -> bool {
        self.same_path_target(app) && app.ui.tool == self.tool
    }

    fn same_path_target(&self, app: &PhotocraftApp) -> bool {
        app.session.active().is_some_and(|st| {
            st.doc.id == self.document
                && st.active_layer == self.layer
                && (app.ui.vector_mask_target || st.channel_view.target == photocraft_engine::channel_cmds::ChannelTarget::VectorMask) == self.vector_mask
        }) && app.ui.selected_path == self.selected_path
    }

    fn valid(&self, app: &PhotocraftApp) -> bool {
        self.valid_path(app) && app.ui.tool == self.tool
    }

    fn valid_path(&self, app: &PhotocraftApp) -> bool {
        self.same_path_target(app)
            && app.session.active().is_some_and(|st| st.revision == self.revision && self.source.ptr_eq(&std::sync::Arc::downgrade(&st.doc)))
    }
}

/// Clear stale anchors/gestures, retaining pointer ownership until their old release arrives.
pub(crate) fn cancel_stale(app: &mut PhotocraftApp) -> bool {
    let ds = &app.ui.direct_selection;
    let stale = ds.owner.as_ref().map_or(ds.target.is_some() || !ds.anchors.is_empty() || ds.segment.is_some() || ds.drag.is_some(), |owner| {
        // Convert Point works on the handles selected with Direct Selection. Only idle anchor
        // selection can cross those tools; a drag retains its original tool ownership.
        !(owner.valid(app) || (ds.drag.is_none() && owns(owner.tool) && owns(app.ui.tool) && owner.valid_path(app)))
    });
    if !stale {
        return false;
    }
    let cancelled_drag = ds.cancelled_drag || ds.drag.is_some();
    app.ui.direct_selection = DirectSelection { cancelled_drag, ..Default::default() };
    cancelled_drag
}

#[derive(Clone, Debug, PartialEq)]
pub enum Drag {
    /// Editing `target`: `base` is its path at the press, `preview` the edit so far.
    Edit { target: PathRef, base: Path, edit: Edit, start: [f64; 2], end: [f64; 2], preview: Option<Path>, converting: bool },
    /// Marquee-selecting anchors (⇧ adds to the selection).
    Marquee { start: [f64; 2], end: [f64; 2], add: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Anchors(Vec<[usize; 2]>),
    /// A handle, where it was at the press, and whether it moves alone (⌥).
    Handle([usize; 2], Handle, Point, bool),
    Segment([usize; 2], f64),
    /// Pen + ⌥ on an anchor: a click makes a corner, a drag pulls out smooth handles.
    Convert([usize; 2]),
}

impl Edit {
    /// The edit for a drag from `start` to `end`, as a command and its params (without target).
    fn command(&self, start: [f64; 2], end: [f64; 2]) -> (&'static str, Value) {
        let d = [end[0] - start[0], end[1] - start[1]];
        match self {
            Edit::Anchors(a) => ("path.moveAnchors", json!({"anchors": a, "move": d})),
            Edit::Handle([s, k], h, from, independent) => {
                ("path.moveHandle", json!({"subpath": s, "knot": k, "handle": h.name(), "to": [from.x + d[0], from.y + d[1]], "independent": independent}))
            }
            Edit::Segment([s, k], t) => ("path.bendSegment", json!({"subpath": s, "knot": k, "t": t, "move": d})),
            Edit::Convert([s, k]) => ("path.convertPoint", json!({"subpath": s, "knot": k, "out": end})),
        }
    }

    /// `base` edited as the command would: the drag preview.
    fn preview(&self, base: &Path, start: [f64; 2], end: [f64; 2]) -> Option<Path> {
        let d = [end[0] - start[0], end[1] - start[1]];
        let mut p = base.clone();
        match self {
            Edit::Anchors(a) => edit::move_anchors(&mut p, a, d),
            Edit::Handle(k, h, from, independent) => edit::move_handle(&mut p, *k, *h, Point::new(from.x + d[0], from.y + d[1]), *independent),
            Edit::Segment(k, t) => edit::bend_segment(&mut p, *k, *t, d),
            Edit::Convert(k) => edit::convert_point(&mut p, *k, Some(Point::new(end[0], end[1]))),
        }
        .ok()?;
        Some(p)
    }
}

/// Is Direct Selection what the pointer does now: its own tool, or a Pen (the Pen, Freeform Pen
/// or Curvature Pen), an anchor point tool or Convert Point with ⌘/Ctrl held.
fn direct(tool: Tool, mods: Modifiers) -> bool {
    tool == Tool::DirectSelection
        || (matches!(tool, Tool::Pen | Tool::FreeformPen | Tool::CurvaturePen | Tool::AddAnchorPoint | Tool::DeleteAnchorPoint | Tool::ConvertPoint)
            && mods.command)
}

/// Is Convert Point what the pointer does now: its own tool, or the Pen with ⌥ held while no path
/// is being drawn. ⌘/Ctrl wins over both (Direct Selection).
fn converting(tool: Tool, mods: Modifiers, pen_idle: bool) -> bool {
    !mods.command && (tool == Tool::ConvertPoint || (tool == Tool::Pen && mods.alt && pen_idle))
}

/// The tools that take every pointer event on the canvas, not just a press that hits a path.
fn owns(tool: Tool) -> bool {
    matches!(tool, Tool::DirectSelection | Tool::ConvertPoint)
}

/// Does the canvas draw Direct Selection's overlay instead of the plain path outlines? The Add /
/// Delete Anchor Point tools show it too: they edit the same paths.
pub fn shows(app: &PhotocraftApp, mods: Modifiers) -> bool {
    direct(app.ui.tool, mods)
        || (app.ui.tool == Tool::Pen && app.ui.pen.is_none() && (mods.alt || app.ui.tool_options.pen_auto_add_delete))
        || app.ui.direct_selection.drag.is_some()
        || matches!(app.ui.tool, Tool::AddAnchorPoint | Tool::DeleteAnchorPoint | Tool::ConvertPoint)
}

/// Paths available to automatic Pen editing: selected saved/work paths and targeted layers.
pub(crate) fn active_paths(app: &PhotocraftApp) -> Vec<(PathRef, Path)> {
    candidates(app)
        .into_iter()
        .filter(|(target, _)| match target {
            PathRef::Work => app.ui.selected_path.as_deref() == Some("work"),
            PathRef::Saved(name) => app.ui.selected_path.as_deref() == Some(name.as_str()),
            PathRef::Layer(id) => {
                crate::vector_ui::targeted_vector_mask(app).or_else(|| crate::vector_ui::active_shape_path(app)).is_some_and(|(active, _)| active == *id)
            }
        })
        .filter(|(target, _)| writable(app, target))
        .collect()
}

/// The selected work/saved path wins overlapping hits; otherwise the targeted layer is first.
pub(crate) fn candidates(app: &PhotocraftApp) -> Vec<(PathRef, Path)> {
    let layer = crate::vector_ui::targeted_vector_mask(app).or_else(|| crate::vector_ui::active_shape_path(app));
    let Some(st) = app.session.active() else { return Vec::new() };
    let mut paths = Vec::new();
    match app.ui.selected_path.as_deref() {
        Some("work") => paths.extend(st.doc.work_path.clone().map(|path| (PathRef::Work, path))),
        Some(name) if name != "layer" => {
            paths.extend(st.doc.paths.iter().find(|path| path.name == name).map(|path| (PathRef::Saved(name.into()), path.path.clone())));
        }
        _ => {}
    }
    paths.extend(layer.map(|(id, path)| (PathRef::Layer(id), path)));
    for (path, _, candidate) in st.doc.walk().iter().rev() {
        if !candidate.visible || !(1..=path.len()).all(|n| st.doc.layer_at(&path[..n]).is_some_and(|parent| parent.visible)) {
            continue;
        }
        if let photocraft_doc::LayerContent::Shape(shape) = &candidate.content
            && !paths.iter().any(|(target, _)| *target == PathRef::Layer(candidate.id.0))
        {
            paths.push((PathRef::Layer(candidate.id.0), shape.path.clone()));
        }
    }
    if !paths.iter().any(|(target, _)| *target == PathRef::Work) {
        paths.extend(st.doc.work_path.clone().map(|path| (PathRef::Work, path)));
    }
    paths
}

fn retracted(k: &photocraft_doc::Knot, h: Handle) -> bool {
    let c = if h == Handle::In { k.in_ctrl } else { k.out_ctrl };
    (c.x - k.anchor.x).abs() < 1e-9 && (c.y - k.anchor.y).abs() < 1e-9
}

/// The handles shown on `path`: both of every selected anchor and the two of a clicked segment
/// (retracted handles sit on their anchor and don't show).
fn shown_handles(ds: &DirectSelection, path: &Path) -> Vec<([usize; 2], Handle)> {
    let knot = |r: [usize; 2]| path.subpaths.get(r[0]).and_then(|s| s.knots.get(r[1]));
    let mut out: Vec<([usize; 2], Handle)> = ds.anchors.iter().flat_map(|&r| [(r, Handle::In), (r, Handle::Out)]).collect();
    if let Some([s, k]) = ds.segment
        && let Some(sp) = path.subpaths.get(s)
    {
        let n = sp.knots.len();
        let next = k.checked_add(1).filter(|&j| j < n).or((sp.closed && n > 1).then_some(0));
        out.push(([s, k], Handle::Out));
        out.extend(next.map(|j| ([s, j], Handle::In)));
    }
    out.retain(|&(r, h)| knot(r).is_some_and(|k| !retracted(k, h)));
    out
}

/// Editable targets share the same lock and hit rules for the cursor and pointer press.
pub(crate) fn writable(app: &PhotocraftApp, target: &PathRef) -> bool {
    match target {
        PathRef::Layer(id) => app.session.active().is_some_and(|st| {
            let id = photocraft_doc::LayerId(*id);
            let locks = st.doc.effective_locks(id);
            st.doc.layer(id).is_some() && !locks.all && !locks.pixels
        }),
        _ => app.session.active().is_some(),
    }
}

pub(crate) fn hit_target(app: &PhotocraftApp, p: [f64; 2], handles: bool) -> Option<(PathRef, Path, Hit)> {
    hit_target_with(app, p, handles, |_| true)
}

pub(crate) fn hit_target_with(app: &PhotocraftApp, p: [f64; 2], handles: bool, accepts: impl Fn(&PathRef) -> bool) -> Option<(PathRef, Path, Hit)> {
    let tol = 6.0 / f64::from(app.point_zoom().max(0.01));
    let ds = &app.ui.direct_selection;
    candidates(app).into_iter().filter(|(r, _)| accepts(r) && writable(app, r)).find_map(|(r, path)| {
        let valid_selection =
            ds.owner.as_ref().is_some_and(|owner| owner.valid(app) || (ds.drag.is_none() && owns(owner.tool) && owns(app.ui.tool) && owner.valid_path(app)));
        let handles = if handles && valid_selection && ds.target.as_ref() == Some(&r) { shown_handles(ds, &path) } else { Vec::new() };
        edit::hit(&path, Point::new(p[0], p[1]), tol, &handles).map(|hit| (r, path, hit))
    })
}

pub(crate) fn conversion_drag(app: &PhotocraftApp) -> bool {
    matches!(app.ui.tool, Tool::Pen | Tool::ConvertPoint)
        && captured_drag(app)
        && matches!(app.ui.direct_selection.drag, Some(Drag::Edit { converting: true, .. }))
}

pub(crate) fn captured_drag(app: &PhotocraftApp) -> bool {
    app.ui.direct_selection.owner.as_ref().is_some_and(|owner| owner.valid(app)) && app.ui.direct_selection.drag.is_some()
}

/// Canvas pointer events for Direct Selection and the Pen's ⌘ / ⌥ modes; false when the event is
/// not theirs.
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: Modifiers) -> bool {
    cancel_stale(app);
    if app.ui.direct_selection.cancelled_drag {
        match ev {
            ToolEvent::Down { .. } => app.ui.direct_selection.cancelled_drag = false,
            ToolEvent::Up { .. } => {
                app.ui.direct_selection.cancelled_drag = false;
                return true;
            }
            ToolEvent::Move { .. } => return true,
        }
    }
    let tool = app.ui.tool;
    match ev {
        ToolEvent::Down { x, y, .. } => {
            let pen_idle = app.ui.pen.is_none();
            // Pen + ⌥ is Convert Point only while no path is being drawn.
            let convert = converting(tool, mods, pen_idle);
            if !(direct(tool, mods) || convert) {
                return false;
            }
            if !pen_idle && tool != Tool::DirectSelection {
                // ⌘-click ends the path being drawn, which becomes editable, as in Photoshop.
                crate::vector_ui::pen_commit(app, false);
                if app.ui.pen.is_some() {
                    app.ui.direct_selection.cancelled_drag = true;
                    return true;
                }
            }
            if tool == Tool::FreeformPen {
                // The same for a Freeform Pen path waiting for its next ⌥-click.
                crate::lasso_ui::commit(app);
                if app.ui.pen.is_some() {
                    app.ui.direct_selection.cancelled_drag = true;
                    return true;
                }
            }
            press(app, [x, y], mods, convert)
        }
        ToolEvent::Move { x, y, .. } => {
            let ds = &mut app.ui.direct_selection;
            match &mut ds.drag {
                Some(Drag::Edit { base, edit, start, end, preview, .. }) => {
                    *end = [x, y];
                    *preview = edit.preview(base, *start, *end);
                }
                Some(Drag::Marquee { end, .. }) => *end = [x, y],
                None => return owns(tool),
            }
            true
        }
        ToolEvent::Up { x, y } => match app.ui.direct_selection.drag.take() {
            Some(Drag::Edit { target, edit, start, .. }) => {
                release(app, target, &edit, start, [x, y]);
                true
            }
            Some(Drag::Marquee { start, add, .. }) => {
                marquee(app, start, [x, y], add);
                true
            }
            None => owns(tool),
        },
    }
}

/// A press: pick what's under the pointer and start a drag. False lets the Pen handle a ⌥-press
/// that hit nothing; the Convert Point tool has nothing else to do, so it keeps the press.
fn press(app: &mut PhotocraftApp, p: [f64; 2], mods: Modifiers, convert: bool) -> bool {
    let own_tool = app.ui.tool == Tool::ConvertPoint;
    let found = hit_target(app, p, true);
    let Some((target, base, hit)) = found else {
        if convert {
            return own_tool;
        }
        let owner = Owner::current(app);
        let ds = &mut app.ui.direct_selection;
        ds.drag = Some(Drag::Marquee { start: p, end: p, add: mods.shift });
        ds.owner = owner;
        return true;
    };
    if let PathRef::Layer(id) = &target
        && app.session.active().and_then(|st| st.active_layer).map(|layer| layer.0) != Some(*id)
    {
        let _ = app.run("layer.select", json!({"layer": id}));
    }
    let owner = Owner::current(app);
    let ds = &mut app.ui.direct_selection;
    if ds.target.as_ref() != Some(&target) {
        *ds = DirectSelection { target: Some(target.clone()), ..Default::default() };
    }
    ds.owner = owner;
    let edit = match hit {
        Hit::Handle(k, h) => {
            let kn = base.subpaths.get(k[0]).and_then(|s| s.knots.get(k[1]));
            let from = kn.map_or(Point::new(p[0], p[1]), |kn| if h == Handle::In { kn.in_ctrl } else { kn.out_ctrl });
            // Converting moves a handle on its own, as ⌥ does with Direct Selection.
            Edit::Handle(k, h, from, mods.alt || convert)
        }
        Hit::Anchor(k) if convert => {
            if own_tool || app.ui.tool == Tool::Pen {
                // The tool selects the anchor it works on, so the anchor's handles show.
                ds.segment = None;
                ds.anchors = vec![k];
            }
            Edit::Convert(k)
        }
        // Pen + ⌥ on a segment does nothing.
        Hit::Segment(..) if convert => return true,
        Hit::Anchor(k) => {
            ds.segment = None;
            if mods.alt {
                ds.anchors = edit::subpath_anchors(&base, k[0]);
            } else if mods.shift {
                // ⇧-click toggles the anchor; a click that deselects drags nothing.
                if let Some(i) = ds.anchors.iter().position(|a| *a == k) {
                    ds.anchors.remove(i);
                    return true;
                }
                ds.anchors.push(k);
            } else if !ds.anchors.contains(&k) {
                ds.anchors = vec![k];
            }
            Edit::Anchors(ds.anchors.clone())
        }
        Hit::Segment(k, _) if mods.alt => {
            ds.segment = None;
            ds.anchors = edit::subpath_anchors(&base, k[0]);
            Edit::Anchors(ds.anchors.clone())
        }
        Hit::Segment(k, t) => {
            ds.anchors.clear();
            ds.segment = Some(k);
            Edit::Segment(k, t)
        }
    };
    ds.drag = Some(Drag::Edit { target, base, edit, start: p, end: p, preview: None, converting: convert });
    true
}

/// End of an edit drag: commit it as one command. A click (no movement) commits nothing, except
/// Pen + ⌥ on an anchor, which makes it a corner.
fn release(app: &mut PhotocraftApp, target: PathRef, edit: &Edit, start: [f64; 2], end: [f64; 2]) {
    let moved = (end[0] - start[0]).hypot(end[1] - start[1]) * f64::from(app.point_zoom()) >= 1.0;
    let (id, mut params) = match edit {
        Edit::Convert([s, k]) if !moved => ("path.convertPoint", json!({"subpath": s, "knot": k})),
        _ if !moved => return,
        e => e.command(start, end),
    };
    if let (Some(p), Some(t)) = (params.as_object_mut(), target.params().as_object()) {
        p.extend(t.clone());
    }
    // sync_views validates ownership during command dispatch. Keep this selection off the
    // live UI until its own command finishes, then bind it to the resulting revision.
    let mut selection = std::mem::take(&mut app.ui.direct_selection);
    match app.run(id, params) {
        Ok(_) if selection.owner.as_ref().is_some_and(|owner| owner.same_target(app)) => {
            selection.owner = Owner::current(app);
            app.ui.direct_selection = selection;
        }
        Ok(_) => {}
        Err(e) => {
            if selection.owner.as_ref().is_some_and(|owner| owner.valid(app)) {
                app.ui.direct_selection = selection;
            }
            app.ui.status = e;
            app.ui.status_error = true;
        }
    }
}

/// End of a marquee: select the anchors inside it on the frontmost path that has any (⇧ adds).
/// A click on empty canvas deselects.
fn marquee(app: &mut PhotocraftApp, start: [f64; 2], end: [f64; 2], add: bool) {
    let rect = [start[0], start[1], end[0], end[1]];
    let hits = candidates(app).into_iter().map(|(r, path)| (r, edit::anchors_in(&path, rect))).find(|(_, a)| !a.is_empty());
    let owner = Owner::current(app);
    let ds = &mut app.ui.direct_selection;
    match hits {
        Some((r, anchors)) if add && ds.target.as_ref() == Some(&r) => {
            for a in anchors {
                if !ds.anchors.contains(&a) {
                    ds.anchors.push(a);
                }
            }
            ds.segment = None;
        }
        Some((r, anchors)) => *ds = DirectSelection { target: Some(r), anchors, ..Default::default() },
        None if add => {}
        None => {
            ds.anchors.clear();
            ds.segment = None;
        }
    }
    ds.owner = owner;
}

/// Paths with their anchors (selected ones filled), the shown handles, the drag's preview in
/// place of its path, and the marquee. `paths` false (View › Show › Target Path off) leaves only
/// the marquee.
pub fn draw_overlay(app: &PhotocraftApp, painter: &egui::Painter, to_scr: &dyn Fn([f64; 2]) -> Pos2, accent: Color32, paths: bool) {
    let ds = &app.ui.direct_selection;
    for (r, path) in if paths { candidates(app) } else { Vec::new() } {
        let path = match &ds.drag {
            Some(Drag::Edit { target, preview: Some(p), .. }) if target == &r => p.clone(),
            _ => path,
        };
        crate::vector_ui::draw_outline(painter, &path, to_scr, accent);
        let ours = ds.target.as_ref() == Some(&r);
        for &(k, h) in if ours { shown_handles(ds, &path) } else { Vec::new() }.iter() {
            if let Some(kn) = path.subpaths.get(k[0]).and_then(|s| s.knots.get(k[1])) {
                let c = if h == Handle::In { kn.in_ctrl } else { kn.out_ctrl };
                crate::vector_ui::draw_handle(painter, to_scr([kn.anchor.x, kn.anchor.y]), to_scr([c.x, c.y]), accent);
            }
        }
        for (s, sp) in path.subpaths.iter().enumerate() {
            for (k, kn) in sp.knots.iter().enumerate() {
                crate::vector_ui::draw_anchor(painter, to_scr([kn.anchor.x, kn.anchor.y]), ours && ds.anchors.contains(&[s, k]), accent);
            }
        }
    }
    if let Some(Drag::Marquee { start, end, .. }) = &ds.drag {
        let pts = [*start, [end[0], start[1]], *end, [start[0], end[1]]].map(to_scr);
        crate::tool_feedback::draw_ants(painter, &pts, true);
    }
}

#[cfg(test)]
#[path = "direct_select_tests.rs"]
mod tests;

#[cfg(test)]
mod freeform_command_tests {
    use super::*;

    fn app(tool: Tool) -> PhotocraftApp {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 200, "height": 200})).unwrap();
        for name in ["work", "Saved drawing"] {
            app.run("path.set", json!({"name": name, "path":{"subpaths":[{"knots":[[20,20],[120,20]]}]}})).unwrap();
        }
        app.sync_views();
        app.ui.tool = tool;
        app.ui.selected_path = Some("Saved drawing".into());
        app
    }

    #[test]
    fn command_with_freeform_and_curvature_edits_the_selected_saved_path_after_modifier_release() {
        for tool in [Tool::FreeformPen, Tool::CurvaturePen] {
            let mut app = app(tool);
            let original = app.session.active().unwrap().doc.clone();
            let n = app.session.active().unwrap().history.past_len();
            let command = Modifiers { command: true, ctrl: true, ..Default::default() };
            assert!(pointer(&mut app, ToolEvent::Down { x: 20.0, y: 20.0, pressure: 1.0 }, command));
            assert_eq!(app.ui.direct_selection.target, Some(PathRef::Saved("Saved drawing".into())));
            assert!(captured_drag(&app));
            assert!(pointer(&mut app, ToolEvent::Move { x: 30.0, y: 40.0, pressure: 1.0 }, Modifiers::NONE));
            assert!(pointer(&mut app, ToolEvent::Up { x: 30.0, y: 40.0 }, Modifiers::NONE));
            let st = app.session.active().unwrap();
            assert_eq!(st.doc.work_path, original.work_path);
            assert_eq!(st.doc.paths[0].path.subpaths[0].knots[0].anchor, Point::new(30.0, 40.0));
            assert_eq!(st.history.past_len(), n + 1);
            assert!(app.ui.direct_selection.owner.as_ref().unwrap().valid(&app));
            assert!(!captured_drag(&app));
            assert!(app.session.undo());
            assert_eq!(app.session.active().unwrap().doc.paths, original.paths);
        }
    }

    #[test]
    fn command_cannot_edit_an_existing_path_when_the_freeform_trace_failed_to_finish() {
        let mut app = app(Tool::FreeformPen);
        let original = app.session.active().unwrap().doc.clone();
        let n = app.session.active().unwrap().history.past_len();
        for [x, y] in [[40.0, 60.0], [1e40, 60.0], [140.0, 80.0]] {
            assert!(crate::lasso_ui::pointer(&mut app, ToolEvent::Down { x, y, pressure: 1.0 }, Modifiers::ALT));
            assert!(crate::lasso_ui::pointer(&mut app, ToolEvent::Up { x, y }, Modifiers::ALT));
        }
        let command = Modifiers { command: true, ctrl: true, ..Default::default() };
        assert!(pointer(&mut app, ToolEvent::Down { x: 20.0, y: 20.0, pressure: 1.0 }, command));
        let pen = app.ui.pen.as_ref().expect("failed Freeform commit must preserve its fitted draft");
        assert!(pen.commit_failed && pen.owner.is_some());
        assert!(app.ui.direct_selection.drag.is_none());
        assert!(app.ui.direct_selection.cancelled_drag);
        assert!(pointer(&mut app, ToolEvent::Up { x: 30.0, y: 40.0 }, command));
        assert!(app.ui.direct_selection.drag.is_none());
        assert_eq!(app.session.active().unwrap().doc.paths, original.paths);
        assert_eq!(app.session.active().unwrap().doc.work_path, original.work_path);
        assert_eq!(app.session.active().unwrap().history.past_len(), n);
    }
}
