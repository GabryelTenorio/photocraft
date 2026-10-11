use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, LayerId, Pattern};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::Session;

/// Document 0 (`w`×`h`, `depth`) with a red 10×10 square at (4, 6) on layer "paint" above the
/// Background, and document 1 (60×50, 8-bit RGB). Document 0 is active.
fn two_docs(w: u32, h: u32, depth: u32) -> (Session, LayerId) {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": w, "height": h, "depth": depth})).unwrap();
    let paint = LayerId(s.execute("layer.new.layer", json!({"name": "paint"})).unwrap()["layer"].as_u64().unwrap());
    s.execute("select.rect", json!({"x": 4, "y": 6, "width": 10, "height": 10})).unwrap();
    s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    s.execute("file.new", json!({"width": 60, "height": 50})).unwrap();
    s.set_active(0);
    (s, paint)
}

fn doc(s: &Session, i: usize) -> &Document {
    &s.documents()[i].doc
}

fn copied(r: &Value) -> LayerId {
    LayerId(r["layers"][0].as_u64().unwrap())
}

fn bounds(s: &Session, i: usize, id: LayerId) -> Rect {
    crate::layer_multi_cmds::layer_bounds(doc(s, i).layer(id).unwrap()).unwrap()
}

#[test]
fn copies_the_selected_layers_above_the_destinations_active_layer_in_one_step() {
    let (mut s, paint) = two_docs(40, 30, 8);
    let source_before = doc(&s, 0).clone();
    let r = s.execute("layer.copyToDocument", json!({"document": 1})).unwrap();
    assert_eq!(r["document"], 1);
    assert_eq!(s.active_index(), Some(1), "the destination becomes the active document");
    let d = doc(&s, 1);
    let id = copied(&r);
    assert_ne!(id, paint, "a copy has its own id");
    assert_eq!(d.layers.len(), 2);
    assert_eq!(d.layers[1].id, id, "above the Background, the destination's active layer");
    assert_eq!(d.layers[1].name, "paint");
    assert_eq!(s.active().unwrap().active_layer, Some(id));
    assert_eq!(bounds(&s, 1, id), Rect::new(4, 6, 14, 16), "same canvas position by default");
    assert_eq!(d.layers[1].surface().unwrap().read_region(Rect::new(5, 7, 6, 8)), vec![1.0, 0.0, 0.0, 1.0]);
    assert_eq!(*doc(&s, 0), source_before, "the source is untouched");
    // One history step in the destination.
    assert!(s.undo());
    assert_eq!(doc(&s, 1).layers.len(), 1);
}

#[test]
fn converts_to_the_destinations_depth_and_colour_mode() {
    for depth in [8, 16, 32] {
        let (mut s, _) = two_docs(40, 30, depth);
        s.set_active(1);
        s.execute("image.mode.grayscale", json!({})).unwrap();
        s.execute("image.mode.bits16", json!({})).unwrap();
        s.set_active(0);
        let r = s.execute("layer.copyToDocument", json!({"document": 1})).unwrap();
        let id = copied(&r);
        let surf = doc(&s, 1).layer(id).unwrap().surface().unwrap();
        assert_eq!(surf.format(), PixelFormat::new(ColorMode::Grayscale, SampleType::U16, true), "from {depth}-bit RGB");
        let px = surf.read_region(Rect::new(5, 7, 6, 8));
        assert!(px[0] > 0.1 && px[0] < 0.9 && px[1] == 1.0, "red as a mid grey, opaque: {px:?}");
    }
}

#[test]
fn places_the_copies_centred_on_the_canvas_on_a_point_or_offset() {
    let (mut s, paint) = two_docs(40, 30, 8);
    let place = |s: &mut Session, p: Value| {
        s.set_active(0);
        let mut p = p;
        p["document"] = json!(1);
        p["layers"] = json!([paint.0]);
        let r = s.execute("layer.copyToDocument", p).unwrap();
        bounds(s, 1, copied(&r))
    };
    assert_eq!(place(&mut s, json!({"center": true})), Rect::new(25, 20, 35, 30));
    assert_eq!(place(&mut s, json!({"at": [10.0, 40.0]})), Rect::new(5, 35, 15, 45));
    assert_eq!(place(&mut s, json!({"offset": [-4, 20]})), Rect::new(0, 26, 10, 36));
    assert_eq!(doc(&s, 1).layers.len(), 4);
}

