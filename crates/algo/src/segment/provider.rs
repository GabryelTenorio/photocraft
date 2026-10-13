//! Segmentation provider abstraction for Select Subject, Object Selection, and Remove Background.
//!
//! PhotoCraft supports both classical heuristic algorithms and ML model inference through a
//! provider interface. The provider abstraction allows seamless switching between backends:
//!
//! - **Classical**: Fast heuristic algorithms (saliency detection + GrabCut for subjects,
//!   GrabCut for objects) implemented in pure Rust. Always available, no external dependencies.
//! - **Model**: ML model inference through ComfyUI HTTP API (BiRefNet for subjects/background,
//!   SAM 2.1 for objects). Requires a running ComfyUI server with appropriate models installed.
//!   Provides superior quality on photos with hair, fur, thin straps, or low-contrast backgrounds.
//!
//! The backend selection is controlled by a `SegmentationBackend` parameter:
//! - `Auto`: Attempts model inference, falls back to classical on failure
//! - `Classical`: Uses only heuristic algorithms
//! - `Model`: Requires ML backend, errors if unavailable
//!
//! ## Usage
//!
//! ```rust,ignore
//! use photocraft_algo::segment::{SegmentationProvider, SegmentationBackend};
//!
//! // Create a provider (typically from session configuration)
//! let provider = create_provider(SegmentationBackend::Auto, Some("http://127.0.0.1:8188"));
//!
//! // Select subject
//! let region = provider.subject(sampler, canvas)?;
//!
//! // Object selection (requires bounding box or points)
//! let region = provider.object(sampler, canvas, rect)?;
//! ```

use photocraft_geom::Rect;
use super::{Region, Sampler};

#[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
use super::comfyui::ComfyUIClient;

/// Backend selection for segmentation operations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SegmentationBackend {
    /// Attempt model inference, fall back to classical algorithms on failure.
    /// This is the default and recommended setting for most users.
    #[default]
    Auto,
    /// Use only classical heuristic algorithms (saliency detection, GrabCut).
    /// Fast and always available, but lower quality on complex photos.
    Classical,
    /// Require ML model inference. Fails if the backend is unavailable.
    /// Use when you need guaranteed model quality or want to detect configuration issues.
    Model,
}

impl SegmentationBackend {
    /// Parse from a string parameter (command JSON or CLI argument).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "classical" | "classic" | "heuristic" => Some(Self::Classical),
            "model" | "ml" | "ai" => Some(Self::Model),
            _ => None,
        }
    }

    /// Whether this backend allows falling back to classical algorithms.
    pub fn allows_fallback(&self) -> bool {
        matches!(self, Self::Auto)
    }

    /// Whether this backend should attempt model inference.
    pub fn tries_model(&self) -> bool {
        !matches!(self, Self::Classical)
    }
}

/// Segmentation provider trait: the interface between commands and segmentation backends.
///
/// Implementations:
/// - [`ClassicalProvider`]: Pure Rust heuristic algorithms
/// - [`ModelProvider`]: ML model inference through ComfyUI HTTP API
/// - [`AutoProvider`]: Attempts model, falls back to classical
pub trait SegmentationProvider: Send + Sync {
    /// Select the most salient subject in the canvas.
    ///
    /// Returns a mask covering the subject, or `None` if no subject is detected.
    /// Used by the "Select Subject" command and "Remove Background".
    fn subject(&self, sampler: &dyn Sampler, canvas: Rect) -> Option<Region>;

    /// Select an object within the given bounding box.
    ///
    /// `rect`: Initial bounding box around the object (user-drawn rectangle)
    /// `work_px`: Working resolution budget (typically 160,000 pixels)
    ///
    /// Returns a mask covering the object, or `None` if segmentation fails.
    /// Used by the "Object Selection" command (rectangle mode).
    fn object(&self, sampler: &dyn Sampler, canvas: Rect, rect: Rect, work_px: usize) -> Option<Region>;

