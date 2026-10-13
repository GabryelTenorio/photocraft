//! Batch options (#219): files in subfolders, file naming, stop on error, an error log, and
//! progress with cancel. Shared by File › Automate › Batch (which also takes the open
//! documents as its source), Image Processor, the Lens Correction batch and the CLI's `batch`.
//!
//! The naming template follows Photoshop's File Naming fields as tokens in one string:
//! `{name}` / `{NAME}` / `{Name}` (the document name as is, upper case, capitalised),
//! `{serial}` to `{serial:4}` (a serial number, padded to that many digits, from
//! `startSerial`), `{letter}` / `{LETTER}` (a serial letter: a, b … z, aa, ab …),
//! `{date:mmddyy}`, `{date:mmdd}`, `{date:yyyymmdd}`, `{date:yymmdd}`, `{date:yyddmm}`,
//! `{date:ddmmyy}` (today's date), `{ext}` / `{EXT}` (the output extension), and any other
//! text literally. The default is `{name}.{ext}`.

use serde_json::{Value, json};

use crate::{EngineError, Result};

/// One input of a batch: a file, and the folder it was found in relative to the input folder
/// (`""` at the top). The output mirrors that folder, so files of the same name in different
/// subfolders don't collide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub path: String,
    pub subdir: String,
}

impl Input {
    pub fn file(path: &str) -> Self {
        Self { path: path.to_string(), subdir: String::new() }
    }
}

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

/// The inputs of a batch command: `"input"` is a folder (with `"includeSubfolders"`, its
/// subfolders too) or an array of files.
pub(crate) fn inputs(p: &Value, cmd: &str) -> Result<Vec<Input>> {
    let recurse = p.get("includeSubfolders").and_then(Value::as_bool).unwrap_or(false);
    match p.get("input").or_else(|| p.get("files")) {
        Some(Value::Array(a)) => Ok(a.iter().filter_map(Value::as_str).map(Input::file).collect()),
        Some(Value::String(dir)) if recurse => list_images_recursive(dir),
        Some(Value::String(dir)) => Ok(crate::file_cmds::list_images(dir)?.into_iter().map(|path| Input::file(&path)).collect()),
        _ => Err(bad(cmd, "missing \"input\" (folder or array of files)")),
    }
}

/// Deepest subfolder a recursive listing enters; deeper trees are refused rather than walked
/// forever (a loop of symbolic links is not followed either: links to folders are skipped).
pub const MAX_DEPTH: usize = 32;
/// Most files a recursive listing returns.
pub const MAX_FILES: usize = 100_000;

