//! Capability-based filesystem policy for untrusted automation paths.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use photocraft_format::atomic::RenameRetry;
use serde_json::Value;

use crate::AutomationError;

const DENIED: &str = "automation filesystem access is not granted";

#[derive(Clone)]
struct RootCapability {
    dir: Arc<Dir>,
    /// The root as an absolute path (trusted launch configuration), to recognise absolute request
    /// paths that name a file beneath it.
    path: PathBuf,
}

/// Separate directory capabilities for automation reads and writes.
///
/// Root paths are trusted launch-time configuration. Request paths are always
/// untrusted, forward-slash relative paths resolved by `cap-std` beneath the
/// held directory handle, or absolute paths whose text lies beneath the root
/// (translated to relative ones, see [`request_path`]). Parent traversal,
/// alternate separators, Windows prefixes and empty components are rejected
/// before I/O.
#[derive(Clone, Default)]
pub struct AuthorizedWorkspace {
    read: Option<RootCapability>,
    write: Option<RootCapability>,
}

impl AuthorizedWorkspace {
    /// Open the configured roots as capabilities. Either authority may be
    /// omitted; omitted authority fails closed.
    pub fn new(read_root: Option<&Path>, write_root: Option<&Path>) -> Result<Self, AutomationError> {
        Ok(Self { read: open_root(read_root, "read")?, write: open_root(write_root, "write")? })
    }

    /// Read one regular file below the configured read root.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, AutomationError> {
        let relative = request_path(path, self.read.as_ref())?;
        let root = self.read.as_ref().ok_or_else(|| AutomationError::BadRequest(format!("{DENIED}: read authority is absent")))?;
        let mut file = root.dir.open(&relative).map_err(|e| file_error("read", path, e))?;
        let metadata = file.metadata().map_err(|e| file_error("read", path, e))?;
        if !metadata.is_file() {
            return Err(AutomationError::BadRequest(format!("automation read path is not a regular file: `{path}`")));
        }
        read_to_observed_length(&mut file, metadata.len())
    }

    /// Load one `.pcraft` document in directory form — the native on-disk layout — below the
    /// read root. `read` serves regular files only (the zip form), so a regular file at the
    /// path is reported as `Ok(None)` for the caller to fall back to.
    pub fn open_pcraft(&self, path: &str) -> Result<Option<photocraft_doc::Document>, AutomationError> {
        let relative = request_path(path, self.read.as_ref())?;
        let root = self.read.as_ref().ok_or_else(|| AutomationError::BadRequest(format!("{DENIED}: read authority is absent")))?;
        let dir = match root.dir.open_dir(&relative) {
            Ok(dir) => dir,
            Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => return Ok(None),
            Err(e) => return Err(file_error("read", path, e)),
        };
        let source = DirHandle { dir };
        Ok(Some(photocraft_format::load_bytes_source(&source, &photocraft_format::LoadOptions::default())?))
    }

    /// Create or replace one file below the configured write root, crash-safely: the bytes go to
    /// a temporary file beside the target, which is synced and renamed over it (the same steps
    /// as [`photocraft_format::atomic_write`], through the directory capability). On failure
    /// the previous file is untouched and the temporary file is removed.
    ///
    /// The parent directory must already exist. `cap-std` performs path
    /// resolution and file creation relative to the held directory handle, so
    /// a non-existent final target is supported without ambient path access.
    pub fn write(&self, path: &str, bytes: &[u8]) -> Result<(), AutomationError> {
        let relative = request_path(path, self.write.as_ref())?;
        let root = self.write.as_ref().ok_or_else(|| AutomationError::BadRequest(format!("{DENIED}: write authority is absent")))?;
        let leaf = relative.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parent = relative.parent().map(Path::to_path_buf).unwrap_or_default();
        let tmp = parent.join(photocraft_format::atomic::temp_name(&leaf));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let written = root.dir.open_with(&tmp, &options).and_then(|mut file| {
            file.write_all(bytes)?;
            // Falls back to a plain fsync on shares that refuse a full flush (#1336).
            photocraft_format::atomic::sync_file(&file.into_std())
        });
        let renamed = written.and_then(|()| photocraft_format::atomic::retry_rename(RenameRetry::platform(), || root.dir.rename(&tmp, &root.dir, &relative)));
        if let Err(e) = renamed {
            let _ = root.dir.remove_file(&tmp);
            return Err(file_error("write", path, e));
        }
        // Flush the directory entry (Unix); best effort, the new bytes are already in place.
        #[cfg(unix)]
        {
            let dir = if parent.as_os_str().is_empty() { PathBuf::from(".") } else { parent };
            if let Ok(d) = root.dir.open(&dir) {
                let _ = photocraft_format::atomic::sync_file(&d.into_std());
            }
        }
        Ok(())
    }
}

fn read_to_observed_length(reader: &mut impl Read, observed_length: u64) -> Result<Vec<u8>, AutomationError> {
    // Do not reserve from a potentially stale or attacker-controlled metadata length. Read in
    // chunks, and reject growth beyond the length observed on the opened file before appending.
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 64 << 10];
    loop {
        let remaining = observed_length.saturating_sub(bytes.len() as u64);
        let ask = chunk.len().min(usize::try_from(remaining.saturating_add(1)).unwrap_or(chunk.len()));
        let n = match reader.read(&mut chunk[..ask]) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(AutomationError::Io(format!("automation read failed: {e}"))),
        };
        if n == 0 {
            return Ok(bytes);
        }
        if n as u64 > remaining {
            return Err(AutomationError::BadRequest(format!("automation file grew beyond its observed length of {observed_length} bytes")));
        }
        let needed = bytes.len().checked_add(n).ok_or_else(|| AutomationError::BadRequest("automation file is too large to read".into()))?;
        if needed > bytes.capacity() {
            let target = bytes.capacity().saturating_add(bytes.capacity() / 2).max(needed).min(usize::try_from(observed_length).unwrap_or(usize::MAX));
            bytes.try_reserve_exact(target - bytes.len()).map_err(|e| AutomationError::Io(format!("automation read allocation failed: {e}")))?;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
}

fn open_root(path: Option<&Path>, authority: &str) -> Result<Option<RootCapability>, AutomationError> {
    let Some(path) = path else { return Ok(None) };
    if path.as_os_str().is_empty() {
        return Err(AutomationError::BadRequest(format!("automation {authority} root is empty")));
    }
    let dir = Dir::open_ambient_dir(path, ambient_authority())
        .map_err(|e| AutomationError::Io(format!("cannot open automation {authority} root `{}`: {e}", path.display())))?;
    // Textual (no link resolution), like the request paths it is compared with.
    let absolute =
        std::path::absolute(path).map_err(|e| AutomationError::Io(format!("cannot resolve automation {authority} root `{}`: {e}", path.display())))?;
    Ok(Some(RootCapability { dir: Arc::new(dir), path: absolute }))
}

