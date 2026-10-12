use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer, Size};
use photocraft_raster::from_rgba;
use serde_json::json;
use std::time::Instant;

use super::*;

/// The single pass over the content bounds that the tiled `adjust_surface` replaced (#1772).
fn one_pass(s: &Surface, adj: &Adjustment, selection: Option<&Surface>, mode: ColorMode) -> Surface {
    let mut s = s.clone();
    let r = s.content_bounds();
    if !r.is_empty() {
        let out = adjusted(&s, r, adj, selection, mode);
        s.write_region(r, &out);
    }
    s
}

/// Two patches over a 3×3 tile grid, not tile-aligned, with partial alpha; the bottom-left tiles
/// inside the content bounds are never allocated.
fn layer(fmt: PixelFormat) -> Surface {
    let mut s = Surface::new(fmt);
    for r in [Rect::new(200, 240, 300, 300), Rect::new(530, 250, 560, 530)] {
        let mut data = Vec::new();
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let a = if fmt.alpha { 0.3 + 0.7 * ((x + y) % 7) as f32 / 6.0 } else { 1.0 };
                data.extend(from_rgba(&fmt, [x as f32 / 600.0, y as f32 / 540.0, 0.5 + 0.4 * (x as f32 * 0.05).sin(), a]));
            }
        }
        s.write_region(r, &data);
    }
    assert!(!s.has_tiles_in(Rect::new(200, 512, 256, 530)), "a hole inside the content bounds");
    s
}

/// Release-only comparison for the 5000×7073 px group-merge report (#1561). The reference path
/// models the old full-canvas composite plus a second converted float buffer; the optimized path
/// renders only the group's visible content bounds in bands.
#[test]
#[ignore = "manual 35 MP merge benchmark; run in release with --ignored --nocapture"]
fn benchmark_large_group_merge() {
    let fmt = PixelFormat::RGBA8;
    let mut doc = Document::new("merge benchmark", Size::new(5000, 7073), ColorMode::Rgb, SampleType::U8);
    let content = Rect::new(1700, 2200, 2700, 3200);
    let mut children = Vec::new();
    for i in 0..4 {
        let mut child = Layer::raster(format!("Layer {i}"), fmt);
        if let Some(surface) = child.surface_mut() {
            surface.fill_rect(content, &[(i as f32 + 1.0) / 5.0, 0.2, 0.4, 0.5]);
        }
        children.push(child);
    }
    doc.layers.push(Layer::group("Group", children));
    let expected_area = photocraft_compose::layer_bounds(&doc.layers[0], doc.bounds()).intersect(&doc.bounds());

    let start = Instant::now();
    let reference = photocraft_compose::flatten(&doc);
    let reference_data: Vec<f32> = reference.px.iter().flat_map(|p| photocraft_raster::from_rgba(&fmt, *p)).collect();
    let mut reference_surface = Surface::new(fmt);
    reference_surface.write_region(doc.bounds(), &reference_data);
    reference_surface.prune();
    let reference_time = start.elapsed();

    let start = Instant::now();
    let optimized = composite_layers(&doc, fmt, None);
    let optimized_time = start.elapsed();
    assert_eq!(reference_surface.read_region(expected_area), optimized.read_region(expected_area));
    println!("#1561 5000x7073, 4x 1000x1000 layers: full-canvas {:?}, bounded/banded {:?}", reference_time, optimized_time);
}

#[test]
fn tiled_adjustment_matches_one_pass_over_the_content() {
    let adjustments = [
        ("invert", json!({})),
        ("levels", json!({"inBlack": 20, "gamma": 1.3, "inWhite": 230})),
        ("curves", json!({"points": [[0, 10], [128, 160], [255, 240]]})),
        ("hueSaturation", json!({"hue": 40, "saturation": 20})),
        ("brightnessContrast", json!({"brightness": 30, "contrast": 20})),
        ("exposure", json!({"exposure": 0.8})),
        ("posterize", json!({"levels": 5})),
        ("threshold", json!({"level": 120})),
        ("colorBalance", json!({"midtones": [20, -10, 5]})),
        ("blackWhite", json!({"tint": true})),
        ("photoFilter", json!({"density": 50})),
        ("channelMixer", json!({"red": [80, 30, -10, 5]})),
        ("vibrance", json!({"vibrance": 40})),
        ("selectiveColor", json!({"reds": [10, -20, 30, 5]})),
        // Dithered kinds vary by position, so they show a tile's pixels kept their coordinates.
        ("gradientMap", json!({"stops": [[0, "#102030"], [1, "#f0e0a0"]], "dither": true})),
        ("colorLookup", json!({"lut": "warm", "dither": true})),
    ];
    let mut formats = Vec::new();
    for mode in [ColorMode::Rgb, ColorMode::Cmyk, ColorMode::Lab, ColorMode::Grayscale] {
        for sample in [SampleType::U8, SampleType::U16, SampleType::F32] {
            formats.push(PixelFormat { mode, sample, alpha: true });
        }
    }
    formats.push(PixelFormat { mode: ColorMode::Rgb, sample: SampleType::U8, alpha: false });
    formats.push(PixelFormat { mode: ColorMode::Grayscale, sample: SampleType::U16, alpha: false });
    // A soft selection that ends inside the content.
    let mut soft = Surface::new(PixelFormat::GRAY8);
    let sr = Rect::new(0, 0, 500, 400);
    soft.write_region(sr, &(0..sr.height()).flat_map(|_| (0..sr.width()).map(|x| x as f32 / 500.0)).collect::<Vec<_>>());
    for fmt in formats {
        let s = layer(fmt);
        for (kind, p) in &adjustments {
            let adj = crate::adjust_params::from_params(kind, p, None, fmt.mode).unwrap_or_else(|e| panic!("{kind}: {e}"));
            for sel in [None, Some(&soft)] {
                let want = one_pass(&s, &adj, sel, fmt.mode);
                let mut got = s.clone();
                adjust_surface(&mut got, &adj, sel, fmt.mode);
                assert!(want != s, "{kind} {fmt:?}: the adjustment changes the layer");
                assert!(got == want, "{kind} {fmt:?} selection {}: tiled result differs", sel.is_some());
            }
        }
    }
}

