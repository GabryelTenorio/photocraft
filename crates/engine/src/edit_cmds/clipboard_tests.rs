use super::*;
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::LayerMask;
use std::sync::Arc;

fn masked(depth: u32) -> (Session, LayerId) {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48, "depth": depth})).unwrap();
    s.execute("layer.new.layer", json!({"name": "Masked colour"})).unwrap();
    let id = s.active().unwrap().active_layer.unwrap();
    s.edit("fixture", |doc, _| {
        let l = doc.layer_mut(id).unwrap();
        l.surface_mut().unwrap().fill_rect(Rect::new(10, 8, 30, 24), &[0.8, 0.2, 0.4, 1.0]);
        let mut mask = Surface::with_default(PixelFormat::new(ColorMode::Grayscale, doc_depth(depth), false), &[1.0]);
        mask.fill_rect(Rect::new(6, 4, 20, 28), &[0.0]);
        mask.fill_rect(Rect::new(20, 4, 24, 28), &[0.5]);
        l.mask = Some(LayerMask { surface: mask, enabled: true, linked: false, density: 0.75, feather: 1.25 });
        Ok(())
    })
    .unwrap();
    (s, id)
}

fn doc_depth(depth: u32) -> SampleType {
    match depth {
        16 => SampleType::U16,
        32 => SampleType::F32,
        _ => SampleType::U8,
    }
}

fn layer(s: &Session, id: LayerId) -> &Layer {
    s.active().unwrap().doc.layer(id).unwrap()
}

fn pasted(s: &mut Session, params: Value) -> LayerId {
    LayerId(s.execute("edit.paste", params).unwrap()["layer"].as_u64().unwrap())
}

#[test]
fn copy_then_paste_keeps_an_editable_nonuniform_mask() {
    for depth in [8, 16, 32] {
        let (mut s, src) = masked(depth);
        let original = layer(&s, src).clone();
        s.execute("edit.copy", json!({})).unwrap();
        let id = pasted(&mut s, json!({}));
        assert!(layer(&s, id).mask == original.mask, "{depth}-bit editable mask and metadata");
        assert!(layer(&s, id).surface() == original.surface(), "independent colour data");
        for (a, b) in original.surface().unwrap().tiles().zip(layer(&s, id).surface().unwrap().tiles()) {
            assert_eq!(a.0, b.0);
            assert!(Arc::ptr_eq(a.1, b.1), "matching colour tiles remain COW-shared");
        }
        for (a, b) in original.mask.as_ref().unwrap().surface.tiles().zip(layer(&s, id).mask.as_ref().unwrap().surface.tiles()) {
            assert_eq!(a.0, b.0);
            assert!(Arc::ptr_eq(a.1, b.1), "matching mask tiles remain COW-shared");
        }
        assert_ne!(id, src);
        let area = Rect::new(10, 8, 30, 24);
        let a = photocraft_compose::render_layer(&original, area);
        let b = photocraft_compose::render_layer(layer(&s, id), area);
        assert_eq!(a.px, b.px, "the composited appearance matches");
        let steps = s.active().unwrap().history.past_len();
        s.undo();
        assert!(s.active().unwrap().doc.layer(id).is_none());
        s.redo();
        assert!(layer(&s, id).mask == original.mask);
        assert_eq!(s.active().unwrap().history.past_len(), steps);
        s.edit("edit pasted mask and colour", |doc, _| {
            let l = doc.layer_mut(id).unwrap();
            l.mask.as_mut().unwrap().surface.write_pixel(12, 12, &[1.0]);
            l.surface_mut().unwrap().write_pixel(25, 12, &[0.0, 1.0, 0.0, 1.0]);
            Ok(())
        })
        .unwrap();
        assert!(layer(&s, src).mask == original.mask, "editing the copy leaves the source mask alone");
        assert!(layer(&s, src).surface() == original.surface());
        let next = pasted(&mut s, json!({}));
        assert_ne!(next, id);
        assert!(layer(&s, next).mask == original.mask, "the clipboard is an independent snapshot");
        assert!(layer(&s, next).surface() == original.surface());
    }
}

