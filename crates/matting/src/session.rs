//! ONNX Runtime session management: provider selection with an explicit,
//! user-visible fallback chain (DirectML → CPU), model discovery, and a
//! process-wide session cache so a 1 GB model loads once per PhotoCraft run.
//!
//! The design follows the crate-level contract in `lib.rs`: never a silent
//! fallback — the returned [`CutoutResult`] always says which provider served
//! the request and on which adapter, and failure of *both* providers surfaces
//! as [`MattingError::NoProvider`] with both error strings.

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
};

use crate::{
    device::{self, GpuInfo},
    error::{MattingError, Result},
    postprocess::build_alpha,
    preprocess::build_input,
};

/// Which model export to load. RMBG-2.0 ships at 1024; the 2048 variant is a
/// sharper BiRefNet export some users keep next to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelVariant {
    /// RMBG-2.0 (BRIA), 1024×1024 — the default and the fast one.
    #[default]
    Rmbg2,
    /// 2048×2048 sharp export (file `RMBG-2.0-2048.onnx`), ~4× the compute.
    Rmbg2Sharp,
}

impl ModelVariant {
    pub fn resolution(self) -> u32 {
        match self {
            Self::Rmbg2 => crate::DEFAULT_RESOLUTION,
            Self::Rmbg2Sharp => 2048,
        }
    }

    fn file_names(self) -> [&'static str; 2] {
        match self {
            Self::Rmbg2 => ["RMBG-2.0.onnx", "rmbg-2.0.onnx"],
            Self::Rmbg2Sharp => ["RMBG-2.0-2048.onnx", "rmbg-2.0-2048.onnx"],
        }
    }
}

/// Device preference for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DevicePref {
    /// DirectML on the best adapter when one exists, else CPU.
    #[default]
    Auto,
    /// Force the CPU provider (also the escape hatch when a GPU misbehaves).
    Cpu,
    /// Force the DirectML device id (the index from [`crate::enumerate_gpus`]).
    Gpu(u32),
}

/// Inference options; defaults follow the crate docs.
#[derive(Debug, Clone)]
pub struct CutoutOptions {
    pub variant: ModelVariant,
    pub device: DevicePref,
}

impl Default for CutoutOptions {
    fn default() -> Self {
        Self { variant: ModelVariant::default(), device: DevicePref::default() }
    }
}

/// What one [`cutout`] call reports back.
#[derive(Debug, Clone)]
pub struct CutoutResult {
    /// `width * height` alpha values, 255 = keep.
    pub alpha: Vec<u8>,
    /// `"DirectML"` or `"CPU"` — which provider actually served the run.
    pub provider: String,
    /// The adapter used, when a GPU served the run.
    pub gpu: Option<GpuInfo>,
    /// Wall-clock milliseconds of preprocess + inference + postprocess.
    pub ms: u128,
}

/// Where the model file is searched: next to the executable (portable), the
/// data dir a deploy script creates, then the user cache.
pub fn model_candidates(variant: ModelVariant) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(d) = exe_dir {
        roots.push(d.join("PhotoCraftData").join("models").join("rmbg-2.0"));
        roots.push(d.join("models").join("rmbg-2.0"));
    }
    if let Ok(c) = std::env::var("LOCALAPPDATA") {
        roots.push(Path::new(&c).join("PhotoCraft").join("models").join("rmbg-2.0"));
    }
    if let Ok(extra) = std::env::var("PHOTOCRAFT_MODEL_DIR") {
        roots.push(PathBuf::from(extra).join("rmbg-2.0"));
    }
    for name in variant.file_names() {
        for root in &roots {
            out.push(root.join(name));
        }
    }
    out
}

fn find_model(variant: ModelVariant) -> Result<PathBuf> {
    let searched = model_candidates(variant);
    for p in &searched {
        if p.is_file() {
            return Ok(p.clone());
        }
    }
    Err(MattingError::ModelNotFound { searched: searched.iter().map(|p| p.display().to_string()).collect() })
}

