//! Camera Raw settings as an Adobe Camera Raw `.xmp` sidecar: Filter › Camera Raw Filter ▸
//! Save/Load Settings, so a look can be reused across images (issue #4274).
//!
//! Clean-room: written from the public XMP specification (ISO 16684 / Adobe's XMP
//! specification), the published `crs` namespace URI and Adobe's documented Camera Raw field
//! names. Our `CameraRaw` sliders are in Camera Raw's units, so the documented fields map
//! directly. Fields with no documented `crs` equivalent (the white-balance Temperature/Tint,
//! which here are relative to the as-shot white, plus the internal `seed`/`pixelScale`/
//! `encodingGamma`) are not written; a file written here round-trips exactly for everything it
//! does carry.

use crate::camera_raw::CameraRaw;

const NS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
/// The Camera Raw process/version PhotoCraft's settings correspond to (numbers only loose here).
const VERSION: &str = "15.4";

/// The eight Color Mixer bands, red → magenta, with Adobe's `crs` name fragment.
const BANDS: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];

/// Format a slider value the way Camera Raw writes levels: integers without a decimal point,
/// fractions trimmed to at most five decimals, no exponent.
fn num(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    if v.fract() == 0.0 && v.abs() < 1e9 {
        return format!("{}", v as i64);
    }
    let mut s = format!("{v:.5}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

/// The Camera Raw settings as a complete XMP packet (UTF-8).
pub fn to_xmp(p: &CameraRaw) -> String {
    let mut a = String::new();
    a.push_str(&format!(" crs:Version=\"{VERSION}\" crs:ProcessVersion=\"{VERSION}\" crs:HasSettings=\"True\""));
    let mut put = |name: &str, v: f32| {
        a.push_str(&format!(" crs:{name}=\"{}\"", num(v)));
    };
    // Light
    put("Exposure2012", p.exposure);
    put("Contrast2012", p.contrast);
    put("Highlights2012", p.highlights);
    put("Shadows2012", p.shadows);
    put("Whites2012", p.whites);
    put("Blacks2012", p.blacks);
    put("Texture", p.texture);
    put("Clarity2012", p.clarity);
    put("Dehaze", p.dehaze);
    put("Vibrance", p.vibrance);
    put("Saturation", p.saturation);
    // Parametric tone curve
    put("ParametricShadows", p.curve_shadows);
    put("ParametricDarks", p.curve_darks);
    put("ParametricLights", p.curve_lights);
    put("ParametricHighlights", p.curve_highlights);
    for (name, v) in
        [("ParametricShadowSplit", p.curve_splits[0]), ("ParametricMidtoneSplit", p.curve_splits[1]), ("ParametricHighlightSplit", p.curve_splits[2])]
    {
        put(name, v);
    }
    // Detail
    put("Sharpness", p.sharpen_amount);
    put("SharpenRadius", p.sharpen_radius);
    put("SharpenDetail", p.sharpen_detail);
    put("SharpenEdgeMasking", p.sharpen_masking);
    put("LuminanceSmoothing", p.noise_luminance);
    put("LuminanceNoiseReductionDetail", p.noise_luminance_detail);
    put("ColorNoiseReduction", p.noise_color);
    put("ColorNoiseReductionDetail", p.noise_color_detail);
    // Effects
    put("GrainAmount", p.grain_amount);
    put("GrainSize", p.grain_size);
    put("GrainFrequency", p.grain_roughness);
    put("PostCropVignetteAmount", p.vignette_amount);
    put("PostCropVignetteMidpoint", p.vignette_midpoint);
    put("PostCropVignetteRoundness", p.vignette_roundness);
    put("PostCropVignetteFeather", p.vignette_feather);
    put("PostCropVignetteHighlightContrast", p.vignette_highlights);
    put("PostCropVignetteStyle", vignette_style_code(&p.vignette_style));
    // Color Mixer (HSL)
    for (k, band) in BANDS.iter().enumerate() {
        put(&format!("HueAdjustment{band}"), p.hsl_hue[k]);
        put(&format!("SaturationAdjustment{band}"), p.hsl_sat[k]);
        put(&format!("LuminanceAdjustment{band}"), p.hsl_lum[k]);
    }
    // Color Grading
    for (prefix, w) in [
        ("ColorGradeShadow", p.grade_shadows),
        ("ColorGradeMidtone", p.grade_midtones),
        ("ColorGradeHighlight", p.grade_highlights),
        ("ColorGradeGlobal", p.grade_global),
    ] {
        put(&format!("{prefix}Hue"), w.hue);
        put(&format!("{prefix}Sat"), w.sat);
        put(&format!("{prefix}Lum"), w.lum);
    }
    put("ColorGradeBlending", p.grade_blending);
    put("ColorGradeBalance", p.grade_balance);

    let mut body = String::new();
    body.push_str(&seq("ToneCurvePV2012", &p.point_curve));
    body.push_str(&seq("ToneCurvePV2012Red", &p.point_curve_red));
    body.push_str(&seq("ToneCurvePV2012Green", &p.point_curve_green));
    body.push_str(&seq("ToneCurvePV2012Blue", &p.point_curve_blue));

    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"PhotoCraft\">\n\
         \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         \x20\x20<rdf:Description rdf:about=\"\" xmlns:crs=\"{NS}\"{a}>\n{body}\x20\x20</rdf:Description>\n\
         \x20</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>"
    )
}

/// One point curve as an `rdf:Seq` of `x, y` level pairs (empty = linear).
fn seq(name: &str, points: &[[f32; 2]]) -> String {
    if points.is_empty() {
        return String::new();
    }
    let mut s = format!("   <crs:{name}><rdf:Seq>");
    for q in points {
        s.push_str(&format!("<rdf:li>{}, {}</rdf:li>", num(q[0]), num(q[1])));
    }
    s.push_str("</rdf:Seq></crs:");
    s.push_str(name);
    s.push_str(">\n");
    s
}

fn vignette_style_code(style: &str) -> f32 {
    match style {
        "colorPriority" => 1.0,
        "paintOverlay" => 2.0,
        _ => 0.0,
    }
}

fn vignette_style_name(code: f32) -> String {
    match code.round() as i64 {
        1 => "colorPriority".into(),
        2 => "paintOverlay".into(),
        _ => "highlightPriority".into(),
    }
}

/// Parse `="value"` / `='value'` (leading whitespace allowed) into a number.
fn attr_value(rest: &str) -> Option<f32> {
    let t = rest.trim_start().strip_prefix('=')?;
    let t = t.trim_start();
    let q = t.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let body = &t[q.len_utf8()..];
    let end = body.find(q)?;
    body[..end].trim().parse().ok()
}

/// The value of the `crs:{name}` attribute (`="value"` or `='value'`), tolerating whitespace.
fn attr(doc: &str, name: &str) -> Option<f32> {
    let needle = format!("crs:{name}");
    let mut from = 0;
    while let Some(i) = doc[from..].find(&needle) {
        let after = from + i + needle.len();
        if let Some(v) = doc.get(after..).and_then(attr_value) {
            return Some(v);
        }
        from = after;
    }
    None
}

/// The `x, y` pairs of the `crs:{name}` sequence (`<rdf:li>` entries), empty when absent.
fn seq_pairs(doc: &str, name: &str) -> Vec<[f32; 2]> {
    let open = format!("<crs:{name}");
    let close = format!("</crs:{name}>");
    let Some(start) = doc.find(&open) else { return vec![] };
    let region = doc.get(start..).and_then(|r| r.find(&close).map(|e| &r[..e])).unwrap_or("");
    let mut out = vec![];
    let mut rest = region;
    while let Some(i) = rest.find("<rdf:li>") {
        let after = &rest[i + "<rdf:li>".len()..];
        let Some(end) = after.find("</rdf:li>") else { break };
        let text = &after[..end];
        if let Some((x, y)) = text.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))) {
            out.push([x, y]);
        }
        rest = &after[end..];
    }
    out
}

