//! Playing a recorded action on a second document gives the same result as running its commands
//! there directly (#3410): a crop, a filter with a parameter, selection operations followed by a
//! fill, and a styled type layer, at every bit depth the command supports. Photoshop plays an
//! action's recorded commands, in order, against the active document, which need not be the one
//! it was recorded on.

use photocraft_doc::{Layer, LayerContent, LayerId};
use photocraft_engine::Session;
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::{Value, json};

const W: i32 = 48;
const H: i32 = 40;

/// A fresh document with a gradient background and two coloured rectangles on a second layer,
/// so crops, blurs and selection fills all change visible pixels. The active layer is the top one.
fn scene(s: &mut Session, depth: u32) {
    s.execute("file.new", json!({"width": W, "height": H, "depth": depth, "background": "white"})).unwrap();
    s.edit("gradient", |doc, _| {
        let surf = doc.layers[0].surface_mut().unwrap();
        let ch = surf.channels();
        let mut data = Vec::with_capacity((W * H) as usize * ch);
        for y in 0..H {
            for x in 0..W {
                let px = [x as f32 / (W - 1) as f32, y as f32 / (H - 1) as f32, 0.25, 1.0];
                data.extend((0..ch).map(|c| px.get(c).copied().unwrap_or(1.0)));
            }
        }
        surf.write_region(Rect::from_xywh(0, 0, W as u32, H as u32), &data);
        Ok(())
    })
    .unwrap();
    s.execute("layer.new.layer", json!({"name": "Shapes"})).unwrap();
    s.edit("shapes", |doc, _| {
        let surf = doc.layers[1].surface_mut().unwrap();
        surf.fill_rect(Rect::from_xywh(4, 6, 14, 10), &[1.0, 0.0, 0.0, 1.0]);
        surf.fill_rect(Rect::from_xywh(26, 18, 16, 16), &[0.0, 0.0, 1.0, 1.0]);
        Ok(())
    })
    .unwrap();
}

/// What a replay must reproduce: canvas size, every layer (kind, content, pixels, style), and
/// the selection mask. Layer ids are session-wide, so they are left out of the comparison.
#[derive(PartialEq)]
struct State {
    width: u32,
    height: u32,
    layers: Vec<Layer>,
    selection: Option<Surface>,
}

impl std::fmt::Debug for State {
    // The derived output dumps every tile; a summary keeps failures readable.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let layers: Vec<String> =
            self.layers.iter().map(|l| format!("{} ({}, {:?})", l.name, l.content.kind_name(), l.surface().map(|s| s.content_bounds()))).collect();
        write!(f, "{}x{} layers {:?} selection {:?}", self.width, self.height, layers, self.selection.as_ref().map(|m| m.content_bounds()))
    }
}

fn state(s: &Session) -> State {
    let d = &s.active().unwrap().doc;
    let layers = d
        .layers
        .iter()
        .map(|l| {
            let mut l = l.clone();
            l.id = LayerId(0);
            l
        })
        .collect();
    State { width: d.size.width, height: d.size.height, layers, selection: d.selection.clone() }
}

fn run(s: &mut Session, steps: &[(&str, Value)]) {
    for (id, p) in steps {
        s.execute(id, p.clone()).unwrap_or_else(|e| panic!("{id} {p}: {e}"));
    }
}

/// Records `steps` on document A, plays the action on a fresh identical document B, runs the
/// same commands directly on a third identical document C, and checks B == C (and A == C).
/// Returns the untouched and the replayed state for case-specific checks.
fn replay_matches_direct(name: &str, depth: u32, steps: &[(&str, Value)]) -> (State, State) {
    let mut s = Session::new();
    scene(&mut s, depth);
    let untouched = state(&s);
    s.execute("actions.record", json!({ "name": name })).unwrap();
    run(&mut s, steps);
    assert_eq!(s.execute("actions.stop", json!({})).unwrap()["steps"], steps.len(), "{name} depth {depth}: every step recorded");
    let recorded = state(&s);
    let got = s.execute("actions.get", json!({ "action": name })).unwrap();
    let ids: Vec<&str> = got["steps"].as_array().unwrap().iter().map(|st| st[0].as_str().unwrap()).collect();
    assert_eq!(ids, steps.iter().map(|(id, _)| *id).collect::<Vec<_>>(), "{name} depth {depth}");

    // B: play the action on another document.
    scene(&mut s, depth);
    assert_eq!(state(&s), untouched, "{name} depth {depth}: B starts like A did");
    let before = s.active().unwrap().history.past_len();
    let played = s.execute("actions.play", json!({ "action": name })).unwrap();
    assert!(played.get("failed").is_none(), "{name} depth {depth}: {played}");
    assert_eq!(played["ran"], steps.len(), "{name} depth {depth}");
    assert_eq!(s.active().unwrap().history.past_len(), before + steps.len(), "{name} depth {depth}: one history step per played step");
    assert_eq!(s.actions.playing, 0);
    let replayed = state(&s);

    // C: the same commands run directly.
    let mut direct = Session::new();
    scene(&mut direct, depth);
    run(&mut direct, steps);
    let expected = state(&direct);

    assert_ne!(expected, untouched, "{name} depth {depth}: the steps change the document");
    assert_eq!(replayed, expected, "{name} depth {depth}: playback matches running the commands");
    assert_eq!(recorded, expected, "{name} depth {depth}: and matches the recording document");
    (untouched, replayed)
}

