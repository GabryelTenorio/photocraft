use photocraft_doc::{LayerContent, LayerId, PathOp};
use serde_json::json;

use crate::{Session, layer_multi_cmds};

fn fixture() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":64,"height":64})).unwrap();
    s
}

fn rect(s: &mut Session, x: i32) -> LayerId {
    LayerId(s.execute("shape.create", json!({"rect":[x,0,12,12]})).unwrap()["layer"].as_u64().unwrap())
}

#[test]
fn mixed_selection_leaves_locked_raster_untouched_and_supports_undo_redo() {
    let mut s = fixture();
    let a = rect(&mut s, 0);
    let raster = LayerId(s.execute("layer.new.layer", json!({})).unwrap()["layer"].as_u64().unwrap());
    s.execute("layer.lockLayers", json!({"layer":raster.0,"all":true})).unwrap();
    assert!(s.active().unwrap().doc.effective_locks(raster).all);
    let b = rect(&mut s, 6);
    layer_multi_cmds::set_selection(&mut s, vec![a, raster, b], Some(raster), None).unwrap();
    let before = s.active().unwrap().doc.clone();
    let history = s.active().unwrap().history.past_len();

    assert_eq!(s.execute("layer.combineShapes.unite", json!({})).unwrap()["layer"], b.0);
    let after = s.active().unwrap().doc.clone();
    assert_eq!(after.layer(raster), before.layer(raster));
    assert!(after.layer(a).is_none());
    assert_eq!(s.active().unwrap().history.past_len(), history + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc, before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc, after);
}

#[test]
fn selected_group_protects_selected_shape_children_before_shape_filtering() {
    let mut s = fixture();
    let background = s.active().unwrap().active_layer.unwrap();
    let child = rect(&mut s, 20);
    let group = LayerId(s.execute("layer.groupLayers", json!({})).unwrap()["layer"].as_u64().unwrap());
    s.select_layer(background).unwrap();
    let a = rect(&mut s, 0);
    let b = rect(&mut s, 6);
    layer_multi_cmds::set_selection(&mut s, vec![group, child, a, b], Some(b), None).unwrap();
    assert!(s.active().unwrap().selected_layers().contains(&child));
    let before = s.active().unwrap().doc.clone();

    assert_eq!(s.execute("layer.combineShapes.unite", json!({})).unwrap()["layer"], b.0);
    let after = &s.active().unwrap().doc;
    assert_eq!(after.layer(group), before.layer(group));
    assert_eq!(after.layer(child), before.layer(child));
    assert!(after.layer(a).is_none());
    assert!(after.layer(b).is_some());
}

#[test]
fn mixed_selection_with_one_shape_keeps_single_layer_component_editing() {
    let mut s = fixture();
    let raster = s.active().unwrap().active_layer.unwrap();
    let shape = rect(&mut s, 0);
    s.execute("shape.create", json!({"addTo":shape.0,"rect":[6,0,12,12]})).unwrap();
    layer_multi_cmds::set_selection(&mut s, vec![raster, shape], Some(shape), None).unwrap();
    let before = s.active().unwrap().doc.clone();

    assert_eq!(s.execute("layer.combineShapes.subtractFrontShape", json!({})).unwrap()["layer"], shape.0);
    let after = &s.active().unwrap().doc;
    assert_eq!(after.layer(raster), before.layer(raster));
    assert_eq!(after.walk().len(), before.walk().len());
    let LayerContent::Shape(edited) = &after.layer(shape).unwrap().content else { panic!("shape") };
    assert_eq!(edited.path.subpaths[1].op, PathOp::Subtract);
}

#[test]
fn combining_invalidates_original_psd_geometry_but_undo_restores_it() {
    use std::sync::Arc;

    let mut s = fixture();
    let a = rect(&mut s, 0);
    let b = rect(&mut s, 6);
    let st = s.active_mut().unwrap();
    let LayerContent::Shape(front) = &mut Arc::make_mut(&mut st.doc).layer_mut(b).unwrap().content else { panic!("shape") };
    let raw = Arc::new(photocraft_io::vector_map::vmsk_bytes(&front.path, 0, 64, 64));
    assert_eq!(photocraft_io::vector_map::path_from_vmsk(&raw, 64, 64).unwrap().0, front.path);
    front.psd_raw = Some(raw.clone());
    let original = front.clone();
    layer_multi_cmds::set_selection(&mut s, vec![a, b], Some(b), None).unwrap();
    let before = s.active().unwrap().doc.clone();

    s.execute("layer.combineShapes.unite", json!({})).unwrap();
    let after = s.active().unwrap().doc.clone();
    let LayerContent::Shape(merged) = &after.layer(b).unwrap().content else { panic!("shape") };
    assert_ne!(merged.path, original.path);
    assert!(merged.psd_raw.is_none());
    assert!(merged.live.is_none());
    assert_eq!(merged.fill, original.fill);
    assert_eq!(merged.stroke, original.stroke);
    assert_eq!(after.layer(b).unwrap().name, before.layer(b).unwrap().name);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc, before);
    let LayerContent::Shape(restored) = &s.active().unwrap().doc.layer(b).unwrap().content else { panic!("shape") };
    assert_eq!(restored.psd_raw.as_ref(), Some(&raw));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.active().unwrap().doc, after);
}