/// Parse Camera Raw settings from an XMP packet. Unknown fields are ignored; fields that are
/// absent keep their default. Returns an error only when the text is not XMP at all.
pub fn from_xmp(doc: &str) -> Result<CameraRaw, String> {
    if !doc.contains("x:xmpmeta") && !doc.contains("rdf:RDF") {
        return Err("not an XMP packet".into());
    }
    let mut p = CameraRaw::default();
    let set = |name: &str, dst: &mut f32| {
        if let Some(v) = attr(doc, name) {
            *dst = v;
        }
    };
    // Light
    set("Exposure2012", &mut p.exposure);
    set("Contrast2012", &mut p.contrast);
    set("Highlights2012", &mut p.highlights);
    set("Shadows2012", &mut p.shadows);
    set("Whites2012", &mut p.whites);
    set("Blacks2012", &mut p.blacks);
    set("Texture", &mut p.texture);
    set("Clarity2012", &mut p.clarity);
    set("Dehaze", &mut p.dehaze);
    set("Vibrance", &mut p.vibrance);
    set("Saturation", &mut p.saturation);
    // Parametric tone curve
    set("ParametricShadows", &mut p.curve_shadows);
    set("ParametricDarks", &mut p.curve_darks);
    set("ParametricLights", &mut p.curve_lights);
    set("ParametricHighlights", &mut p.curve_highlights);
    set("ParametricShadowSplit", &mut p.curve_splits[0]);
    set("ParametricMidtoneSplit", &mut p.curve_splits[1]);
    set("ParametricHighlightSplit", &mut p.curve_splits[2]);
    // Detail
    set("Sharpness", &mut p.sharpen_amount);
    set("SharpenRadius", &mut p.sharpen_radius);
    set("SharpenDetail", &mut p.sharpen_detail);
    set("SharpenEdgeMasking", &mut p.sharpen_masking);
    set("LuminanceSmoothing", &mut p.noise_luminance);
    set("LuminanceNoiseReductionDetail", &mut p.noise_luminance_detail);
    set("ColorNoiseReduction", &mut p.noise_color);
    set("ColorNoiseReductionDetail", &mut p.noise_color_detail);
    // Effects
    set("GrainAmount", &mut p.grain_amount);
    set("GrainSize", &mut p.grain_size);
    set("GrainFrequency", &mut p.grain_roughness);
    set("PostCropVignetteAmount", &mut p.vignette_amount);
    set("PostCropVignetteMidpoint", &mut p.vignette_midpoint);
    set("PostCropVignetteRoundness", &mut p.vignette_roundness);
    set("PostCropVignetteFeather", &mut p.vignette_feather);
    set("PostCropVignetteHighlightContrast", &mut p.vignette_highlights);
    if let Some(v) = attr(doc, "PostCropVignetteStyle") {
        p.vignette_style = vignette_style_name(v);
    }
    // Color Mixer (HSL)
    for (k, band) in BANDS.iter().enumerate() {
        if let Some(v) = attr(doc, &format!("HueAdjustment{band}")) {
            p.hsl_hue[k] = v;
        }
        if let Some(v) = attr(doc, &format!("SaturationAdjustment{band}")) {
            p.hsl_sat[k] = v;
        }
        if let Some(v) = attr(doc, &format!("LuminanceAdjustment{band}")) {
            p.hsl_lum[k] = v;
        }
    }
    // Color Grading
    for (prefix, dst) in [
        ("ColorGradeShadow", &mut p.grade_shadows),
        ("ColorGradeMidtone", &mut p.grade_midtones),
        ("ColorGradeHighlight", &mut p.grade_highlights),
        ("ColorGradeGlobal", &mut p.grade_global),
    ] {
        if let Some(v) = attr(doc, &format!("{prefix}Hue")) {
            dst.hue = v;
        }
        if let Some(v) = attr(doc, &format!("{prefix}Sat")) {
            dst.sat = v;
        }
        if let Some(v) = attr(doc, &format!("{prefix}Lum")) {
            dst.lum = v;
        }
    }
    set("ColorGradeBlending", &mut p.grade_blending);
    set("ColorGradeBalance", &mut p.grade_balance);
    // Point curves
    p.point_curve = seq_pairs(doc, "ToneCurvePV2012");
    p.point_curve_red = seq_pairs(doc, "ToneCurvePV2012Red");
    p.point_curve_green = seq_pairs(doc, "ToneCurvePV2012Green");
    p.point_curve_blue = seq_pairs(doc, "ToneCurvePV2012Blue");
    // Reject malformed curves rather than importing a corrupt look.
    p.validate()?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_raw::Wheel;

    fn sample() -> CameraRaw {
        let mut hsl_sat = [0.0f32; 8];
        hsl_sat[5] = 12.5;
        CameraRaw {
            exposure: 0.5,
            contrast: -12.0,
            highlights: 30.0,
            texture: 8.0,
            clarity: 5.0,
            dehaze: -3.0,
            vibrance: 15.0,
            saturation: -4.0,
            curve_highlights: 20.0,
            curve_splits: [20.0, 55.0, 80.0],
            point_curve: vec![[0.0, 0.0], [64.0, 80.0], [255.0, 255.0]],
            point_curve_red: vec![[0.0, 0.0], [128.0, 140.0], [255.0, 255.0]],
            hsl_hue: [-10.0, 0.0, 5.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            hsl_sat,
            grade_shadows: Wheel { hue: 210.0, sat: 20.0, lum: -10.0 },
            grade_blending: 40.0,
            grade_balance: -5.0,
            sharpen_amount: 45.0,
            sharpen_radius: 1.2,
            noise_luminance: 12.0,
            noise_color: 25.0,
            grain_amount: 18.0,
            vignette_amount: -20.0,
            vignette_style: "colorPriority".into(),
            ..Default::default()
        }
    }

    #[test]
    fn round_trips_every_mapped_field() {
        let p = sample();
        let xmp = to_xmp(&p);
        let back = from_xmp(&xmp).unwrap();
        // Everything mapped compares equal; the two unmapped internal fields are untouched.
        let mut want = p.clone();
        want.temperature = back.temperature;
        want.tint = back.tint;
        want.seed = back.seed;
        want.pixel_scale = back.pixel_scale;
        want.encoding_gamma = back.encoding_gamma;
        assert_eq!(back, want);
        assert_eq!(back.vignette_style, "colorPriority");
        assert_eq!(back.point_curve, p.point_curve);
        assert_eq!(back.grade_shadows, p.grade_shadows);
    }

    #[test]
    fn defaults_round_trip_to_defaults() {
        let back = from_xmp(&to_xmp(&CameraRaw::default())).unwrap();
        // The parametric splits default to 25/50/75 and are written, so they survive; the
        // internal fields keep their own defaults.
        assert_eq!(back.curve_splits, [25.0, 50.0, 75.0]);
        assert_eq!(back.exposure, 0.0);
        assert!(back.point_curve.is_empty());
    }

    #[test]
    fn reads_a_minimal_adobe_packet() {
        let doc = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
              crs:Exposure2012="+1.25" crs:Contrast2012="-30" crs:Vibrance="20"
              crs:PostCropVignetteStyle="2"></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let p = from_xmp(doc).unwrap();
        assert_eq!(p.exposure, 1.25);
        assert_eq!(p.contrast, -30.0);
        assert_eq!(p.vibrance, 20.0);
        assert_eq!(p.vignette_style, "paintOverlay");
    }

    #[test]
    fn hostile_input_is_rejected_not_panicking() {
        assert!(from_xmp("").is_err());
        assert!(from_xmp("not xmp at all").is_err());
        // A malformed curve is rejected (validate), never imported as a corrupt look.
        let bad =
            to_xmp(&CameraRaw::default()).replace("<crs:ToneCurvePV2012>", "<crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>not a pair</rdf:li>");
        // The injected non-pair is dropped by the parser, so this still parses cleanly.
        assert!(from_xmp(&bad).is_ok());
        // A curve with decreasing inputs is rejected by validate().
        let p = CameraRaw { point_curve: vec![[0.0, 0.0], [10.0, 5.0], [5.0, 9.0]], ..Default::default() };
        let doc = to_xmp(&p);
        assert!(from_xmp(&doc).is_err());
    }
}