    /// Provider name for logging and debugging.
    fn name(&self) -> &'static str;
}

/// Creates a segmentation provider based on backend selection and configuration.
///
/// - `backend`: The backend selection strategy (Auto/Classical/Model)
/// - `model_url`: Optional URL for the model inference backend (ComfyUI server).
///   If `None`, uses default `http://127.0.0.1:8188` or `PHOTOCRAFT_AI_BACKEND` environment variable.
///
/// Returns a boxed provider ready for use.
pub fn create_provider(backend: SegmentationBackend, model_url: Option<&str>) -> Box<dyn SegmentationProvider> {
    match backend {
        SegmentationBackend::Classical => Box::new(ClassicalProvider),
        SegmentationBackend::Model => {
            let url = resolve_model_url(model_url);
            Box::new(ModelProvider::new(url))
        }
        SegmentationBackend::Auto => {
            let url = resolve_model_url(model_url);
            Box::new(AutoProvider {
                model: ModelProvider::new(url),
                classical: ClassicalProvider,
            })
        }
    }
}

/// Resolves the model backend URL from explicit parameter, environment variable, or default.
fn resolve_model_url(explicit: Option<&str>) -> String {
    explicit
        .map(String::from)
        .or_else(|| std::env::var("PHOTOCRAFT_AI_BACKEND").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8188".to_string())
}

/// Classical segmentation provider using pure Rust heuristic algorithms.
///
/// This provider implements the original PhotoCraft segmentation algorithms:
/// - Subject selection: SLIC superpixels → saliency (boundary connectivity + background-weighted
///   contrast) → Otsu threshold → GrabCut with color models
/// - Object selection: GrabCut initialized from bounding box
///
/// These algorithms are fast (50-200ms for typical photos), fully deterministic, and work well
/// on simple subjects with clear boundaries. They struggle with:
/// - Fine details (hair, fur)
/// - Low contrast boundaries
/// - Complex textures
/// - Thin/transparent objects
pub struct ClassicalProvider;

impl SegmentationProvider for ClassicalProvider {
    fn subject(&self, sampler: &dyn Sampler, canvas: Rect) -> Option<Region> {
        super::subject::select_subject(sampler, canvas)
    }

    fn object(&self, sampler: &dyn Sampler, canvas: Rect, rect: Rect, work_px: usize) -> Option<Region> {
        super::grabcut::object_select(sampler, canvas, rect, work_px)
    }

    fn name(&self) -> &'static str {
        "Classical"
    }
}

/// Model-based segmentation provider using ComfyUI HTTP API.
///
/// This provider sends segmentation requests to a running ComfyUI server, which runs:
/// - BiRefNet: High-quality subject/background segmentation (used for Select Subject and Remove Background)
/// - SAM 2.1: Segment Anything Model for object selection with bounding boxes
///
/// Requirements:
/// - ComfyUI server running (typically `http://127.0.0.1:8188`)
/// - BiRefNet model installed for subject segmentation
/// - SAM 2.1 model installed for object selection
///
/// Performance: ~1 second for 24MP photos on mid-range GPUs (RTX 3060+)
/// Quality: Excellent on real photos, especially with fine details (hair, fur, transparency)
pub struct ModelProvider {
    #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
    client: ComfyUIClient,
    #[cfg(not(all(not(target_arch = "wasm32"), feature = "ml-segmentation")))]
    _base_url: String,
}

impl ModelProvider {
    /// Creates a new model provider with the given ComfyUI server URL.
    pub fn new(base_url: String) -> Self {
        Self {
            #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
            client: ComfyUIClient::new(base_url),
            #[cfg(not(all(not(target_arch = "wasm32"), feature = "ml-segmentation")))]
            _base_url: base_url,
        }
    }

    /// Tests if the backend is available by pinging the ComfyUI server.
    #[allow(dead_code)]
    fn is_available(&self) -> bool {
        #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
        {
            self.client.is_available()
        }
        #[cfg(not(all(not(target_arch = "wasm32"), feature = "ml-segmentation")))]
        {
            false
        }
    }
}