/// A request path as the relative path the root's capability resolves. An absolute path is
/// accepted only when its text names a file beneath `root`: the root's components are
/// stripped (the drive letter compared without case) and the rest must pass
/// [`relative_path`], so `..`, prefixes and device names are refused as before. Nothing is
/// resolved on the filesystem; the held directory handle still does all the opening, so
/// links cannot escape the root.
fn request_path(raw: &str, root: Option<&RootCapability>) -> Result<PathBuf, AutomationError> {
    let Some(root) = root.filter(|_| Path::new(raw).is_absolute()) else { return relative_path(raw) };
    let mut request = Path::new(raw).components();
    for want in root.path.components() {
        let same = match (request.next(), want) {
            (Some(Component::Prefix(a)), Component::Prefix(b)) => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
            (Some(got), want) => got == want,
            (None, _) => false,
        };
        if !same {
            return Err(path_error(raw, "absolute paths must be inside the automation root"));
        }
    }
    let rest: Vec<&str> = request.map(|c| c.as_os_str().to_str().unwrap_or("\u{FFFD}")).collect();
    relative_path(&rest.join("/"))
}

fn relative_path(raw: &str) -> Result<PathBuf, AutomationError> {
    if raw.is_empty() {
        return Err(path_error(raw, "path is empty"));
    }
    if raw.contains('\\') {
        return Err(path_error(raw, "alternate separators are not allowed; use `/`"));
    }
    if raw.starts_with('/') {
        return Err(path_error(raw, "absolute paths are not allowed"));
    }
    if raw.contains(':') {
        return Err(path_error(raw, "drive, device and stream prefixes are not allowed"));
    }
    for component in raw.split('/') {
        if component.is_empty() {
            return Err(path_error(raw, "empty path components are not allowed"));
        }
        if component == "." {
            return Err(path_error(raw, "`.` path components are not allowed"));
        }
        if component == ".." {
            return Err(path_error(raw, "parent traversal is not allowed"));
        }
        if component.ends_with(['.', ' ']) {
            return Err(path_error(raw, "path components ending in a dot or space are not allowed"));
        }
        if is_windows_device_name(component) {
            return Err(path_error(raw, "reserved device names are not allowed"));
        }
    }
    Ok(PathBuf::from(raw))
}

fn is_windows_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$")
        || stem.strip_prefix("COM").is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
        || stem.strip_prefix("LPT").is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

fn path_error(path: &str, reason: &str) -> AutomationError {
    AutomationError::BadRequest(format!("automation path rejected: {reason}: `{path}`"))
}

/// A `.pcraft` directory behind the read-root capability: bounded reads, no ambient paths.
struct DirHandle {
    dir: Dir,
}

impl photocraft_format::ByteSource for DirHandle {
    fn get(&self, path: &str, max: usize) -> photocraft_format::Result<Vec<u8>> {
        let missing = || photocraft_format::FormatError::Corrupt(format!("missing `{path}`"));
        let mut file = self.dir.open(path).map_err(|_| missing())?;
        let metadata = file.metadata().map_err(|_| missing())?;
        if !metadata.is_file() {
            // A FIFO or device would block the open and stream without end.
            return Err(photocraft_format::FormatError::Corrupt(format!("`{path}` is not a regular file")));
        }
        photocraft_format::read_entry(&mut file, metadata.len(), max, path)
    }
}

fn file_error(operation: &str, path: &str, error: std::io::Error) -> AutomationError {
    let kind = error.kind();
    AutomationError::Io(format!("automation {operation} `{path}` failed ({kind:?}): {error}"))
}

/// Filesystem-bearing engine commands have not yet been converted to consume
/// directory capabilities. Automation must use `doc.open`, `doc.save` and
/// `doc.render` for file effects until those commands are migrated.
pub fn authorize_engine_command(id: &str, params: &Value) -> Result<(), AutomationError> {
    let safe_file_command = matches!(
        id,
        "file.new"
            | "file.newFromClipboard"
            | "file.close"
            | "file.closeAll"
            | "file.closeOthers"
            | "file.fileInfo"
            | "file.automate.fitImage"
            | "file.automate.conditionalModeChange"
            | "file.automate.cropAndStraightenPhotos"
            | "file.scripts.flattenAllLayerEffects"
            | "file.scripts.flattenAllMasks"
            | "file.scripts.deleteAllEmptyLayers"
            | "file.export.exportPreferences"
    );
    if id.starts_with("file.") && !safe_file_command {
        return Err(command_error(id));
    }
    // Contents commands re-check the resolved source in the engine before any disk read.
    // Photoshop's embedded linked-layer blocks count as in-memory sources too.
    let in_memory_smart_command = matches!(
        id,
        "layer.smartObjects.convertToSmartObject"
            | "layer.smartObjects.editContents"
            | "layer.smartObjects.convertToLayers"
            | "layer.smartObjects.saveContents"
            | "layer.smartObjects.exportSvg"
            | "layer.smartObjects.updateSvg"
    );
    if (id.starts_with("layer.smartObjects.") && !in_memory_smart_command)
        || matches!(
            id,
            "pattern.import"
                | "pattern.export"
                | "edit.presets.migratePresets"
                | "measurementLog.export"
                | "layer.videoLayers.newVideoLayerFromFile"
                | "layer.videoLayers.replaceFootage"
                | "layer.videoLayers.reloadFrame"
        )
        || command_uses_ambient_path(id, params)
        || profile_command_may_read_ambient(id, params)
        || preferences_may_grant_ambient_paths(id, params)
        || params_contain_ambient_path(id, params)
    {
        return Err(command_error(id));
    }
    Ok(())
}

/// Desktop sessions may already contain user-configured ambient colour-profile
/// paths. Commands which implicitly resolve those settings are denied even
/// when their request parameters contain no path. Fresh headless sessions
/// cannot acquire such settings through the automation interface.
pub fn authorize_desktop_engine_command(id: &str, params: &Value) -> Result<(), AutomationError> {
    authorize_engine_command(id, params)?;
    if id.starts_with("image.mode.") {
        return Err(command_error(id));
    }
    Ok(())
}

/// [`authorize_engine_command`] as the function pointer [`photocraft_engine::Session::authorize`]
/// stores. `actions.play` calls it for every nested step.
pub fn authorize_engine_step(id: &str, params: &Value) -> photocraft_engine::Result<()> {
    authorize_engine_command(id, params).map_err(|e| photocraft_engine::EngineError::Other(e.to_string()))
}

/// [`authorize_desktop_engine_command`] as a [`photocraft_engine::Session::authorize`] hook.
pub fn authorize_desktop_engine_step(id: &str, params: &Value) -> photocraft_engine::Result<()> {
    authorize_desktop_engine_command(id, params).map_err(|e| photocraft_engine::EngineError::Other(e.to_string()))
}

fn params_contain_ambient_path(id: &str, params: &Value) -> bool {
    let keys: &[&str] = match id {
        "image.adjustments.colorLookup" | "layer.newAdjustmentLayer.colorLookup" | "layer.setAdjustment" => &["file"],
        "filter.distort.displace" => &["mapPath"],
        "layer.quickExportAsPng"
        | "layer.exportAs"
        | "image.applyDataSet"
        | "layer.smartObjects.editContents"
        | "layer.smartObjects.convertToLayers"
        | "layer.smartObjects.exportSvg"
        | "layer.smartObjects.updateSvg" => &["path"],
        "image.mode.rgb" | "image.mode.grayscale" | "image.mode.cmyk" | "image.mode.lab" => &["profile"],
        "edit.assignProfile" | "edit.convertToProfile" | "edit.profileInfo" | "view.proofSetup" | "view.gamutWarning" => &["profile"],
        "edit.colorSettings" => &["workingRgb", "workingCmyk", "workingGray"],
        _ => &[],
    };
    keys.iter().any(|key| {
        params
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|value| if matches!(*key, "file" | "mapPath" | "path") { !value.is_empty() } else { looks_like_path(value) })
    })
}