/// The images of `dir` and its subfolders: each folder's files sorted by name, then its
/// subfolders sorted by name.
#[cfg(not(target_arch = "wasm32"))]
pub fn list_images_recursive(dir: &str) -> Result<Vec<Input>> {
    fn walk(root: &str, sub: &str, depth: usize, out: &mut Vec<Input>) -> Result<()> {
        let dir = if sub.is_empty() { root.to_string() } else { crate::file_cmds::join(root, sub) };
        let rd = std::fs::read_dir(&dir).map_err(|e| EngineError::Other(format!("{dir}: {e}")))?;
        let mut entries: Vec<std::fs::DirEntry> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        let mut folders = Vec::new();
        for e in entries {
            // `file_type` does not follow symbolic links, so a linked folder is not entered.
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            if ft.is_dir() {
                folders.push(name);
            } else if path.is_file() && crate::file_cmds::openable_name(&name) {
                if out.len() >= MAX_FILES {
                    return Err(EngineError::Other(format!("{root}: more than {MAX_FILES} images")));
                }
                out.push(Input { path: path.to_string_lossy().into_owned(), subdir: sub.to_string() });
            }
        }
        for name in folders {
            if depth >= MAX_DEPTH {
                return Err(EngineError::Other(format!("{dir}/{name}: folders nested deeper than {MAX_DEPTH} levels")));
            }
            let sub = if sub.is_empty() { name } else { format!("{sub}/{name}") };
            walk(root, &sub, depth + 1, out)?;
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, "", 0, &mut out)?;
    Ok(out)
}
#[cfg(target_arch = "wasm32")]
pub fn list_images_recursive(dir: &str) -> Result<Vec<Input>> {
    Err(EngineError::Other(format!("cannot list {dir}: no file system on the web")))
}

/// Today's date, for the naming template's date tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    /// The local date. On the web (where the batch commands don't run) the epoch.
    pub fn today() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use chrono::Datelike;
            let now = chrono::Local::now();
            Self { year: now.year(), month: now.month(), day: now.day() }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self { year: 1970, month: 1, day: 1 }
        }
    }

    fn format(self, fmt: DateFmt) -> String {
        let (y, m, d) = (self.year.rem_euclid(10_000), self.month, self.day);
        let yy = y % 100;
        match fmt {
            DateFmt::Mmddyy => format!("{m:02}{d:02}{yy:02}"),
            DateFmt::Mmdd => format!("{m:02}{d:02}"),
            DateFmt::Yyyymmdd => format!("{y:04}{m:02}{d:02}"),
            DateFmt::Yymmdd => format!("{yy:02}{m:02}{d:02}"),
            DateFmt::Yyddmm => format!("{yy:02}{d:02}{m:02}"),
            DateFmt::Ddmmyy => format!("{d:02}{m:02}{yy:02}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DateFmt {
    Mmddyy,
    Mmdd,
    Yyyymmdd,
    Yymmdd,
    Yyddmm,
    Ddmmyy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    AsIs,
    Upper,
    Title,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    Lit(String),
    Name(Case),
    /// A serial number padded to this many digits.
    Serial(usize),
    /// A serial letter, upper case when true.
    Letter(bool),
    Date(DateFmt),
    /// The output extension, upper case when true.
    Ext(bool),
}

/// A parsed file naming template (see the module doc).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Naming {
    segs: Vec<Seg>,
    start: u64,
}

impl Naming {
    /// The template every batch uses unless told otherwise: `<document name>.<extension>`.
    pub const DEFAULT: &'static str = "{name}.{ext}";

    /// Parse `template`; serial numbers count from `start`.
    pub fn parse(template: &str, start: u64) -> std::result::Result<Self, String> {
        let mut segs = Vec::new();
        let mut lit = String::new();
        let mut rest = template;
        while let Some(i) = rest.find('{') {
            lit.push_str(rest.get(..i).unwrap_or_default());
            let after = rest.get(i + 1..).unwrap_or_default();
            let Some(j) = after.find('}') else { return Err("unclosed `{`".into()) };
            let token = after.get(..j).unwrap_or_default();
            let seg = match token {
                "name" => Seg::Name(Case::AsIs),
                "NAME" => Seg::Name(Case::Upper),
                "Name" => Seg::Name(Case::Title),
                "serial" => Seg::Serial(1),
                "letter" => Seg::Letter(false),
                "LETTER" => Seg::Letter(true),
                "ext" => Seg::Ext(false),
                "EXT" => Seg::Ext(true),
                "date:mmddyy" => Seg::Date(DateFmt::Mmddyy),
                "date:mmdd" => Seg::Date(DateFmt::Mmdd),
                "date:yyyymmdd" => Seg::Date(DateFmt::Yyyymmdd),
                "date:yymmdd" => Seg::Date(DateFmt::Yymmdd),
                "date:yyddmm" => Seg::Date(DateFmt::Yyddmm),
                "date:ddmmyy" => Seg::Date(DateFmt::Ddmmyy),
                _ => match token.strip_prefix("serial:").and_then(|n| n.parse::<usize>().ok()) {
                    Some(n) if (1..=4).contains(&n) => Seg::Serial(n),
                    _ => {
                        return Err(format!(
                            "unknown token `{{{token}}}` (tokens: {{name}}, {{NAME}}, {{Name}}, {{serial}}, {{serial:2}}, {{serial:3}}, {{serial:4}}, {{letter}}, {{LETTER}}, {{date:mmddyy}}, {{date:mmdd}}, {{date:yyyymmdd}}, {{date:yymmdd}}, {{date:yyddmm}}, {{date:ddmmyy}}, {{ext}}, {{EXT}})"
                        ));
                    }
                },
            };
            if !lit.is_empty() {
                segs.push(Seg::Lit(std::mem::take(&mut lit)));
            }
            segs.push(seg);
            rest = after.get(j + 1..).unwrap_or_default();
        }
        if rest.contains('}') {
            return Err("stray `}`".into());
        }
        lit.push_str(rest);
        if !lit.is_empty() {
            segs.push(Seg::Lit(lit));
        }
        if segs.is_empty() {
            return Err("empty".into());
        }
        let text: String = segs.iter().filter_map(|s| if let Seg::Lit(t) = s { Some(t.as_str()) } else { None }).collect();
        if text.contains(['/', '\\', ':']) || text.split('.').any(|part| part == "..") {
            return Err("file names can't contain path separators or `..`".into());
        }
        Ok(Self { segs, start })
    }

    /// `<name><suffix>.<ext>`, the naming of the commands without a template.
    pub fn plain(suffix: &str) -> Self {
        let mut segs = vec![Seg::Name(Case::AsIs)];
        if !suffix.is_empty() {
            segs.push(Seg::Lit(suffix.to_string()));
        }
        segs.extend([Seg::Lit(".".into()), Seg::Ext(false)]);
        Self { segs, start: 1 }
    }

    /// The file name of the `index`-th (0-based) output named `stem` with extension `ext`.
    pub fn file_name(&self, stem: &str, ext: &str, index: u64, date: Date) -> String {
        let serial = self.start.saturating_add(index);
        let mut out = String::new();
        for seg in &self.segs {
            match seg {
                Seg::Lit(t) => out.push_str(t),
                Seg::Name(Case::AsIs) => out.push_str(stem),
                Seg::Name(Case::Upper) => out.push_str(&stem.to_uppercase()),
                Seg::Name(Case::Title) => {
                    let mut chars = stem.chars();
                    if let Some(first) = chars.next() {
                        out.extend(first.to_uppercase());
                        out.push_str(&chars.as_str().to_lowercase());
                    }
                }
                Seg::Serial(digits) => out.push_str(&format!("{serial:0width$}", width = *digits)),
                Seg::Letter(upper) => out.push_str(&letters(serial, *upper)),
                Seg::Date(fmt) => out.push_str(&date.format(*fmt)),
                Seg::Ext(false) => out.push_str(&ext.to_lowercase()),
                Seg::Ext(true) => out.push_str(&ext.to_uppercase()),
            }
        }
        out
    }
}

/// Serial letters: 1 → a, 26 → z, 27 → aa, 28 → ab … (0 → an empty string).
fn letters(mut n: u64, upper: bool) -> String {
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        let c = b'a' + (n % 26) as u8;
        out.push(if upper { c.to_ascii_uppercase() } else { c });
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// How a batch run names its files and handles errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub naming: Naming,
    /// Stop at the first file that fails (the rest are reported as skipped) instead of going
    /// on with the next.
    pub stop_on_error: bool,
    /// Write the errors of the run to this text file (Photoshop's "Log Errors to File").
    pub error_log: Option<String>,
}

impl Options {
    /// `naming`, `startSerial`, `stopOnError` and `errorLog` from command `cmd`'s params.
    pub(crate) fn from_params(p: &Value, cmd: &str) -> Result<Self> {
        let template = p.get("naming").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).unwrap_or(Naming::DEFAULT);
        let start = match p.get("startSerial") {
            None | Some(Value::Null) => 1,
            Some(v) => v.as_u64().ok_or_else(|| bad(cmd, "\"startSerial\" must be a non-negative integer"))?,
        };
        let naming = Naming::parse(template, start).map_err(|m| bad(cmd, format!("\"naming\": {m}")))?;
        let stop_on_error = p.get("stopOnError").and_then(Value::as_bool).unwrap_or(false);
        let error_log = p.get("errorLog").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        Ok(Self { naming, stop_on_error, error_log })
    }

    /// The options of the commands without them: `<name><suffix>.<ext>`, every file, no log.
    pub fn plain(suffix: &str) -> Self {
        Self { naming: Naming::plain(suffix), stop_on_error: false, error_log: None }
    }
}

/// Write the errors of a run to `path`, one per line.
pub(crate) fn write_error_log(path: &str, errors: &[Value], date: Date) -> Result<()> {
    let mut text = format!("PhotoCraft Batch: {} error(s) on {}\n", errors.len(), date.format(DateFmt::Yyyymmdd));
    for e in errors {
        let what = e.get("file").or_else(|| e.get("document")).and_then(Value::as_str).unwrap_or("?");
        let why = e.get("error").and_then(Value::as_str).unwrap_or("?");
        text.push_str(&format!("{what}: {why}\n"));
    }
    crate::file_cmds::write_file(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("error log: {e}")))
}

/// The outcome of a run, the shape every batch command returns.
pub(crate) fn outcome(files: Vec<String>, errors: Vec<Value>, stopped: Option<(String, Vec<String>)>, error_log: Option<&str>) -> Value {
    let logged = !errors.is_empty();
    let mut r = json!({"files": files, "errors": errors});
    if let Some((at, skipped)) = stopped {
        r["stopped"] = json!(at);
        r["skipped"] = json!(skipped);
    }
    if let (Some(log), true) = (error_log, logged) {
        r["errorLog"] = json!(log);
    }
    r
}

#[cfg(test)]
mod tests;
