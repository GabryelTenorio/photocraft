//! Headless backend: a `photocraft_engine::Session` plus file I/O. Synchronous
//! and UI-free; the MCP server and the CLI both drive it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use photocraft_engine::{Session, command_specs, file_cmds};
use photocraft_format::PcraftWriter;
use photocraft_io::ExportOptions;
use serde_json::{Value, json};

use crate::workspace::{ExportSequence, authorize_engine_command, authorize_engine_step};
use crate::{AuthorizedWorkspace, AutomationError, files};

fn untrusted(filesystem: Filesystem) -> Headless {
    let mut session = Session::new();
    session.authorize = Some(authorize_engine_step);
    Headless { session, writers: HashMap::new(), filesystem }
}

/// The key an automation open dedupes on by path spelling: `/`-separated components with empty
/// and `.` components dropped and `..` resolved lexically, without consulting the filesystem.
/// A path that would climb above its own start keeps its raw spelling, so comparison falls back
/// to exact text. A trusted-local open also dedupes by file identity, so a hard-link alias
/// activates the open document. Either way activation only switches documents — the file is
/// never re-read — so the reply warns.
fn dedupe_key(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return path.to_string();
                }
            }
            _ => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if path.starts_with('/') { format!("/{joined}") } else { joined }
}

/// On-disk identity of a path: device and file index, so hard links and spelling variants of
/// one file are seen as the same file. `None` when the file cannot be statted.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some((m.dev(), m.ino()))
}

#[cfg(target_os = "windows")]
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some((m.volume_serial_number(), m.file_index().unwrap_or(0)))
}

/// Other targets dedupe by path spelling only.
#[cfg(not(any(all(unix, not(target_arch = "wasm32")), target_os = "windows")))]
fn file_identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

enum Filesystem {
    Denied,
    TrustedLocal,
    Workspace(AuthorizedWorkspace),
}

/// A headless editing session.
pub struct Headless {
    pub session: Session,
    /// Incremental `.pcraft` writers per document id.
    writers: HashMap<u64, PcraftWriter>,
    filesystem: Filesystem,
}

impl Default for Headless {
    fn default() -> Self {
        untrusted(Filesystem::Denied)
    }
}

impl Headless {
    /// Create an automation session with no filesystem authority.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a session for an explicit local CLI invocation. The CLI caller,
    /// not a remote automation client, supplies these host paths. Nested
    /// `actions.play` steps are not re-checked: this caller is already trusted.
    pub fn trusted_local() -> Self {
        Self { session: Session::new(), writers: HashMap::new(), filesystem: Filesystem::TrustedLocal }
    }

    /// Create an automation session with capability-scoped file access.
    /// Each step of `actions.play` is checked with [`authorize_engine_step`].
    pub fn with_workspace(workspace: AuthorizedWorkspace) -> Self {
        untrusted(Filesystem::Workspace(workspace))
    }

    /// Apply background jobs that finished since the last request, so every request (save,
    /// export, inspect, preview, `session.list`, commands) sees their result. Cheap when no job
    /// runs. The JSON-lines server and the MCP server call it before each request.
    pub fn sync_jobs(&mut self) {
        self.session.poll_jobs();
    }

    fn doc_index(&self, index: Option<usize>) -> Result<usize, AutomationError> {
        match index {
            Some(i) if i < self.session.documents().len() => Ok(i),
            Some(i) => Err(AutomationError::BadRequest(format!("no document at index {i}"))),
            None => self.session.active_index().ok_or_else(|| AutomationError::BadRequest("no document open".into())),
        }
    }

