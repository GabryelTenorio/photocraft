//! Explicit, bounded SVG clipboard import. Preparation never changes a document; confirmation
//! applies one edit using the existing SVG importer and cross-document colour conversion.

use std::sync::Arc;

use photocraft_color::{BlendMode, ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer, LayerContent, MAX_GROUP_DEPTH, NamedPath, Path, SmartObject, SmartSource};
use photocraft_geom::{Affine, Size};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const CMD: &str = "edit.pasteVector";
pub const MAX_VECTOR_CLIPBOARD_BYTES: usize = 16 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteMode {
    Layers,
    SmartObject,
    Pixels,
    Path,
    Shape,
}

impl PasteMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Layers => "layers",
            Self::SmartObject => "smartObject",
            Self::Pixels => "pixels",
            Self::Path => "path",
            Self::Shape => "shape",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Layers => "Layers",
            Self::SmartObject => "Smart Object",
            Self::Pixels => "Pixels",
            Self::Path => "Path",
            Self::Shape => "Shape Layer",
        }
    }

    fn parse(value: &Value) -> Result<Self> {
        match value.as_str() {
            Some("layers") => Ok(Self::Layers),
            Some("smartObject") => Ok(Self::SmartObject),
            Some("pixels") => Ok(Self::Pixels),
            Some("path") => Ok(Self::Path),
            Some("shape") => Ok(Self::Shape),
            _ => Err(bad("`mode` must be layers, smartObject, pixels, path or shape")),
        }
    }
}

/// An immutable parsed snapshot. Reusing it does not share imported layer identities.
#[derive(Clone, Debug)]
pub struct PreparedSvg {
    source: Arc<Vec<u8>>,
    document: Document,
    warnings: Vec<String>,
    path_capability: std::result::Result<(), String>,
    shape_capability: std::result::Result<(), String>,
}

impl PreparedSvg {
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn size(&self) -> [u32; 2] {
        [self.document.size.width, self.document.size.height]
    }

    pub fn capability(&self, mode: PasteMode) -> std::result::Result<(), String> {
        match mode {
            PasteMode::Layers => Ok(()),
            PasteMode::Pixels | PasteMode::SmartObject => {
                if self.document.size.area() > 24_000_000 {
                    Err("The SVG is too large to rasterise (24 million pixel limit)".into())
                } else {
                    Ok(())
                }
            }
            PasteMode::Path => self.path_capability.clone(),
            PasteMode::Shape => self.shape_capability.clone(),
        }
    }

    /// Destination-dependent availability for ordinary centred paste. Confirmation checks the
    /// actual requested centre again, so an explicit placement cannot bypass the budget.
    pub fn capability_on_canvas(&self, mode: PasteMode, size: Size) -> std::result::Result<(), String> {
        self.capability_at(mode, size, [f64::from(size.width) / 2.0, f64::from(size.height) / 2.0])
    }

    /// Availability for a captured placement, computed once before showing the dialog.
    pub fn capability_at(&self, mode: PasteMode, size: Size, center: [f64; 2]) -> std::result::Result<(), String> {
        self.capability(mode)?;
        if matches!(mode, PasteMode::Layers | PasteMode::Shape) {
            let dx = (center[0] - f64::from(self.document.size.width) / 2.0).round() as i32;
            let dy = (center[1] - f64::from(self.document.size.height) / 2.0).round() as i32;
            destination_cache_budget(self, mode, size, dx, dy)?;
        }
        Ok(())
    }
}

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: CMD.into(), msg: msg.into() }
}

