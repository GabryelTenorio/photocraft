//! A type layer whose font is not installed keeps the pixels it was drawn with when it is moved,
//! transformed or resized with the canvas, as Photoshop does until the text is edited. Before,
//! every transform re-rendered it in the fallback face.

use photocraft_doc::{LayerContent, LayerId, TextLayer};
use photocraft_engine::Session;
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::json;

const MISSING: &str = "PhotoCraft Test Face That Is Not Installed";
/// The stand-in for the pixels a PSD stored for the layer: a square no font would draw.
const DRAWN: Rect = Rect { x0: 20, y0: 30, x1: 40, y1: 50 };
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn text(s: &Session, id: LayerId) -> TextLayer {
    let LayerContent::Text(t) = &s.active().unwrap().doc.layer(id).unwrap().content else { panic!("not type") };
    t.clone()
}

/// A type layer as a PSD with a missing font opens: the font named in the file, the file's
/// pixels (`DRAWN`) and its `TySh` block. With `missing` false the font is an installed one.
fn fixture(missing: bool) -> (Session, LayerId) {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 200, "height": 120, "background": "white"})).unwrap();
    let id = LayerId(s.execute("type.create", json!({"text": "Hi", "x": 20, "y": 50, "size": 24, "font": "Inter"})).unwrap()["layer"].as_u64().unwrap());
    s.edit("as opened", |doc, _| {
        let Some(LayerContent::Text(t)) = doc.layer_mut(id).map(|l| &mut l.content) else { panic!("not type") };
        if missing {
            for r in &mut t.runs {
                r.style.font_family = MISSING.into();
                r.style.postscript_name = Some("PhotoCraftTestFaceThatIsNotInstalled-Regular".into());
            }
            t.sync_summary();
        }
        let mut drawn = Surface::new(t.cache.as_ref().unwrap().format());
        drawn.fill_rect(DRAWN, &BLUE);
        t.cache = Some(drawn);
        Ok(())
    })
    .unwrap();
    (s, id)
}

fn bounds(t: &TextLayer) -> Rect {
    t.cache.as_ref().unwrap().content_bounds()
}

fn is_blue(t: &TextLayer, x: i32, y: i32) -> bool {
    let c = t.cache.as_ref().unwrap();
    (0..4).all(|ch| (c.sample_channel(x, y, ch) - BLUE[ch]).abs() < 0.02)
}

/// The `TySh` transform matches the layer's, and everything after it is the file's.
fn assert_tysh_follows(before: &TextLayer, after: &TextLayer) {
    let (b, a) = (before.psd_raw.as_deref().unwrap(), after.psd_raw.as_deref().unwrap());
    assert_eq!(photocraft_text::psd::parse_tysh(a).unwrap().transform, after.transform);
    assert_eq!(a.get(50..), b.get(50..), "the text data is kept as it was");
}

#[test]
fn moving_keeps_the_pixels() {
    let (mut s, id) = fixture(true);
    let before = text(&s, id);
    s.execute("layer.translate", json!({"layer": id.0, "dx": 15, "dy": 7})).unwrap();
    let after = text(&s, id);
    assert_eq!(bounds(&after), DRAWN.translate(15, 7), "the pixels moved instead of re-rendering in a fallback face");
    assert!(is_blue(&after, 45, 47));
    assert_eq!(after.transform.m[4] - before.transform.m[4], 15.0);
    assert_eq!(after.transform.m[5] - before.transform.m[5], 7.0);
    assert_eq!(after.runs[0].style.font_family, MISSING, "the file's font is kept");
    assert_tysh_follows(&before, &after);
    // Undo restores the layer as it was.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(bounds(&text(&s, id)), DRAWN);
}

#[test]
fn free_transform_transforms_the_pixels() {
    let (mut s, id) = fixture(true);
    let before = text(&s, id);
    s.execute("edit.transform", json!({"layer": id.0, "matrix": [2, 0, 0, 2, 0, 0]})).unwrap();
    let after = text(&s, id);
    let b = bounds(&after);
    assert!((b.x0 - 40).abs() <= 1 && (b.y0 - 60).abs() <= 1 && (b.x1 - 80).abs() <= 1 && (b.y1 - 100).abs() <= 1, "{b:?}");
    assert!(is_blue(&after, 60, 80));
    assert!((after.transform.m[0] - 2.0 * before.transform.m[0]).abs() < 1e-9);
    assert_tysh_follows(&before, &after);
}

#[test]
fn image_size_resamples_the_pixels() {
    let (mut s, id) = fixture(true);
    s.execute("image.imageSize", json!({"width": 400, "height": 240})).unwrap();
    let after = text(&s, id);
    let b = bounds(&after);
    assert!((b.x0 - 40).abs() <= 1 && (b.y1 - 100).abs() <= 1, "{b:?}");
    assert!(is_blue(&after, 60, 80));
}

#[test]
fn arbitrary_rotation_rotates_the_pixels() {
    let (mut s, id) = fixture(true);
    s.execute("image.rotation.arbitrary", json!({"angle": 10})).unwrap();
    let after = text(&s, id);
    let b = bounds(&after);
    assert!(b.width() > DRAWN.width() && b.height() > DRAWN.height(), "a rotated square is wider: {b:?}");
    let (cx, cy) = ((b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2);
    assert!(is_blue(&after, cx, cy));
}

#[test]
fn editing_the_text_renders_it_again() {
    let (mut s, id) = fixture(true);
    s.execute("type.edit", json!({"layer": id.0, "text": "Hello"})).unwrap();
    assert_ne!(bounds(&text(&s, id)), DRAWN, "an edit lays the text out (in the substitute face)");
}

#[test]
fn installed_fonts_still_render_again() {
    let (mut s, id) = fixture(false);
    s.execute("edit.transform", json!({"layer": id.0, "matrix": [2, 0, 0, 2, 0, 0]})).unwrap();
    let after = text(&s, id);
    assert!(!is_blue(&after, 60, 80), "type in an installed font is laid out again at its new size");
}

#[test]
fn a_matching_postscript_name_is_not_missing() {
    let ps = {
        let mut eng = photocraft_text::shared().lock().unwrap();
        eng.fonts.faces("Inter").into_iter().find_map(|f| f.postscript_name).expect("bundled Inter has a PostScript name")
    };
    let (mut s, id) = fixture(true);
    let missing = s.execute("type.resolveMissingFonts", json!({})).unwrap();
    assert_eq!(missing["missing"], json!([MISSING]));
    s.edit("name the installed face", |doc, _| {
        let Some(LayerContent::Text(t)) = doc.layer_mut(id).map(|l| &mut l.content) else { panic!("not type") };
        for r in &mut t.runs {
            r.style.postscript_name = Some(ps.clone());
        }
        Ok(())
    })
    .unwrap();
    let missing = s.execute("type.resolveMissingFonts", json!({})).unwrap();
    assert_eq!(missing["missing"], json!([]), "the face is installed under its PostScript name");
    // And it is not treated as missing: a transform renders it again.
    s.execute("edit.transform", json!({"layer": id.0, "matrix": [2, 0, 0, 2, 0, 0]})).unwrap();
    assert!(!is_blue(&text(&s, id), 60, 80));
}
