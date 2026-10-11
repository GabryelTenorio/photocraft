//! #2561: the Layers panel New Group button. Like Photoshop CC 2014 and later, a click with two or
//! more layers selected puts them in the new group, and with one layer it adds an empty group.
//! Shift-click (the older way) groups the selection too.

use egui::Modifiers;
use egui_kittest::kittest::Queryable;
use photocraft_doc::{Layer, LayerId};
use serde_json::json;

use crate::PhotocraftApp;

/// Background, then `A` and `B` on top of it; `A` and `B` are selected when `both`, otherwise
/// only `B` is (`B` active either way).
fn selected(both: bool) -> (egui_kittest::Harness<'static, PhotocraftApp>, LayerId, LayerId) {
    let mut s = photocraft_engine::Session::new();
    s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
    let a = LayerId(s.execute("layer.new.layer", json!({"name": "A"})).unwrap()["layer"].as_u64().unwrap());
    let b = LayerId(s.execute("layer.new.layer", json!({"name": "B"})).unwrap()["layer"].as_u64().unwrap());
    if both {
        s.execute("layer.select", json!({"layer": a.0})).unwrap();
        s.execute("layer.select", json!({"layer": b.0, "mode": "add"})).unwrap();
    }
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).with_max_steps(64).build_eframe(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        PhotocraftApp::new(s, crate::Services::default())
    });
    h.run_steps(4);
    (h, a, b)
}

fn groups(h: &egui_kittest::Harness<'static, PhotocraftApp>) -> Vec<Layer> {
    h.state().session.active().unwrap().doc.walk().into_iter().map(|(_, _, l)| l).filter(|l| l.is_group()).cloned().collect()
}

/// The one group holds `[a, b]` in order, sits next to the background and is the only selected layer.
fn assert_grouped(h: &egui_kittest::Harness<'static, PhotocraftApp>, a: LayerId, b: LayerId) {
    let groups = groups(h);
    assert_eq!(groups.len(), 1);
    let children: Vec<LayerId> = groups[0].children().unwrap().iter().map(|l| l.id).collect();
    assert_eq!(children, vec![a, b]);
    let st = h.state().session.active().unwrap();
    assert_eq!(st.doc.layers.len(), 2, "background and the new group stay at the top level");
    assert_eq!(st.selected_layers(), vec![groups[0].id], "the new group is selected");
}

#[test]
fn plain_click_groups_the_selected_layers_in_order() {
    let (mut h, a, b) = selected(true);
    h.get_by_label("Create a new group").click();
    h.run_steps(4);
    assert_grouped(&h, a, b);
    // One step: a single undo puts both layers back where they were.
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert!(groups(&h).is_empty());
    let top: Vec<LayerId> = h.state().session.active().unwrap().doc.layers.iter().map(|l| l.id).collect();
    assert_eq!(top[1..], [a, b]);
}

#[test]
fn shift_click_groups_the_selected_layers_in_order() {
    let (mut h, a, b) = selected(true);
    h.get_by_label("Create a new group").click_modifiers(Modifiers::SHIFT);
    h.run_steps(4);
    assert_grouped(&h, a, b);
}

#[test]
fn plain_click_with_one_selected_layer_adds_an_empty_group() {
    let (mut h, a, b) = selected(false);
    h.get_by_label("Create a new group").click();
    h.run_steps(4);
    let groups = groups(&h);
    assert_eq!(groups.len(), 1);
    assert!(groups[0].children().unwrap().is_empty());
    let top: Vec<LayerId> = h.state().session.active().unwrap().doc.layers.iter().map(|l| l.id).collect();
    assert_eq!(top[1..], [a, b, groups[0].id], "the layers stay where they were; the group goes above the active one");
}
