use super::*;
use photocraft_doc::{LayerId, Locks, Subpath};
use photocraft_geom::Rect;

const RECT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"><rect id="red" width="8" height="6" fill="#ff0000"/></svg>"##;
const GROUP: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="6"><g id="pair" opacity="0.5"><rect id="red" width="6" height="6" fill="red"/><rect id="blue" x="6" width="6" height="6" fill="blue"/></g></svg>"##;
const FILTER: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"><defs><filter id="blur"><feGaussianBlur stdDeviation="1"/></filter></defs><rect width="8" height="6" fill="red" filter="url(#blur)"/></svg>"##;

#[test]
fn pasted_shape_preserves_svg_paints_and_uses_shared_shape_parity_controls() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"><rect x="1" y="1" width="6" height="4" fill="#ff0000" stroke="#0000ff" stroke-width="1"/></svg>"##;
    for depth in [8, 16, 32] {
        let mut s = session(depth);
        s.tools.foreground = [0.0, 1.0, 0.0, 1.0];
        let result = s.execute("edit.pasteVector", json!({"svg":svg,"mode":"shape"})).unwrap();
        let layer = inserted(&s, &result);
        let LayerContent::Shape(imported) = &layer.content else { panic!("native shape") };
        let before = s.active().unwrap().doc.clone();
        let original_path = imported.path.clone();
        let info = s.execute("shape.info", json!({"layer":layer.id.0})).unwrap();
        let photocraft_doc::Fill::Solid(fill) = imported.fill.as_ref().unwrap() else { panic!("solid SVG fill") };
        let photocraft_doc::Fill::Solid(stroke) = &imported.stroke.as_ref().unwrap().paint else { panic!("solid SVG stroke") };
        for (actual, expected) in fill.to_rgb().into_iter().zip([1.0, 0.0, 0.0]) {
            assert!((actual - expected).abs() < 1e-5);
        }
        for (actual, expected) in stroke.to_rgb().into_iter().zip([0.0, 0.0, 1.0]) {
            assert!((actual - expected).abs() < 1e-5);
        }
        s.execute("shape.edit", json!({"layer":layer.id.0,"stroke":{"dashes":[3,1],"cap":"round","join":"bevel","align":"inside"}})).unwrap();
        let edited = s.execute("shape.info", json!({"layer":layer.id.0})).unwrap();
        assert_eq!(edited["fill"], info["fill"]);
        assert_eq!(edited["stroke"]["fill"], info["stroke"]["fill"]);
        assert_eq!(edited["stroke"]["dashes"], json!([3.0, 1.0]));
        assert_eq!(edited["stroke"]["cap"], "round");
        assert_eq!(edited["stroke"]["join"], "bevel");
        assert_eq!(edited["stroke"]["align"], "inside");
        let LayerContent::Shape(shape) = &s.active().unwrap().doc.layer(layer.id).unwrap().content else { panic!("native shape") };
        assert_eq!(shape.path, original_path);
        assert_eq!(shape.cache.as_ref().unwrap().format(), s.active().unwrap().doc.pixel_format());
        assert!(s.undo());
        assert_eq!(s.active().unwrap().doc, before);
        s.execute("shape.edit", json!({"layer":layer.id.0,"transform":[2,0,0,3,0,0]})).unwrap();
        let LayerContent::Shape(shape) = &s.active().unwrap().doc.layer(layer.id).unwrap().content else { panic!("native shape") };
        let (x0, y0, x1, y1) = original_path.control_bounds().unwrap();
        assert_eq!(shape.path.control_bounds(), Some((x0 * 2.0, y0 * 3.0, x1 * 2.0, y1 * 3.0)));
        assert_eq!(shape.fill, imported.fill);
        assert_eq!(shape.stroke.as_ref().unwrap().paint, imported.stroke.as_ref().unwrap().paint);
    }
}

#[test]
fn zero_offset_refreshes_shape_caches_on_the_destination_canvas() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="24" height="24" fill="red"/></svg>"#;
    let prepared = prepare_svg(svg.as_bytes()).unwrap();
    for mode in [PasteMode::Layers, PasteMode::Shape] {
        let mut s = session(16);
        let result = paste_svg(&mut s, &prepared, mode, &json!({"center":[0.5,0.5]})).unwrap();
        let layer = inserted(&s, &result);
        assert_eq!(layer.surface().unwrap().pixel(20, 20), vec![1.0, 0.0, 0.0, 1.0]);
        assert_eq!(layer.surface().unwrap().format(), s.active().unwrap().doc.pixel_format());
    }
}