#[test]
fn a_copied_background_is_an_ordinary_layer_and_several_layers_keep_their_order() {
    let (mut s, paint) = two_docs(40, 30, 8);
    let bg = doc(&s, 0).layers[0].id;
    let r = s.execute("layer.copyToDocument", json!({"document": 1, "layers": [paint.0, bg]})).unwrap();
    let d = doc(&s, 1);
    let names: Vec<&str> = d.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Background", "Layer 1", "paint"]);
    assert!(!d.layers[1].locks.transparency && !d.layers[1].locks.position);
    assert_eq!(r["layers"].as_array().unwrap().len(), 2);
    assert_eq!(s.active().unwrap().selected_layers().len(), 2, "the copies are selected");
}

#[test]
fn the_sources_patterns_come_along_once() {
    let (mut s, _) = two_docs(40, 30, 8);
    let mut tile = Surface::new(PixelFormat::new(ColorMode::Rgb, SampleType::U8, true));
    tile.fill_rect(Rect::new(0, 0, 2, 2), &[0.0, 1.0, 0.0, 1.0]);
    let pat = Pattern::new("dots", tile, 2, 2);
    s.edit("add pattern", |d, _| {
        d.patterns.push(pat.clone());
        Ok(())
    })
    .unwrap();
    s.execute("layer.copyToDocument", json!({"document": 1})).unwrap();
    assert_eq!(doc(&s, 1).patterns.iter().filter(|p| p.id == pat.id).count(), 1);
    s.set_active(0);
    s.execute("layer.copyToDocument", json!({"document": 1})).unwrap();
    assert_eq!(doc(&s, 1).patterns.iter().filter(|p| p.id == pat.id).count(), 1, "not twice");
}

#[test]
fn bad_params_and_states_fail_without_changing_anything() {
    let (mut s, _) = two_docs(40, 30, 8);
    let cmd = "layer.copyToDocument";
    for p in [
        json!({}),
        json!({"document": 0}),
        json!({"document": 7}),
        json!({"document": -1}),
        json!({"document": "1"}),
        json!({"document": 1, "source": 9}),
        json!({"document": 1, "layers": [999_999]}),
        json!({"document": 1, "layers": "all"}),
        json!({"document": 1, "layers": []}),
        json!({"document": 1, "at": [1.0]}),
        json!({"document": 1, "at": [f64::MAX, 0.0]}),
        json!({"document": 1, "offset": ["a", 2]}),
        Value::Null,
    ] {
        assert!(s.execute(cmd, p.clone()).is_err(), "{p}");
        assert_eq!(s.active_index(), Some(0), "{p}: the source stays active");
        assert_eq!(doc(&s, 1).layers.len(), 1, "{p}");
    }
    // An Indexed destination has no layers to add to.
    s.set_active(1);
    s.edit("indexed", |d, _| {
        d.mode = ColorMode::Indexed;
        Ok(())
    })
    .unwrap();
    s.set_active(0);
    assert!(s.execute(cmd, json!({"document": 1})).is_err());
    // A single open document: nothing to copy to.
    s.close(1);
    assert!(!s.is_enabled(cmd));
}