#[test]
fn an_empty_layer_is_left_alone() {
    let mut s = Surface::new(PixelFormat::RGBA8);
    adjust_surface(&mut s, &Adjustment::Invert, None, ColorMode::Rgb);
    assert_eq!(s.tile_count(), 0);
}

/// One `fmt` pixel holding `ch`, after `adj` through an optional `k`-strength selection.
fn adjusted_pixel(fmt: PixelFormat, ch: &[f32], adj: &Adjustment, k: Option<f32>) -> Vec<f32> {
    let unit = Rect::new(0, 0, 1, 1);
    let mut s = Surface::new(fmt);
    s.write_region(unit, ch);
    let mut sel = Surface::new(PixelFormat::GRAY8);
    if let Some(k) = k {
        sel.write_region(unit, &[k]);
    }
    adjust_surface(&mut s, adj, k.map(|_| &sel), fmt.mode);
    s.read_region(unit)
}

fn assert_levels(got: &[f32], want_255: &[f32], what: &str) {
    let want: Vec<f32> = want_255.iter().map(|v| v / 255.0).collect();
    assert!(got.iter().zip(&want).all(|(g, w)| (g - w).abs() < 1e-4), "{what}: got {:?}, want {want_255:?}", got.iter().map(|v| v * 255.0).collect::<Vec<_>>());
}

/// Image > Adjustments > Invert, Posterize and Threshold change a Lab or CMYK layer's stored
/// channels exactly as Photoshop's do (photoshop corpus lab8, lab16, cmyk8 and cmyk16 invert,
/// posterize-4 and threshold-128 match these rules on every pixel), with no trip through RGB.
#[test]
fn invert_posterize_and_threshold_change_lab_and_cmyk_channels_as_stored() {
    let f = |v: [f32; 4]| v.map(|x| x / 255.0);
    let cmyk = PixelFormat::new(ColorMode::Cmyk, SampleType::U8, true);
    let ink = [51.0, 102.0, 153.0, 26.0, 255.0].map(|x| x / 255.0);
    assert_levels(&adjusted_pixel(cmyk, &ink, &Adjustment::Invert, None), &[204.0, 153.0, 102.0, 229.0, 255.0], "CMYK invert");
    assert_levels(&adjusted_pixel(cmyk, &ink, &Adjustment::Posterize { levels: 4 }, None), &[0.0, 85.0, 170.0, 0.0, 255.0], "CMYK posterize");
    // Threshold: the black ink alone, or paper.
    let dark = [200.0, 180.0, 180.0, 200.0, 255.0].map(|x| x / 255.0);
    assert_levels(&adjusted_pixel(cmyk, &dark, &Adjustment::Threshold { level: 0.5 }, None), &[0.0, 0.0, 0.0, 255.0, 255.0], "CMYK threshold dark");
    assert_levels(
        &adjusted_pixel(cmyk, &[0.0, 0.0, 0.0, 0.0, 1.0], &Adjustment::Threshold { level: 0.5 }, None),
        &[0.0, 0.0, 0.0, 0.0, 255.0],
        "CMYK threshold paper",
    );
    // A half-strength selection mixes the stored inks.
    let half = adjusted_pixel(cmyk, &[0.0, 0.0, 0.0, 0.0, 1.0], &Adjustment::Invert, Some(0.5));
    assert!(half.iter().take(4).all(|v| (v - 0.5).abs() < 0.01), "{half:?}");

    let lab8 = PixelFormat::new(ColorMode::Lab, SampleType::U8, true);
    let mut px = f([77.0, 128.0, 200.0, 255.0]).to_vec();
    assert_levels(&adjusted_pixel(lab8, &px, &Adjustment::Invert, None), &[178.0, 127.0, 55.0, 255.0], "Lab 8-bit invert");
    // 16-bit mirrors a and b around neutral.
    let lab16 = PixelFormat::new(ColorMode::Lab, SampleType::U16, true);
    assert_levels(&adjusted_pixel(lab16, &px, &Adjustment::Invert, None), &[178.0, 128.0, 56.0, 255.0], "Lab 16-bit invert");
    // A neutral grey posterizes warm: a and b step to +42.
    px = f([128.0, 128.0, 128.0, 255.0]).to_vec();
    assert_levels(&adjusted_pixel(lab8, &px, &Adjustment::Posterize { levels: 4 }, None), &[170.0, 170.0, 170.0, 255.0], "Lab posterize");
    // Threshold compares L alone and keeps black and white neutral.
    let level = Adjustment::Threshold { level: 128.0 / 255.0 };
    assert_levels(&adjusted_pixel(lab8, &f([128.0, 30.0, 220.0, 255.0]), &level, None), &[255.0, 128.0, 128.0, 255.0], "Lab threshold light");
    assert_levels(&adjusted_pixel(lab16, &f([127.0, 30.0, 220.0, 255.0]), &level, None), &[0.0, 128.0, 128.0, 255.0], "Lab threshold dark");
}
