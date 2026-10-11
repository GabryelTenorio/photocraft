//! `photocraft-ml`: optional on-device machine-learning models.
//!
//! Inference is pure Rust ([tract](https://github.com/sonos/tract) running ONNX graphs): no C++
//! runtime, nothing leaves the machine. The model weights are not in the repository; installers
//! ship them next to the executable (see [`model_dirs`]) and a missing or damaged file is a clear
//! [`MlError`], never a panic.
//!
//! Models:
//! * [`SUBJECT_MODEL`]: BiRefNet-lite (Zheng et al., "Bilateral Reference for High-Resolution
//!   Dichotomous Image Segmentation", CAAI AIR 2024), MIT licence, for Select Subject's high
//!   quality mode. Input 1024 × 1024 RGB (ImageNet normalisation), output one logit per pixel.
//!
//! The API is plain data (RGB samples in, probabilities out) so this crate knows nothing about
//! documents; `photocraft-engine` adapts it behind its `ml` feature.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

#[cfg(not(target_arch = "wasm32"))]
use tract_onnx::prelude::*;

/// A model file PhotoCraft knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    /// File name in a models directory.
    pub file: &'static str,
    /// Exact size in bytes (a cheap check against a truncated or wrong file).
    pub bytes: u64,
    /// SHA-256 of the file, checked by packaging when it fetches the file.
    pub sha256: &'static str,
    /// Where packaging fetches it.
    pub url: &'static str,
}

/// BiRefNet-lite, fp32 ONNX export by onnx-community (weights unchanged from ZhengPeng7/BiRefNet).
pub const SUBJECT_MODEL: ModelInfo = ModelInfo {
    file: "birefnet-lite.onnx",
    bytes: 224_005_088,
    sha256: "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333",
    url: "https://huggingface.co/onnx-community/BiRefNet_lite-ONNX/resolve/main/onnx/model.onnx",
};

/// Side of the square image [`SubjectModel`] works on.
pub const SUBJECT_INPUT: usize = 1024;

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Why a model could not run.
#[derive(Debug, thiserror::Error)]
pub enum MlError {
    #[error("the AI model `{file}` is not installed (looked in: {looked})")]
    Missing { file: &'static str, looked: String },
    #[error("the AI model `{file}` is damaged or not the expected version: {why}")]
    Invalid { file: String, why: String },
    #[error("AI models are not available in this build")]
    Unsupported,
    #[error("AI inference failed: {0}")]
    Inference(String),
    #[error("bad input: {0}")]
    BadInput(String),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, MlError>;

/// The directories searched for model files, in order: `$PHOTOCRAFT_MODELS_DIR`, then `models`
/// next to the executable (where the installers put them).
pub fn model_dirs() -> Vec<PathBuf> {
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut dirs = Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(d) = std::env::var_os("PHOTOCRAFT_MODELS_DIR") {
            dirs.push(PathBuf::from(d));
        }
        if let Some(d) = std::env::current_exe().ok().as_deref().and_then(Path::parent) {
            dirs.push(d.join("models"));
        }
    }
    dirs
}

/// The first `dirs` entry holding `info`'s file.
pub fn locate(info: &ModelInfo, dirs: &[PathBuf]) -> Result<PathBuf> {
    dirs.iter().map(|d| d.join(info.file)).find(|p| p.is_file()).ok_or_else(|| MlError::Missing {
        file: info.file,
        looked: if dirs.is_empty() { "nowhere: no models directory on this platform".into() } else { dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ") },
    })
}

#[cfg(not(target_arch = "wasm32"))]
type Plan = Arc<TypedRunnableModel>;
#[cfg(target_arch = "wasm32")]
type Plan = ();

/// The subject segmentation model, ready to run.
pub struct SubjectModel {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    plan: Plan,
}

impl std::fmt::Debug for SubjectModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SubjectModel")
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn invalid(file: &str, why: impl std::fmt::Display) -> MlError {
    MlError::Invalid { file: file.to_string(), why: why.to_string() }
}

/// Runs `f`, turning a panic inside tract (a malformed graph) into an error.
#[cfg(not(target_arch = "wasm32"))]
fn guarded<T>(what: &str, f: impl FnOnce() -> TractResult<T>) -> std::result::Result<T, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(format!("{what}: {e:#}")),
        Err(_) => Err(format!("{what}: internal error in the inference engine")),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn use_all_cores() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let n = std::thread::available_parallelism().map_or(1, |n| n.get());
        tract_linalg::multithread::set_default_executor(tract_linalg::multithread::Executor::multithread(n));
    });
}