    /// Open a file and make it the active document. A path that is already an open document's
    /// file — by path spelling, or by file identity for trusted-local opens — activates that
    /// document instead of loading a second copy, and the reply's warnings say the file was not
    /// re-read.
    pub fn open(&mut self, path: &Path) -> Result<Value, AutomationError> {
        let requested = path.to_str().ok_or_else(|| AutomationError::BadRequest("automation paths must be valid UTF-8".into()))?;
        let key = dedupe_key(requested);
        // A stored path that resolves to the requested file by identity (hard link, symlink,
        // alternate spelling) dedupes too; the capability roots expose no metadata, so
        // identity matching is a trusted-local open only.
        let identity = if matches!(&self.filesystem, Filesystem::TrustedLocal) { file_identity(path) } else { None };
        let mut alias = false;
        let found = self.session.documents().iter().position(|st| {
            let Some(p) = st.path.as_deref() else { return false };
            if dedupe_key(p) == key {
                return true;
            }
            let same_file = identity.is_some() && file_identity(Path::new(p)) == identity;
            alias |= same_file;
            same_file
        });
        if let Some(index) = found {
            let _ = self.session.set_active(index);
            let d = &self.session.documents()[index];
            let mut warnings =
                vec!["already open; the document was activated without re-reading the file — close it first to load the current bytes".to_string()];
            if alias {
                warnings.push("the requested path is another name for the open document's file".to_string());
            }
            return Ok(json!({
                "index": index,
                "name": d.doc.name,
                "width": d.doc.size.width,
                "height": d.doc.size.height,
                "layers": d.doc.layer_count(),
                "warnings": warnings,
                "deduped": true,
            }));
        }
        let mut o = match &self.filesystem {
            Filesystem::Denied => return Err(AutomationError::BadRequest("automation filesystem access is not granted: read authority is absent".into())),
            Filesystem::TrustedLocal => files::open(path)?,
            Filesystem::Workspace(workspace) => {
                let bytes = workspace.read(requested)?;
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or(requested);
                files::open_bytes(name, &bytes)?
            }
        };
        let path = match file_cmds::template_name(&self.session, requested) {
            Some(untitled) => {
                o.document.name = untitled;
                None
            }
            None => Some(requested.to_string()),
        };
        let index = self.session.add_document(o.document, path.filter(|_| !o.source_read_only));
        if let Some(st) = self.session.active_mut() {
            st.source_read_only = o.source_read_only;
        }
        let d = &self.session.documents()[index];
        let mut warnings = o.warnings;
        warnings.extend(photocraft_engine::type_extra_cmds::missing_font_warnings(&d.doc));
        Ok(json!({
            "index": index,
            "name": d.doc.name,
            "width": d.doc.size.width,
            "height": d.doc.size.height,
            "layers": d.doc.layer_count(),
            "warnings": warnings,
        }))
    }

    /// Place a file into a document as a Smart Object layer: the capability-scoped counterpart
    /// of `file.placeEmbedded`, which reads an ambient path and is therefore refused in
    /// automation (#2810). `path` resolves beneath the read root the way `open` resolves one,
    /// and `params` carries the command's own options (`scale`, `fit`, `center`).
    pub fn place(&mut self, index: Option<usize>, path: &str, params: &Value) -> Result<Value, AutomationError> {
        let i = self.doc_index(index)?;
        if self.session.active_index() != Some(i) && !self.session.set_active(i) {
            return Err(AutomationError::BadRequest(format!("no document at index {i}")));
        }
        let mut opts = params.as_object().cloned().unwrap_or_default();
        opts.insert("path".into(), json!(path));
        // Where ambient paths are allowed the command reads the file itself, as everywhere else.
        if matches!(&self.filesystem, Filesystem::TrustedLocal) {
            return self.command_run("file.placeEmbedded", Value::Object(opts));
        }
        let bytes = match &self.filesystem {
            Filesystem::Workspace(workspace) => workspace.read(path)?,
            _ => {
                return Err(AutomationError::BadRequest("automation filesystem access is not granted: read authority is absent".into()));
            }
        };
        let name = Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or(path);
        self.sync_jobs();
        // The bytes are already in memory and the path resolved beneath the root capability, so
        // placing them needs no filesystem check of its own (`place_bytes` is the command body).
        Ok(file_cmds::place_bytes(&mut self.session, name, bytes, None, &Value::Object(opts))?)
    }

