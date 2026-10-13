//! Layer › New › Layer: Photoshop's New Layer options (name, Use Previous Layer to Create Clipping
//! Mask, color label, mode, opacity, and Fill with neutral color), applied in one history step.

use photocraft_doc::{BlendMode, Document, LabelColor, Layer, LayerId};
use serde_json::{Value, json};

use crate::{EngineError, Result, Session};

const CMD: &str = "layer.new.layer";

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

/// Where a newly created layer lands in the stack.
#[derive(Clone, Copy)]
pub(crate) enum Placement {
    /// Directly above the active layer, as Photoshop's Layer › New lands it.
    AboveActive,
    /// Directly above this layer.
    Above(LayerId),
    /// Top of the stack.
    Top,
}

/// The `"above"`/`"position"` params of the layer-creating commands: without either, a new layer
/// lands above the active one (Photoshop's default, and the anchor every client shares).
/// `"above": id` lands directly above that layer; `"position": "top"` lands topmost. Both
/// together, a `position` other than "top", or an `above` that is not a layer id is an error —
/// never a silent landing somewhere else.
pub(crate) fn placement(cmd: &str, p: &Value) -> Result<Placement> {
    let above = match p.get("above") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().ok_or_else(|| bad(cmd, "`above` must be a layer id"))?),
    };
    let top = match p.get("position") {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) if s == "top" => true,
        Some(_) => return Err(bad(cmd, "position must be \"top\"")),
    };
    match (above, top) {
        (None, false) => Ok(Placement::AboveActive),
        (Some(id), false) => Ok(Placement::Above(LayerId(id))),
        (None, true) => Ok(Placement::Top),
        (Some(_), true) => Err(bad(cmd, "give either `above` or `position`, not both")),
    }
}

/// Insert `layer` where `place` says. An `above` id that is not in the document is `NoLayer`,
/// never the not-found fallback that lands the layer at the top of the stack.
pub(crate) fn insert_at(doc: &mut Document, active: Option<LayerId>, place: Placement, layer: Layer) -> Result<LayerId> {
    Ok(match place {
        Placement::AboveActive => doc.insert_above(active, layer),
        Placement::Above(id) => {
            doc.layer(id).ok_or(EngineError::NoLayer(id))?;
            doc.insert_above(Some(id), layer)
        }
        Placement::Top => doc.insert_above(None, layer),
    })
}

/// The color that leaves everything below unchanged in `mode` (Photoshop's "Fill with
/// Multiply-neutral color (white)"), as RGB, and its name. `None` for modes that have none.
pub fn neutral_color(mode: BlendMode) -> Option<([f32; 3], &'static str)> {
    use BlendMode::*;
    match mode {
        Darken | Multiply | ColorBurn | LinearBurn | DarkerColor | Divide => Some(([1.0; 3], "white")),
        Lighten | Screen | ColorDodge | LinearDodge | LighterColor | Difference | Exclusion | Subtract => Some(([0.0; 3], "black")),
        Overlay | SoftLight | HardLight | VividLight | LinearLight | PinLight => Some(([0.5; 3], "50% gray")),
        _ => None,
    }
}