#[test]
fn scaled_svg_smart_object_cannot_unpack_excessive_destination_caches() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="300000" height="300000"/></svg>"#;
    let mut s = Session::new();
    s.execute("file.new", json!({"width":12000,"height":12000})).unwrap();
    s.execute("edit.pasteVector", json!({"svg":svg,"mode":"smartObject","center":[0.5,0.5]})).unwrap();
    s.execute("edit.transform", json!({"matrix":[2,0,0,2,0,0]})).unwrap();
    let before = s.active().unwrap().doc.clone();
    let revision = s.active().unwrap().revision;
    assert!(s.execute("layer.smartObjects.convertToLayers", json!({})).unwrap_err().to_string().contains("cache budget"));
    assert_eq!(s.active().unwrap().doc, before);
    assert_eq!(s.active().unwrap().revision, revision);
}

fn session(depth: u32) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":32,"height":24,"depth":depth})).unwrap();
    s
}

fn inserted(s: &Session, result: &Value) -> Layer {
    let id = LayerId(result["layers"][0].as_u64().unwrap());
    s.active().unwrap().doc.layer(id).unwrap().clone()
}

#[test]
fn each_mode_has_distinct_content_and_one_undo_step() {
    let prepared = prepare_svg(RECT.as_bytes()).unwrap();
    assert_eq!(prepared.size(), [8, 6]);
    for mode in [PasteMode::Layers, PasteMode::SmartObject, PasteMode::Pixels, PasteMode::Path, PasteMode::Shape] {
        let mut s = session(8);
        let before = s.active().unwrap().doc.clone();
        let result = paste_svg(&mut s, &prepared, mode, &json!({})).unwrap();
        assert_eq!(result["mode"], mode.as_str());
        assert_eq!(result["offset"], json!([12, 9]));
        let doc = &s.active().unwrap().doc;
        match mode {
            PasteMode::Path => {
                assert_eq!(doc.layers, before.layers);
                assert_eq!(doc.paths.len(), 1);
                assert_eq!(doc.paths[0].path.control_bounds(), Some((12.0, 9.0, 20.0, 15.0)));
            }
            _ => {
                let layer = inserted(&s, &result);
                match (&layer.content, mode) {
                    (LayerContent::Shape(_), PasteMode::Layers | PasteMode::Shape) => {}
                    (LayerContent::Raster(_), PasteMode::Pixels) => {}
                    (LayerContent::Smart(sm), PasteMode::SmartObject) => {
                        assert_eq!(sm.transform, Affine::translate(12.0, 9.0));
                        assert!(
                            matches!(&sm.source, SmartSource::Embedded {file_name,bytes} if file_name == "Clipboard.svg" && bytes.as_slice() == RECT.as_bytes())
                        );
                    }
                    _ => panic!("wrong content for {mode:?}"),
                }
                let pixel = layer.surface().unwrap().read_region(Rect::new(15, 11, 16, 12));
                assert_eq!(pixel, vec![1.0, 0.0, 0.0, 1.0]);
            }
        }
        assert!(s.undo());
        assert_eq!(*s.active().unwrap().doc, *before);
        assert!(s.redo());
        assert_eq!(s.active().unwrap().doc.paths.len(), usize::from(mode == PasteMode::Path));
    }
}

#[test]
fn repeated_import_preserves_groups_order_opacity_and_assigns_fresh_ids() {
    let prepared = prepare_svg(GROUP.as_bytes()).unwrap();
    let mut s = session(8);
    let a = paste_svg(&mut s, &prepared, PasteMode::Layers, &json!({})).unwrap();
    let b = paste_svg(&mut s, &prepared, PasteMode::Layers, &json!({})).unwrap();
    let (a, b) = (inserted(&s, &a), inserted(&s, &b));
    assert_ne!(a.id, b.id);
    assert_eq!(a.name, "pair");
    assert_eq!(a.opacity, 0.5);
    let (ac, bc) = (a.children().unwrap(), b.children().unwrap());
    assert_eq!(ac.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["red", "blue"]);
    assert!(ac.iter().zip(bc).all(|(x, y)| x.id != y.id));
    assert!(prepared.capability(PasteMode::Shape).is_err());
    assert!(prepared.capability(PasteMode::Path).is_ok());
}