#[test]
fn source_edits_after_copy_do_not_change_the_clipboard() {
    let (mut s, src) = masked(32);
    let original = layer(&s, src).clone();
    s.execute("edit.copy", json!({})).unwrap();
    s.edit("edit source", |doc, _| {
        let l = doc.layer_mut(src).unwrap();
        l.mask.as_mut().unwrap().surface.write_pixel(12, 12, &[1.0]);
        l.surface_mut().unwrap().write_pixel(25, 12, &[0.0, 1.0, 0.0, 1.0]);
        Ok(())
    })
    .unwrap();
    let id = pasted(&mut s, json!({}));
    assert!(layer(&s, id).mask == original.mask);
    assert!(layer(&s, id).surface() == original.surface());
}

#[test]
fn disabled_masks_inverted_samples_and_link_flags_survive_whole_layer_paste() {
    for enabled in [true, false] {
        for linked in [true, false] {
            let (mut s, src) = masked(32);
            s.edit("inverted fixture", |doc, _| {
                let m = doc.layer_mut(src).unwrap().mask.as_mut().unwrap();
                m.enabled = enabled;
                m.linked = linked;
                m.surface = Surface::with_default(m.surface.format(), &[0.0]);
                m.surface.fill_rect(Rect::new(10, 8, 20, 24), &[1.0]);
                Ok(())
            })
            .unwrap();
            let original = layer(&s, src).mask.as_ref().unwrap().clone();
            s.execute("edit.copy", json!({})).unwrap();
            let id = pasted(&mut s, json!({"center": [40, 36]}));
            let m = layer(&s, id).mask.as_ref().unwrap();
            assert_eq!((m.enabled, m.linked, m.density, m.feather), (original.enabled, original.linked, original.density, original.feather));
            assert_eq!(m.surface.sample_channel(32, 32, 0), 1.0);
            assert_eq!(m.surface.sample_channel(45, 32, 0), 0.0);
            assert_eq!(m.surface.default_pixel(), vec![0.0]);
        }
    }
}

#[test]
fn cross_document_paste_moves_both_masks_even_when_unlinked_and_converts_colour() {
    use photocraft_doc::vector::{Path, Subpath, VectorMask};
    for source_depth in [8, 16, 32] {
        for (mode, dest_depth) in [("grayscale", 16), ("cmyk", 32), ("lab", 8), ("rgb", 32)] {
            let (mut s, src) = masked(source_depth);
            s.edit("vector fixture", |doc, _| {
                doc.layer_mut(src).unwrap().vector_mask = Some(VectorMask {
                    path: Path::new(vec![Subpath::polygon(&[(11.0, 9.0), (29.0, 9.0), (29.0, 23.0), (11.0, 23.0)])]),
                    enabled: false,
                    linked: false,
                    density: 0.6,
                    feather: 2.0,
                });
                Ok(())
            })
            .unwrap();
            let original = layer(&s, src).clone();
            s.execute("edit.copy", json!({})).unwrap();
            s.execute("file.new", json!({"width": 100, "height": 80, "depth": dest_depth})).unwrap();
            if mode != "rgb" {
                s.execute(&format!("image.mode.{mode}"), json!({})).unwrap();
            }
            let id = pasted(&mut s, json!({"center": [50, 40]}));
            // Source colour extent is (10,8)-(30,24), so centre (20,16) moves by (30,24).
            let l = layer(&s, id);
            assert_eq!(l.surface().unwrap().content_bounds(), Rect::new(40, 32, 60, 48));
            let m = l.mask.as_ref().unwrap();
            let old = original.mask.as_ref().unwrap();
            assert_eq!((m.enabled, m.linked, m.density, m.feather), (old.enabled, old.linked, old.density, old.feather));
            assert!((m.surface.sample_channel(51, 36, 0) - old.surface.sample_channel(21, 12, 0)).abs() < 0.005);
            assert_eq!(m.surface.sample_channel(42, 36, 0), 0.0);
            assert_eq!(m.surface.default_pixel(), vec![1.0]);
            assert_eq!(m.surface.format().sample, doc_depth(dest_depth));
            let vm = l.vector_mask.as_ref().unwrap();
            let old = original.vector_mask.as_ref().unwrap();
            assert_eq!(vm.path, old.path.transform(&photocraft_geom::Affine::translate(30.0, 24.0)));
            assert_eq!((vm.enabled, vm.linked, vm.density, vm.feather), (old.enabled, old.linked, old.density, old.feather));
            assert_eq!(l.surface().unwrap().format().sample, doc_depth(dest_depth));
            let pixel = l.surface().unwrap().pixel(55, 36);
            assert_eq!(pixel.last().copied(), Some(1.0));
            // Validate the converted colour against the CMS transform, independent of paste.
            let source_doc = &s.documents()[0].doc;
            let dest_doc = &s.active().unwrap().doc;
            let t = photocraft_cms::Transform::new(
                &crate::color_cmds::document_profile(source_doc),
                &crate::color_cmds::document_profile(dest_doc),
                s.color.settings.intent(),
                s.color.settings.bpc,
            )
            .unwrap();
            let source_pixel = original.surface().unwrap().pixel(25, 12);
            let mut expected = vec![0.0; dest_doc.mode.color_channels()];
            t.eval(&source_pixel[..3], &mut expected);
            for (actual, expected) in pixel.iter().zip(expected) {
                assert!((actual - expected).abs() < 0.015, "{source_depth} -> {mode}/{dest_depth}: {pixel:?}");
            }
            assert!(s.documents()[0].doc.layer(src).unwrap().mask == original.mask);
            s.undo();
            assert!(s.active().unwrap().doc.layer(id).is_none());
            s.redo();
            assert_eq!(layer(&s, id).mask.as_ref().unwrap().surface.sample_channel(42, 36, 0), 0.0);
        }
    }
}