impl SubjectModel {
    /// Parses and optimises an ONNX graph (slow: seconds to a minute for BiRefNet-lite).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_bytes(name: &str, bytes: &[u8]) -> Result<Self> {
        use_all_cores();
        let n = SUBJECT_INPUT;
        let plan = guarded("loading", || {
            tract_onnx::onnx()
                .model_for_read(&mut std::io::Cursor::new(bytes))?
                .with_input_fact(0, f32::fact([1, 3, n, n]).into())?
                .into_optimized()?
                .into_runnable()
        })
        .map_err(|e| invalid(name, e))?;
        Ok(Self { plan })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn from_bytes(_: &str, _: &[u8]) -> Result<Self> {
        Err(MlError::Unsupported)
    }

    /// Loads `path`, checking its size against [`SUBJECT_MODEL`] first.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_path(path: &Path) -> Result<Self> {
        let name = path.display().to_string();
        let len = std::fs::metadata(path).map_err(|e| invalid(&name, e))?.len();
        if len != SUBJECT_MODEL.bytes {
            return Err(invalid(&name, format!("{len} bytes, expected {}", SUBJECT_MODEL.bytes)));
        }
        let bytes = std::fs::read(path).map_err(|e| invalid(&name, e))?;
        Self::from_bytes(&name, &bytes)
    }

    #[cfg(target_arch = "wasm32")]
    pub fn from_path(_: &Path) -> Result<Self> {
        Err(MlError::Unsupported)
    }

    /// The installed model, loaded once per process and shared (loading is the slow part).
    pub fn shared() -> Result<Arc<Self>> {
        static CACHE: Mutex<Option<Arc<SubjectModel>>> = Mutex::new(None);
        // Held while loading, so two callers don't load twice.
        let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(m) = cache.as_ref() {
            return Ok(Arc::clone(m));
        }
        let m = Arc::new(Self::from_path(&locate(&SUBJECT_MODEL, &model_dirs())?)?);
        *cache = Some(Arc::clone(&m));
        Ok(m)
    }

    /// Foreground probability (0–1) of every pixel of a `SUBJECT_INPUT`² resampling of `rgb`
    /// (`w × h`, row-major, samples in 0–1), row-major. `cancelled` is polled around the run.
    #[cfg(target_arch = "wasm32")]
    pub fn predict(&self, _: &[[f32; 3]], _: usize, _: usize, _: &dyn Fn() -> bool) -> Result<Vec<f32>> {
        Err(MlError::Unsupported)
    }

    /// Foreground probability (0–1) of every pixel of a `SUBJECT_INPUT`² resampling of `rgb`
    /// (`w × h`, row-major, samples in 0–1), row-major. `cancelled` is polled around the run.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn predict(&self, rgb: &[[f32; 3]], w: usize, h: usize, cancelled: &dyn Fn() -> bool) -> Result<Vec<f32>> {
        if w == 0 || h == 0 || w.checked_mul(h) != Some(rgb.len()) {
            return Err(MlError::BadInput(format!("{} samples for a {w} × {h} image", rgb.len())));
        }
        let n = SUBJECT_INPUT;
        let input = normalised_input(rgb, w, h, n);
        if cancelled() {
            return Err(MlError::Cancelled);
        }
        let tensor: Tensor = Tensor::from_shape(&[1, 3, n, n], &input).map_err(|e| MlError::Inference(e.to_string()))?;
        let out = guarded("inference", || self.plan.run(tvec!(tensor.into()))).map_err(MlError::Inference)?;
        if cancelled() {
            return Err(MlError::Cancelled);
        }
        let logits = out.first().ok_or_else(|| MlError::Inference("the model returned nothing".into()))?;
        let view = logits.to_plain_array_view::<f32>().map_err(|e| MlError::Inference(e.to_string()))?;
        if view.len() != n * n {
            return Err(MlError::Inference(format!("unexpected output shape {:?}", view.shape())));
        }
        Ok(view.iter().map(|v| sigmoid(*v)).collect())
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn sigmoid(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { 1.0 / (1.0 + (-v).exp()) }
}