#[test]
fn nested_svg_clips_require_layers_and_preserve_both_masks() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="8">
        <defs>
            <clipPath id="outerClip"><rect width="4" height="8"/></clipPath>
            <clipPath id="innerClip"><rect width="12" height="4"/></clipPath>
        </defs>
        <g id="outer" clip-path="url(#outerClip)">
            <g id="inner" clip-path="url(#innerClip)"><rect width="12" height="8" fill="red"/></g>
        </g></svg>"##;
    let prepared = prepare_svg(svg.as_bytes()).unwrap();
    assert_eq!(prepared.document.walk().iter().filter(|(_, _, layer)| layer.vector_mask.is_some()).count(), 2);
    assert!(prepared.capability(PasteMode::Shape).is_err());
    assert!(prepared.capability(PasteMode::Layers).is_ok());

    let mut s = session(8);
    let before = s.active().unwrap().doc.clone();
    let revision = s.active().unwrap().revision;
    assert!(paste_svg(&mut s, &prepared, PasteMode::Shape, &json!({"center":[6,4]})).is_err());
    assert_eq!(s.active().unwrap().doc, before);
    assert_eq!(s.active().unwrap().revision, revision);

    let result = paste_svg(&mut s, &prepared, PasteMode::Layers, &json!({"center":[6,4]})).unwrap();
    let after = s.active().unwrap().doc.clone();
    let mut pasted = (*after).clone();
    pasted.layers = vec![inserted(&s, &result)];
    assert_eq!(pasted.walk().iter().filter(|(_, _, layer)| layer.vector_mask.is_some()).count(), 2);
    let pixels = photocraft_compose::flatten(&pasted);
    assert_eq!(pixels.get(3, 3), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(pixels.get(6, 2), [0.0, 0.0, 0.0, 0.0]);
    assert_eq!(pixels.get(2, 6), [0.0, 0.0, 0.0, 0.0]);
    assert!(s.undo());
    assert_eq!(s.active().unwrap().doc, before);
    assert!(s.redo());
    assert_eq!(s.active().unwrap().doc, after);
}

#[test]
fn rasterised_elements_preserve_warnings_and_disable_geometry_modes() {
    let prepared = prepare_svg(FILTER.as_bytes()).unwrap();
    assert!(prepared.warnings().iter().any(|w| w.contains("filter")));
    assert!(prepared.capability(PasteMode::Path).is_err());
    assert!(prepared.capability(PasteMode::Shape).is_err());
    let mut s = session(8);
    let before = s.active().unwrap().doc.clone();
    let revision = s.active().unwrap().revision;
    assert!(paste_svg(&mut s, &prepared, PasteMode::Path, &json!({})).is_err());
    assert_eq!(s.active().unwrap().revision, revision);
    assert_eq!(*s.active().unwrap().doc, *before);
    let result = paste_svg(&mut s, &prepared, PasteMode::Layers, &json!({})).unwrap();
    assert_eq!(result["warnings"], json!(prepared.warnings()));
}

#[test]
fn aggregate_clipboard_cache_budget_warns_and_disables_geometry_modes() {
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"256\" height=\"256\">{}</svg>",
        "<rect width=\"256\" height=\"256\" fill=\"red\"/>".repeat(400)
    );
    let prepared = prepare_svg(svg.as_bytes()).unwrap();
    assert!(prepared.warnings().iter().any(|w| w.contains("cache budget")));
    assert_eq!(prepared.document.layers.len(), 1);
    for mode in [PasteMode::Path, PasteMode::Shape] {
        assert!(prepared.capability(mode).is_err());
    }
    for mode in [PasteMode::Layers, PasteMode::Pixels, PasteMode::SmartObject] {
        assert!(prepared.capability(mode).is_ok());
    }
    let oversized = br#"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="5000"><rect width="5000" height="5000"/></svg>"#;
    assert!(prepare_svg(oversized).unwrap_err().to_string().contains("cache budget"));
}

#[test]
fn destination_preflight_bounds_geometry_without_allocating_large_caches() {
    let size = Size::new(12_000, 12_000);
    let tiny = prepare_svg(RECT.as_bytes()).unwrap();
    assert!(tiny.capability_on_canvas(PasteMode::Layers, size).is_ok(), "a small shape on a large canvas fits the cache budget");
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="300000" height="300000"/></svg>"#;
    let prepared = prepare_svg(svg).unwrap();
    for mode in [PasteMode::Layers, PasteMode::Shape] {
        assert!(prepared.capability(mode).is_ok(), "source cache contains only one pixel");
        assert!(prepared.capability_on_canvas(mode, size).unwrap_err().contains("cache budget"));
        let mut s = Session::new();
        s.add_document(Document::new("large", size, ColorMode::Rgb, SampleType::U8), None);
        let before = s.active().unwrap().doc.clone();
        let revision = s.active().unwrap().revision;
        assert!(paste_svg(&mut s, &prepared, mode, &json!({})).unwrap_err().to_string().contains("cache budget"));
        assert_eq!(*s.active().unwrap().doc, *before);
        assert_eq!(s.active().unwrap().revision, revision);
        assert!(!s.active().unwrap().history.can_undo());
    }
    assert!(prepared.capability_on_canvas(PasteMode::Pixels, size).is_ok());
    assert!(prepared.capability_on_canvas(PasteMode::SmartObject, size).is_ok());
}