#[test]
fn cross_document_copy_remaps_link_groups_and_isolates_movement() {
    let mut s = Session::new();
    // Document 0 (source) with two linked layers
    s.execute("file.new", json!({"width": 20, "height": 20})).unwrap();
    let src_a = LayerId(s.execute("layer.new.layer", json!({"name": "Source A"})).unwrap()["layer"].as_u64().unwrap());
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 8, "height": 8})).unwrap();
    s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
    let src_b = LayerId(s.execute("layer.new.layer", json!({"name": "Source B"})).unwrap()["layer"].as_u64().unwrap());
    s.execute("select.rect", json!({"x": 10, "y": 10, "width": 8, "height": 8})).unwrap();
    s.execute("edit.fill", json!({"color": "#00ff00"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    s.execute("layer.select", json!({"layer": src_a.0})).unwrap();
    s.execute("layer.select", json!({"layer": src_b.0, "mode": "add"})).unwrap();
    s.execute("layer.linkLayers", json!({})).unwrap();

    // Document 1 (destination) with two linked layers
    s.execute("file.new", json!({"width": 20, "height": 20})).unwrap();
    let dst_a = LayerId(s.execute("layer.new.layer", json!({"name": "Destination A"})).unwrap()["layer"].as_u64().unwrap());
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 8, "height": 8})).unwrap();
    s.execute("edit.fill", json!({"color": "#0000ff"})).unwrap();
    let dst_b = LayerId(s.execute("layer.new.layer", json!({"name": "Destination B"})).unwrap()["layer"].as_u64().unwrap());
    s.execute("select.rect", json!({"x": 10, "y": 10, "width": 8, "height": 8})).unwrap();
    s.execute("edit.fill", json!({"color": "#ffff00"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    s.execute("layer.select", json!({"layer": dst_a.0})).unwrap();
    s.execute("layer.select", json!({"layer": dst_b.0, "mode": "add"})).unwrap();
    s.execute("layer.linkLayers", json!({})).unwrap();

    let dst_group = doc(&s, 1).layer(dst_a).unwrap().link_group;
    assert!(dst_group.is_some());
    assert_eq!(doc(&s, 1).layer(dst_b).unwrap().link_group, dst_group);

    // Copy source linked pair into destination document
    s.set_active(0);
    s.execute("layer.select", json!({"layer": src_a.0})).unwrap();
    s.execute("layer.select", json!({"layer": src_b.0, "mode": "add"})).unwrap();
    let r = s.execute("layer.copyToDocument", json!({"document": 1})).unwrap();
    let copied_ids: Vec<LayerId> = r["layers"].as_array().unwrap().iter().map(|v| LayerId(v.as_u64().unwrap())).collect();
    assert_eq!(copied_ids.len(), 2);

    let copy_a = copied_ids[0];
    let copy_b = copied_ids[1];
    let copy_group = doc(&s, 1).layer(copy_a).unwrap().link_group;
    assert!(copy_group.is_some());
    assert_ne!(copy_group, dst_group, "copied pair gets a distinct link group");
    assert_eq!(doc(&s, 1).layer(copy_b).unwrap().link_group, copy_group);

    // Move Destination A: Destination B moves with it, copied pair stays in place
    s.execute("layer.select", json!({"layer": dst_a.0})).unwrap();
    s.execute("layer.translate", json!({"dx": 1, "dy": 0})).unwrap();
    assert_eq!(bounds(&s, 1, dst_a), Rect::new(1, 0, 9, 8));
    assert_eq!(bounds(&s, 1, dst_b), Rect::new(11, 10, 19, 18));
    assert_eq!(bounds(&s, 1, copy_a), Rect::new(0, 0, 8, 8));
    assert_eq!(bounds(&s, 1, copy_b), Rect::new(10, 10, 18, 18));

    // Move Copy A: Copy B moves with it, destination pair stays in place
    s.execute("layer.select", json!({"layer": copy_a.0})).unwrap();
    s.execute("layer.translate", json!({"dx": 0, "dy": 2})).unwrap();
    assert_eq!(bounds(&s, 1, dst_a), Rect::new(1, 0, 9, 8));
    assert_eq!(bounds(&s, 1, dst_b), Rect::new(11, 10, 19, 18));
    assert_eq!(bounds(&s, 1, copy_a), Rect::new(0, 2, 8, 10));
    assert_eq!(bounds(&s, 1, copy_b), Rect::new(10, 12, 18, 20));

    // Undo restores moved positions
    s.undo();
    assert_eq!(bounds(&s, 1, copy_a), Rect::new(0, 0, 8, 8));
    assert_eq!(bounds(&s, 1, copy_b), Rect::new(10, 10, 18, 18));
    s.undo();
    assert_eq!(bounds(&s, 1, dst_a), Rect::new(0, 0, 8, 8));
    assert_eq!(bounds(&s, 1, dst_b), Rect::new(10, 10, 18, 18));
}

#[test]
fn single_linked_layer_copied_without_partner_becomes_unlinked() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 20, "height": 20})).unwrap();
    let src_a = LayerId(s.execute("layer.new.layer", json!({"name": "Source A"})).unwrap()["layer"].as_u64().unwrap());
    let src_b = LayerId(s.execute("layer.new.layer", json!({"name": "Source B"})).unwrap()["layer"].as_u64().unwrap());
    s.execute("layer.select", json!({"layer": src_a.0})).unwrap();
    s.execute("layer.select", json!({"layer": src_b.0, "mode": "add"})).unwrap();
    s.execute("layer.linkLayers", json!({})).unwrap();

    s.execute("file.new", json!({"width": 20, "height": 20})).unwrap();
    s.set_active(0);
    s.execute("layer.select", json!({"layer": src_a.0})).unwrap();
    let r = s.execute("layer.copyToDocument", json!({"document": 1})).unwrap();
    let copy_id = copied(&r);
    assert_eq!(doc(&s, 1).layer(copy_id).unwrap().link_group, None, "lone copied layer is unlinked");
}