/// `layer.new.layer`: a pixel layer above the active one (or where `"above"`/`"position"` says),
/// joining the clip group it lands in.
pub fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let place = placement(CMD, p)?;
    let flag = |k: &str| match p.get(k) {
        None | Some(Value::Null) => Ok(false),
        Some(v) => v.as_bool().ok_or_else(|| bad(CMD, format!("`{k}` must be true or false"))),
    };
    let clip = flag("clipToPrevious")?;
    let fill_neutral = flag("fillNeutral")?;
    let mode = match p.get("mode") {
        None | Some(Value::Null) => BlendMode::Normal,
        Some(v) => {
            let name = v.as_str().ok_or_else(|| bad(CMD, "`mode` must be a blend mode name"))?;
            crate::commands::blend_from_str(name).filter(|m| *m != BlendMode::PassThrough).ok_or_else(|| bad(CMD, format!("unknown layer blend mode `{name}`")))?
        }
    };
    let opacity = match p.get("opacity") {
        None | Some(Value::Null) => 1.0,
        Some(v) => v.as_f64().filter(|o| o.is_finite()).ok_or_else(|| bad(CMD, "`opacity` must be a number from 0 to 1"))?.clamp(0.0, 1.0) as f32,
    };
    let label = match p.get("color") {
        None | Some(Value::Null) => LabelColor::None,
        Some(v) => v.as_str().and_then(LabelColor::from_id).ok_or_else(|| bad(CMD, format!("`color` must be a layer color id, not {v}")))?,
    };
    let neutral =
        if fill_neutral { Some(neutral_color(mode).ok_or_else(|| bad(CMD, format!("{} has no neutral color to fill with", mode.label())))?.0) } else { None };
    let name = match p.get("name") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_str().ok_or_else(|| bad(CMD, "`name` must be a string"))?.to_string()),
    };
    let id = s.edit("New Layer", |doc, active| {
        let name = name.clone().unwrap_or_else(|| doc.next_layer_name("Layer"));
        let mut layer = Layer::raster(name, doc.pixel_format());
        layer.blend = mode;
        layer.opacity = opacity;
        layer.label = label;
        if let Some(rgb) = neutral {
            let bounds = doc.bounds();
            let surface = layer.surface_mut().ok_or_else(|| bad(CMD, "a new pixel layer has no pixels"))?;
            let px = photocraft_raster::from_rgba(&surface.format(), [rgb[0], rgb[1], rgb[2], 1.0]);
            surface.fill_rect(bounds, &px);
        }
        let id = insert_at(doc, *active, place, layer)?;
        crate::clip_group::settle(doc, &[id], &Default::default(), clip);
        *active = Some(id);
        Ok(id)
    })?;
    crate::layer_multi_cmds::note_inert_insert(s, id);
    Ok(json!({ "layer": id.0 }))
}

#[cfg(test)]
mod tests {
    use photocraft_doc::{BlendMode, LabelColor, LayerId};
    use photocraft_geom::Rect;
    use serde_json::json;

    use super::neutral_color;
    use crate::{EngineError, Session};

