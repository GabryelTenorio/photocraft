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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DockRegions {
    pub enabled: bool,
    pub sides: BTreeMap<String, DockSide>,
    /// Left/right widths and top/bottom heights, in logical points.
    pub sizes: [f32; 4],
}

impl Default for DockRegions {
    fn default() -> Self {
        Self { enabled: false, sides: BTreeMap::new(), sizes: [290.0, 290.0, 220.0, 220.0] }
    }
}

impl DockRegions {
    pub(crate) fn side(&self, panel: &str) -> DockSide {
        self.sides.get(panel).copied().unwrap_or_default()
    }

    pub(crate) fn valid(&self) -> bool {
        self.sides.len() <= 64
            && self.sides.keys().all(|id| crate::panel_docking::normalize(id) == Some(id.as_str()))
            && self.sizes.iter().all(|size| size.is_finite() && (80.0..=4096.0).contains(size))
    }

    /// A projection preserves every tab, stack and divider within a region. Each visible
    /// split remembers its original path so resizing never addresses a synthetic divider.
    pub(crate) fn render(&self, layout: &Layout<String>) -> RenderLayout {
        let mut rendered = Projected { node: Node::Tabs { panels: vec![CANVAS.into()], active: 0 }, splits: BTreeMap::new() };
        // Wrap inside-out: top/bottom span the full width, left/right share the middle band.
        for side in [DockSide::Right, DockSide::Left, DockSide::Bottom, DockSide::Top] {
            let Some(region) = layout.root.as_ref().and_then(|root| project(root, self, side, &mut Vec::new(), 0)) else { continue };
            let first_region = matches!(side, DockSide::Left | DockSide::Top);
            let mut splits = BTreeMap::new();
            for (path, target) in rendered.splits {
                splits.insert(prefixed(!first_region, path), target);
            }
            for (path, target) in region.splits {
                splits.insert(prefixed(first_region, path), target);
            }
            splits.insert(Vec::new(), SplitTarget::Region(side));
            let size = self.sizes.get(side.index()).copied().unwrap_or(290.0);
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
        RenderLayout { layout: Layout { root: Some(rendered.node), floating: layout.floating.clone() }, splits: rendered.splits }
    }
}

pub(crate) enum SplitTarget {
    Region(DockSide),
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

fn project(node: &Node<String>, regions: &DockRegions, side: DockSide, path: &mut Vec<bool>, depth: usize) -> Option<Projected> {
    if depth >= craft_ui::docking::MAX_DEPTH {
        return None;
    }
    match node {
        Node::Tabs { panels, active } => {
            let selected = panels.get(*active);
            let panels: Vec<_> = panels.iter().filter(|id| regions.side(id) == side).cloned().collect();
            if panels.is_empty() {
                return None;
            }
            let active = selected.and_then(|id| panels.iter().position(|p| p == id)).unwrap_or(0);
            Some(Projected { node: Node::Tabs { panels, active }, splits: BTreeMap::new() })
        }
        Node::Stack { entries } => {
            let entries: Vec<_> = entries.iter().filter(|entry| regions.side(&entry.panel) == side).cloned().collect();
            (!entries.is_empty()).then_some(Projected { node: Node::Stack { entries }, splits: BTreeMap::new() })
        }
        Node::Split { axis, size, first, second } => {
            path.push(false);
            let a = project(first, regions, side, path, depth + 1);
            path.pop();
            path.push(true);
            let b = project(second, regions, side, path, depth + 1);
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
    if distance > 24.0 {
        return None;
    }
    Some(side)
}
