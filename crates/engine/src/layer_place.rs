//! Where a new layer goes, as in Photoshop. Above the active layer, or as the top layer inside the
//! active group when that group is open (an artboard always takes it); with nothing active, at the
//! top of the stack. A layer that lands inside a clipping mask joins it: it is clipped when the layer
//! just above its slot is clipped, the rule Photoshop follows on every insert (#2203), so a new
//! layer (or group) between a base and its clipped layers never becomes their (empty) base.

use photocraft_doc::{Document, Layer, LayerContent, LayerId, LayerPath, MAX_GROUP_DEPTH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// New Layer, New Group, new adjustment, fill, type and shape layers, Paste…: inside the anchor
    /// when it is an open group (or an artboard), else directly above it.
    Auto,
    /// Directly above the anchor, never inside it: a layer made from another layer, or the second
    /// and later of several new layers.
    Above,
    /// Directly below the anchor (⌘-click on the Layers panel's New Layer or New Group button). Never
    /// under the Background, where it goes above instead.
    Below,
}

/// The slot `layer` would take: its parent's path (empty for the root) and its index among the
/// parent's children (bottom to top). A missing or stale `anchor` means the top of the stack.
pub fn insertion_point(doc: &Document, anchor: Option<LayerId>, layer: &Layer, place: Place) -> (LayerPath, usize) {
    let root_top = (Vec::new(), doc.layers.len());
    let Some(path) = anchor.and_then(|a| doc.path_of(a)) else { return root_top };
    let (Some((&k, parent)), Some(target)) = (path.split_last(), doc.layer_at(&path)) else { return root_top };
    match place {
        Place::Auto if takes_new_layers(target, path.len(), layer) => (path.clone(), target.children().map_or(0, <[Layer]>::len)),
        Place::Below if crate::background_cmds::background_id(doc) != Some(target.id) => (parent.to_vec(), k),
        _ => (parent.to_vec(), k + 1),
    }
}

/// Inserts `layer` at its [`insertion_point`], clipped when it lands inside a clipping mask (a
/// flag it already has is kept); returns its id. Never fails: a slot that can't be reached puts
/// the layer on top of the stack.
pub fn insert_new(doc: &mut Document, anchor: Option<LayerId>, mut layer: Layer, place: Place) -> LayerId {
    let id = layer.id;
    let (parent, index) = insertion_point(doc, anchor, &layer, place);
    let sib = if parent.is_empty() { Some(&mut doc.layers) } else { doc.layer_at_mut(&parent).and_then(Layer::children_mut) };
    if let Some(sib) = sib {
        let index = index.min(sib.len());
        if sib.get(index).is_some_and(|above| above.clipped) {
            layer.clipped = true;
        }
        sib.insert(index, layer);
    } else {
        doc.layers.push(layer);
    }
    id
}

/// Does the group `target` at depth `depth` (its path length) take a new `layer` as its top
/// child? An open group or an artboard does, unless it is fully locked (the layer would be born
/// locked) or nesting would pass [`MAX_GROUP_DEPTH`]. Artboards stay at the top level.
fn takes_new_layers(target: &Layer, depth: usize, layer: &Layer) -> bool {
    let LayerContent::Group(g) = &target.content else { return false };
    (g.expanded || g.artboard.is_some()) && !target.locks.all && layer.artboard().is_none() && depth + subtree_depth(layer) <= MAX_GROUP_DEPTH
}

/// Group nesting inside `layer` itself: 0 for a layer or an empty group, as
/// [`Document::max_group_depth`] counts it. Explicit stack: pasted groups can be deep.
fn subtree_depth(layer: &Layer) -> usize {
    let mut max = 0;
    let mut stack: Vec<(&Layer, usize)> = vec![(layer, 0)];
    while let Some((l, depth)) = stack.pop() {
        if let Some(children) = l.children().filter(|c| !c.is_empty()) {
            max = max.max(depth + 1);
            stack.extend(children.iter().map(|c| (c, depth + 1)));
        }
    }
    max
}

#[cfg(test)]
#[path = "layer_place_tests.rs"]
mod tests;