    /// A document with a busy Background: every pixel a different colour.
    fn busy(mode: &str, depth: u32) -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 32, "height": 32, "mode": mode, "depth": depth})).unwrap();
        s.edit("busy", |doc, active| {
            let surf = doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap();
            let fmt = surf.format();
            for y in 0..32 {
                for x in 0..32 {
                    let rgba = [x as f32 / 31.0, y as f32 / 31.0, ((x * 7 + y * 3) % 32) as f32 / 31.0, 1.0];
                    surf.fill_rect(Rect::new(x, y, x + 1, y + 1), &photocraft_raster::from_rgba(&fmt, rgba));
                }
            }
            Ok(())
        })
        .unwrap();
        s
    }

    fn max_diff(a: &photocraft_compose::Buffer, b: &photocraft_compose::Buffer) -> f32 {
        a.px.iter().zip(&b.px).flat_map(|(p, q)| p.iter().zip(q).map(|(x, y)| (x - y).abs())).fold(0.0, f32::max)
    }

    /// Fill with neutral color leaves the composite unchanged in every mode that offers it, at
    /// every depth; the wrong neutral (gray for Multiply) does change it.
    #[test]
    fn neutral_fills_leave_the_image_unchanged() {
        for (mode, depth) in [("rgb", 8), ("rgb", 16), ("rgb", 32), ("gray", 8), ("gray", 16)] {
            let s0 = busy(mode, depth);
            let doc0 = s0.active().unwrap().doc.clone();
            let shown = photocraft_compose::render(&doc0, doc0.bounds());
            for m in BlendMode::LAYER_MODES.into_iter().filter(|m| neutral_color(*m).is_some()) {
                let mut s = busy(mode, depth);
                s.execute("layer.new.layer", json!({"mode": m.label(), "fillNeutral": true})).unwrap();
                let doc = &s.active().unwrap().doc;
                let top = doc.layers.last().unwrap();
                assert_eq!(top.blend, m);
                assert!(top.surface().unwrap().tile_count() > 0, "{m:?} filled");
                let d = max_diff(&photocraft_compose::render(doc, doc.bounds()), &shown);
                // 8-bit stores 50% gray as 128 (0.502), which can move a pixel by one level.
                let tolerance = if depth == 8 { 1.0 / 255.0 + 1e-6 } else { 1e-3 };
                assert!(d <= tolerance, "{mode}/{depth}: {m:?} changed the image by {d}");
            }
            let mut s = busy(mode, depth);
            s.execute("layer.new.layer", json!({"mode": "Multiply"})).unwrap();
            s.execute("edit.fill", json!({"contents": "gray"})).unwrap();
            let doc = &s.active().unwrap().doc;
            assert!(max_diff(&photocraft_compose::render(doc, doc.bounds()), &shown) > 0.1, "{mode}/{depth}: the control changes it");
        }
        for m in [BlendMode::Normal, BlendMode::Dissolve, BlendMode::HardMix, BlendMode::Hue, BlendMode::Saturation, BlendMode::Color, BlendMode::Luminosity] {
            assert!(neutral_color(m).is_none(), "{m:?}");
        }
    }

    #[test]
    fn options_apply_in_one_step() {
        let mut s = busy("rgb", 8);
        let steps = s.active().unwrap().history.past_len();
        let id =
            s.execute("layer.new.layer", json!({"name": "Shade", "mode": "Overlay", "opacity": 0.4, "color": "blue", "fillNeutral": true})).unwrap()["layer"]
                .as_u64()
                .unwrap();
        let st = s.active().unwrap();
        assert_eq!(st.history.past_len(), steps + 1);
        let l = st.doc.layer(photocraft_doc::LayerId(id)).unwrap();
        assert_eq!((l.name.as_str(), l.blend, l.opacity, l.label), ("Shade", BlendMode::Overlay, 0.4, LabelColor::Blue));
        assert_eq!(st.active_layer, Some(l.id));
        // Defaults: what the Layers panel button makes.
        s.execute("layer.new.layer", json!({})).unwrap();
        let st = s.active().unwrap();
        let l = st.doc.layers.last().unwrap();
        assert_eq!((l.blend, l.opacity, l.label, l.clipped), (BlendMode::Normal, 1.0, LabelColor::None, false));
        assert_eq!(l.surface().unwrap().tile_count(), 0);
    }

    /// Bad params are errors, never panics, and change nothing.
    #[test]
    fn bad_params_are_errors() {
        let mut s = busy("rgb", 8);
        let n = s.active().unwrap().doc.layers.len();
        for p in [
            json!({"mode": "Pass Through"}),
            json!({"mode": "Sparkle"}),
            json!({"mode": 3}),
            json!({"opacity": "half"}),
            json!({"color": "plaid"}),
            json!({"color": 1}),
            json!({"fillNeutral": true}),
            json!({"fillNeutral": true, "mode": "Hue"}),
            json!({"fillNeutral": "yes"}),
            json!({"clipToPrevious": 1}),
            json!({"name": 7}),
        ] {
            assert!(s.execute("layer.new.layer", p.clone()).is_err(), "{p}");
        }
        assert_eq!(s.active().unwrap().doc.layers.len(), n);
        // Out-of-range opacity is clamped, as layer.setProps does.
        s.execute("layer.new.layer", json!({"opacity": 7})).unwrap();
        assert_eq!(s.active().unwrap().doc.layers.last().unwrap().opacity, 1.0);
    }

    /// A document with a Background plus `names` (bottom→top), and the shared active layer moved
    /// to the Background, as another client's `layer.select` leaves it.
    fn shared_anchor_on_background(names: &[&str]) -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
        for n in names {
            s.execute("layer.new.layer", json!({"name": n})).unwrap();
        }
        let bg = s.active().unwrap().doc.layers.first().unwrap().id;
        s.execute("layer.select", json!({"layer": bg.0})).unwrap();
        s
    }

    /// The stack's layer names, bottom→top (storage order).
    fn stack(s: &Session) -> Vec<&str> {
        s.active().map(|st| st.doc.layers.iter().map(|l| l.name.as_str()).collect()).unwrap_or_default()
    }

    /// The FIFO interleaving that hid a worker's fresh layer under an opaque veil: with the
    /// shared active layer on the Background, a plain create lands directly above it — below the
    /// veil, not at the top — and the new layer becomes the active one.
    #[test]
    fn new_layer_lands_above_shared_active_layer_not_at_top() {
        let mut s = shared_anchor_on_background(&["Veil"]);
        let forest = s.execute("layer.new.layer", json!({"name": "forest"})).unwrap();
        let st = s.active().unwrap();
        assert_eq!(stack(&s), ["Background", "forest", "Veil"]);
        assert_eq!(forest["layer"].as_u64(), st.active_layer.map(|id| id.0));
    }

    /// `"position":"top"` lands the new layer topmost, wherever the shared active layer is.
    #[test]
    fn position_top_lands_the_layer_topmost() {
        let mut s = shared_anchor_on_background(&["Veil"]);
        s.execute("layer.new.layer", json!({"name": "forest", "position": "top"})).unwrap();
        assert_eq!(stack(&s), ["Background", "Veil", "forest"]);
    }

    /// `"above":id` lands directly above that layer, even with the shared active layer elsewhere.
    #[test]
    fn above_lands_directly_above_that_layer() {
        let mut s = shared_anchor_on_background(&["Mid", "Veil"]);
        let mid = s.active().unwrap().doc.layers.iter().find(|l| l.name == "Mid").unwrap().id;
        s.execute("layer.new.layer", json!({"name": "forest", "above": mid.0})).unwrap();
        assert_eq!(stack(&s), ["Background", "Mid", "forest", "Veil"]);
    }

    /// An `above` id that is not in the document is an error that creates nothing and records no
    /// history step — never the not-found fallback that lands the layer at the top.
    #[test]
    fn unknown_above_id_creates_nothing() {
        let mut s = shared_anchor_on_background(&["Veil"]);
        let st = s.active().unwrap();
        let (n, steps) = (st.doc.layers.len(), st.history.past_len());
        assert!(matches!(s.execute("layer.new.layer", json!({"above": 9_999})), Err(EngineError::NoLayer(LayerId(9_999)))));
        let st = s.active().unwrap();
        assert_eq!(st.doc.layers.len(), n);
        assert_eq!(st.history.past_len(), steps);
    }

    /// Positioning params that name no landing spot are errors that create nothing.
    #[test]
    fn bad_positioning_params_are_errors() {
        let mut s = shared_anchor_on_background(&["Veil"]);
        let n = s.active().unwrap().doc.layers.len();
        for p in [
            json!({"position": "bottom"}),
            json!({"position": "Top"}),
            json!({"position": 3}),
            json!({"above": "Veil"}),
            json!({"above": -1}),
            json!({"above": 1, "position": "top"}),
        ] {
            assert!(matches!(s.execute("layer.new.layer", p.clone()), Err(EngineError::BadParams { .. })), "{p}");
        }
        assert_eq!(s.active().unwrap().doc.layers.len(), n);
    }

    /// `layer.new.group` lands with the same default and positioning params as `layer.new.layer`.
    #[test]
    fn new_group_takes_the_positioning_params_too() {
        let mut s = shared_anchor_on_background(&["Veil"]);
        s.execute("layer.new.group", json!({"name": "G", "position": "top"})).unwrap();
        assert_eq!(stack(&s), ["Background", "Veil", "G"]);
        let bg = s.active().unwrap().doc.layers.first().unwrap().id;
        s.execute("layer.select", json!({"layer": bg.0})).unwrap();
        s.execute("layer.new.group", json!({"name": "H", "above": bg.0})).unwrap();
        assert_eq!(stack(&s), ["Background", "H", "Veil", "G"]);
        s.execute("layer.select", json!({"layer": bg.0})).unwrap();
        s.execute("layer.new.group", json!({"name": "I"})).unwrap();
        assert_eq!(stack(&s), ["Background", "I", "H", "Veil", "G"]);
        let n = s.active().unwrap().doc.layers.len();
        assert!(matches!(s.execute("layer.new.group", json!({"above": 9_999})), Err(EngineError::NoLayer(LayerId(9_999)))));
        assert!(matches!(s.execute("layer.new.group", json!({"position": "bottom"})), Err(EngineError::BadParams { .. })));
        assert_eq!(s.active().unwrap().doc.layers.len(), n);
    }
}