#[test]
fn compound_path_holes_and_existing_work_path_are_preserved() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><path fill-rule="evenodd" d="M0 0H10V10H0Z M3 3H7V7H3Z"/></svg>"#;
    let prepared = prepare_svg(svg.as_bytes()).unwrap();
    let mut s = session(8);
    let work = Path::new(vec![Subpath::polyline(&[(1.0, 2.0), (3.0, 4.0)])]);
    s.edit("work", |doc, _| {
        doc.work_path = Some(work.clone());
        Ok(())
    })
    .unwrap();
    let a = paste_svg(&mut s, &prepared, PasteMode::Path, &json!({"center":[5,5]})).unwrap();
    let b = paste_svg(&mut s, &prepared, PasteMode::Path, &json!({"center":[5,5]})).unwrap();
    assert_ne!(a["path"], b["path"]);
    let doc = &s.active().unwrap().doc;
    assert_eq!(doc.work_path, Some(work));
    let path = &doc.paths[0].path;
    assert_eq!(path.subpaths.len(), 2);
    assert_eq!(path.subpaths[1].op, photocraft_doc::PathOp::Join);
    assert_eq!(path.fill_rule, photocraft_doc::FillRule::EvenOdd);
    let coverage = photocraft_vector::path_coverage(path, Rect::new(0, 0, 10, 10));
    assert!(coverage[11] > 0.99);
    assert!(coverage[44] < 0.01, "hole remains empty");
}

#[test]
fn incompatible_fill_rules_are_not_silently_merged() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><path fill-rule="evenodd" d="M0 0H5V5H0Z"/><path fill-rule="nonzero" d="M5 5H10V10H5Z"/></svg>"#;
    let prepared = prepare_svg(svg.as_bytes()).unwrap();
    assert!(prepared.capability(PasteMode::Path).unwrap_err().contains("fill rules"));
    assert!(prepared.capability(PasteMode::Layers).is_ok());
}

#[test]
fn pixel_and_smart_caches_convert_depth_and_colour_model() {
    let prepared = prepare_svg(RECT.as_bytes()).unwrap();
    for depth in [8, 16, 32] {
        for mode in [PasteMode::Pixels, PasteMode::SmartObject] {
            let mut s = session(depth);
            s.execute("image.mode.grayscale", json!({})).unwrap();
            let result = paste_svg(&mut s, &prepared, mode, &json!({})).unwrap();
            let layer = inserted(&s, &result);
            let surface = layer.surface().unwrap();
            assert_eq!(surface.format(), s.active().unwrap().doc.pixel_format());
            let pixel = surface.read_region(Rect::new(15, 11, 16, 12));
            assert!(pixel[0] > 0.1 && pixel[0] < 0.9, "{depth} {mode:?} {pixel:?}");
            assert_eq!(pixel[1], 1.0);
        }
    }
}

#[test]
fn editable_shapes_render_on_destination_canvas_at_every_depth() {
    let prepared = prepare_svg(RECT.as_bytes()).unwrap();
    for depth in [8, 16, 32] {
        for mode in [PasteMode::Layers, PasteMode::Shape] {
            let mut s = session(depth);
            let result = paste_svg(&mut s, &prepared, mode, &json!({})).unwrap();
            let layer = inserted(&s, &result);
            let surface = layer.surface().unwrap();
            assert_eq!(surface.format(), s.active().unwrap().doc.pixel_format());
            assert_eq!(surface.read_region(Rect::new(15, 11, 16, 12)), vec![1.0, 0.0, 0.0, 1.0]);
            assert_eq!(surface.read_region(Rect::new(0, 0, 1, 1)), vec![0.0, 0.0, 0.0, 0.0]);
        }
    }
}