pub fn prepare_svg(bytes: &[u8]) -> Result<PreparedSvg> {
    if bytes.is_empty() || bytes.len() > MAX_VECTOR_CLIPBOARD_BYTES {
        return Err(bad("SVG clipboard data must contain 1 byte to 16 MiB"));
    }
    if !photocraft_io::svg::is_svg(bytes) {
        return Err(bad("clipboard data is not an SVG document"));
    }
    let mut imported = photocraft_io::svg::import_svg_bounded("Clipboard.svg", bytes, 24_000_000)
        .map_err(|e| EngineError::Other(format!("can't import clipboard SVG: {e}")))?;
    if imported.document.layers.is_empty() {
        return Err(bad("the SVG contains no drawable objects"));
    }
    // SVG paints and resvg pixels are sRGB, independent of the user's working RGB space.
    imported.document.icc_profile = Some(photocraft_cms::Builtin::Srgb.profile().to_bytes());
    if bytes.split(|b| *b == b'<').skip(1).any(|element| {
        let name = element.split(|b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/')).next().unwrap_or_default();
        name.rsplit(|b| *b == b':').next() == Some(b"text".as_slice())
    }) {
        imported.warnings.push("SVG: text is imported as outlines, not editable text".into());
    }
    let path_capability = combined_path(&imported.document).map(|_| ());
    let shape_capability = single_shape(&imported.document).map(|_| ());
    Ok(PreparedSvg { source: Arc::new(bytes.to_vec()), document: imported.document, warnings: imported.warnings, path_capability, shape_capability })
}

fn destination_cache_budget(prepared: &PreparedSvg, _mode: PasteMode, size: Size, dx: i32, dy: i32) -> std::result::Result<(), String> {
    check_destination_shapes(&prepared.document.layers, Affine::translate(f64::from(dx), f64::from(dy)), size)
}

/// Preflight transformed SVG shape caches before an operation refreshes their raster caches.
pub(crate) fn check_destination_shapes(layers: &[Layer], transform: Affine, size: Size) -> std::result::Result<(), String> {
    let canvas = photocraft_geom::Rect::from_size(size);
    let mut pixels = 0u64;
    let mut pending: Vec<&Layer> = layers.iter().collect();
    while let Some(layer) = pending.pop() {
        pending.extend(layer.children().into_iter().flatten());
        let LayerContent::Shape(shape) = &layer.content else { continue };
        let mut shape = shape.clone();
        crate::vector_cmds::transform_shape(&mut shape, &transform);
        let compiled = photocraft_vector::CompiledShape::new(&shape, photocraft_vector::DEFAULT_TOLERANCE, canvas);
        let area = compiled.bounds().map_or(0, |bounds| bounds.intersect(&canvas).size().area());
        pixels = pixels.checked_add(area).ok_or("The pasted shape caches exceed the clipboard cache budget; use Pixels or Smart Object")?;
        if pixels > 24_000_000 {
            return Err("The pasted shape caches exceed the clipboard cache budget; use Pixels or Smart Object".into());
        }
    }
    Ok(())
}

fn combined_path(doc: &Document) -> std::result::Result<Path, String> {
    let mut result: Option<Path> = None;
    for (_, _, layer) in doc.walk() {
        match &layer.content {
            LayerContent::Group(_) => {}
            LayerContent::Shape(shape) => {
                if let Some(path) = &mut result {
                    if path.fill_rule != shape.path.fill_rule || path.inverted != shape.path.inverted {
                        return Err("The SVG has incompatible fill rules; use Layers or Smart Object".into());
                    }
                    path.subpaths.extend(shape.path.subpaths.clone());
                } else {
                    result = Some(shape.path.clone());
                }
            }
            _ => return Err("The SVG contains rasterised objects without editable paths".into()),
        }
    }
    result.filter(|p| !p.is_empty()).ok_or_else(|| "The SVG contains no editable paths".into())
}

fn single_shape(doc: &Document) -> std::result::Result<&Layer, String> {
    let mut found = None;
    for (_, _, layer) in doc.walk() {
        match &layer.content {
            LayerContent::Group(_)
                if layer.opacity == 1.0 && layer.vector_mask.is_none() && matches!(layer.blend, BlendMode::Normal | BlendMode::PassThrough) => {}
            LayerContent::Shape(_) if found.is_none() => found = Some(layer),
            _ => return Err("Shape Layer requires one shape without group blending; use Layers to preserve this SVG".into()),
        }
    }
    found.ok_or_else(|| "The SVG contains no editable shape".into())
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    let state = s.active().ok_or("no document open")?;
    if s.tools.mask_targeted
        || state.doc.quick_mask.is_some()
        || state.channel_view.layer_mask.is_some()
        || state.channel_view.target != crate::channel_cmds::ChannelTarget::Composite
    {
        return Err("vector paste requires the layer composite target".into());
    }
    if !crate::layer_copy_cmds::layered(state.doc.mode) {
        return Err("vector paste requires a layered RGB, Grayscale, CMYK or Lab document".into());
    }
    if s.job_on(state.doc.id).is_some() {
        return Err("the document has a running background job".into());
    }
    Ok(())
}

fn params(s: &Session, p: &Value) -> Result<[f64; 2]> {
    if !p.is_object() {
        return Err(bad("parameters must be an object"));
    }
    if let Some(v) = p.get("document") {
        let id = v.as_u64().ok_or_else(|| bad("`document` must be an integer document id"))?;
        if s.active().map(|state| state.doc.id.0) != Some(id) {
            return Err(bad("the destination document is no longer active"));
        }
    }
    if p.get("target").is_some() {
        return Err(bad("vector paste accepts only the current layer composite target"));
    }
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    match p.get("center") {
        None => Ok([f64::from(doc.size.width) / 2.0, f64::from(doc.size.height) / 2.0]),
        Some(v) => {
            let xy = v.as_array().filter(|a| a.len() == 2).and_then(|a| Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?]));
            xy.filter(|a| a.iter().all(|n| n.is_finite() && n.abs() <= 1e7))
                .ok_or_else(|| bad("`center` must contain two finite coordinates within ±10 million pixels"))
        }
    }
}