fn profile_command_may_read_ambient(id: &str, params: &Value) -> bool {
    // Installed profiles scan OS and user directories even without a request path.
    if matches!(id, "color.profileMismatch" | "edit.colorSettings" | "edit.installedProfiles" | "view.proofSetup") {
        return true;
    }
    matches!(id, "edit.assignProfile" | "edit.convertToProfile" | "edit.profileInfo")
        && params.get("profile").and_then(Value::as_str).is_some_and(|profile| {
            matches!(profile, "working" | "default" | "working-cmyk" | "workingCmyk" | "working-rgb" | "workingRgb" | "working-gray" | "workingGray")
        })
}

fn preferences_may_grant_ambient_paths(id: &str, params: &Value) -> bool {
    if id != "prefs.set" {
        return false;
    }
    let direct = params.get("path").and_then(Value::as_str).is_some_and(preference_uses_ambient_filesystem);
    let batch = params.get("values").and_then(Value::as_object).is_some_and(|values| values.keys().any(|path| preference_uses_ambient_filesystem(path)));
    direct || batch
}

fn preference_uses_ambient_filesystem(path: &str) -> bool {
    // The engine skips empty segments (`.scriptEvents.enabled` sets `scriptEvents.enabled`), so
    // judge the first non-empty one; no segment at all is the whole-preferences update.
    let Some(section) = path.split('.').find(|segment| !segment.is_empty()) else { return true };
    matches!(section, "colorSettings" | "scriptEvents" | "historyLog" | "plugIns" | "scratchDisks")
}

fn command_uses_ambient_path(id: &str, params: &Value) -> bool {
    match id {
        "actions.load" | "brush.presets.importAbr" | "gradient.presets.importGrd" | "plugin.install" | "swatches.import" => {
            // `data` wins over `path` in these commands; any `path` without it reads the filesystem.
            params.get("data").is_none() && params.get("path").is_some()
        }
        // With a `path` the file is written there; without one the bytes come back as `data`.
        "actions.save" | "swatches.export" => params.get("path").is_some(),
        "plugin.reload" => true,
        _ => false,
    }
}

fn looks_like_path(value: &str) -> bool {
    value.contains('/')
        || value.contains('\\')
        || value.contains(':')
        || value.to_ascii_lowercase().ends_with(".icc")
        || value.to_ascii_lowercase().ends_with(".icm")
}

fn command_error(id: &str) -> AutomationError {
    AutomationError::BadRequest(format!("automation command `{id}` uses ambient filesystem paths and is disabled; use capability-scoped document methods"))
}

/// An export owns only files it created through this held directory capability. Refuse
/// collisions (including symlinks) so cancellation cannot delete or replace an earlier export.
pub(crate) struct ExportSequence {
    dir: Dir,
    created: Vec<String>,
}

impl AuthorizedWorkspace {
    pub(crate) fn export_sequence(&self, path: &str) -> Result<ExportSequence, AutomationError> {
        let relative = request_path(path, self.write.as_ref())?;
        let root = self.write.as_ref().ok_or_else(|| AutomationError::BadRequest(format!("{DENIED}: write authority is absent")))?;
        root.dir.create_dir_all(&relative).map_err(|e| file_error("mkdir", path, e))?;
        let dir = root.dir.open_dir(&relative).map_err(|e| file_error("open export directory", path, e))?;
        Ok(ExportSequence { dir, created: Vec::new() })
    }
}

impl ExportSequence {
    pub(crate) fn write(&mut self, name: &str, bytes: &[u8]) -> Result<(), AutomationError> {
        let relative = relative_path(name)?;
        if relative.components().count() != 1 {
            return Err(path_error(name, "export filename must be a single component"));
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = self.dir.open_with(&relative, &options).map_err(|e| file_error("create export", name, e))?;
        // Record immediately after creation, before any fallible write, including partial writes.
        self.created.push(name.to_owned());
        file.write_all(bytes).and_then(|()| file.sync_all()).map_err(|e| file_error("write export", name, e))
    }

    pub(crate) fn commit(&mut self) {
        self.created.clear();
    }

    pub(crate) fn cleanup(&mut self) -> Result<(), AutomationError> {
        let mut failed = Vec::new();
        self.created.retain(|name| match self.dir.remove_file(name) {
            Ok(()) => false,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                failed.push(format!("{name}: {e}"));
                true
            }
        });
        if failed.is_empty() { Ok(()) } else { Err(AutomationError::Io(format!("export cleanup failed: {}", failed.join("; ")))) }
    }
}