#[test]
fn same_mode_profile_conversion_changes_colour_but_keeps_mask_coverage() {
    use photocraft_cms::{Builtin, Transform};
    let (mut s, src) = masked(16);
    s.edit("source profile", |doc, _| {
        doc.icc_profile = Some(Builtin::AdobeRgbCompat.profile().to_bytes());
        Ok(())
    })
    .unwrap();
    let original = layer(&s, src).clone();
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("file.new", json!({"width": 64, "height": 48, "depth": 16})).unwrap();
    s.edit("destination profile", |doc, _| {
        doc.icc_profile = Some(Builtin::DisplayP3.profile().to_bytes());
        Ok(())
    })
    .unwrap();
    let id = pasted(&mut s, json!({}));
    let t = Transform::new(Builtin::AdobeRgbCompat.profile(), Builtin::DisplayP3.profile(), s.color.settings.intent(), s.color.settings.bpc).unwrap();
    let mut expected = [0.0; 3];
    t.eval(&original.surface().unwrap().pixel(25, 12)[..3], &mut expected);
    let actual = layer(&s, id).surface().unwrap().pixel(25, 12);
    for (a, b) in actual.iter().zip(expected) {
        assert!((a - b).abs() < 0.001);
    }
    assert!(layer(&s, id).mask == original.mask);
    assert_ne!(actual, original.surface().unwrap().pixel(25, 12));
}