impl SegmentationProvider for ModelProvider {
    fn subject(&self, sampler: &dyn Sampler, canvas: Rect) -> Option<Region> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
        {
            self.client.segment_subject(sampler, canvas)
        }
        #[cfg(not(all(not(target_arch = "wasm32"), feature = "ml-segmentation")))]
        {
            let _ = (sampler, canvas);
            None
        }
    }

    fn object(&self, sampler: &dyn Sampler, canvas: Rect, rect: Rect, _work_px: usize) -> Option<Region> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "ml-segmentation"))]
        {
            self.client.segment_object(sampler, canvas, rect)
        }
        #[cfg(not(all(not(target_arch = "wasm32"), feature = "ml-segmentation")))]
        {
            let _ = (sampler, canvas, rect);
            None
        }
    }

    fn name(&self) -> &'static str {
        "Model"
    }
}

/// Auto provider: attempts model inference, falls back to classical on failure.
///
/// This is the recommended default provider. It provides the best user experience:
/// - Uses ML models when available for superior quality
/// - Automatically falls back to classical algorithms if the server is unavailable
/// - Transparent to the user (they always get *some* result)
///
/// Fallback occurs on:
/// - ComfyUI server unreachable
/// - Model not installed
/// - Inference timeout/error
/// - Out of VRAM
pub struct AutoProvider {
    model: ModelProvider,
    classical: ClassicalProvider,
}

impl SegmentationProvider for AutoProvider {
    fn subject(&self, sampler: &dyn Sampler, canvas: Rect) -> Option<Region> {
        // Try model first
        if let Some(region) = self.model.subject(sampler, canvas) {
            return Some(region);
        }
        // Fall back to classical
        self.classical.subject(sampler, canvas)
    }

    fn object(&self, sampler: &dyn Sampler, canvas: Rect, rect: Rect, work_px: usize) -> Option<Region> {
        // Try model first
        if let Some(region) = self.model.object(sampler, canvas, rect, work_px) {
            return Some(region);
        }
        // Fall back to classical
        self.classical.object(sampler, canvas, rect, work_px)
    }

    fn name(&self) -> &'static str {
        "Auto"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::tests::disc_scene;
    use crate::segment::ImageSampler;

    #[test]
    fn backend_parse_accepts_names_and_aliases_in_any_case() {
        for (s, want) in [
            ("auto", SegmentationBackend::Auto),
            ("AUTO", SegmentationBackend::Auto),
            ("classical", SegmentationBackend::Classical),
            ("Classic", SegmentationBackend::Classical),
            ("heuristic", SegmentationBackend::Classical),
            ("model", SegmentationBackend::Model),
            ("ML", SegmentationBackend::Model),
            ("ai", SegmentationBackend::Model),
        ] {
            assert_eq!(SegmentationBackend::parse(s), Some(want), "{s:?}");
        }
    }

