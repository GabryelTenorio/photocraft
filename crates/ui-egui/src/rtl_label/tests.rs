//! #1408: right-to-left layer names are laid out by the text engine, in bidi order, and cut at
//! their logical end.

use egui::{FontFamily, FontId, vec2};
use egui_kittest::Harness;
use serde_json::json;

use super::*;
use crate::PhotocraftApp;

const ARABIC: &str = "طبقة النص";
const HEBREW: &str = "שכבה חדשה";

fn key(text: &str) -> Key {
    Key {
        text: text.into(),
        family: "Inter",
        weight: 400,
        italic: false,
        size_px: 13 * 64,
        row_px: 16 * 64,
        baseline_px: 12 * 64,
        ppp: 1f32.to_bits(),
        fonts: (0, 0),
    }
}

#[test]
fn only_right_to_left_text_needs_the_engine() {
    for text in [ARABIC, HEBREW, "Layer 1 طبقة", "ܫܠܡܐ", "ދިވެހި", "\u{200F}x", "a\u{202E}bc", "\u{FEFB}"] {
        assert!(needs_engine(text), "{text:?}");
    }
    for text in ["", "Layer 1", "Background", "背景のテクスチャ", "한글 레이어", "हिन्दी", "ไทย", "Ελληνικά", "Кириллица", "Ω — “quoted” …"]
    {
        assert!(!needs_engine(text), "{text:?}");
    }
    // Arabic-Indic digits are Arabic-block characters: bidi orders them too.
    assert!(needs_engine("١٢٣"));
}

#[test]
fn ui_fonts_map_to_the_bundled_faces() {
    assert_eq!(face(&FontId::proportional(13.0)), Some(("Inter", 400)));
    assert_eq!(face(&crate::theme::medium(13.0)), Some(("Inter", 500)));
    assert_eq!(face(&crate::theme::semibold(11.0)), Some(("Inter", 600)));
    assert_eq!(face(&FontId::monospace(12.0)), Some(("JetBrains Mono", 400)));
    assert_eq!(face(&FontId::new(12.0, FontFamily::Name("icons".into()))), None, "unknown families stay on egui's path");
}

#[test]
fn the_longest_fitting_prefix_is_found_by_bisection() {
    assert_eq!(longest_fitting(0, |_| true), None);
    assert_eq!(longest_fitting(10, |_| false), None);
    assert_eq!(longest_fitting(10, |_| true), Some(9));
    assert_eq!(longest_fitting(1, |_| true), Some(0));
    for limit in 0..10 {
        let mut probes = 0;
        assert_eq!(
            longest_fitting(10, |k| {
                probes += 1;
                k <= limit
            }),
            Some(limit)
        );
        assert!(probes <= 5, "{probes} probes for 10 lengths");
    }
}

#[test]
fn a_cut_keeps_the_logical_start_and_drops_trailing_spaces() {
    assert_eq!(cut(ARABIC, 4), "طبقة…");
    assert_eq!(cut(ARABIC, 5), "طبقة…", "the space before the cut is dropped");
    assert_eq!(cut(ARABIC, 6), "طبقة ا…");
    assert_eq!(cut(ARABIC, 0), "…");
    assert_eq!(cut(ARABIC, 99), format!("{ARABIC}…"));
}

#[test]
fn keys_follow_text_size_and_scale() {
    let base = key(ARABIC);
    assert_eq!(base, key(ARABIC));
    assert_ne!(base, key(HEBREW));
    assert_ne!(base, Key { size_px: 14 * 64, ..key(ARABIC) });
    assert_ne!(base, Key { ppp: 2f32.to_bits(), ..key(ARABIC) });
    assert_ne!(base, Key { weight: 500, ..key(ARABIC) });
    assert_ne!(base, Key { fonts: (1, 0), ..key(ARABIC) }, "a font that arrives later renders again");
}

/// Bidi doesn't depend on the font: even without a Hebrew face the logical start sits right.
#[test]
fn right_to_left_names_are_laid_out_from_the_right() {
    let mut engine = photocraft_text::TextEngine::new();
    let layout = engine.layout(&layer(HEBREW, &key(HEBREW)), 72.0);
    let first = layout.clusters.iter().find(|c| c.range.start == 0).expect("first cluster");
    let last = layout.clusters.iter().max_by_key(|c| c.range.start).expect("last cluster");
    assert!(first.rtl && first.x > last.x, "first {first:?} right of last {last:?}");
    // A Latin word inside keeps its own order, and the line still starts from the right.
    let mixed = "שכבה PhotoCraft";
    let layout = engine.layout(&layer(mixed, &key(mixed)), 72.0);
    let at = |i: usize| layout.clusters.iter().find(|c| c.range.start == i).expect("cluster").x;
    let p = mixed.find('P').unwrap_or_default();
    assert!(at(p) < at(p + 1), "PhotoCraft reads left to right");
    assert!(at(0) > at(p), "the Hebrew word is on the right");
}