// ---- session cache -------------------------------------------------------

struct Cached {
    key: (PathBuf, u32, u8),
    session: ort::session::Session,
    provider: String,
    gpu: Option<GpuInfo>,
}

static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();

/// Release the cached session (call on application shutdown so ONNX Runtime's
/// teardown happens before process teardown, never on a lazy static drop).
pub fn release() {
    if let Some(c) = CACHE.get() {
        if let Ok(mut guard) = c.lock() {
            *guard = None;
        }
    }
}

/// Human-readable runtime report for the command's status output.
pub fn ort_status() -> String {
    format!("ONNX Runtime {}", ort::info())
}

fn cache() -> &'static Mutex<Option<Cached>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn device_tag(d: DevicePref) -> u8 {
    match d {
        DevicePref::Auto => 0,
        DevicePref::Cpu => 1,
        DevicePref::Gpu(id) => 2 + id as u8,
    }
}

/// Open (or reuse) a session for `variant` honouring `device`, returning the
/// live session, which provider served it, and the adapter when a GPU did.
/// The caller holds the cache lock; this only builds.
fn session_for(path: &Path, variant: ModelVariant, device: DevicePref) -> Result<(ort::session::Session, String, Option<GpuInfo>)> {
    let _ = variant;
    build_session(path, variant, device)
}

/// The process-wide ONNX Runtime environment (created lazily on first use).
/// With `load-dynamic` this loads `onnxruntime.dll` from the executable's
/// directory; failure is remembered so we never retry-load every call.
fn env() -> Result<ort::environment::Environment> {
    static ENV: std::sync::OnceLock<Option<ort::environment::Environment>> = std::sync::OnceLock::new();
    let slot = ENV.get_or_init(|| ort::init().build().ok());
    slot.clone().ok_or(MattingError::OrtLibraryMissing)
}

fn build_session(path: &Path, _variant: ModelVariant, device: DevicePref) -> Result<(ort::session::Session, String, Option<GpuInfo>)> {
    log::debug!("[matting] building a session (device {device:?})");
    let gpu: Option<GpuInfo> = match device {
        DevicePref::Cpu => None,
        DevicePref::Auto => device::pick_gpu(),
        DevicePref::Gpu(id) => device::enumerate_gpus().into_iter().find(|g| g.index == id),
    };

    if device != DevicePref::Cpu {
        if let Some(g) = &gpu {
            log::debug!("[matting] DirectML candidate: {}", g.describe());
            let attempt = ort::session::Session::builder(&env()?)
                .map_err(|e| MattingError::Session(e.to_string()))?
                .with_execution_providers([ort::ep::DirectML::default().with_device_id(g.index as i32).build()])
                .map_err(|e| MattingError::Session(e.to_string()))?
                .commit_from_file(path)
                .map_err(|e| MattingError::Session(e.to_string()));
            match attempt {
                Ok(s) => return Ok((s, "DirectML".to_owned(), Some(g.clone()))),
                Err(dml_err) => {
                    // DirectML failed (missing DLL, device id, driver) — the
                    // CPU fallback is honest only if we say why we fell back.
                    log::warn!("DirectML session failed ({dml_err}); falling back to CPU");
                    let cpu_err = match cpu_session(path) {
                        Ok(s) => return Ok((s, "CPU".to_owned(), None)),
                        Err(e) => e,
                    };
                    return Err(MattingError::NoProvider { detail: format!("DirectML: {dml_err}; CPU: {cpu_err}") });
                }
            }
        }
    }
    match cpu_session(path) {
        Ok(s) => Ok((s, "CPU".to_owned(), None)),
        Err(e) => Err(match device {
            DevicePref::Auto if gpu.is_none() => e,
            _ => e,
        }),
    }
}