#[test]
fn replayed_crop_matches_direct_crop_at_each_depth() {
    for depth in [8, 16, 32] {
        let steps = [("image.crop", json!({"x": 3, "y": 5, "width": 30, "height": 22}))];
        let (_, got) = replay_matches_direct("Crop", depth, &steps);
        assert_eq!((got.width, got.height), (30, 22), "depth {depth}");
        // The red rectangle (4,6) moved to (1,1) on the cropped canvas.
        assert_eq!(got.layers[1].surface().unwrap().pixel(1, 1), vec![1.0, 0.0, 0.0, 1.0], "depth {depth}");
    }
}

#[test]
fn replayed_gaussian_blur_matches_direct_blur_at_each_depth() {
    for depth in [8, 16, 32] {
        let steps = [("filter.blur.gaussianBlur", json!({"radius": 3.5}))];
        let (before, got) = replay_matches_direct("Blur", depth, &steps);
        // The blur reached the active (Shapes) layer, softening the red rectangle's edge.
        let (b, a) = (before.layers[1].surface().unwrap().pixel(4, 10), got.layers[1].surface().unwrap().pixel(4, 10));
        assert!(a[3] < b[3] && a[3] > 0.0, "depth {depth}: {b:?} -> {a:?}");
        assert_eq!(got.layers[0], before.layers[0], "depth {depth}: other layers untouched");
    }
}

#[test]
fn replayed_selection_operations_and_fill_match_direct_at_each_depth() {
    for depth in [8, 16, 32] {
        let steps = [
            ("select.rect", json!({"x": 6, "y": 4, "width": 30, "height": 24, "ellipse": true})),
            ("select.rect", json!({"x": 30, "y": 20, "width": 12, "height": 12, "mode": "add"})),
            ("select.rect", json!({"x": 14, "y": 10, "width": 8, "height": 8, "mode": "subtract"})),
            ("select.modify.feather", json!({"radius": 2})),
            ("select.inverse", json!({})),
            ("edit.fill", json!({"color": "#00ff00"})),
        ];
        let (_, got) = replay_matches_direct("Select", depth, &steps);
        let sel = got.selection.as_ref().expect("a selection after inverse");
        // Inverse of (ellipse + rect - hole), feathered: the corner outside the ellipse and the
        // subtracted hole are selected; the ellipse's inside and the added rect are not.
        assert!(sel.pixel(1, 38)[0] > 0.99, "depth {depth}: corner selected");
        assert!(sel.pixel(28, 20)[0] < 0.05, "depth {depth}: inside the ellipse not selected");
        assert!(sel.pixel(18, 14)[0] > 0.8, "depth {depth}: subtracted hole selected after inverse");
        assert!(sel.pixel(36, 26)[0] < 0.05, "depth {depth}: added rect not selected after inverse");
        let edge = sel.pixel(6, 16)[0];
        assert!(edge > 0.05 && edge < 0.95, "depth {depth}: feathered edge {edge}");
        let fill = got.layers[1].surface().unwrap();
        assert_eq!(fill.pixel(1, 38), vec![0.0, 1.0, 0.0, 1.0], "depth {depth}: filled through the selection");
    }
}

#[test]
fn replayed_styled_type_layer_matches_direct_at_each_depth() {
    for depth in [8, 16, 32] {
        let steps = [(
            "type.create",
            json!({"x": 4, "y": 28, "text": "Replay", "font": "Inter", "size": 18, "color": "#cc3311",
                   "tracking": 50, "underline": true, "name": "Title"}),
        )];
        let (before, got) = replay_matches_direct("Type", depth, &steps);
        assert_eq!(got.layers.len(), before.layers.len() + 1, "depth {depth}");
        let layer = &got.layers[2];
        assert_eq!(layer.name, "Title");
        let LayerContent::Text(t) = &layer.content else { panic!("depth {depth}: not a type layer: {}", layer.content.kind_name()) };
        assert_eq!(t.text, "Replay");
        let style = &t.runs[0].style;
        assert!((style.size_pt - 18.0).abs() < 1e-3, "depth {depth}: size {}", style.size_pt);
        let want = [0xcc as f32 / 255.0, 0x33 as f32 / 255.0, 0x11 as f32 / 255.0];
        assert!(style.color.c.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3), "depth {depth}: colour {:?}", style.color);
        assert_eq!(style.tracking, 50.0, "depth {depth}");
        assert!(style.underline, "depth {depth}");
        assert_eq!(style.font_family, "Inter");
        let px = layer.surface().expect("rendered type");
        assert!(!px.content_bounds().is_empty(), "depth {depth}: type rendered pixels");
    }
}