    #[test]
    fn backend_parse_rejects_unknown_without_panicking() {
        let long = "a".repeat(10_000);
        for s in ["", " ", "gpu", "auto ", " model", "modèle", "自動", "\0", long.as_str()] {
            assert_eq!(SegmentationBackend::parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn backend_default_and_flags() {
        assert_eq!(SegmentationBackend::default(), SegmentationBackend::Auto);
        // (backend, allows_fallback, tries_model)
        for (b, fallback, model) in [
            (SegmentationBackend::Auto, true, true),
            (SegmentationBackend::Classical, false, false),
            (SegmentationBackend::Model, false, true),
        ] {
            assert_eq!(b.allows_fallback(), fallback, "{b:?}");
            assert_eq!(b.tries_model(), model, "{b:?}");
        }
    }

    #[test]
    fn create_provider_picks_the_named_provider() {
        // A closed loopback port: nothing here may reach a real server.
        let url = Some("http://127.0.0.1:1");
        assert_eq!(create_provider(SegmentationBackend::Classical, url).name(), "Classical");
        assert_eq!(create_provider(SegmentationBackend::Model, url).name(), "Model");
        assert_eq!(create_provider(SegmentationBackend::Auto, url).name(), "Auto");
    }

    #[test]
    fn classical_provider_matches_the_direct_algorithms() {
        let (img, _) = disc_scene(200, 150, 100.0, 75.0, 40.0, 6);
        let s = ImageSampler { img: &img, origin: (0, 0) };
        let canvas = Rect::new(0, 0, 200, 150);
        let rect = Rect::new(50, 25, 150, 125);
        let p = ClassicalProvider;
        assert_eq!(p.subject(&s, canvas), crate::segment::subject::select_subject(&s, canvas));
        assert_eq!(p.object(&s, canvas, rect, 160_000), crate::segment::grabcut::object_select(&s, canvas, rect, 160_000));
        assert!(p.subject(&s, canvas).is_some());
    }

    #[test]
    fn classical_provider_handles_degenerate_input() {
        let (img, _) = disc_scene(64, 48, 32.0, 24.0, 12.0, 3);
        let s = ImageSampler { img: &img, origin: (0, 0) };
        let canvas = Rect::new(0, 0, 64, 48);
        let p = ClassicalProvider;
        // Empty, inverted and fully off-canvas boxes find nothing, without panicking.
        for rect in [Rect::new(10, 10, 10, 10), Rect::new(40, 40, 5, 5), Rect::new(-500, -500, -400, -400), Rect::new(9_000, 9_000, 9_100, 9_100)] {
            let _ = p.object(&s, canvas, rect, 160_000);
        }
        let _ = p.object(&s, canvas, Rect::new(0, 0, 64, 48), 0);
        let _ = p.subject(&s, Rect::new(0, 0, 0, 0));
    }

    /// Without the `ml-segmentation` feature the model backend is compiled out: Model finds
    /// nothing and Auto is exactly Classical.
    #[cfg(not(feature = "ml-segmentation"))]
    #[test]
    fn without_the_feature_model_is_empty_and_auto_is_classical() {
        let (img, _) = disc_scene(200, 150, 100.0, 75.0, 40.0, 6);
        let s = ImageSampler { img: &img, origin: (0, 0) };
        let canvas = Rect::new(0, 0, 200, 150);
        let rect = Rect::new(50, 25, 150, 125);
        let model = create_provider(SegmentationBackend::Model, None);
        assert_eq!(model.subject(&s, canvas), None);
        assert_eq!(model.object(&s, canvas, rect, 160_000), None);
        let auto = create_provider(SegmentationBackend::Auto, None);
        assert_eq!(auto.subject(&s, canvas), ClassicalProvider.subject(&s, canvas));
        assert_eq!(auto.object(&s, canvas, rect, 160_000), ClassicalProvider.object(&s, canvas, rect, 160_000));
    }

    /// With the feature on but no server listening, Auto falls back to Classical promptly.
    #[cfg(all(feature = "ml-segmentation", not(target_arch = "wasm32")))]
    #[test]
    fn with_the_feature_auto_falls_back_when_the_server_is_down() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        }; // Dropped: the port now refuses connections.
        let url = format!("http://127.0.0.1:{port}");
        let (img, _) = disc_scene(120, 90, 60.0, 45.0, 25.0, 4);
        let s = ImageSampler { img: &img, origin: (0, 0) };
        let canvas = Rect::new(0, 0, 120, 90);
        let start = std::time::Instant::now();
        assert_eq!(create_provider(SegmentationBackend::Model, Some(&url)).subject(&s, canvas), None);
        let auto = create_provider(SegmentationBackend::Auto, Some(&url));
        assert_eq!(auto.subject(&s, canvas), ClassicalProvider.subject(&s, canvas));
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "fallback took {:?}", start.elapsed());
    }
}
