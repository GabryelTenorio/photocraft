//! An in-app file browser for File › Open / Save as. Shared storage is read with plain `std::fs`
//! once the user grants "All files access" (the Storage Access Framework needs a Java result
//! callback, which a NativeActivity without Java glue can't receive).

use std::path::{Path, PathBuf};

use photocraft_ui_egui::{FileDialogAnswer, FileDialogReply, FileDialogRequest};

const ROOT: &str = "/storage/emulated/0";
const PACKAGE: &str = "ai.storyteller.photocraft";

/// A dialog waiting for the user, queued by `Services::file_dialog`.
pub type Queue = std::sync::Arc<std::sync::Mutex<Option<(FileDialogRequest, FileDialogReply)>>>;

pub struct Browser {
    request: FileDialogRequest,
    reply: Option<FileDialogReply>,
    dir: PathBuf,
    picked: Vec<String>,
    name: String,
}

impl Browser {
    pub fn new(request: FileDialogRequest, reply: FileDialogReply) -> Self {
        let (dir, name) = match &request {
            FileDialogRequest::Open { initial_dir, .. } => (initial_dir.as_deref().map(PathBuf::from), String::new()),
            FileDialogRequest::Save { suggested } => {
                let p = Path::new(suggested);
                (p.parent().map(Path::to_path_buf), p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            }
        };
        let dir = dir.filter(|d| d.is_absolute() && d.is_dir()).unwrap_or_else(|| PathBuf::from(ROOT));
        Self { request, reply: Some(reply), dir, picked: Vec::new(), name }
    }

    fn finish(&mut self, answer: Option<FileDialogAnswer>) {
        if let Some(reply) = self.reply.take() {
            reply.send(answer);
        }
    }

    /// Draws the dialog; `false` once it has answered.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let screen = ctx.content_rect().size();
        let size = egui::vec2((screen.x * 0.9).min(900.0), screen.y * 0.9);
        let save = matches!(self.request, FileDialogRequest::Save { .. });
        let mut cancel = false;
        egui::Window::new(if save { "Save As" } else { "Open" })
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size(size)
            .collapsible(false)
            .show(ctx, |ui| match std::fs::read_dir(&self.dir) {
                Err(_) if self.dir == Path::new(ROOT) => {
                    ui.label("PhotoCraft needs “All files access” to open and save your files.");
                    ui.horizontal(|ui| {
                        if ui.button("Grant access").clicked() {
                            grant_access();
                        }
                        ui.label("(or Settings › Apps › PhotoCraft › All files access)");
                    });
                    ui.label("Then come back and reopen this dialog.");
                    cancel = ui.button("Close").clicked();
                }
                Err(e) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, e.to_string());
                    if ui.button("Back").clicked() {
                        self.dir = PathBuf::from(ROOT);
                    }
                }
                Ok(entries) => self.listing(ui, entries, size.y - 150.0, save, &mut cancel),
            });
        if cancel {
            self.finish(None);
        }
        self.reply.is_some()
    }

    fn listing(&mut self, ui: &mut egui::Ui, entries: std::fs::ReadDir, height: f32, save: bool, cancel: &mut bool) {
        let (multiple, exts) = match &self.request {
            FileDialogRequest::Open { multiple, extensions, .. } => (*multiple, extensions.clone()),
            FileDialogRequest::Save { .. } => (false, None),
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if e.path().is_dir() {
                dirs.push(name);
            } else if !save && exts.as_ref().is_none_or(|x| ext_of(&name).is_some_and(|ext| x.iter().any(|y| y.eq_ignore_ascii_case(&ext)))) {
                files.push(name);
            }
        }
        dirs.sort_by_key(|n| n.to_lowercase());
        files.sort_by_key(|n| n.to_lowercase());
        ui.label(self.dir.display().to_string());
        let up = self.dir.parent().filter(|_| self.dir != Path::new(ROOT)).map(Path::to_path_buf);
        let mut go = None;
        egui::ScrollArea::vertical().max_height(height).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().button_padding = egui::vec2(10.0, 12.0);
            if let Some(up) = up
                && ui.add_sized([ui.available_width(), 0.0], egui::Button::new("⬆  ..")).clicked()
            {
                go = Some(up);
            }
            for d in &dirs {
                if ui.add_sized([ui.available_width(), 0.0], egui::Button::new(format!("📁  {d}"))).clicked() {
                    go = Some(self.dir.join(d));
                }
            }
            for f in &files {
                let path = self.dir.join(f).to_string_lossy().into_owned();
                let on = self.picked.contains(&path);
                if ui.add_sized([ui.available_width(), 0.0], egui::Button::selectable(on, format!("🖻  {f}"))).clicked() {
                    if !multiple {
                        self.picked.clear();
                    }
                    match self.picked.iter().position(|p| *p == path) {
                        Some(i) => drop(self.picked.remove(i)),
                        None => self.picked.push(path),
                    }
                }
            }
        });
        if let Some(d) = go {
            self.dir = d;
            self.picked.clear();
        }
        ui.horizontal(|ui| {
            if save {
                ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(ui.available_width() - 200.0));
                if ui.add_enabled(!self.name.trim().is_empty(), egui::Button::new("Save")).clicked() {
                    let path = self.dir.join(self.name.trim()).to_string_lossy().into_owned();
                    self.finish(Some(FileDialogAnswer::SaveTo(path)));
                }
            } else if ui.add_enabled(!self.picked.is_empty(), egui::Button::new(format!("Open ({})", self.picked.len()))).clicked() {
                let picked = std::mem::take(&mut self.picked);
                self.finish(Some(FileDialogAnswer::Paths(picked)));
            }
            *cancel = ui.button("Cancel").clicked();
        });
    }
}

fn ext_of(name: &str) -> Option<String> {
    Path::new(name).extension().map(|e| e.to_string_lossy().into_owned())
}

/// Opens the system's "All files access" page for this app. ponytail: shells out to `am` instead
/// of JNI (no `startActivity` binding); if the system refuses, the dialog's hint names the path.
fn grant_access() {
    let uri = format!("package:{PACKAGE}");
    if let Err(e) = std::process::Command::new("am").args(["start", "-a", "android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION", "-d", &uri]).status() {
        log::warn!("couldn't open the All files access settings: {e}");
    }
}