    /// Export each exported layer of a document as its own file beneath the write root: the
    /// capability-scoped `file.export.layersToFiles`, which takes an ambient `dir` and is
    /// therefore refused in automation (#3216). The bytes come from the command's own core, so
    /// both paths write the same files. The write capability refuses to overwrite a file that
    /// already exists, where the ambient command would, and it creates the directory before the
    /// export runs, so a refused export can leave an empty directory behind — never a file.
    pub fn export_layers(&mut self, index: Option<usize>, dir: &str, params: &Value) -> Result<Value, AutomationError> {
        let i = self.doc_index(index)?;
        if self.session.active_index() != Some(i) && !self.session.set_active(i) {
            return Err(AutomationError::BadRequest(format!("no document at index {i}")));
        }
        if matches!(&self.filesystem, Filesystem::TrustedLocal) {
            let mut opts = params.as_object().cloned().unwrap_or_default();
            opts.insert("dir".into(), json!(dir));
            return self.command_run("file.export.layersToFiles", Value::Object(opts));
        }
        self.sync_jobs();
        let workspace = match &self.filesystem {
            Filesystem::Workspace(workspace) => workspace,
            _ => return Err(AutomationError::BadRequest("automation filesystem access is not granted: write authority is absent".into())),
        };
        let mut outputs = workspace.export_sequence(dir)?;
        let written = file_cmds::export_layers_with(&self.session, params, |name, bytes| {
            outputs.write(name, bytes).map_err(|e| photocraft_engine::EngineError::Other(e.to_string()))
        });
        exported(dir, written, &mut outputs)
    }

    /// Export each data set of a document as its own flattened file beneath the write root: the
    /// capability-scoped `file.export.dataSetsAsFiles` (#3216). As in the command, the whole
    /// batch is planned first and colliding names are refused before any file is written; the
    /// directory itself is created before that, so a refused batch can leave it empty behind.
    pub fn export_data_sets(&mut self, index: Option<usize>, dir: &str, params: &Value) -> Result<Value, AutomationError> {
        let i = self.doc_index(index)?;
        if self.session.active_index() != Some(i) && !self.session.set_active(i) {
            return Err(AutomationError::BadRequest(format!("no document at index {i}")));
        }
        if matches!(&self.filesystem, Filesystem::TrustedLocal) {
            let mut opts = params.as_object().cloned().unwrap_or_default();
            opts.insert("dir".into(), json!(dir));
            return self.command_run("file.export.dataSetsAsFiles", Value::Object(opts));
        }
        self.sync_jobs();
        let workspace = match &self.filesystem {
            Filesystem::Workspace(workspace) => workspace,
            _ => return Err(AutomationError::BadRequest("automation filesystem access is not granted: write authority is absent".into())),
        };
        let mut outputs = workspace.export_sequence(dir)?;
        let written = photocraft_engine::export_data_sets_with(&self.session, params, |name, bytes| {
            outputs.write(name, bytes).map_err(|e| photocraft_engine::EngineError::Other(e.to_string()))
        });
        exported(dir, written, &mut outputs)
    }

    /// Import a data set CSV from the read root: the capability-scoped
    /// `file.import.variableDataSets`, which takes an ambient `path` and is therefore refused
    /// in automation (#3216). The file is read beneath the root the way `open` reads one and
    /// handed to the command's own parser over the text it already has.
    pub fn import_data_sets(&mut self, index: Option<usize>, path: &str, params: &Value) -> Result<Value, AutomationError> {
        let i = self.doc_index(index)?;
        if self.session.active_index() != Some(i) && !self.session.set_active(i) {
            return Err(AutomationError::BadRequest(format!("no document at index {i}")));
        }
        if matches!(&self.filesystem, Filesystem::TrustedLocal) {
            let mut opts = params.as_object().cloned().unwrap_or_default();
            opts.insert("path".into(), json!(path));
            return self.command_run("file.import.variableDataSets", Value::Object(opts));
        }
        let text = match &self.filesystem {
            Filesystem::Workspace(workspace) => {
                let bytes = workspace.read(path)?;
                String::from_utf8(bytes).map_err(|e| AutomationError::BadRequest(format!("automation read `{path}` is not valid UTF-8: {e}")))?
            }
            _ => return Err(AutomationError::BadRequest("automation filesystem access is not granted: read authority is absent".into())),
        };
        self.sync_jobs();
        Ok(photocraft_engine::import_data_sets_text(&mut self.session, &text, params)?)
    }