impl Drop for ExportSequence {
    fn drop(&mut self) {
        // Also cover an escaped rendering panic; normal errors report cleanup failures above.
        let _ = self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(name: &str) -> (PathBuf, PathBuf, AuthorizedWorkspace) {
        let base = std::env::temp_dir().join(format!("photocraft-workspace-{}-{name}", std::process::id()));
        let inside = base.join("inside");
        let outside = base.join("outside");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let workspace = AuthorizedWorkspace::new(Some(&inside), Some(&inside)).unwrap();
        (inside, outside, workspace)
    }

    #[test]
    fn export_cleanup_owns_only_created_files_even_during_unwind() {
        let (inside, _, workspace) = roots("export-cleanup");
        std::fs::create_dir(inside.join("frames")).unwrap();
        std::fs::write(inside.join("frames/clip_0009.png"), b"old").unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut export = workspace.export_sequence("frames").unwrap();
            export.write("clip_0000.png", b"new").unwrap();
            assert!(export.write("clip_0009.png", b"overwrite").is_err());
            assert!(export.write("../escape.png", b"escape").is_err());
            panic!("rendering panic");
        }));
        assert!(panic.is_err());
        assert_eq!(std::fs::read_dir(inside.join("frames")).unwrap().count(), 1);
        assert_eq!(std::fs::read(inside.join("frames/clip_0009.png")).unwrap(), b"old");
        assert!(AuthorizedWorkspace::default().export_sequence("frames").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn export_directory_symlinks_cannot_escape_the_granted_root() {
        let (inside, outside, workspace) = roots("export-symlink");
        std::os::unix::fs::symlink(&outside, inside.join("escape")).unwrap();
        assert!(workspace.export_sequence("escape").is_err());
        assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    }

    #[test]
    fn valid_read_and_nonexistent_output_write_stay_in_root() {
        let (inside, _, workspace) = roots("valid");
        std::fs::write(inside.join("read.txt"), b"canary").unwrap();
        assert_eq!(workspace.read("read.txt").unwrap(), b"canary");
        workspace.write("new-output.txt", b"created").unwrap();
        assert_eq!(std::fs::read(inside.join("new-output.txt")).unwrap(), b"created");
    }

    #[test]
    fn large_observed_length_does_not_trigger_a_fixed_limit_or_upfront_reservation() {
        let mut shortened = std::io::Cursor::new(b"psb");
        assert_eq!(read_to_observed_length(&mut shortened, (1 << 30) + 1).unwrap(), b"psb");
    }

    #[test]
    fn read_growth_is_rejected_at_the_observed_length() {
        let mut growing = std::io::repeat(b'x').take(65);
        let error = read_to_observed_length(&mut growing, 64).unwrap_err().to_string();
        assert!(error.contains("grew beyond its observed length of 64 bytes"), "{error}");
        let mut allowed = std::io::repeat(b'x').take(64);
        assert_eq!(read_to_observed_length(&mut allowed, 64).unwrap().len(), 64);
        let mut empty = std::io::Cursor::new(b"x");
        assert!(read_to_observed_length(&mut empty, 0).is_err());
    }

    #[test]
    fn open_pcraft_loads_a_directory_below_the_read_root() {
        let (inside, _, workspace) = roots("pcraft-dir");
        let doc = photocraft_doc::Document::new("Native", photocraft_doc::Size::new(16, 12), photocraft_doc::ColorMode::Rgb, photocraft_doc::SampleType::U8);
        photocraft_format::PcraftWriter::new().save_dir(&doc, &inside.join("native.pcraft"), &photocraft_format::SaveOptions::default()).unwrap();
        let loaded = workspace.open_pcraft("native.pcraft").unwrap().expect("the directory form loads");
        assert_eq!(loaded.name, doc.name);
        assert_eq!((loaded.size.width, loaded.size.height), (16, 12));
    }

    #[test]
    fn open_pcraft_reports_zip_files_and_missing_paths_clearly() {
        let (inside, _, workspace) = roots("pcraft-errors");
        let doc = photocraft_doc::Document::new("Zipped", photocraft_doc::Size::new(4, 4), photocraft_doc::ColorMode::Rgb, photocraft_doc::SampleType::U8);
        let zip = inside.join("zipped.pcraft");
        photocraft_format::PcraftWriter::new().save_path(&doc, &zip, &photocraft_format::SaveOptions::default()).unwrap();
        assert!(zip.is_file());
        assert!(workspace.open_pcraft("zipped.pcraft").unwrap().is_none(), "the zip form falls back to the bytes path");
        let error = workspace.open_pcraft("missing.pcraft").unwrap_err().to_string();
        assert!(error.contains("failed"), "a missing path propagates its io error: {error}");
        assert!(AuthorizedWorkspace::default().open_pcraft("x.pcraft").is_err(), "no read authority fails closed");
    }

    #[cfg(unix)]
    #[test]
    fn open_pcraft_rejects_non_regular_entries_and_escaping_symlinks() {
        let (inside, _, workspace) = roots("pcraft-nonregular");
        let bundle = inside.join("native.pcraft");
        std::fs::create_dir_all(bundle.join("manifest.json")).unwrap();
        let error = workspace.open_pcraft("native.pcraft").unwrap_err().to_string();
        assert!(error.contains("not a regular file"), "a directory entry is refused, not streamed: {error}");

        let (inside2, _, workspace2) = roots("pcraft-symlink");
        let bundle2 = inside2.join("native.pcraft");
        std::fs::create_dir(&bundle2).unwrap();
        std::os::unix::fs::symlink("/dev/zero", bundle2.join("manifest.json")).unwrap();
        assert!(workspace2.open_pcraft("native.pcraft").is_err(), "an escaping symlink entry is refused");
    }

    fn temp_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".tmp")).collect()
    }

    #[test]
    fn write_replaces_atomically_and_leaves_no_temp_file() {
        let (inside, _, workspace) = roots("atomic");
        std::fs::create_dir_all(inside.join("sub")).unwrap();
        workspace.write("sub/out.psd", b"original").unwrap();
        workspace.write("sub/out.psd", b"replacement").unwrap();
        assert_eq!(std::fs::read(inside.join("sub/out.psd")).unwrap(), b"replacement");
        assert!(temp_files(&inside.join("sub")).is_empty());
        // Renaming over a directory fails: nothing is left behind and the directory survives.
        std::fs::create_dir_all(inside.join("adir")).unwrap();
        assert!(workspace.write("adir", b"x").is_err());
        assert!(inside.join("adir").is_dir());
        assert!(temp_files(&inside).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn write_into_read_only_folder_keeps_the_original() {
        use std::os::unix::fs::PermissionsExt;
        let (inside, _, workspace) = roots("readonly");
        let ro = inside.join("ro");
        std::fs::create_dir_all(&ro).unwrap();
        std::fs::write(ro.join("doc.psd"), b"original").unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let root_user = std::fs::File::create(ro.join("probe")).is_ok();
        if !root_user {
            assert!(workspace.write("ro/doc.psd", b"new").is_err());
            assert_eq!(std::fs::read(ro.join("doc.psd")).unwrap(), b"original");
            assert!(temp_files(&ro).is_empty());
        }
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn rejects_absolute_traversal_mixed_prefix_and_malformed_paths_without_panicking() {
        let (_, _, workspace) = roots("reject");
        for path in [
            "",
            "/absolute",
            "../escape",
            "a/../escape",
            "a\\..\\escape",
            "a\\b",
            "C:/escape",
            "a//b",
            "./a",
            "name:stream",
            "NUL",
            "con.txt",
            "folder/COM1.log",
        ] {
            assert!(workspace.read(path).is_err(), "read accepted {path:?}");
            assert!(workspace.write(path, b"x").is_err(), "write accepted {path:?}");
        }
    }

    #[test]
    fn read_and_write_authority_are_separate() {
        let (inside, _, _) = roots("separate");
        std::fs::write(inside.join("read.txt"), b"canary").unwrap();
        let read_only = AuthorizedWorkspace::new(Some(&inside), None).unwrap();
        assert_eq!(read_only.read("read.txt").unwrap(), b"canary");
        assert!(read_only.write("blocked.txt", b"x").is_err());
        let write_only = AuthorizedWorkspace::new(None, Some(&inside)).unwrap();
        assert!(write_only.read("read.txt").is_err());
        write_only.write("written.txt", b"ok").unwrap();
    }

    #[test]
    fn missing_parent_is_a_stable_error_before_any_file_is_created() {
        let (inside, _, workspace) = roots("missing-parent");
        let error = workspace.write("missing/output.txt", b"x").unwrap_err().to_string();
        assert!(error.contains("automation write"), "{error}");
        assert!(!inside.join("missing").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        use std::os::unix::fs::symlink;

        let (inside, outside, workspace) = roots("symlink");
        std::fs::write(outside.join("secret.txt"), b"outside").unwrap();
        symlink(&outside, inside.join("link")).unwrap();
        assert!(workspace.read("link/secret.txt").is_err());
        assert!(workspace.write("link/new.txt", b"blocked").is_err());
        assert!(!outside.join("new.txt").exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_symlink_escape_is_rejected_when_supported() {
        use std::os::windows::fs::symlink_dir;

        let (inside, outside, workspace) = roots("windows-link");
        std::fs::write(outside.join("secret.txt"), b"outside").unwrap();
        if symlink_dir(&outside, inside.join("link")).is_err() {
            return;
        }
        assert!(workspace.read("link/secret.txt").is_err());
        assert!(workspace.write("link/new.txt", b"blocked").is_err());
        assert!(!outside.join("new.txt").exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_junction_escape_is_rejected_when_supported() {
        use std::process::Command;

        let (inside, outside, workspace) = roots("windows-junction");
        std::fs::write(outside.join("secret.txt"), b"outside").unwrap();
        let link = inside.join("junction");
        let status = Command::new("cmd.exe").args(["/D", "/C", "mklink", "/J"]).arg(&link).arg(&outside).status();
        if !status.is_ok_and(|status| status.success()) {
            return;
        }
        assert!(workspace.read("junction/secret.txt").is_err());
        assert!(workspace.write("junction/new.txt", b"blocked").is_err());
        assert!(!outside.join("new.txt").exists());
        let _ = std::fs::remove_dir(link);
    }

    #[test]
    fn filesystem_commands_fail_closed() {
        for id in [
            "file.open",
            "file.openAs",
            "file.save",
            "file.saveAs",
            "file.saveACopy",
            "file.export.saveForWebLegacy",
            "pattern.import",
            "layer.smartObjects.exportContents",
            "layer.smartObjects.replaceContents",
            "layer.smartObjects.relinkToFile",
            "layer.smartObjects.convertToLinked",
            "measurementLog.export",
            "layer.videoLayers.reloadFrame",
            "edit.colorSettings",
        ] {
            assert!(authorize_engine_command(id, &serde_json::json!({})).is_err());
        }
        assert!(authorize_engine_command("file.new", &serde_json::json!({})).is_ok());
        assert!(authorize_engine_command("file.automate.cropAndStraightenPhotos", &serde_json::json!({})).is_ok());
        assert!(authorize_desktop_engine_command("file.automate.cropAndStraightenPhotos", &serde_json::json!({})).is_ok());
        for id in ["layer.smartObjects.editContents", "layer.smartObjects.convertToLayers", "layer.smartObjects.saveContents"] {
            assert!(authorize_desktop_engine_command(id, &serde_json::json!({"layer": 19})).is_ok(), "{id}");
        }
        for id in ["layer.smartObjects.editContents", "layer.smartObjects.convertToLayers"] {
            assert!(authorize_engine_command(id, &serde_json::json!({"path": "/outside/source.psb"})).is_err(), "{id}");
        }
        // The UI-level examples in docs/control-protocol.md.
        for id in ["view.zoomIn", "window.theme.pro", "edit.search"] {
            assert!(authorize_desktop_engine_command(id, &serde_json::json!({})).is_ok(), "{id}");
        }
        assert!(authorize_engine_command("image.mode.cmyk", &serde_json::json!({})).is_ok());
        assert!(authorize_desktop_engine_command("image.mode.cmyk", &serde_json::json!({})).is_err());
        assert!(authorize_engine_command("image.mode.rgb", &serde_json::json!({"profile": "/outside/profile.icc"})).is_err());
        assert!(authorize_engine_command("image.mode.rgb", &serde_json::json!({"profile": "srgb"})).is_ok());
        assert!(authorize_engine_command("filter.distort.displace", &serde_json::json!({"mapPath": "outside.png"})).is_err());
        assert!(authorize_engine_command("layer.setAdjustment", &serde_json::json!({"file": "outside.cube"})).is_err());
        assert!(authorize_engine_command("prefs.set", &serde_json::json!({"path": "colorSettings.workingRgb", "value": "outside.icc"})).is_err());
        assert!(authorize_engine_step("file.open", &serde_json::json!({})).is_err());
        assert!(authorize_engine_step("actions.play", &serde_json::json!({})).is_ok());
        // An allowed command can't reach a denied one by running it on its own behalf.
        let mut session = photocraft_engine::Session::new();
        session.execute("file.new", serde_json::json!({"width": 4, "height": 4})).unwrap();
        session.authorize = Some(authorize_desktop_engine_step);
        let params = serde_json::json!({"to": "grayscale"});
        assert!(authorize_desktop_engine_command("file.automate.conditionalModeChange", &params).is_ok());
        assert!(session.execute("file.automate.conditionalModeChange", params).is_err());
        assert_eq!(session.active().unwrap().doc.mode, photocraft_engine::doc::ColorMode::Rgb);
        assert!(authorize_desktop_engine_step("file.saveACopy", &serde_json::json!({})).is_err());
    }

    #[test]
    fn installed_profile_enumeration_is_denied_even_with_workspace_roots() {
        let (inside, _, workspace) = roots("installed-profile-policy");
        for mut headless in [crate::Headless::new(), crate::Headless::with_workspace(workspace)] {
            headless.command_run("file.new", serde_json::json!({"width": 4, "height": 4})).unwrap();
            let before = headless.session.active().unwrap().doc.clone();
            let revision = headless.session.active().unwrap().revision;
            let history = headless.session.active().unwrap().history.past_len();
            for params in [serde_json::json!({}), serde_json::json!({"class": "display", "colorSpace": "rgb"})] {
                // Filters are applied only after the engine's ambient directory scan.
                let error = authorize_engine_command("edit.installedProfiles", &params).unwrap_err();
                assert!(error.to_string().contains("ambient filesystem paths"), "{error}");
                assert!(authorize_desktop_engine_command("edit.installedProfiles", &params).is_err());
                let error = headless.command_run("edit.installedProfiles", params).unwrap_err();
                assert!(error.to_string().contains("ambient filesystem paths"), "{error}");
            }
            let after = headless.session.active().unwrap();
            assert_eq!(after.doc, before);
            assert_eq!(after.revision, revision);
            assert_eq!(after.history.past_len(), history);
        }
        std::fs::remove_dir_all(inside.parent().unwrap()).unwrap();
    }

    #[test]
    fn installed_profile_enumeration_cannot_bypass_policy_through_actions() {
        for authorize in [authorize_engine_step, authorize_desktop_engine_step] {
            let mut session = photocraft_engine::Session::new();
            session.execute("file.new", serde_json::json!({"width": 4, "height": 4})).unwrap();
            session.authorize = Some(authorize);
            session.actions.list.push(photocraft_engine::actions_cmds::Action::new(
                "Profiles",
                vec![("edit.installedProfiles".into(), serde_json::json!({})), ("layer.new.layer".into(), serde_json::json!({}))],
            ));
            let before = session.active().unwrap().doc.clone();
            let revision = session.active().unwrap().revision;
            let history = session.active().unwrap().history.past_len();
            // Also exercise the hook through direct Session dispatch, without Headless's gate.
            let error = session.execute("edit.installedProfiles", serde_json::json!({})).unwrap_err();
            assert!(error.to_string().contains("ambient filesystem paths"), "{error}");
            let result = session.execute("actions.play", serde_json::json!({"action": "Profiles"})).unwrap();
            assert_eq!(result["ran"], 0);
            assert_eq!(result["failed"]["step"], 0);
            assert_eq!(result["failed"]["id"], "edit.installedProfiles");
            assert!(result["failed"]["error"].as_str().unwrap().contains("ambient filesystem paths"), "{result}");
            let after = session.active().unwrap();
            assert_eq!(after.doc, before);
            assert_eq!(after.revision, revision);
            assert_eq!(after.history.past_len(), history);
        }
    }

    #[test]
    fn apply_data_set_is_judged_by_the_values_it_applies() {
        let mut headless = crate::Headless::new();
        headless.command_run("file.new", serde_json::json!({"width": 8, "height": 8})).unwrap();
        let layer = headless.command_run("layer.new.layer", serde_json::json!({})).unwrap()["layer"].clone();
        let defs = [
            serde_json::json!({"name": "shown", "layer": layer, "type": "visibility"}),
            serde_json::json!({"name": "photo", "layer": layer, "type": "pixelReplacement"}),
        ];
        headless.command_run("image.variables.define", serde_json::json!({"defs": defs})).unwrap();
        let sets = serde_json::json!({"dataSets": [
            {"name": "hidden", "values": [{"variable": "shown", "kind": "visibility", "value": false}]},
            {"name": "swap", "values": [
                {"variable": "shown", "kind": "visibility", "value": false},
                {"variable": "photo", "kind": "pixels", "value": "/outside/photo.png"},
            ]},
        ]});
        headless.command_run("image.variables.dataSets", sets).unwrap();
        let visible = |headless: &crate::Headless| headless.session.active().unwrap().doc.layers.iter().all(|l| l.visible);
        let refused = headless.command_run("image.applyDataSet", serde_json::json!({"name": "swap"})).unwrap_err();
        assert!(refused.to_string().contains("ambient filesystem paths"), "{refused}");
        assert!(visible(&headless), "a refused data set applies none of its values");
        assert!(headless.command_run("image.applyDataSet", serde_json::json!({"name": "hidden"})).is_ok());
        assert!(!visible(&headless));
    }

    #[test]
    fn automation_rejects_ambient_path_commands_and_preferences() {
        for (id, params) in [
            ("brush.presets.importAbr", serde_json::json!({"path": "/outside/set.abr"})),
            ("gradient.presets.importGrd", serde_json::json!({"path": "/outside/set.grd"})),
            ("plugin.install", serde_json::json!({"path": "/outside/plugin.wasm"})),
            ("plugin.reload", serde_json::json!({"path": "/outside/plugins"})),
            ("plugin.reload", serde_json::json!({})),
            ("plugin.install", serde_json::json!({"path": " "})),
            ("swatches.import", serde_json::json!({"path": "/outside/set.aco"})),
            ("swatches.export", serde_json::json!({"path": "/outside/set.ase"})),
            ("actions.load", serde_json::json!({"path": "/outside/set.atn"})),
            ("actions.save", serde_json::json!({"path": "/outside/set.atn"})),
        ] {
            assert!(authorize_engine_command(id, &params).is_err(), "{id}: {params}");
        }
        for (id, params) in [
            ("brush.presets.importAbr", serde_json::json!({"data": "QUJD"})),
            ("gradient.presets.importGrd", serde_json::json!({"data": "QUJD"})),
            ("plugin.install", serde_json::json!({"data": "QUJD"})),
            ("swatches.import", serde_json::json!({"data": "QUJD"})),
            ("swatches.export", serde_json::json!({"format": "ase"})),
            ("actions.load", serde_json::json!({"data": "QUJD"})),
            ("actions.save", serde_json::json!({"set": "S"})),
        ] {
            assert!(authorize_engine_command(id, &params).is_ok(), "{id}: {params}");
        }
        for path in [
            "",
            "colorSettings",
            "colorSettings.workingRgb",
            "scriptEvents",
            "scriptEvents.enabled",
            ".scriptEvents",
            ".scriptEvents.enabled",
            "historyLog.filePath",
            ".historyLog.filePath",
            "plugIns.additionalPluginsFolder",
            "scratchDisks.disks",
            ".",
            "..historyLog.filePath",
        ] {
            assert!(authorize_engine_command("prefs.set", &serde_json::json!({"path": path, "value": {}})).is_err(), "{path}");
        }
        assert!(
            authorize_engine_command("prefs.set", &serde_json::json!({"values": {"interface.language": "fr", "historyLog.filePath": "/outside/log"}})).is_err()
        );
        assert!(authorize_engine_command("prefs.set", &serde_json::json!({"path": "interface.language", "value": "fr"})).is_ok());
    }

    #[test]
    fn registry_filesystem_path_params_are_classified() {
        let probe = serde_json::json!({
            "path": "/outside/photocraft-probe",
            "file": "/outside/photocraft-probe.icc",
            "mapPath": "/outside/photocraft-probe.png",
            "profile": "/outside/photocraft-probe.icc",
            "workingRgb": "/outside/photocraft-probe.icc",
            "workingCmyk": "/outside/photocraft-probe.icc",
            "workingGray": "/outside/photocraft-probe.icc",
            "input": "/outside/photocraft-input",
            "output": "/outside/photocraft-output",
            "paths": ["/outside/photocraft-probe.psd"],
        });
        let preference_probe = serde_json::json!({"path": ".scriptEvents", "value": {}});
        let unclassified: Vec<_> = photocraft_engine::command_specs()
            .iter()
            .filter(|spec| documents_filesystem_path_params(spec.id, spec.params))
            .filter(|spec| {
                let params = if spec.id == "prefs.set" { &preference_probe } else { &probe };
                authorize_engine_command(spec.id, params).is_ok()
            })
            .map(|spec| spec.id)
            .collect();
        assert!(unclassified.is_empty(), "filesystem path parameters in the command registry need an automation policy: {unclassified:?}");
    }

    #[test]
    fn inline_vector_paste_needs_no_filesystem_authority() {
        let params = serde_json::json!({
            "svg": "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2\" height=\"2\"><rect width=\"2\" height=\"2\"/></svg>",
            "mode": "path",
        });
        assert!(authorize_engine_command("edit.pasteVector", &params).is_ok());
        assert!(authorize_desktop_engine_command("edit.pasteVector", &params).is_ok());
        let mut headless = crate::Headless::new();
        headless.command_run("file.new", serde_json::json!({"width": 8, "height": 8})).unwrap();
        let layers = headless.session.active().unwrap().doc.layer_count();
        let result = headless.command_run("edit.pasteVector", params).unwrap();
        assert_eq!(result["mode"], "path");
        assert_eq!(headless.session.active().unwrap().doc.paths.len(), 1);
        assert_eq!(headless.session.active().unwrap().doc.layer_count(), layers);
        // The vector-path registry exception does not authorise any host-file command.
        assert!(headless.command_run("file.placeEmbedded", serde_json::json!({"path": "/outside/source.svg"})).is_err());
    }

    #[test]
    fn shape_parity_commands_use_document_geometry_without_filesystem_authority() {
        let mut headless = crate::Headless::new();
        headless.command_run("file.new", serde_json::json!({"width": 16, "height": 16})).unwrap();
        let created = headless.command_run("shape.create", serde_json::json!({"kind": "rect", "rect": [2, 2, 4, 4]})).unwrap();
        let geometry = headless.command_run("shape.info", serde_json::json!({"layer": created["layer"]})).unwrap()["path"].clone();
        // A saved document path may have a filename-looking name. Resolution stays inside
        // the document; it never opens a host file with that name.
        let name = "/outside/photocraft-probe.svg";
        headless.command_run("path.set", serde_json::json!({"name": name, "path": geometry})).unwrap();
        for (id, params) in [
            ("path.align", serde_json::json!({"name": name, "action": "left", "to": "canvas"})),
            ("path.arrange", serde_json::json!({"name": name, "action": "front"})),
            ("path.merge", serde_json::json!({"name": name})),
        ] {
            let spec = photocraft_engine::command_specs().iter().find(|spec| spec.id == id).unwrap();
            assert!(!documents_filesystem_path_params(id, spec.params), "{id}");
            assert!(authorize_engine_command(id, &params).is_ok(), "{id}");
            assert!(authorize_desktop_engine_command(id, &params).is_ok(), "{id}");
            assert!(headless.command_run(id, params).is_ok(), "{id}");
            assert!(headless.session.active().unwrap().doc.paths.iter().any(|path| path.name == name));
        }
        // Pixel shape drawing accepts inline vector data, not a file path. Select a new
        // raster layer so the command has an ordinary document-owned pixel destination.
        headless.command_run("layer.new.layer", serde_json::json!({"name": "Pixel target"})).unwrap();
        let params = serde_json::json!({"kind": "path", "path": geometry, "color": "#ff0000"});
        let spec = photocraft_engine::command_specs().iter().find(|spec| spec.id == "shape.drawPixels").unwrap();
        assert!(!documents_filesystem_path_params(spec.id, spec.params));
        assert!(authorize_engine_command(spec.id, &params).is_ok());
        assert!(authorize_desktop_engine_command(spec.id, &params).is_ok());
        assert!(headless.command_run(spec.id, params).is_ok());
        assert!(headless.command_run(spec.id, serde_json::json!({"kind": "path", "path": name})).is_err());
        assert!(headless.command_run("file.placeEmbedded", serde_json::json!({"path": name})).is_err());
        assert!(headless.command_run("layer.smartObjects.replaceContents", serde_json::json!({"path": name})).is_err());
    }

    #[test]
    fn svg_smart_handoff_is_memory_only_and_linked_sources_stay_denied() {
        let mut headless = crate::Headless::new();
        headless.command_run("file.new", serde_json::json!({"width": 16, "height": 16})).unwrap();
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>"#;
        let inserted = headless.command_run("edit.pasteVector", serde_json::json!({"svg": svg, "mode": "smartObject"})).unwrap();
        let layer = inserted["layers"][0].clone();
        let handoff = headless.command_run("layer.smartObjects.exportSvg", serde_json::json!({"layer": layer})).unwrap();
        assert_eq!(handoff["svg"], svg);
        let params = serde_json::json!({
            "document": handoff["document"], "layer": layer,
            "contentsId": handoff["contentsId"], "fingerprint": handoff["fingerprint"],
            "svg": svg.replace("red", "blue"),
        });
        assert!(authorize_desktop_engine_command("layer.smartObjects.updateSvg", &params).is_ok());
        assert_eq!(headless.command_run("layer.smartObjects.updateSvg", params.clone()).unwrap()["updated"], true);
        for id in ["layer.smartObjects.exportSvg", "layer.smartObjects.updateSvg"] {
            let mut ambient = params.clone();
            ambient["path"] = serde_json::json!("/outside/source.svg");
            assert!(authorize_engine_command(id, &ambient).is_err(), "{id}");
            assert!(authorize_desktop_engine_command(id, &ambient).is_err(), "{id}");
            assert!(headless.command_run(id, ambient).is_err(), "{id}");
        }
        let doc = Arc::make_mut(&mut headless.session.active_mut().unwrap().doc);
        let id = photocraft_doc::LayerId(layer.as_u64().unwrap());
        let photocraft_doc::LayerContent::Smart(sm) = &mut doc.layer_mut(id).unwrap().content else { panic!("SVG smart object") };
        sm.source = photocraft_doc::SmartSource::Linked { path: "/outside/source.svg".into() };
        let before = headless.session.active().unwrap().doc.clone();
        assert!(headless.command_run("layer.smartObjects.exportSvg", serde_json::json!({"layer": layer})).is_err());
        assert!(headless.command_run("layer.smartObjects.updateSvg", params).is_err());
        assert_eq!(headless.session.active().unwrap().doc, before);
        assert!(headless.command_run("layer.smartObjects.exportContents", serde_json::json!({"layer": layer, "path": "/outside/export.svg"})).is_err());
    }

    #[test]
    fn svg_update_cannot_replay_desktop_lut_file_in_any_shared_instance() {
        let (_, outside, _) = roots("svg-stored-filter");
        let lut = outside.join("private.cube");
        let cube = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        std::fs::write(&lut, cube).unwrap();
        for shared_only in [false, true] {
            let mut headless = crate::Headless::new();
            headless.command_run("file.new", serde_json::json!({"width": 16, "height": 16})).unwrap();
            let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>"#;
            let pasted = headless.command_run("edit.pasteVector", serde_json::json!({"svg": svg,"mode":"smartObject"})).unwrap();
            let target = photocraft_doc::LayerId(pasted["layers"][0].as_u64().unwrap());
            let filtered = if shared_only {
                let doc = Arc::make_mut(&mut headless.session.active_mut().unwrap().doc);
                let duplicate = doc.layer(target).unwrap().duplicate();
                let id = duplicate.id;
                doc.layers.push(duplicate);
                id
            } else {
                target
            };
            headless.session.select_layer(filtered).unwrap();
            // Record a natural desktop Color Lookup operation with its original file params,
            // then restore the untrusted automation hook as for an imported document.
            let authorization = headless.session.authorize.take();
            headless.session.execute("image.adjustments.colorLookup", serde_json::json!({"file":lut.to_string_lossy()})).unwrap();
            headless.session.authorize = authorization;
            let photocraft_doc::LayerContent::Smart(sm) = &headless.session.active().unwrap().doc.layer(filtered).unwrap().content else {
                panic!("smart object")
            };
            assert_eq!(sm.smart_filters.last().unwrap().params["file"], lut.to_string_lossy().as_ref());
            let handoff = headless.command_run("layer.smartObjects.exportSvg", serde_json::json!({"layer":target.0})).unwrap();
            let params = serde_json::json!({"document":handoff["document"],"layer":target.0,"contentsId":handoff["contentsId"],"fingerprint":handoff["fingerprint"],"svg":svg.replace("red","blue")});
            let before = headless.session.active().unwrap().doc.clone();
            let revision = headless.session.active().unwrap().revision;
            let error = headless.command_run("layer.smartObjects.updateSvg", params.clone()).unwrap_err();
            assert!(error.to_string().contains("ambient filesystem paths"), "{error}");
            assert_eq!(headless.session.active().unwrap().doc, before);
            assert_eq!(headless.session.active().unwrap().revision, revision);
            // A hidden stored filter is not executed, so its unused filename grants no access.
            let doc = Arc::make_mut(&mut headless.session.active_mut().unwrap().doc);
            let photocraft_doc::LayerContent::Smart(sm) = &mut doc.layer_mut(filtered).unwrap().content else { panic!("smart object") };
            sm.smart_filters.last_mut().unwrap().visible = false;
            assert_eq!(headless.command_run("layer.smartObjects.updateSvg", params).unwrap()["updated"], true);
        }
    }

    #[test]
    fn svg_update_keeps_inline_and_builtin_smart_filters_working() {
        let cube = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        for filter_params in [serde_json::json!({"lut":"warm"}), serde_json::json!({"data":cube,"fileName":"look.cube"})] {
            let mut headless = crate::Headless::new();
            headless.command_run("file.new", serde_json::json!({"width":16,"height":16})).unwrap();
            let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>"#;
            let pasted = headless.command_run("edit.pasteVector", serde_json::json!({"svg":svg,"mode":"smartObject"})).unwrap();
            let id = photocraft_doc::LayerId(pasted["layers"][0].as_u64().unwrap());
            headless.command_run("image.adjustments.colorLookup", filter_params).unwrap();
            let handoff = headless.command_run("layer.smartObjects.exportSvg", serde_json::json!({"layer":id.0})).unwrap();
            let result = headless.command_run("layer.smartObjects.updateSvg",serde_json::json!({"document":handoff["document"],"layer":id.0,"contentsId":handoff["contentsId"],"fingerprint":handoff["fingerprint"],"svg":svg.replace("red","blue")})).unwrap();
            assert_eq!(result["updated"], true);
            let photocraft_doc::LayerContent::Smart(sm) = &headless.session.active().unwrap().doc.layer(id).unwrap().content else { panic!("smart object") };
            assert_eq!(sm.smart_filters.len(), 1);
            assert!(!sm.cache.as_ref().unwrap().content_bounds().is_empty());
        }
    }

    fn documents_filesystem_path_params(id: &str, params: &str) -> bool {
        if matches!(id, "prefs.get" | "prefs.reset") {
            return false;
        }
        // These registry descriptions refer to document vector paths, not host files.
        if matches!(
            id,
            "filter.blurGallery.pathBlur"
                | "filter.render.flame"
                | "shape.create"
                | "shape.drawPixels"
                | "shape.edit"
                | "shape.info"
                | "shape.presets.list"
                | "shape.presets.new"
                | "path.list"
                | "path.info"
                | "path.set"
                | "path.merge"
                | "path.align"
                | "path.arrange"
                | "path.transform"
                | "path.moveAnchors"
                | "path.moveHandle"
                | "path.bendSegment"
                | "path.convertPoint"
                | "path.addAnchor"
                | "path.deleteAnchor"
                | "path.clippingPath.set"
                | "path.rename"
                | "path.stroke"
                | "type.attachToPath"
                | "type.detachFromPath"
                | "select.toWorkPath"
                | "layer.vectorMask.add"
                | "layer.vectorMask.edit"
                | "layer.vectorMask.info"
                | "paint.symmetryFromPath"
                | "edit.defineCustomShape"
                | "edit.pasteVector"
                | "layer.combineShapes.unite"
                | "layer.combineShapes.subtractFrontShape"
                | "layer.combineShapes.intersectShapeAreas"
                | "layer.combineShapes.excludeOverlappingShapes"
                | "layer.combineShapes.mergeShapeComponents"
        ) || id.starts_with("layer.newFillLayer.")
            || id.starts_with("layer.newAdjustmentLayer.")
        {
            // New fill and adjustment layers take a vector path as their vector mask (#1419).
            return false;
        }
        params.to_ascii_lowercase().contains("path")
    }

    #[test]
    fn whole_preferences_update_cannot_enable_automation_script_events() {
        let mut headless = crate::headless::Headless::new();
        let result = headless.command_run(
            "prefs.set",
            serde_json::json!({
                "path": "",
                "value": {
                    "scriptEvents": {
                        "enabled": true,
                        "bindings": [{
                            "event": "newDocument",
                            "steps": [["file.saveACopy", {"path": "/outside/canary.psd"}]]
                        }]
                    }
                }
            }),
        );
        assert!(result.is_err());

        for (id, params) in [
            ("brush.presets.importAbr", serde_json::json!({"path": "/outside/set.abr"})),
            ("gradient.presets.importGrd", serde_json::json!({"path": "/outside/set.grd"})),
            ("plugin.install", serde_json::json!({"path": "/outside/plugin.wasm"})),
            ("plugin.reload", serde_json::json!({"path": "/outside/plugins"})),
            ("prefs.set", serde_json::json!({"path": "historyLog.filePath", "value": "/outside/log"})),
            ("prefs.set", serde_json::json!({"values": {"interface.language": "fr", "historyLog.filePath": "/outside/log"}})),
        ] {
            assert!(headless.command_run(id, params).is_err(), "{id}");
        }
        headless.command_run("file.new", serde_json::json!({"width": 5, "height": 5})).unwrap();
        assert!(headless.session.file_menu.event_log.is_empty());
    }

    /// #2176: an absolute path beneath the root names the same file as its relative path; every
    /// other absolute path is refused before any file effect.
    #[test]
    fn absolute_paths_beneath_the_root_are_translated_and_others_refused() {
        let (inside, outside, workspace) = roots("absolute-paths");
        let abs = |p: &Path| p.to_string_lossy().into_owned();

        workspace.write(&abs(&inside.join("a.bin")), b"one").unwrap();
        assert_eq!(std::fs::read(inside.join("a.bin")).unwrap(), b"one");
        assert_eq!(workspace.read(&abs(&inside.join("a.bin"))).unwrap(), b"one");
        // Forward slashes too (on Windows the native spelling uses `\`).
        assert_eq!(workspace.read(&abs(&inside.join("a.bin")).replace('\\', "/")).unwrap(), b"one");
        std::fs::create_dir(inside.join("sub")).unwrap();
        workspace.write(&abs(&inside.join("sub").join("b.bin")), b"two").unwrap();
        assert_eq!(std::fs::read(inside.join("sub/b.bin")).unwrap(), b"two");
        assert_eq!(workspace.read("sub/b.bin").unwrap(), b"two", "relative paths still work");

        let refused = |path: String| {
            let error = workspace.write(&path, b"x").unwrap_err().to_string();
            assert!(error.contains("automation path rejected"), "{path}: {error}");
        };
        refused(abs(&outside.join("escape.bin")));
        refused(abs(&inside.join("..").join("outside").join("traversal.bin")));
        refused(abs(&inside));
        refused(abs(&inside.join("CON")));
        refused(format!("{}:hidden", abs(&inside.join("a.bin"))));
        // A sibling whose name merely starts with the root's name is not beneath it.
        let sibling = inside.with_file_name("inside-sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        refused(abs(&sibling.join("lookalike.bin")));
        refused(format!("{}-sibling/lookalike.bin", abs(&inside)));
        // Climbing back out through a subdirectory is refused too.
        refused(abs(&inside.join("sub").join("..").join("..").join("outside").join("climb.bin")));
        assert!(!sibling.join("lookalike.bin").exists());
        assert!(!outside.join("climb.bin").exists());
        assert!(!outside.join("escape.bin").exists());
        assert!(!outside.join("traversal.bin").exists());
        assert_eq!(std::fs::read(inside.join("a.bin")).unwrap(), b"one");
    }

    /// Each authority is checked against its own root, and none granted refuses absolute paths.
    #[test]
    fn absolute_paths_are_checked_against_the_authority_s_own_root() {
        let (inside, outside, _) = roots("absolute-authority");
        let workspace = AuthorizedWorkspace::new(Some(&outside), Some(&inside)).unwrap();
        let target = inside.join("written.bin");
        workspace.write(&target.to_string_lossy(), b"w").unwrap();
        assert!(workspace.read(&target.to_string_lossy()).is_err(), "not under the read root");
        let none = AuthorizedWorkspace::default();
        assert!(none.read(&target.to_string_lossy()).is_err());
    }
}