#[test]
fn bad_parameters_stale_document_and_non_layer_targets_do_not_edit() {
    let prepared = prepare_svg(RECT.as_bytes()).unwrap();
    let mut s = session(8);
    let before = s.active().unwrap().doc.clone();
    let revision = s.active().unwrap().revision;
    let stale_id = s.active().unwrap().doc.id.0.saturating_add(1);
    for params in [
        json!(null),
        json!({"center":[]}),
        json!({"center":[1,"2"]}),
        json!({"center":[1e20,0]}),
        json!({"document":stale_id}),
        json!({"document":0.5}),
        json!({"target":"quickMask"}),
    ] {
        assert!(paste_svg(&mut s, &prepared, PasteMode::Layers, &params).is_err(), "{params}");
        assert_eq!(*s.active().unwrap().doc, *before);
        assert_eq!(s.active().unwrap().revision, revision);
    }
    s.tools.mask_targeted = true;
    assert!(paste_svg(&mut s, &prepared, PasteMode::Layers, &json!({})).is_err());
    s.tools.mask_targeted = false;
    s.active_mut().unwrap().channel_view.target = crate::channel_cmds::ChannelTarget::Color(0);
    assert!(paste_svg(&mut s, &prepared, PasteMode::Layers, &json!({})).is_err());
    assert_eq!(*s.active().unwrap().doc, *before);
}

#[test]
fn locked_enclosing_group_rejects_new_layers_but_locked_background_accepts_siblings() {
    let prepared = prepare_svg(RECT.as_bytes()).unwrap();
    let mut s = session(8);
    assert!(paste_svg(&mut s, &prepared, PasteMode::Pixels, &json!({})).is_ok());
    let child = Layer::raster("child", s.active().unwrap().doc.pixel_format());
    let child_id = child.id;
    let mut group = Layer::group("locked", vec![child]);
    group.locks = Locks { all: true, ..Default::default() };
    s.edit("group", |doc, active| {
        doc.layers.push(group);
        *active = Some(child_id);
        Ok(())
    })
    .unwrap();
    let before = s.active().unwrap().doc.clone();
    let revision = s.active().unwrap().revision;
    assert!(paste_svg(&mut s, &prepared, PasteMode::Layers, &json!({})).is_err());
    assert_eq!(*s.active().unwrap().doc, *before);
    assert_eq!(s.active().unwrap().revision, revision);
}

#[test]
fn headless_paste_in_mask_view_does_not_mutate_document_or_history() {
    let mut s = session(8);
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("layer.layerMask.revealAll", json!({})).unwrap();
    s.execute("view.layerMask", json!({"mode":"gray"})).unwrap();
    s.tools.mask_targeted = false;
    let before = s.active().unwrap().doc.clone();
    let revision = s.active().unwrap().revision;
    let undo = s.active().unwrap().history.can_undo();
    let spec = specs().remove(0);
    assert!((spec.run)(&mut s, &json!({"svg":RECT,"mode":"pixels"})).is_err());
    assert_eq!(*s.active().unwrap().doc, *before);
    assert_eq!(s.active().unwrap().revision, revision);
    assert_eq!(s.active().unwrap().history.can_undo(), undo);
    assert!(s.active().unwrap().channel_view.layer_mask.is_some());
}

#[test]
fn untrusted_svg_budget_empty_and_external_images_are_rejected_safely() {
    for bytes in [b"".as_slice(), b"not SVG", b"<svg", br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"/>"#] {
        assert!(prepare_svg(bytes).is_err());
    }
    assert!(prepare_svg(&vec![b' '; MAX_VECTOR_CLIPBOARD_BYTES + 1]).is_err());
    let external = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"><image href="file:///not-accessed.png" width="8" height="6"/></svg>"#;
    assert!(prepare_svg(external).is_err());
}

#[test]
fn outlined_text_warning_includes_namespaced_text_elements() {
    for svg in [
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"><rect width="8" height="6"/><text y="4">A</text></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:svg="http://www.w3.org/2000/svg" width="8" height="6"><rect width="8" height="6"/><svg:text y="4">A</svg:text></svg>"#,
    ] {
        let prepared = prepare_svg(svg.as_bytes()).unwrap();
        assert!(prepared.warnings().iter().any(|w| w.contains("outlines")));
    }
}

#[test]
fn headless_command_requires_explicit_svg_and_mode() {
    let mut s = session(8);
    let spec = specs().remove(0);
    for p in [json!({}), json!({"svg":RECT}), json!({"svg":RECT,"mode":false}), json!({"svg":RECT,"mode":"unknown"}), json!({"svg":32,"mode":"pixels"})] {
        assert!((spec.run)(&mut s, &p).is_err());
    }
    let doc_id = s.active().unwrap().doc.id.0;
    let result = (spec.run)(&mut s, &json!({"svg":RECT,"mode":"pixels","document":doc_id,"center":[4,3]})).unwrap();
    assert_eq!(result["offset"], json!([0, 0]));
}