#[test]
fn selection_copy_copy_merged_and_cut_keep_bitmap_semantics() {
    let (mut s, src) = masked(16);
    s.execute("select.rect", json!({"x": 10, "y": 8, "width": 6, "height": 8})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    assert!(s.clipboard.as_ref().unwrap().layers.is_none());
    let id = pasted(&mut s, json!({}));
    assert!(layer(&s, id).mask.is_none());
    assert_eq!(layer(&s, id).surface().unwrap().content_bounds(), Rect::new(10, 8, 16, 16));
    assert_eq!(layer(&s, id).surface().unwrap().pixel(12, 12), layer(&s, src).surface().unwrap().pixel(12, 12));
    s.select_layer(src).unwrap();
    s.execute("select.rect", json!({"x": 10, "y": 8, "width": 6, "height": 8})).unwrap();
    s.execute("edit.copyMerged", json!({})).unwrap();
    assert!(s.clipboard.as_ref().unwrap().layers.is_none());
    s.execute("edit.cut", json!({})).unwrap();
    assert!(s.clipboard.as_ref().unwrap().layers.is_none());
    s.undo();
    s.execute("select.deselect", json!({})).unwrap();
    s.execute("edit.cut", json!({})).unwrap();
    assert!(s.clipboard.as_ref().unwrap().layers.is_none(), "Cut still takes pixels without a selection");
}

#[test]
fn vector_only_and_fully_hidden_masks_stay_editable() {
    use photocraft_doc::vector::{Path, Subpath, VectorMask};
    for vector_only in [false, true] {
        let (mut s, src) = masked(8);
        s.edit("hidden fixture", |doc, _| {
            let l = doc.layer_mut(src).unwrap();
            if vector_only {
                l.mask = None;
                l.vector_mask = Some(VectorMask::new(Path::new(vec![Subpath::polygon(&[(100.0, 100.0), (110.0, 100.0), (110.0, 110.0), (100.0, 110.0)])])));
            } else {
                l.mask = Some(LayerMask { surface: Surface::with_default(PixelFormat::GRAY8, &[0.0]), ..LayerMask::reveal_all() });
            }
            Ok(())
        })
        .unwrap();
        let original = layer(&s, src).clone();
        s.execute("edit.copy", json!({})).unwrap();
        let clip = s.clipboard.as_ref().unwrap();
        assert_eq!(clip.bounds, Rect::new(10, 8, 30, 24), "hidden colour still determines layer placement");
        assert_eq!(clip.surface.rgba(25, 12)[3], 0.0, "external bitmap respects the mask");
        s.execute("edit.pasteSpecial.pasteInPlace", json!({})).unwrap();
        let id = s.active().unwrap().active_layer.unwrap();
        assert_ne!(id, src);
        assert!(layer(&s, id).mask == original.mask);
        assert_eq!(layer(&s, id).vector_mask, original.vector_mask);
        assert!(layer(&s, id).surface() == original.surface(), "hidden colour is preserved");
    }
}

#[test]
fn multiple_selected_layers_keep_order_masks_and_one_history_step() {
    let (mut s, first) = masked(8);
    s.execute("layer.new.layer", json!({"name": "Second"})).unwrap();
    let second = s.active().unwrap().active_layer.unwrap();
    s.edit("second pixels", |doc, _| {
        doc.layer_mut(second).unwrap().surface_mut().unwrap().fill_rect(Rect::new(32, 8, 40, 24), &[0.0, 1.0, 0.0, 1.0]);
        Ok(())
    })
    .unwrap();
    crate::layer_multi_cmds::set_selection(&mut s, vec![second, first], Some(second), None).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    let copied = &s.clipboard.as_ref().unwrap().layers.as_ref().unwrap().layers;
    assert_eq!(copied.iter().map(|l| l.id).collect::<Vec<_>>(), vec![first, second], "stack order, not click order");
    let steps = s.active().unwrap().history.past_len();
    s.execute("edit.pasteSpecial.pasteInPlace", json!({})).unwrap();
    let doc = &s.active().unwrap().doc;
    let ids: Vec<_> = doc.layers.iter().filter(|l| l.id != first && l.id != second && l.name != "Background").map(|l| l.id).collect();
    assert_eq!(ids.len(), 2);
    assert!(doc.layer(ids[0]).unwrap().mask == doc.layer(first).unwrap().mask);
    assert_eq!(doc.layer(ids[1]).unwrap().name, "Second");
    assert_eq!(s.active().unwrap().history.past_len(), steps + 1);
    s.undo();
    assert!(ids.iter().all(|id| s.active().unwrap().doc.layer(*id).is_none()));
    s.redo();
    assert!(ids.iter().all(|id| s.active().unwrap().doc.layer(*id).is_some()));
}

#[test]
fn selected_group_and_child_are_copied_only_once() {
    let (mut s, child) = masked(8);
    s.execute("layer.groupLayers", json!({"name": "Masked group"})).unwrap();
    let group = s.active().unwrap().active_layer.unwrap();
    crate::layer_multi_cmds::set_selection(&mut s, vec![group, child], Some(child), None).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    assert_eq!(s.clipboard.as_ref().unwrap().layers.as_ref().unwrap().layers.len(), 1);
    let id = pasted(&mut s, json!({}));
    let children = layer(&s, id).children().unwrap();
    assert_ne!(id, group);
    assert_eq!(children.len(), 1);
    assert_ne!(children[0].id, child);
    assert!(children[0].mask == layer(&s, child).mask);
}

#[test]
fn paste_into_and_outside_use_the_bitmap_of_a_whole_masked_layer() {
    let (mut s, _) = masked(8);
    s.execute("edit.copy", json!({})).unwrap();
    let clipboard = s.clipboard.as_ref().unwrap().clone();
    assert!(clipboard.layers.is_some());
    for command in ["edit.pasteSpecial.pasteInto", "edit.pasteSpecial.pasteOutside"] {
        s.execute("select.rect", json!({"x": 24, "y": 8, "width": 6, "height": 16})).unwrap();
        s.execute(command, json!({"center": [20, 16]})).unwrap();
        let id = s.active().unwrap().active_layer.unwrap();
        assert!(layer(&s, id).vector_mask.is_none());
        assert!(layer(&s, id).surface() == Some(&clipboard.surface), "Paste Into/Outside use the flattened bitmap");
        assert!(layer(&s, id).mask.is_some(), "destination selection supplies the new mask");
        s.undo();
    }
}