/// Bilinear resampling of `rgb` to `n × n`, normalised and laid out as planar CHW.
pub fn normalised_input(rgb: &[[f32; 3]], w: usize, h: usize, n: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; 3 * n * n];
    let at = |x: usize, y: usize| rgb.get(y.min(h - 1) * w + x.min(w - 1)).copied().unwrap_or_default();
    let (sx, sy) = (w as f32 / n as f32, h as f32 / n as f32);
    for oy in 0..n {
        let fy = ((oy as f32 + 0.5) * sy - 0.5).max(0.0);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        for ox in 0..n {
            let fx = ((ox as f32 + 0.5) * sx - 0.5).max(0.0);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
            for k in 0..3 {
                let top = a[k] + (b[k] - a[k]) * tx;
                let bot = c[k] + (d[k] - c[k]) * tx;
                let v = (top + (bot - top) * ty).clamp(0.0, 1.0);
                if let Some(o) = out.get_mut(k * n * n + oy * n + ox) {
                    *o = (v - MEAN[k]) / STD[k];
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalised_input_resamples_and_normalises() {
        // A 2 × 1 image, black then white, to 4 × 4: left half dark, right half bright.
        let v = normalised_input(&[[0.0; 3], [1.0; 3]], 2, 1, 4);
        assert_eq!(v.len(), 48);
        assert!((v[0] - (0.0 - MEAN[0]) / STD[0]).abs() < 1e-5);
        assert!((v[3] - (1.0 - MEAN[0]) / STD[0]).abs() < 1e-5);
        assert!(v[1] < v[2], "bilinear ramp in the middle");
        // Green plane uses its own mean.
        assert!((v[16] - (0.0 - MEAN[1]) / STD[1]).abs() < 1e-5);
    }

    #[test]
    fn garbage_and_missing_models_are_errors() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            assert!(matches!(SubjectModel::from_bytes("junk", b"not an onnx file"), Err(MlError::Invalid { .. })));
            assert!(matches!(SubjectModel::from_bytes("empty", &[]), Err(MlError::Invalid { .. })));
        }
        let e = locate(&SUBJECT_MODEL, &[PathBuf::from("definitely/not/here")]).unwrap_err();
        assert!(matches!(e, MlError::Missing { .. }) && e.to_string().contains("definitely"), "{e}");
        assert!(matches!(locate(&SUBJECT_MODEL, &[]), Err(MlError::Missing { .. })));
        #[cfg(not(target_arch = "wasm32"))]
        assert!(matches!(SubjectModel::from_path(Path::new("definitely/not/here.onnx")), Err(MlError::Invalid { .. })));
    }

    #[test]
    fn sigmoid_is_safe() {
        assert_eq!(sigmoid(f32::NAN), 0.0);
        assert!(sigmoid(1e9) > 0.999 && sigmoid(-1e9) < 1e-3);
    }

    /// The real model on a synthetic scene: a red disc on blue. Needs the model file
    /// (`PHOTOCRAFT_MODELS_DIR` or `models/` next to the test binary); slow (about a minute).
    #[test]
    #[ignore = "needs the BiRefNet-lite model file; slow"]
    fn subject_model_finds_a_disc() {
        let model = SubjectModel::shared().unwrap();
        let (w, h) = (300, 200);
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - 150.0, (i / w) as f32 - 100.0);
                if x * x + y * y < 60.0 * 60.0 { [0.9, 0.15, 0.1] } else { [0.2, 0.4, 0.8] }
            })
            .collect();
        let p = model.predict(&rgb, w, h, &|| false).unwrap();
        let n = SUBJECT_INPUT;
        assert!(p[(n / 2) * n + n / 2] > 0.5, "centre");
        assert!(p[10 * n + 10] < 0.5, "corner");
    }
}