fn cpu_session(path: &Path) -> std::result::Result<ort::session::Session, MattingError> {
    log::debug!("[matting] building the CPU session");
    ort::session::Session::builder(&env()?)
        .map_err(|e| MattingError::Session(e.to_string()))?
        .with_execution_providers([ort::ep::CPU::default().build()])
        .map_err(|e| MattingError::Session(e.to_string()))?
        .commit_from_file(path)
        .map_err(|e| MattingError::Session(e.to_string()))
}

// ---- the one entry point ---------------------------------------------------

/// Run RMBG-2.0 over straight RGBA8 pixels and return the alpha matte plus
/// the provenance of the run (provider, adapter, timings).
///
/// A GPU session is serialised through the cache lock: one 200 M-parameter
/// model per process is enough, and ONNX Runtime sessions are thread-safe for
/// `run` — the lock protects the *cache*, letting concurrent callers share the
/// session they find there.
pub fn cutout(rgba: &[u8], width: u32, height: u32, opts: &CutoutOptions) -> Result<CutoutResult> {
    let start = Instant::now();
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected {
        return Err(MattingError::BadBuffer { expected, got: rgba.len(), width, height, channels: 4 });
    }
    let path = find_model(opts.variant)?;
    let (input, padding) = build_input(rgba, width, height, opts.variant.resolution());

    let mut guard = cache().lock().map_err(|_| MattingError::Session("cache poisoned".to_owned()))?;
    let key = (path.clone(), opts.variant.resolution(), device_tag(opts.device));
    let cache_hit = guard.as_ref().is_some_and(|c| c.key == key);
    if !cache_hit {
        let (session, provider, gpu) = session_for(&path, opts.variant, opts.device)?;
        *guard = Some(Cached { key: key.clone(), session, provider: provider.clone(), gpu: gpu.clone() });
    }
    let cached = guard.as_mut().ok_or_else(|| MattingError::Session("cache empty after fill".to_owned()))?;

    let (shape, logits) = {
        use ort::value::Tensor;
        let side = opts.variant.resolution() as usize;
        let tensor = Tensor::from_array(([1usize, 3, side, side], input.into_boxed_slice()))
            .map_err(|e| MattingError::Inference(e.to_string()))?;
        let outputs = cached
            .session
            .run(ort::inputs![tensor])
            .map_err(|e| MattingError::Inference(e.to_string()))?;
        if outputs.len() == 0 {
            return Err(MattingError::Inference("model returned no outputs".to_owned()));
        }
        let ref out = outputs[0];
        let (_shape, data) = out.try_extract_tensor::<f32>().map_err(|e| MattingError::Inference(e.to_string()))?;
        // Copy: the output buffer lives inside the Value we drop next.
        let owned = data.to_vec();
        (owned.len(), owned)
    };
    let side = opts.variant.resolution() as usize;
    if shape != side * side {
        // Some exports emit 1×S×S, others 1×1×S×S; the plane is the same.
        return Err(MattingError::Inference(format!("unexpected output plane: {shape} samples, expected {}", side * side)));
    }
    let alpha = build_alpha(&logits, &padding, width, height);
    Ok(CutoutResult {
        alpha,
        provider: cached.provider.clone(),
        gpu: cached.gpu.clone(),
        ms: start.elapsed().as_millis(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_search_is_deterministic_and_named() {
        let c = model_candidates(ModelVariant::Rmbg2);
        assert!(c.iter().any(|p| p.file_name().is_some_and(|f| f == "RMBG-2.0.onnx")));
        assert_eq!(c.len(), model_candidates(ModelVariant::Rmbg2Sharp).len());
    }

    #[test]
    fn bad_buffer_is_a_value_not_a_panic() {
        let e = cutout(&[0u8; 3], 2, 2, &CutoutOptions::default()).unwrap_err();
        assert!(matches!(e, MattingError::BadBuffer { .. }), "{e}");
    }
}
