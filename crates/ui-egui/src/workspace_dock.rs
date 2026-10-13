//! App-owned placement around a protected canvas, using the shared docking tree and renderer.
//! Panel IDs retain their last side while floating or hidden so they can return there.

use std::collections::BTreeMap;

use craft_ui::docking::{Layout, Node, Zone};
use craft_ui::layout::{SplitAxis, SplitSize};
use serde::{Deserialize, Serialize};

pub(crate) const CANVAS: &str = "__photocraft_canvas";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DockSide {
    Left,
    #[default]
    Right,
    Top,
    Bottom,
}

impl DockSide {
    pub(crate) const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Top, Self::Bottom];

    pub(crate) fn parse(value: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value.clone()).map_err(|_| "side must be left, right, top or bottom".into())
    }

    pub(crate) fn from_zone(zone: Zone) -> Option<Self> {
        match zone {
            Zone::Left => Some(Self::Left),
            Zone::Right => Some(Self::Right),
            Zone::Top => Some(Self::Top),
            Zone::Bottom => Some(Self::Bottom),
            Zone::Center => None,
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Top => 2,
            Self::Bottom => 3,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DockHost {
    #[default]
    Workspace,
    Document,
    DocumentView(u64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DockRegions {
    pub enabled: bool,
    pub sides: BTreeMap<String, DockSide>,
    /// Left/right widths and top/bottom heights, in logical points.
    pub sizes: [f32; 4],
    pub hosts: BTreeMap<String, DockHost>,
    /// Zero is the legacy side region; larger bands nest closer to the document.
    pub bands: BTreeMap<String, u32>,
    pub band_sizes: BTreeMap<u32, f32>,
    pub document_sizes: [f32; 4],
    pub view_sizes: BTreeMap<u64, [f32; 4]>,
    pub floating_memory: BTreeMap<String, [f32; 4]>,
    pub dock_returns: BTreeMap<String, craft_ui::docking::Location<String>>,
}

impl Default for DockRegions {
    fn default() -> Self {
        Self {
            enabled: false,
            sides: BTreeMap::new(),
            sizes: [290.0, 290.0, 220.0, 220.0],
            hosts: BTreeMap::new(),
            bands: BTreeMap::new(),
            band_sizes: BTreeMap::new(),
            document_sizes: [290.0, 290.0, 220.0, 220.0],
            view_sizes: BTreeMap::new(),
            floating_memory: BTreeMap::new(),
            dock_returns: BTreeMap::new(),
        }
    }
}

impl DockRegions {
    pub(crate) fn side(&self, panel: &str) -> DockSide {
        self.sides.get(panel).copied().unwrap_or_default()
    }

    pub(crate) fn host(&self, panel: &str) -> DockHost {
        self.hosts.get(panel).copied().unwrap_or_default()
    }

    pub(crate) fn band(&self, panel: &str) -> u32 {
        self.bands.get(panel).copied().unwrap_or(0)
    }

    pub(crate) fn inherit(&mut self, panel: &str, anchor: &str) {
        self.bands.insert(panel.into(), self.band(anchor));
    }

    pub(crate) fn sizes_for(&self, host: DockHost) -> [f32; 4] {
        match host {
            DockHost::Workspace => self.sizes,
            DockHost::Document => self.document_sizes,
            DockHost::DocumentView(id) => self.view_sizes.get(&id).copied().unwrap_or(self.document_sizes),
        }
    }

    pub(crate) fn remember(&mut self, layout: &Layout<String>) {
        self.band_sizes.retain(|band, _| self.bands.values().any(|value| value == band));
        for panel in layout.panels() {
            if let Ok(location) = layout.location(panel)
                && location.floating.is_none()
            {
                self.dock_returns.insert(panel.clone(), location);
            }
        }
        for group in &layout.floating {
            for panel in &group.panels {
                self.floating_memory.insert(panel.clone(), group.rect);
            }
        }
    }

    /// Named workspaces do not capture session-specific document identities.
    pub(crate) fn portable(&self) -> Self {
        let mut result = self.clone();
        for host in result.hosts.values_mut() {
            if matches!(host, DockHost::DocumentView(_)) {
                *host = DockHost::Document;
            }
        }
        result.view_sizes.clear();
        result
    }

    pub(crate) fn valid(&self) -> bool {
        self.bands.len() <= 64
            && self.bands.keys().all(|id| crate::panel_docking::normalize(id) == Some(id.as_str()))
            && self.band_sizes.len() <= 64
            && self.band_sizes.iter().all(|(band, size)| *band > 0 && size.is_finite() && (80.0..=4096.0).contains(size))
            && self.sides.len() <= 64
            && self.hosts.len() <= 64
            && self.dock_returns.len() <= 64
            && self.dock_returns.keys().all(|id| crate::panel_docking::normalize(id) == Some(id.as_str()))
            && self.floating_memory.len() <= 64
            && self.view_sizes.len() <= 64
            && self.hosts.keys().chain(self.floating_memory.keys()).all(|id| crate::panel_docking::normalize(id) == Some(id.as_str()))
            && self
                .floating_memory
                .values()
                .all(|r| r.iter().all(|v| v.is_finite() && v.abs() <= 1_000_000.0) && r.get(2).is_some_and(|v| *v > 0.0) && r.get(3).is_some_and(|v| *v > 0.0))
            && self.document_sizes.iter().chain(self.view_sizes.values().flatten()).all(|size| size.is_finite() && (80.0..=4096.0).contains(size))
            && self.sides.keys().all(|id| crate::panel_docking::normalize(id) == Some(id.as_str()))
            && self.sizes.iter().all(|size| size.is_finite() && (80.0..=4096.0).contains(size))
    }

    /// A projection preserves every tab, stack and divider within a region. Each visible
    /// split remembers its original path so resizing never addresses a synthetic divider.
    pub(crate) fn render(&self, layout: &Layout<String>) -> RenderLayout {
        self.render_host(layout, DockHost::Workspace, CANVAS)
    }

    pub(crate) fn region_tree(&self, layout: &Layout<String>, panel: &str) -> Option<Node<String>> {
        project(layout.root.as_ref()?, self, (self.host(panel), self.side(panel), self.band(panel)), &mut Vec::new(), 0)
            .map(|projected| projected.node)
    }

    pub(crate) fn render_host(&self, layout: &Layout<String>, host: DockHost, canvas: &str) -> RenderLayout {
        let mut rendered = Projected { node: Node::Tabs { panels: vec![canvas.into()], active: 0 }, splits: BTreeMap::new() };
        let hosts = if host == DockHost::Workspace { vec![DockHost::Document, host] } else { vec![host] };
        for host in hosts {
            let mut bands: Vec<_> = self.bands.values().copied().chain(std::iter::once(0)).collect();
            bands.sort_unstable();
            bands.dedup();
            for band in bands.into_iter().rev() {
                for side in [DockSide::Right, DockSide::Left, DockSide::Bottom, DockSide::Top] {
                    let Some(region) = layout.root.as_ref().and_then(|root| project(root, self, (host, side, band), &mut Vec::new(), 0)) else { continue };
                    let first_region = matches!(side, DockSide::Left | DockSide::Top);
                    let mut splits = BTreeMap::new();
                    for (path, target) in rendered.splits {
                        splits.insert(prefixed(!first_region, path), target);
                    }
                    for (path, target) in region.splits {
                        splits.insert(prefixed(first_region, path), target);
                    }
                    splits.insert(Vec::new(), SplitTarget::Region(host, side, band));
                    let size = self
                        .band_sizes
                        .get(&band)
                        .copied()
                        .unwrap_or_else(|| self.sizes_for(host).get(side.index()).copied().unwrap_or(290.0));
                    let (first, second) = if first_region { (region.node, rendered.node) } else { (rendered.node, region.node) };
                    rendered = Projected {
                        node: Node::Split {
                            axis: if matches!(side, DockSide::Left | DockSide::Right) { SplitAxis::Horizontal } else { SplitAxis::Vertical },
                            size: if first_region { SplitSize::FixedFirst(size) } else { SplitSize::FixedSecond(size) },
                            first: Box::new(first),
                            second: Box::new(second),
                        },
                        splits,
                    };
                }
            }
        }
        RenderLayout {
            layout: Layout { root: Some(rendered.node), floating: if host == DockHost::Workspace { layout.floating.clone() } else { Vec::new() } },
            splits: rendered.splits,
        }
    }
}

pub(crate) enum SplitTarget {
    Region(DockHost, DockSide, u32),
    Tree(Vec<bool>),
}

pub(crate) struct RenderLayout {
    pub layout: Layout<String>,
    pub splits: BTreeMap<Vec<bool>, SplitTarget>,
}

struct Projected {
    node: Node<String>,
    splits: BTreeMap<Vec<bool>, SplitTarget>,
}

fn prefixed(first: bool, mut path: Vec<bool>) -> Vec<bool> {
    path.insert(0, !first);
    path
}

fn project(node: &Node<String>, regions: &DockRegions, region: (DockHost, DockSide, u32), path: &mut Vec<bool>, depth: usize) -> Option<Projected> {
    let (host, side, band) = region;
    if depth >= craft_ui::docking::MAX_DEPTH {
        return None;
    }
    match node {
        Node::Tabs { panels, active } => {
            let selected = panels.get(*active);
            let panels: Vec<_> = panels
                .iter()
                .filter(|id| regions.side(id) == side && regions.host(id) == host && regions.band(id) == band)
                .cloned()
                .collect();
            if panels.is_empty() {
                return None;
            }
            let active = selected.and_then(|id| panels.iter().position(|p| p == id)).unwrap_or(0);
            Some(Projected { node: Node::Tabs { panels, active }, splits: BTreeMap::new() })
        }
        Node::Stack { entries } => {
            let entries: Vec<_> = entries
                .iter()
                .filter(|entry| regions.side(&entry.panel) == side && regions.host(&entry.panel) == host && regions.band(&entry.panel) == band)
                .cloned()
                .collect();
            (!entries.is_empty()).then_some(Projected { node: Node::Stack { entries }, splits: BTreeMap::new() })
        }
        Node::Split { axis, size, first, second } => {
            path.push(false);
            let a = project(first, regions, region, path, depth + 1);
            path.pop();
            path.push(true);
            let b = project(second, regions, region, path, depth + 1);
            path.pop();
            match (a, b) {
                (Some(a), Some(b)) => {
                    let mut splits = BTreeMap::new();
                    splits.insert(Vec::new(), SplitTarget::Tree(path.clone()));
                    for (p, target) in a.splits {
                        splits.insert(prefixed(true, p), target);
                    }
                    for (p, target) in b.splits {
                        splits.insert(prefixed(false, p), target);
                    }
                    Some(Projected { node: Node::Split { axis: *axis, size: *size, first: Box::new(a.node), second: Box::new(b.node) }, splits })
                }
                (Some(p), None) | (None, Some(p)) => Some(p),
                (None, None) => None,
            }
        }
    }
}

/// Edge destinations have a small, deterministic hit band. Corners choose the closest edge.
pub(crate) fn edge_at(rect: egui::Rect, pointer: egui::Pos2) -> Option<DockSide> {
    edge_within(rect, pointer, 12.0)
}

pub(crate) fn edge_within(rect: egui::Rect, pointer: egui::Pos2, width: f32) -> Option<DockSide> {
    if !rect.is_finite() || !pointer.is_finite() || !rect.contains(pointer) || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let (distance, side) = [
        (pointer.x - rect.left(), DockSide::Left),
        (rect.right() - pointer.x, DockSide::Right),
        (pointer.y - rect.top(), DockSide::Top),
        (rect.bottom() - pointer.y, DockSide::Bottom),
    ]
    .into_iter()
    .min_by(|a, b| a.0.total_cmp(&b.0))?;
    if distance > width {
        return None;
    }
    Some(side)
}