/// Confirm a prepared clipboard snapshot, preserving masks, channels and the existing work path.
pub fn paste_svg(s: &mut Session, prepared: &PreparedSvg, mode: PasteMode, p: &Value) -> Result<Value> {
    let center = params(s, p)?;
    enabled(s).map_err(EngineError::Other)?;
    prepared.capability(mode).map_err(EngineError::Other)?;
    let state = s.active().ok_or(EngineError::NoDocument)?;
    let dest = state.doc.clone();
    let [w, h] = prepared.size();
    let (dx, dy) = ((center[0] - f64::from(w) / 2.0).round() as i32, (center[1] - f64::from(h) / 2.0).round() as i32);
    if matches!(mode, PasteMode::Layers | PasteMode::Shape) {
        destination_cache_budget(prepared, mode, dest.size, dx, dy).map_err(EngineError::Other)?;
    }
    if mode == PasteMode::Path {
        let path = combined_path(&prepared.document).map_err(EngineError::Other)?.transform(&Affine::translate(f64::from(dx), f64::from(dy)));
        let name = (1..=dest.paths.len().saturating_add(1))
            .map(|i| format!("Pasted Path {i}"))
            .find(|name| !dest.paths.iter().any(|p| &p.name == name))
            .ok_or_else(|| EngineError::Other("no free path name".into()))?;
        s.edit("Paste Vector Path", |doc, _| {
            doc.paths.push(NamedPath { name: name.clone(), path, psd_raw: None });
            Ok(())
        })?;
        return Ok(json!({"mode":mode.as_str(),"path":name,"warnings":prepared.warnings(),"offset":[dx,dy]}));
    }
    // Insertion is beside the active layer. Its own lock (e.g. Background) does not prevent a
    // new sibling, but an enclosing locked group must not be modified.
    let parent = state
        .active_layer
        .and_then(|id| dest.path_of(id))
        .map(|mut path| {
            path.pop();
            path
        })
        .unwrap_or_default();
    let locks = dest.locks_at(&parent);
    if locks.all || locks.pixels || locks.position {
        return Err(EngineError::Other("the destination group is locked".into()));
    }
    let mut copies = prepared.document.clone();
    copies.layers = match mode {
        PasteMode::Layers => copies.layers.iter().map(Layer::duplicate).collect(),
        PasteMode::Shape => vec![single_shape(&prepared.document).map_err(EngineError::Other)?.duplicate()],
        PasteMode::Pixels | PasteMode::SmartObject => {
            let image = crate::smart_cmds::svg_source_image(&prepared.source, PixelFormat::new(ColorMode::Rgb, SampleType::F32, true), 1.0)?;
            copies.depth = SampleType::F32;
            vec![Layer::new("Pasted SVG", LayerContent::Raster((*image.surface).clone()))]
        }
        PasteMode::Path => return Err(bad("path insertion requires a path target")),
    };
    if copies.walk().into_iter().any(|(path, _, _)| parent.len().saturating_add(path.len()) > MAX_GROUP_DEPTH.saturating_add(1)) {
        return Err(EngineError::Other("pasting these groups would exceed the document nesting limit".into()));
    }
    crate::layer_copy_cmds::adapt_copies(s, &mut copies, &dest)?;
    // Moving vectors re-renders their caches. It must use the destination canvas and depth,
    // not clip centred shapes against the clipboard's smaller source canvas.
    copies.size = dest.size;
    copies.depth = dest.depth;
    crate::layer_copy_cmds::translate_copies(&mut copies, dx, dy);
    if dx == 0 && dy == 0 && matches!(mode, PasteMode::Layers | PasteMode::Shape) {
        let snapshot = copies.clone();
        for layer in &mut copies.layers {
            crate::vector_cmds::translate_vectors(&snapshot, layer, 0.0, 0.0);
        }
    }
    if mode == PasteMode::SmartObject {
        let cache = copies.layers.first().and_then(Layer::surface).cloned().ok_or_else(|| EngineError::Other("the SVG could not be rendered".into()))?;
        let source = SmartSource::Embedded { file_name: "Clipboard.svg".into(), bytes: prepared.source.clone() };
        let mut smart = SmartObject::new(source, Affine::translate(f64::from(dx), f64::from(dy)), Some(cache));
        if let Some(rendered) = crate::smart_cmds::render(&dest, &smart)? {
            smart.cache = Some(rendered);
        }
        copies.layers = vec![Layer::new("Pasted SVG", LayerContent::Smart(smart))];
    }
    let ids = s.edit("Paste Vector", |doc, active| {
        let mut above = *active;
        let mut ids = Vec::with_capacity(copies.layers.len());
        for layer in copies.layers {
            let id = doc.insert_above(above, layer);
            above = Some(id);
            ids.push(id);
        }
        *active = above;
        Ok(ids)
    })?;
    crate::layer_multi_cmds::reselect(s, ids.clone(), ids.last().copied());
    Ok(json!({"mode":mode.as_str(),"layers":ids.iter().map(|id|id.0).collect::<Vec<_>>(),"warnings":prepared.warnings(),"offset":[dx,dy]}))
}

fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    let mode = PasteMode::parse(p.get("mode").ok_or_else(|| bad("missing `mode`"))?)?;
    let svg = p.get("svg").and_then(Value::as_str).ok_or_else(|| bad("`svg` must be an SVG string"))?;
    let prepared = prepare_svg(svg.as_bytes())?;
    paste_svg(s, &prepared, mode, p)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Paste Vector",
        menu: &[],
        shortcut: None,
        params: r##"{"svg":"<svg …>…</svg>","mode":"layers|smartObject|pixels|path|shape","document":id? (must be active),"center":[x,y]?=canvas centre} → {mode,layers?|path?,warnings,offset}"##,
        enabled,
        run: paste,
        journal: true,
    }]
}

#[cfg(test)]
mod tests;