#[test]
fn a_long_right_to_left_name_is_cut_with_the_ellipsis_on_its_left() {
    let mut engine = photocraft_text::TextEngine::new();
    let long = format!("{HEBREW} {HEBREW} {HEBREW}");
    let k = key(&long);
    let full = measure(&mut engine, &long, &k).expect("measured");
    assert_eq!(fitted(&mut engine, &long, &k, full + 1.0).as_deref(), Some(long.as_str()), "fits whole");
    let shown = fitted(&mut engine, &long, &k, (full / 2.0).floor()).expect("cut");
    assert!(shown.ends_with(ELLIPSIS) && long.starts_with(shown.trim_end_matches(ELLIPSIS)), "{shown:?}");
    assert!(measure(&mut engine, &shown, &k).is_some_and(|w| w <= (full / 2.0).floor()));
    let layout = engine.layout(&layer(&shown, &k), 72.0);
    let ellipsis = layout.clusters.iter().max_by_key(|c| c.range.start).expect("ellipsis");
    assert!(layout.clusters.iter().all(|c| c.x >= ellipsis.x), "the … is leftmost: {:?}", layout.clusters);
    assert_eq!(fitted(&mut engine, &long, &k, 1.0), None, "not even … fits");
}

#[test]
fn a_rendered_label_is_a_white_mask_with_room_for_marks() {
    let mut engine = photocraft_text::TextEngine::new();
    let k = key(HEBREW);
    let (image, advance) = render(&mut engine, &k).expect("rendered");
    assert_eq!(image.size[0], (advance + 2.0 * MARGIN_PX).ceil() as usize);
    assert_eq!(image.size[1], (16.0 + 2.0 * PAD).ceil() as usize);
    assert!(image.pixels.iter().any(|p| p.a() > 0), "ink");
    assert!(image.pixels.iter().all(|p| p.r() == p.a() && p.g() == p.a() && p.b() == p.a()), "white coverage, tinted when painted");
    let (double, _) = render(&mut engine, &Key { size_px: 26 * 64, row_px: 32 * 64, baseline_px: 24 * 64, ppp: 2f32.to_bits(), ..k }).expect("2x");
    assert!(double.size[0] > image.size[0] && double.size[1] > image.size[1], "HiDPI renders at device pixels");
    assert!(render(&mut engine, &key("")).is_none());
}

/// Waits for the shared engine's installed-font scan, as a running app does.
fn wait_for_fonts() {
    let start = std::time::Instant::now();
    while photocraft_text::try_shared().is_none() && start.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn layers_panel(names: &[&str], ppp: f32, dock_width: f32) -> (Harness<'static, PhotocraftApp>, Vec<u64>) {
    wait_for_fonts();
    let mut s = photocraft_engine::Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
    let ids: Vec<u64> = names.iter().map(|n| s.execute("layer.new.layer", json!({"name": n})).unwrap()["layer"].as_u64().unwrap()).collect();
    let mut h = Harness::builder().with_size(vec2(1440.0, 900.0)).with_pixels_per_point(ppp).with_max_steps(64).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        PhotocraftApp::new(s, crate::Services::default())
    });
    let ctx = h.ctx.clone();
    let (req, _rx) =
        crate::control::ControlRequest::new("ui.set", json!({"dockWidth": dock_width, "dock": {"collapsed": ["color", "properties", "history", "navigator"]}}));
    crate::control::handle(h.state_mut(), &ctx, &req);
    h.run_steps(8);
    (h, ids)
}

fn engine_labels(ctx: &egui::Context) -> usize {
    ctx.tex_manager().read().allocated().filter(|(_, meta)| meta.name == TEXTURE_NAME).count()
}

#[test]
fn the_layers_panel_draws_right_to_left_names_with_the_engine() {
    for ppp in [1.0, 2.0] {
        let (h, ids) = layers_panel(&[ARABIC, HEBREW, "Layer 3"], ppp, 300.0);
        let rows = crate::layer_row_ui::recorded(&h.ctx);
        for id in &ids {
            let row = rows.iter().find(|r| r.layer == *id).expect("row drawn");
            let name = row.name.expect("name drawn");
            assert!(name.width() > 10.0 && row.row.contains_rect(name.shrink(0.5)), "@{ppp}x: {name:?} inside {:?}", row.row);
        }
        assert_eq!(engine_labels(&h.ctx), 2, "@{ppp}x: the Arabic and Hebrew names, not the Latin one");
    }
}

#[test]
fn a_narrow_layers_panel_cuts_right_to_left_names_inside_the_row() {
    let long = format!("{ARABIC} {ARABIC} {ARABIC} {ARABIC} {ARABIC} {ARABIC}");
    let name = |dock_width: f32| {
        let (h, ids) = layers_panel(&[&long], 1.0, dock_width);
        let row = crate::layer_row_ui::recorded(&h.ctx).into_iter().find(|r| r.layer == ids[0]).expect("row drawn");
        let name = row.name.expect("name drawn");
        assert!(name.right() <= row.row.right() - crate::layer_row_ui::RIGHT_PAD, "{name:?} in {:?}", row.row);
        assert!(engine_labels(&h.ctx) >= 1, "drawn by the engine");
        name.width()
    };
    let (narrow, wide) = (name(220.0), name(600.0));
    assert!(narrow < wide, "cut to fit: {narrow} < {wide}");
}