    /// Save (`.pcraft` or any export format by extension). With no path,
    /// saves to the document's own path.
    pub fn save(&mut self, index: Option<usize>, path: Option<&Path>, format: Option<&str>, opts: &ExportOptions) -> Result<Value, AutomationError> {
        let i = self.doc_index(index)?;
        let (stored_path, doc) = {
            let state = &self.session.documents()[i];
            (state.path.clone(), state.doc.clone())
        };
        let target: PathBuf = match (path, stored_path.as_deref()) {
            (Some(p), _) => p.to_path_buf(),
            // Like the desktop's File › Save: without a new path, only a layered file is written
            // back, in its own format; a flattened or converted copy never replaces it (#416).
            (None, Some(p)) => {
                let own = file_cmds::extension(p);
                let written = format.map(|f| f.trim_start_matches('.').to_ascii_lowercase()).or_else(|| own.clone());
                if !file_cmds::saves_in_place(p) || written != own {
                    return Err(AutomationError::BadRequest(format!(
                        "pass `path`: without one, only a PSD, PSB or .pcraft file is written back, in its own format, so `{p}` was left unchanged"
                    )));
                }
                PathBuf::from(p)
            }
            (None, None) => {
                return Err(AutomationError::BadRequest("document has no path; pass `path`".into()));
            }
        };
        let target_text = target.to_str().ok_or_else(|| AutomationError::BadRequest("automation paths must be valid UTF-8".into()))?;
        let mut warnings = match &self.filesystem {
            Filesystem::Denied => return Err(AutomationError::BadRequest("automation filesystem access is not granted: write authority is absent".into())),
            Filesystem::TrustedLocal => {
                let writer = self.writers.entry(doc.id.0).or_default();
                files::save(&doc, &target, format, opts, Some(writer))?
            }
            Filesystem::Workspace(workspace) => {
                let (bytes, warnings) = files::save_bytes(&doc, target_text, format, opts)?;
                workspace.write(target_text, &bytes)?;
                warnings
            }
        };
        warnings.extend(photocraft_engine::type_extra_cmds::missing_font_warnings(&doc));
        // A layered document save (PSD, PSB, OpenRaster or .pcraft) is a full write, not a flattened copy:
        // record the current revision as saved so the session stops reporting `dirty`, and make
        // the file the document's path. A flat export (PNG, JPEG, …) is a copy and leaves both
        // alone (#1547).
        let ext =
            format.map(|f| f.trim_start_matches('.').to_ascii_lowercase()).or_else(|| target.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()));
        if ext.as_deref().is_some_and(file_cmds::layered_extension) {
            // Saving by index must not retarget the next automation command.
            let previously_active = self.session.active_index();
            self.session.set_active(i);
            if let Some(st) = self.session.active_mut() {
                st.saved_to(target.to_string_lossy().into_owned());
            }
            if let Some(active) = previously_active {
                self.session.set_active(active);
            }
        }
        Ok(json!({ "path": target.to_string_lossy(), "warnings": warnings }))
    }

    /// Write rendered bytes through the configured write authority.
    pub fn write_render(&self, path: &Path, bytes: &[u8]) -> Result<(), AutomationError> {
        let requested = path.to_str().ok_or_else(|| AutomationError::BadRequest("automation paths must be valid UTF-8".into()))?;
        match &self.filesystem {
            Filesystem::Denied => Err(AutomationError::BadRequest("automation filesystem access is not granted: write authority is absent".into())),
            Filesystem::TrustedLocal => photocraft_format::atomic_write(path, bytes).map_err(|error| AutomationError::Io(error.to_string())),
            Filesystem::Workspace(workspace) => workspace.write(requested, bytes),
        }
    }

    pub fn inspect(&self, index: Option<usize>) -> Result<Value, AutomationError> {
        let i = self.doc_index(index)?;
        Ok(photocraft_engine::inspect::document(&self.session.documents()[i]))
    }

    pub fn render_png(&self, index: Option<usize>, max_side: u32) -> Result<Vec<u8>, AutomationError> {
        let i = self.doc_index(index)?;
        let doc = &self.session.documents()[i].doc;
        if !matches!(&self.filesystem, Filesystem::TrustedLocal) {
            crate::budgets::check_preview(doc.size.width, doc.size.height, max_side)?;
        }
        let png = files::render_png(doc, max_side)?;
        if !matches!(&self.filesystem, Filesystem::TrustedLocal) {
            crate::budgets::check_png(png.len())?;
        }
        Ok(png)
    }

    pub fn session_list(&self) -> Value {
        photocraft_engine::inspect::session(&self.session)
    }

    pub fn select(&mut self, index: usize) -> Result<Value, AutomationError> {
        if self.session.set_active(index) { Ok(self.session_list()) } else { Err(AutomationError::BadRequest(format!("no document at index {index}"))) }
    }

    pub fn close(&mut self, index: Option<usize>) -> Result<Value, AutomationError> {
        let i = self.doc_index(index)?;
        if let Some(d) = self.session.close(i) {
            self.writers.remove(&d.doc.id.0);
        }
        Ok(self.session_list())
    }

    pub fn command_list(&self) -> Value {
        Value::Array(command_specs().iter().map(|c| photocraft_engine::commands::list_entry(&self.session, c, self.session.disabled_reason(c.id))).collect())
    }

    pub fn command_run(&mut self, id: &str, params: Value) -> Result<Value, AutomationError> {
        self.command_start(id, params, true)
    }

    /// Run a command; with `wait` false, a job-capable command (filters, Content-Aware Fill,
    /// Photomerge, …) runs in the background and this returns `{"job": id}` at once (poll with
    /// `jobs.list`, stop with `jobs.cancel`). Other commands finish before returning either way.
    pub fn command_start(&mut self, id: &str, params: Value, wait: bool) -> Result<Value, AutomationError> {
        let params = if params.is_null() { json!({}) } else { params };
        if !matches!(&self.filesystem, Filesystem::TrustedLocal) {
            if id == "file.export.renderVideo" {
                return self.render_video(params, |_, _| true);
            }
            authorize_engine_command(id, &params)?;
        }
        // Background jobs that finished since the last request (or batch step) are applied first.
        self.sync_jobs();
        if wait {
            return Ok(self.session.execute(id, params)?);
        }
        Ok(match self.session.start(id, params)? {
            photocraft_engine::jobs::Started::Done(v) => v,
            photocraft_engine::jobs::Started::Job(job) => json!({"job": job.0, "pending": true}),
        })
    }
}

