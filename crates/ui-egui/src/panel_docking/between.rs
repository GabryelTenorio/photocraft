use super::*;

pub(super) struct Hit {
    pub before: Option<String>,
    pub after: Option<String>,
    pub receivers: Vec<craft_ui::docking::DropTarget<String>>,
}

fn projection(layout: &Layout<String>, regions: &crate::workspace_dock::DockRegions, host: DockHost) -> Layout<String> {
    if !regions.enabled {
        return layout.clone();
    }
    match host {
        DockHost::DocumentView(id) => regions.render_host(layout, host, &crate::document_docking::canvas_id(id)).layout,
        _ => regions.render(layout).layout,
    }
}

fn same_region(regions: &crate::workspace_dock::DockRegions, a: &str, b: &str) -> bool {
    regions.host(a) == regions.host(b) && regions.side(a) == regions.side(b) && regions.band(a) == regions.band(b)
}

pub(super) fn hit(
    app: &PhotocraftApp, pointer: egui::Pos2, members: &[String], targets: &[craft_ui::docking::DropTarget<String>],
) -> Option<Hit> {
    let layout = app.ui.docking.as_ref()?;
    if targets.iter().rev().find(|target| target.rect.contains(pointer))
        .is_some_and(|target| layout.floating.iter().any(|group| group.panels.contains(&target.anchor)))
    {
        return None;
    }
    let collapsed = collapsed_members(app, layout);
    for (host, geometry) in app.docking_geometry.iter().rev() {
        if geometry.generation != app.ui.docking_generation || !geometry.workspace.contains(pointer)
            || matches!(host, DockHost::DocumentView(id) if !app.ui.floating_documents.contains_key(id))
        {
            continue;
        }
        let display = projection(layout, &app.ui.dock_regions, *host);
        let measured = craft_ui::docking::measure_layout_with_boundaries(
            &display, geometry.workspace, geometry.screen, &geometry.style,
            |id| panel_permissions(id, false), |id| panel_limits(id, &collapsed),
        ).ok()?;
        let boundary = measured.boundaries.iter().filter(|boundary| {
            boundary.divider.expand(8.0).intersect(boundary.rect).contains(pointer)
                && boundary.first.first().is_some_and(|anchor| {
                    boundary.first.iter().chain(&boundary.second).all(|id| normalize(id).is_some() && same_region(&app.ui.dock_regions, anchor, id))
                })
        }).min_by(|a, b| {
            let distance = |boundary: &craft_ui::docking::SplitBoundary<String>| match boundary.axis {
                SplitAxis::Horizontal => (boundary.divider.center().x - pointer.x).abs(),
                SplitAxis::Vertical => (boundary.divider.center().y - pointer.y).abs(),
            };
            distance(a).total_cmp(&distance(b)).then_with(|| a.rect.area().total_cmp(&b.rect.area()))
        });
        if let Some(boundary) = boundary {
            let before = boundary.first.iter().find(|panel| !members.contains(panel)).cloned();
            let after = boundary.second.iter().find(|panel| !members.contains(panel)).cloned();
            let mut receivers = Vec::new();
            for target in targets {
                if target.tab_strip.is_none() || !target.accepts_tabs { continue; }
                let anchor = target.tabs.iter().find(|(panel, _)| {
                    !members.contains(panel) && (boundary.first.contains(panel) || boundary.second.contains(panel))
                }).map(|(panel, _)| panel.clone());
                if let Some(anchor) = anchor {
                    let mut target = target.clone();
                    if members.contains(&target.anchor) { target.anchor = anchor; }
                    if !receivers.iter().any(|known: &craft_ui::docking::DropTarget<String>| known.anchor == target.anchor) {
                        receivers.push(target);
                    }
                }
            }
            return Some(Hit { before, after, receivers });
        }
        // A floating document occludes the main host even when its divider is not a target.
        if matches!(host, DockHost::DocumentView(_)) { return None; }
    }
    None
}