impl Headless {
    /// A direct automation Render Video uses only the launch-time write capability. Nested
    /// engine actions still reject this filesystem command until they can carry capabilities.
    pub(crate) fn render_video(&mut self, params: Value, progress: impl FnMut(usize, usize) -> bool) -> Result<Value, AutomationError> {
        self.sync_jobs();
        let Filesystem::Workspace(workspace) = &self.filesystem else {
            return Err(AutomationError::BadRequest("automation filesystem access is not granted: write authority is absent".into()));
        };
        let dir = params.get("dir").and_then(Value::as_str).ok_or_else(|| AutomationError::BadRequest("renderVideo needs `dir`".into()))?;
        let mut outputs = workspace.export_sequence(dir)?;
        let result = photocraft_engine::render_video_with(
            &self.session,
            &params,
            |name, bytes| outputs.write(name, bytes).map_err(|e| photocraft_engine::EngineError::Other(e.to_string())),
            progress,
        );
        match result {
            Ok(value) => {
                outputs.commit();
                Ok(value)
            }
            Err(error) => {
                outputs.cleanup()?;
                Err(error.into())
            }
        }
    }
}

/// Common tail of the two capability-scoped exports: the sequence keeps what it wrote, or drops
/// every file it created, so a half-finished export never looks like a finished one.
fn exported(dir: &str, written: photocraft_engine::Result<Vec<String>>, outputs: &mut ExportSequence) -> Result<Value, AutomationError> {
    match written {
        Ok(names) => {
            outputs.commit();
            let files: Vec<String> = names.iter().map(|name| format!("{dir}/{name}")).collect();
            let count = files.len();
            Ok(json!({"files": files, "count": count}))
        }
        Err(error) => {
            outputs.cleanup()?;
            Err(error.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_key_resolves_lexical_spellings_and_falls_back_to_raw_text() {
        assert_eq!(dedupe_key("x.png"), "x.png");
        assert_eq!(dedupe_key("./x.png"), "x.png");
        assert_eq!(dedupe_key("in//sub/../x.png"), "in/x.png");
        assert_eq!(dedupe_key("/root/in/x.png"), "/root/in/x.png");
        assert_eq!(dedupe_key("../x.png"), "../x.png", "a climb above the start keeps the raw spelling");
        assert_eq!(dedupe_key("a/../../x.png"), "a/../../x.png");
    }

    fn write_png(path: &Path, w: u32, h: u32) {
        let img = photocraft_codecs::Image::from_u8(w, h, photocraft_codecs::ChannelLayout::Rgb, vec![200; (w * h * 3) as usize]).unwrap();
        let bytes = photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &Default::default()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_path_spelling_dedupe_reply_warns_that_the_file_is_not_re_read() {
        let dir = std::env::temp_dir().join(format!("pc-headless-dedupe-warn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.png");
        write_png(&file, 8, 4);
        let mut h = Headless::trusted_local();
        let first = h.open(&file).unwrap();
        assert!(first["deduped"].is_null(), "{first}");
        let second = h.open(&dir.join("./x.png")).unwrap();
        assert_eq!(second["deduped"], json!(true), "{second}");
        let warnings = second["warnings"].as_array().unwrap();
        assert!(warnings.iter().all(|w| w.as_str().is_some_and(|w| !w.is_empty())), "{second}");
        assert!(warnings.iter().any(|w| w.as_str().is_some_and(|w| w.contains("without re-reading"))), "{second}");
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn a_hard_link_alias_dedupes_to_the_open_document_and_names_the_alias() {
        let dir = std::env::temp_dir().join(format!("pc-headless-dedupe-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("original.png");
        write_png(&original, 8, 4);
        let alias = dir.join("alias.png");
        std::fs::hard_link(&original, &alias).unwrap();
        let mut h = Headless::trusted_local();
        let first = h.open(&original).unwrap();
        assert!(first["deduped"].is_null(), "{first}");
        let second = h.open(&alias).unwrap();
        assert_eq!(second["deduped"], json!(true), "{second}");
        assert_eq!(second["index"], first["index"], "the alias opens no second document");
        assert_eq!(h.session.documents().len(), 1);
        let warnings: Vec<&str> = second["warnings"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(warnings.iter().any(|w| w.contains("without re-reading")), "{second}");
        assert!(warnings.iter().any(|w| w.contains("another name for the open document")), "{second}");
        let third = h.open(&original).unwrap();
        assert_eq!(third["deduped"], json!(true), "{third}");
        let warnings: Vec<&str> = third["warnings"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert_eq!(warnings.len(), 1, "a spelling-only dedupe has no alias note: {third}");
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