pub(super) fn command(app: &mut PhotocraftApp, params: &Value, whole: bool) -> Result<Value, String> {
    if app.session.prefs().workspace_locked { return Err("workspace is locked".into()); }
    if ["side", "anchor", "zone", "placement", "scope", "documentView"].iter().any(|key| params.get(*key).is_some()) {
        return Err("insertBetween uses before/after neighbors and cannot be combined with other destinations".into());
    }
    let read = |key: &str| params.get(key).and_then(Value::as_str).and_then(normalize).map(str::to_owned)
        .ok_or_else(|| format!("{key} must name a registered panel"));
    let panel = read("panel")?;
    let before = read("before")?;
    let after = read("after")?;
    let original = app.ui.docking.clone().unwrap_or_else(|| legacy(app));
    if !valid(&original) || !app.ui.dock_regions.valid() { return Err("invalid panel layout".into()); }
    if !original.contains(&panel) || !original.contains(&before) || !original.contains(&after) || before == after {
        return Err("insertBetween requires a visible panel and two distinct visible neighbors".into());
    }
    if !same_region(&app.ui.dock_regions, &before, &after)
        || original.floating.iter().any(|group| group.panels.contains(&before) || group.panels.contains(&after))
    {
        return Err("neighbors must share a dock region".into());
    }
    let members = if whole { group_members(&original, &panel) } else { vec![panel.clone()] };
    if members.contains(&before) || members.contains(&after) { return Err("neighbors must remain after the source moves".into()); }
    let selected = if whole { selected_member(&original, &panel) } else { panel };
    let host = match app.ui.dock_regions.host(&before) { DockHost::Document => DockHost::Workspace, host => host };
    let geometry = app.docking_geometry.get(&host).filter(|geometry| geometry.generation == app.ui.docking_generation)
        .ok_or("dock geometry is unavailable; render the workspace before inserting")?;
    let mut layout = original.clone();
    let mut hidden = app.ui.docking_hidden.clone();
    let mut regions = app.ui.dock_regions.clone();
    for member in &members {
        hidden.insert(member.clone(), original.location(member).map_err(|error| error.to_string())?);
        layout.apply(Action::Close { panel: member.clone() }).map_err(|error| error.to_string())?;
    }
    let display = projection(&layout, &regions, host);
    let collapsed = collapsed_members(app, &layout);
    let measured = craft_ui::docking::measure_layout_with_boundaries(
        &display, geometry.workspace, geometry.screen, &geometry.style,
        |id| panel_permissions(id, false), |id| panel_limits(id, &collapsed),
    ).map_err(|error| error.to_string())?;
    let mut region_layout = Layout { root: regions.region_tree(&layout, &before), floating: Vec::new() };
    let region_members: Vec<_> = region_layout.panels().into_iter().cloned().collect();
    let bounds = measured.panels.iter().filter(|(id, _)| region_members.contains(id)).map(|(_, rect)| *rect)
        .reduce(|a, b| a.union(b)).ok_or("missing receiving dock region")?;
    region_layout.floating.push(Floating { panels: members.clone(), active: members.iter().position(|id| id == &selected).unwrap_or(0), rect: [0.0, 0.0, 300.0, 400.0] });
    craft_ui::docking::insert_between(
        &mut region_layout, craft_ui::docking::BetweenRequest { members: &members, selected: &selected, before: &before, after: &after },
        bounds, geometry.screen, &geometry.style, |id| panel_permissions(id, false), |id| panel_limits(id, &collapsed),
    ).map_err(|error| error.to_string())?;
    for member in &region_members {
        layout.apply(Action::Close { panel: member.clone() }).map_err(|error| error.to_string())?;
    }
    let incoming_region = region_layout.root.ok_or("missing inserted dock region")?;
    layout.root = Some(match layout.root.take() {
        Some(root) => Node::Split { axis: SplitAxis::Vertical, size: SplitSize::Ratio(0.5), first: Box::new(root), second: Box::new(incoming_region) },
        None => incoming_region,
    });
    let side = regions.side(&before);
    let destination_host = regions.host(&before);
    for member in &members {
        regions.inherit(member, &before);
        regions.sides.insert(member.clone(), side);
        regions.hosts.insert(member.clone(), destination_host);
    }
    if !valid(&layout) || !regions.valid() { return Err("invalid inserted panel layout".into()); }
    projection(&layout, &regions, host).validate().map_err(|error| error.to_string())?;
    regions.remember(&layout);
    if !regions.valid() { return Err("invalid dock return locations".into()); }
    app.ui.docking = Some(layout);
    app.ui.docking_hidden = hidden;
    app.ui.dock_regions = regions;
    app.ui.panels.dock = true;
    sync_visibility(app);
    Ok(json!({"layout":app.ui.docking}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_between_bad_automation_is_atomic_before_geometry_is_available() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        let before = serde_json::to_value(&app.ui).unwrap();
        for params in [
            json!({"panel":"color","before":"layers","after":"properties"}),
            json!({"panel":"unknown","before":"layers","after":"properties"}),
            json!({"panel":"color","before":false,"after":"properties"}),
            json!({"panel":"color","before":"layers","after":"layers"}),
            json!({"panel":"color","before":"layers","after":"properties","side":"left"}),
        ] {
            assert!(command(&mut app, &params, false).is_err());
            assert_eq!(serde_json::to_value(&app.ui).unwrap(), before);
        }
    }
}
