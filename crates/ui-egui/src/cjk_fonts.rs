//! Lazy CJK, Thai, Khmer, Lao and Burmese fallback fonts for the UI.
//!
//! The bundled Inter / JetBrains Mono have no Japanese, Chinese or Korean glyphs, and the OS
//! fonts that do are large (Hiragino ~10 MB, Apple SD Gothic Neo 28 MB, PingFang 78 MB, Noto
//! Sans CJK ~20 MB). Instead of reading them at startup, an egui plugin scans each frame's text
//! for CJK characters no registered font covers, and registers the next system font for that
//! character's script (`ctx.add_font`, active from the next frame, which it requests). Fonts are
//! appended at the lowest priority to every family, so Latin text keeps Inter. Scripts that are
//! not part of the locale-dependent CJK order get a [`LazyScript`] slot: their own candidate list
//! and one-shot attempt state. Adding one (Lao, Myanmar, Indic) is an enum variant, a character
//! range and a file list.
//!
//! Script order follows the UI locale ([`photocraft_text::cjk::script_order`]): Kana prefers a
//! Japanese font, Hangul a Korean one, Bopomofo a Traditional Chinese one, and Han the
//! locale's script (Japanese forms only for a Japanese locale). At most one font is read per
//! frame, each file at most once, and once every script has been tried the scan stops.
//!
//! Builds made with the optional craft-fonts input (`CRAFT_FONTS_DIR`,
//! [`photocraft_text::craft_fonts`]) carry Japanese fonts (BIZ UDPGothic first): they are tried
//! before the system Japanese fonts, in the Japanese slot of the same locale order. Without
//! craft-fonts (and on the web, which never embeds them) nothing changes.

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::{FontData, FontFamily, FontId, Shape};
use photocraft_text::cjk::{self, CjkChar, CjkScript, FontFile};
use photocraft_text::craft_fonts::{self, CraftFont};
use std::path::PathBuf;

/// Skip absurdly large files (a corrupt or non-font path must not eat memory).
pub const MAX_FONT_BYTES: u64 = 128 << 20;
/// Name prefix of the registered fallback fonts.
pub const FONT_PREFIX: &str = "system-cjk";

/// A script with its own lazy candidate list and attempt state, outside the CJK locale order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LazyScript {
    Thai,
    Khmer,
    Lao,
    Myanmar,
}

/// Every [`LazyScript`] slot.
pub const LAZY_SCRIPTS: [LazyScript; 4] = [LazyScript::Thai, LazyScript::Khmer, LazyScript::Lao, LazyScript::Myanmar];

/// The [`LazyScript`] `c` needs a fallback for, if any.
fn lazy_script(c: char) -> Option<LazyScript> {
    match c {
        '\u{0e00}'..='\u{0e7f}' => Some(LazyScript::Thai),
        '\u{1780}'..='\u{17ff}' | '\u{19e0}'..='\u{19ff}' => Some(LazyScript::Khmer),
        '\u{0e80}'..='\u{0eff}' => Some(LazyScript::Lao),
        '\u{1000}'..='\u{109f}' | '\u{a9e0}'..='\u{a9ff}' | '\u{aa60}'..='\u{aa7f}' => Some(LazyScript::Myanmar),
        _ => None,
    }
}

/// Where fonts come from; swapped out in tests.
#[derive(Clone)]
pub struct Sources {
    pub locale: fn() -> Option<String>,
    pub files: fn(CjkScript) -> Vec<FontFile>,
    pub last_resort: fn() -> Vec<FontFile>,
    /// Fonts for a [`LazyScript`] slot, loaded only when a visible character of that script is missing.
    pub scripts: fn(LazyScript) -> Vec<FontFile>,
    /// Embedded fonts tried before `files` for a script (craft-fonts' Japanese fonts).
    pub embedded: fn(CjkScript) -> Vec<&'static CraftFont>,
}

/// craft-fonts' Japanese fonts for the Japanese script, UI face first (empty without craft-fonts).
pub fn craft_embedded(script: CjkScript) -> Vec<&'static CraftFont> {
    if script == CjkScript::Japanese { craft_fonts::japanese_for_ui() } else { Vec::new() }
}

/// No embedded fonts (tests that pin the system-font behaviour).
pub fn no_embedded(_: CjkScript) -> Vec<&'static CraftFont> {
    Vec::new()
}

impl Sources {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn system() -> Self {
        Self {
            locale: || match crate::i18n::current().code() {
                code @ ("ja" | "ko" | "zh-hans" | "zh-hant") => Some(code.to_string()),
                _ => cjk::ui_locale().map(str::to_string),
            },
            files: cjk::font_files,
            last_resort: cjk::last_resort_files,
            scripts: lazy_font_files,
            embedded: craft_embedded,
        }
    }
    #[cfg(target_arch = "wasm32")]
    pub fn system() -> Self {
        Self { locale: || None, files: |_| Vec::new(), last_resort: Vec::new, scripts: |_| Vec::new(), embedded: craft_embedded }
    }
}

/// The lazy loader's state: which scripts were tried and which files were read.
pub struct CjkFallback {
    sources: Sources,
    /// set_fonts takes effect next pass; do not collide with a previous loader's font names.
    defer_after_reset: bool,
    order: Option<[CjkScript; 4]>,
    tried: Vec<CjkScript>,
    last_resort_tried: bool,
    scripts_tried: Vec<LazyScript>,
    loaded: Vec<PathBuf>,
    /// Fonts registered so far: (name, path, face index).
    pub registered: Vec<(String, PathBuf, u32)>,
}

impl CjkFallback {
    pub fn new(sources: Sources) -> Self {
        Self {
            sources,
            defer_after_reset: false,
            order: None,
            tried: Vec::new(),
            last_resort_tried: false,
            scripts_tried: Vec::new(),
            loaded: Vec::new(),
            registered: Vec::new(),
        }
    }

    /// Script order for the UI locale (resolved on first use, not at startup).
    pub fn order(&mut self) -> [CjkScript; 4] {
        *self.order.get_or_insert_with(|| cjk::script_order((self.sources.locale)().as_deref()))
    }

    /// Every script and the last-resort fonts have been tried: nothing more to load.
    pub fn exhausted(&self) -> bool {
        self.tried.len() >= 4 && self.last_resort_tried
    }

    /// The next font to register for a character of kind `kind` that no current font covers:
    /// the first readable file of the first untried script (the character's preferred script,
    /// then the locale order), then the last-resort fonts. `None` when nothing is left.
    pub fn next_font(&mut self, kind: CjkChar) -> Option<(String, FontData)> {
        let order = self.order();
        let scripts: Vec<CjkScript> = std::iter::once(kind.preferred(&order)).chain(order).collect();
        for s in scripts {
            if self.tried.contains(&s) {
                continue;
            }
            self.tried.push(s);
            if let Some(f) = self.load_embedded(s) {
                return Some(f);
            }
            let files = (self.sources.files)(s);
            if let Some(f) = self.load_first(&files) {
                return Some(f);
            }
        }
        if !self.last_resort_tried {
            self.last_resort_tried = true;
            let files = (self.sources.last_resort)();
            return self.load_first(&files);
        }
        None
    }

    /// Whether `s` was already attempted, whether or not a readable font came of it.
    fn script_tried(&self, s: LazyScript) -> bool {
        self.scripts_tried.contains(&s)
    }

    /// Every [`LazyScript`] slot has been attempted: no script font is left to load.
    fn scripts_exhausted(&self) -> bool {
        self.scripts_tried.len() >= LAZY_SCRIPTS.len()
    }

    /// The next font for script `s`: the first readable candidate, at most once per slot.
    fn next_script_font(&mut self, s: LazyScript) -> Option<(String, FontData)> {
        if self.script_tried(s) {
            return None;
        }
        self.scripts_tried.push(s);
        self.load_first(&(self.sources.scripts)(s))
    }

    /// The first embedded (craft-fonts) font for `s`, registered from its static bytes.
    fn load_embedded(&mut self, s: CjkScript) -> Option<(String, FontData)> {
        let f = (self.sources.embedded)(s).into_iter().find(|f| !f.bytes.is_empty())?;
        let path = PathBuf::from(format!("craft-fonts/{} {}", f.family, f.style));
        if self.loaded.contains(&path) {
            return None;
        }
        self.loaded.push(path.clone());
        let name = format!("{FONT_PREFIX}-{}", self.registered.len());
        log::info!("UI font fallback: registered embedded {} as {name}", path.display());
        self.registered.push((name.clone(), path, 0));
        Some((name, FontData::from_static(f.bytes)))
    }

    fn load_first(&mut self, files: &[FontFile]) -> Option<(String, FontData)> {
        for f in files {
            if self.loaded.contains(&f.path) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&f.path) else { continue };
            if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_FONT_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(&f.path) else { continue };
            let index = cjk::face_index_for_family(&bytes, f.family);
            self.loaded.push(f.path.clone());
            let name = format!("{FONT_PREFIX}-{}", self.registered.len());
            log::info!("UI font fallback: registered {} (face {index}) as {name}", f.path.display());
            self.registered.push((name.clone(), f.path.clone(), index));
            let mut data = FontData::from_owned(bytes);
            data.index = index;
            return Some((name, data));
        }
        None
    }
}

/// First supported missing character for which a fallback has not yet been exhausted.
fn missing_char(ctx: &egui::Context, shapes: &[egui::epaint::ClippedShape], fallback: &CjkFallback) -> Option<char> {
    fn collect(shape: &Shape, out: &mut Vec<char>, cjk_pending: bool, fallback: &CjkFallback) {
        match shape {
            Shape::Text(t) => {
                let text = &t.galley.job.text;
                if !text.is_ascii() {
                    for c in text.chars().filter(|c| (cjk_pending && cjk::classify(*c).is_some()) || lazy_script(*c).is_some_and(|s| !fallback.script_tried(s)))
                    {
                        if out.len() >= 256 {
                            return;
                        }
                        if !out.contains(&c) {
                            out.push(c);
                        }
                    }
                }
            }
            Shape::Vec(v) => v.iter().for_each(|s| collect(s, out, cjk_pending, fallback)),
            _ => {}
        }
    }
    let mut chars = Vec::new();
    for s in shapes {
        collect(&s.shape, &mut chars, !fallback.exhausted(), fallback);
    }
    if chars.is_empty() {
        return None;
    }
    let font = FontId::proportional(12.0);
    ctx.fonts_mut(|f| chars.into_iter().find(|c| !f.has_glyph(&font, *c)))
}

/// (ascent, descent, line gap) of a face in em, from its `hhea` table; `descent` is negative.
fn vertical_metrics(font: &[u8], index: u32) -> Option<(f32, f32, f32)> {
    let u16_at = |o: usize| font.get(o..o.checked_add(2)?).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let u32_at = |o: usize| font.get(o..o.checked_add(4)?).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let i16_at = |o: usize| u16_at(o).map(|v| v as i16 as f32);
    let dir = if font.get(..4)? == b"ttcf" {
        // Collection: the offset table of face `index` follows a 12-byte header.
        u32_at(12usize.checked_add((index as usize).checked_mul(4)?)?)? as usize
    } else {
        0
    };
    let tables = u16_at(dir.checked_add(4)?)? as usize;
    let (mut hhea, mut head) = (None, None);
    for i in 0..tables {
        let rec = dir.checked_add(12)?.checked_add(i.checked_mul(16)?)?;
        let tag = font.get(rec..rec.checked_add(4)?)?;
        let off = u32_at(rec.checked_add(8)?)? as usize;
        match tag {
            b"hhea" => hhea = Some(off),
            b"head" => head = Some(off),
            _ => {}
        }
    }
    let (hhea, head) = (hhea?, head?);
    let upm = u16_at(head.checked_add(18)?)? as f32;
    if upm <= 0.0 {
        return None;
    }
    Some((i16_at(hhea.checked_add(4)?)? / upm, i16_at(hhea.checked_add(6)?)? / upm, i16_at(hhea.checked_add(8)?)? / upm))
}

/// `y_offset_factor` that puts `face`'s baseline where `primary`'s would be. egui centres a
/// fallback face's row in the primary font's row, so a face with a big line gap (Hiragino: 0.5em)
/// otherwise rides about 0.23em high next to Latin text.
fn baseline_offset(face: (f32, f32, f32), primary: (f32, f32, f32)) -> f32 {
    let height = |m: (f32, f32, f32)| m.0 - m.1 + m.2;
    let off = primary.0 - face.0 - 0.5 * (height(primary) - height(face));
    if off.is_finite() { off.clamp(-0.5, 0.5) } else { 0.0 }
}

/// Registers `name` at the lowest priority in every font family.
fn add_to_all_families(ctx: &egui::Context, name: String, mut data: FontData) {
    // Align to Inter, the primary of every UI family; JetBrains Mono differs by under 0.005em.
    let primary = ctx.fonts(|f| f.definitions().font_data.get("Inter").and_then(|p| vertical_metrics(&p.font, p.index)));
    if let (Some(face), Some(primary)) = (vertical_metrics(&data.font, data.index), primary) {
        data.tweak.y_offset_factor = baseline_offset(face, primary);
    }
    let mut families: Vec<FontFamily> = ctx.fonts(|f| f.definitions().families.keys().cloned().collect());
    for f in [FontFamily::Proportional, FontFamily::Monospace] {
        if !families.contains(&f) {
            families.push(f);
        }
    }
    let families = families.into_iter().map(|family| InsertFontFamily { family, priority: FontPriority::Lowest }).collect();
    crate::theme::size_ui_font(ctx, &name, &mut data);
    ctx.add_font(FontInsert { name, data, families });
}

/// The egui plugin that watches drawn text and loads fallback fonts on demand.
pub struct CjkFontPlugin(pub CjkFallback);

impl egui::Plugin for CjkFontPlugin {
    fn debug_name(&self) -> &'static str {
        "photocraft-cjk-fonts"
    }

    fn output_hook(&mut self, ctx: &egui::Context, output: &mut egui::FullOutput) {
        if std::mem::take(&mut self.0.defer_after_reset) {
            ctx.request_repaint();
            return;
        }
        if self.0.exhausted() && self.0.scripts_exhausted() {
            return;
        }
        let Some(c) = missing_char(ctx, &output.shapes, &self.0) else { return };
        let font = match lazy_script(c) {
            Some(s) => self.0.next_script_font(s),
            None => cjk::classify(c).and_then(|kind| self.0.next_font(kind)),
        };
        if let Some((name, data)) = font {
            add_to_all_families(ctx, name, data);
            ctx.request_repaint();
        } else if !self.0.exhausted() || !self.0.scripts_exhausted() {
            // Nothing readable for this script; try the next one on the next frame.
            ctx.request_repaint();
        }
    }
}

/// Installs the lazy CJK and [`LazyScript`] fallback (system fonts; nothing on the web).
pub fn install(ctx: &egui::Context) {
    install_with(ctx, Sources::system());
}

pub fn install_with(ctx: &egui::Context, sources: Sources) {
    // egui keeps the first plugin of a type. set_fonts removes loaded fallbacks, so reset the
    // existing loader too; otherwise its tried/loaded sets prevent fonts being added again.
    let mut fallback = CjkFallback::new(sources.clone());
    fallback.defer_after_reset = true;
    if ctx
        .with_plugin::<CjkFontPlugin, _>(|plugin| {
            plugin.0 = CjkFallback::new(sources.clone());
            plugin.0.defer_after_reset = true;
        })
        .is_none()
    {
        ctx.add_plugin(CjkFontPlugin(fallback));
    }
}

/// The candidate fonts of a [`LazyScript`] slot.
#[cfg(not(target_arch = "wasm32"))]
fn lazy_font_files(s: LazyScript) -> Vec<FontFile> {
    match s {
        LazyScript::Thai => thai_font_files(),
        LazyScript::Khmer => khmer_font_files(),
        LazyScript::Lao => lao_font_files(),
        LazyScript::Myanmar => myanmar_font_files(),
    }
}

/// The system fonts of one script slot, best candidate first. Keep discovery lazy, as with the CJK
/// candidates: no full system font scan on the UI thread and no extra font files in the web build.
/// `mac` paths are relative to `/System/Library/Fonts`; `win` files to `%WINDIR%\Fonts`.
#[cfg(not(target_arch = "wasm32"))]
fn script_font_files(mac: &[(&'static str, &'static str)], win: &[(&'static str, &'static str)], noto_stem: &str, noto_family: &'static str) -> Vec<FontFile> {
    if cfg!(target_os = "macos") {
        return mac.iter().map(|(file, family)| FontFile { path: format!("/System/Library/Fonts/{file}").into(), family }).collect();
    }
    if cfg!(target_os = "windows") {
        let dir = std::env::var_os("WINDIR").map_or_else(|| PathBuf::from("C:\\Windows"), PathBuf::from).join("Fonts");
        return win.iter().map(|(file, family)| FontFile { path: dir.join(file), family }).collect();
    }
    noto_unix_files(noto_stem, noto_family)
}

/// Noto Sans `<stem>` in Fedora's static and variable packages, Debian/Ubuntu's and Arch's
/// installations, and under Flatpak, which exposes the host's `/usr/share/fonts` as `/run/host/fonts`.
#[cfg(not(target_arch = "wasm32"))]
fn noto_unix_files(stem: &str, family: &'static str) -> Vec<FontFile> {
    let files = [
        format!("google-noto/NotoSans{stem}-Regular.ttf"),
        format!("google-noto-vf/NotoSans{stem}[wght].ttf"),
        format!("truetype/noto/NotoSans{stem}-Regular.ttf"),
        format!("noto/NotoSans{stem}-Regular.ttf"),
    ];
    let mut out = Vec::new();
    for file in &files {
        for root in ["/usr/share/fonts", "/run/host/fonts", "/usr/local/share/fonts"] {
            out.push(FontFile { path: std::path::Path::new(root).join(file), family });
        }
    }
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn thai_font_files() -> Vec<FontFile> {
    script_font_files(
        &[("Supplemental/Thonburi.ttc", "Thonburi"), ("Supplemental/SukhumvitSet.ttc", "Sukhumvit Set")],
        &[("Leelawui.ttf", "Leelawadee UI"), ("leelawad.ttf", "Leelawadee"), ("tahoma.ttf", "Tahoma")],
        "Thai",
        "Noto Sans Thai",
    )
}

/// #218: `Leelawadee UI` is Windows' Khmer UI font from 8.1 (`Khmer UI` on 8), so it is listed too.
#[cfg(not(target_arch = "wasm32"))]
fn khmer_font_files() -> Vec<FontFile> {
    script_font_files(
        &[("Supplemental/Khmer Sangam MN.ttf", "Khmer Sangam MN"), ("Supplemental/Khmer MN.ttc", "Khmer MN")],
        &[("KhmerUI.ttf", "Khmer UI"), ("Leelawui.ttf", "Leelawadee UI")],
        "Khmer",
        "Noto Sans Khmer",
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn lao_font_files() -> Vec<FontFile> {
    script_font_files(
        &[("Supplemental/Lao Sangam MN.ttf", "Lao Sangam MN"), ("Supplemental/Lao MN.ttc", "Lao MN")],
        &[("LaoUI.ttf", "Lao UI"), ("Leelawui.ttf", "Leelawadee UI")],
        "Lao",
        "Noto Sans Lao",
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn myanmar_font_files() -> Vec<FontFile> {
    script_font_files(
        &[("Supplemental/Myanmar Sangam MN.ttc", "Myanmar Sangam MN"), ("Supplemental/Myanmar MN.ttc", "Myanmar MN")],
        &[("mmrtext.ttf", "Myanmar Text"), ("mmrtextb.ttf", "Myanmar Text")],
        "Myanmar",
        "Noto Sans Myanmar",
    )
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn lazy_fallbacks_follow_ui_font_size_and_survive_size_changes() {
        use photocraft_engine::prefs::UiFontSize;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        crate::theme::set_ui_font_size(&ctx, UiFontSize::Large);
        let name = "test-lazy-fallback".to_string();
        add_to_all_families(&ctx, name.clone(), FontData::from_static(photocraft_text::fonts::INTER_REGULAR.as_slice()));
        for (size, scale) in [(UiFontSize::Large, 16.0 / 12.0), (UiFontSize::Tiny, 10.0 / 12.0), (UiFontSize::Small, 1.0)] {
            crate::theme::set_ui_font_size(&ctx, size);
            for _ in 0..2 {
                ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
            }
            let fonts = ctx.fonts(|f| f.definitions().clone());
            let fallback = fonts.font_data.get(&name).unwrap();
            assert_eq!(fallback.tweak.scale, scale);
            assert_eq!(fallback.tweak.y_offset_factor, 0.0, "same face as Inter keeps its baseline");
            for stack in fonts.families.values() {
                assert_eq!(stack.iter().filter(|n| *n == &name).count(), 1);
                assert_eq!(stack.last(), Some(&name), "fallback stays at the lowest priority");
            }
        }
    }

    static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

    #[test]
    fn changing_language_resets_the_loader_once_and_reorders_scripts() {
        crate::i18n::with_language(crate::i18n::Lang::EN, || {
            let ctx = egui::Context::default();
            crate::i18n::sync_context(&ctx, "ja");
            ctx.with_plugin::<CjkFontPlugin, _>(|plugin| {
                assert_eq!(plugin.0.order()[0], CjkScript::Japanese);
                plugin.0.tried = plugin.0.order().to_vec();
                plugin.0.last_resort_tried = true;
            })
            .expect("font plugin");
            crate::i18n::sync_context(&ctx, "ja");
            assert!(ctx.with_plugin::<CjkFontPlugin, _>(|plugin| plugin.0.exhausted()).expect("font plugin"), "idle frames must not reload fonts");
            crate::i18n::sync_context(&ctx, "ko");
            ctx.with_plugin::<CjkFontPlugin, _>(|plugin| {
                assert!(!plugin.0.exhausted());
                assert_eq!(plugin.0.order()[0], CjkScript::Korean);
            })
            .expect("font plugin");
        });
    }

    fn fake_dir() -> PathBuf {
        let mut g = DIR.lock().unwrap();
        g.get_or_insert_with(|| {
            let dir = std::env::temp_dir().join(format!("photocraft-cjk-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("font.ttf"), include_bytes!("../../../assets/fonts/Inter-Regular.ttf")).unwrap();
            std::fs::write(dir.join("empty.ttf"), b"").unwrap();
            dir
        })
        .clone()
    }

    fn fake(name: &str) -> FontFile {
        FontFile { path: fake_dir().join(name), family: "" }
    }

    fn fake_sources(locale: fn() -> Option<String>) -> Sources {
        Sources {
            locale,
            // Korean has a readable "font", Japanese only broken files, Chinese none.
            files: |s| match s {
                CjkScript::Korean => vec![fake("missing.ttc"), fake("empty.ttf"), fake("font.ttf")],
                CjkScript::Japanese => vec![fake("empty.ttf")],
                _ => vec![],
            },
            last_resort: || vec![fake("font.ttf")],
            scripts: |_| Vec::new(),
            embedded: no_embedded,
        }
    }

    #[test]
    fn picks_preferred_script_skips_unreadable_and_exhausts() {
        let mut fb = CjkFallback::new(fake_sources(|| Some("en_US".into())));
        assert!(!fb.exhausted());
        // Hangul: Korean first; missing and empty files are skipped.
        let (name, data) = fb.next_font(CjkChar::Hangul).expect("korean font");
        assert!(name.starts_with(FONT_PREFIX));
        assert_eq!(data.index, 0);
        assert_eq!(fb.tried, vec![CjkScript::Korean]);
        // Han on an English locale: SC, TC, JA have nothing readable, and the last-resort file was
        // already loaded, so nothing new; everything is now tried.
        assert!(fb.next_font(CjkChar::Han).is_none());
        assert!(fb.exhausted());
        assert_eq!(fb.tried, vec![CjkScript::Korean, CjkScript::SimplifiedChinese, CjkScript::TraditionalChinese, CjkScript::Japanese]);
        assert_eq!(fb.registered.len(), 1, "a file is read at most once");
        assert!(fb.next_font(CjkChar::Kana).is_none());
    }

    #[test]
    fn han_follows_locale() {
        for (loc, first) in [
            ("ja_JP.UTF-8", CjkScript::Japanese),
            ("zh-Hans-CN", CjkScript::SimplifiedChinese),
            ("zh-Hant-TW", CjkScript::TraditionalChinese),
            ("ko_KR", CjkScript::Korean),
        ] {
            let mut fb = CjkFallback::new(Sources { locale: || None, files: |_| vec![], last_resort: Vec::new, scripts: |_| vec![], embedded: no_embedded });
            fb.order = Some(cjk::script_order(Some(loc)));
            assert!(fb.next_font(CjkChar::Han).is_none());
            assert_eq!(fb.tried[0], first, "{loc}");
        }
    }

    /// Runs frames showing `text` until fonts settle; returns whether every glyph is covered.
    fn render(ctx: &egui::Context, text: &str) -> bool {
        for _ in 0..12 {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                ui.label(text);
            });
            out.textures_delta.clear();
        }
        ctx.fonts_mut(|f| f.has_glyphs(&FontId::proportional(12.0), &text.replace(' ', "")))
    }

    /// Every slot's Linux candidates cover the distro layouts, not just one script's font.
    #[test]
    fn noto_candidates_cover_fedora_static_variable_and_flatpak_fonts() {
        for (stem, family) in [("Thai", "Noto Sans Thai"), ("Khmer", "Noto Sans Khmer"), ("Lao", "Noto Sans Lao"), ("Myanmar", "Noto Sans Myanmar")] {
            let files = noto_unix_files(stem, family);
            for root in ["/usr/share/fonts", "/run/host/fonts"] {
                for file in [format!("google-noto/NotoSans{stem}-Regular.ttf"), format!("google-noto-vf/NotoSans{stem}[wght].ttf")] {
                    assert!(files.iter().any(|f| f.path == std::path::Path::new(root).join(&file) && f.family == family), "{stem}: {file}");
                }
            }
        }
    }

    #[test]
    fn lazy_script_slots_split_the_southeast_asian_scripts_from_cjk_and_latin() {
        for (c, want) in [
            ('ก', Some(LazyScript::Thai)),
            ('\u{0e7f}', Some(LazyScript::Thai)),
            ('ខ', Some(LazyScript::Khmer)),
            ('ៗ', Some(LazyScript::Khmer)),
            ('\u{19e0}', Some(LazyScript::Khmer)),
            ('ລ', Some(LazyScript::Lao)),
            ('\u{0eff}', Some(LazyScript::Lao)),
            ('မ', Some(LazyScript::Myanmar)),
            ('\u{a9e0}', Some(LazyScript::Myanmar)),
            ('\u{aa60}', Some(LazyScript::Myanmar)),
            ('a', None),
            ('日', None),
            ('\u{0f00}', None), // Tibetan is not a slot yet
        ] {
            assert_eq!(lazy_script(c), want, "{c}");
        }
        assert_eq!(LAZY_SCRIPTS.len(), 4, "a new slot needs a candidate list and a test");
    }

    #[test]
    fn script_loading_is_lazy_once_per_install_and_independent_of_cjk() {
        let sources = Sources {
            locale: || Some("en".into()),
            files: |_| vec![],
            last_resort: Vec::new,
            scripts: |s| if s == LazyScript::Thai { vec![fake("missing.ttf"), fake("empty.ttf"), fake("font.ttf")] } else { vec![] },
            embedded: no_embedded,
        };
        let ctx = egui::Context::default();
        for _ in 0..2 {
            crate::theme::install_fonts_with(&ctx, sources.clone());
            assert!(render(&ctx, "Recent file.png"));
            ctx.with_plugin::<CjkFontPlugin, _>(|p| {
                assert!(!p.0.script_tried(LazyScript::Thai), "Latin-only frames do not request Thai fonts");
                assert!(p.0.registered.is_empty());
            });
            // The stand-in does not draw Thai. An unavailable CJK glyph preceding Thai must
            // not prevent trying the Thai font, nor cause registration/repaint loops.
            assert!(!render(&ctx, "日本語/ภาพถ่าย/วันหยุดพักผ่อน.png"));
            ctx.with_plugin::<CjkFontPlugin, _>(|p| {
                assert!(p.0.exhausted());
                assert!(p.0.script_tried(LazyScript::Thai));
                assert!(!p.0.script_tried(LazyScript::Khmer), "a script nobody drew is not attempted");
                assert_eq!(p.0.registered.len(), 1);
            });
            let fonts = ctx.fonts(|f| f.definitions().clone());
            assert_eq!(fonts.families[&FontFamily::Proportional].first().map(String::as_str), Some("Inter"));
            for stack in fonts.families.values() {
                assert_eq!(stack.iter().filter(|n| n.starts_with(FONT_PREFIX)).count(), 1);
                assert_eq!(stack.last().map(String::as_str), Some("system-cjk-0"));
            }
        }
        // Exhausting one script first must likewise leave CJK and the other slots available.
        let mut fallback = CjkFallback::new(fake_sources(|| Some("ko".into())));
        assert!(fallback.next_script_font(LazyScript::Thai).is_none());
        assert!(fallback.next_font(CjkChar::Hangul).is_some());
        assert!(!fallback.scripts_exhausted());
    }

    #[test]
    fn system_fonts_cover_thai_recent_file_names() {
        // Like the CJK system-font test, run when this machine has the script's fonts.
        // The deterministic lazy-loading test above does not require installed fonts.
        let files = thai_font_files();
        if !files.iter().any(|f| f.path.is_file()) {
            eprintln!("skipping: no Thai system font");
            return;
        }
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        assert!(render(&ctx, "ภาพถ่าย/วันหยุดพักผ่อน.png"), "Thai filename and combining marks still have missing glyphs");
    }

    #[test]
    fn system_fonts_cover_southeast_asian_recent_file_names() {
        // #218: like the Thai and CJK system-font tests, run per script when this machine has the
        // font; the deterministic lazy-loading test above needs none.
        for (script, name, file_name) in [
            (LazyScript::Khmer, "Khmer", "ភាសាខ្មែរ/ឯកសារ.png"),
            (LazyScript::Lao, "Lao", "ພາສາລາວ/ເອກະສານ.png"),
            (LazyScript::Myanmar, "Myanmar", "မြန်မာဘာသာ/ဖိုင်.png"),
        ] {
            if !lazy_font_files(script).iter().any(|f| f.path.is_file()) {
                eprintln!("skipping: no {name} system font");
                continue;
            }
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            assert!(render(&ctx, file_name), "{name} filename, subscripts and marks still have missing glyphs");
        }
    }

    #[test]
    fn vertical_metrics_parse_and_reject_garbage() {
        let (asc, desc, gap) = vertical_metrics(include_bytes!("../../../assets/fonts/Inter-Regular.ttf"), 0).unwrap();
        assert!((asc - 1984.0 / 2048.0).abs() < 1e-4 && (desc + 494.0 / 2048.0).abs() < 1e-4 && gap == 0.0);
        for bad in [&b""[..], b"ttcf", b"ttcf\0\0\0\0\0\0\0\0\xff\xff\xff\xff", b"\0\x01\0\0\0\xff\0\0\0\0\0\0"] {
            assert!(vertical_metrics(bad, 0).is_none());
            assert!(vertical_metrics(bad, u32::MAX).is_none());
        }
    }

    #[test]
    fn a_face_with_a_big_line_gap_is_shifted_down_onto_the_primary_baseline() {
        let inter = (1984.0 / 2048.0, -494.0 / 2048.0, 0.0);
        let hiragino = (0.88, -0.12, 0.5);
        assert!((baseline_offset(hiragino, inter) - 0.2338).abs() < 1e-3);
        assert_eq!(baseline_offset(inter, inter), 0.0);
        assert_eq!(baseline_offset((f32::NAN, 0.0, 0.0), inter), 0.0);
        let jb = (1.02, -0.3, 0.0);
        assert!((baseline_offset(hiragino, jb) - baseline_offset(hiragino, inter)).abs() < 0.005, "one offset serves the monospace family too");
    }

    #[test]
    fn lazy_registration_triggers_only_for_supported_scripts() {
        let ctx = egui::Context::default();
        install_with(&ctx, fake_sources(|| Some("ko".into())));
        let _ = render(&ctx, "Layer 1 – café");
        let n = ctx.fonts(|f| f.definitions().font_data.keys().filter(|k| k.starts_with(FONT_PREFIX)).count());
        assert_eq!(n, 0, "Latin text loads nothing");
        // Hangul isn't in the stand-in font, but the Korean candidate must have been registered.
        let _ = render(&ctx, "카드 배경");
        let fonts = ctx.fonts(|f| f.definitions().clone());
        let name = format!("{FONT_PREFIX}-0");
        assert!(fonts.font_data.contains_key(&name));
        for (fam, stack) in &fonts.families {
            assert_eq!(stack.last(), Some(&name), "{fam:?}: appended last so Latin keeps Inter");
        }
    }

    /// No system fonts at all: craft-fonts alone draws Japanese.
    fn craft_only(locale: fn() -> Option<String>) -> Sources {
        Sources { locale, files: |_| vec![], last_resort: Vec::new, scripts: |_| Vec::new(), embedded: craft_embedded }
    }

    #[test]
    fn craft_fonts_render_japanese_without_system_fonts() {
        let Some(ui_font) = craft_fonts::japanese_for_ui().first().copied() else {
            eprintln!("skipping: built without craft-fonts (set CRAFT_FONTS_DIR to a craft-fonts checkout)");
            return;
        };
        // An English locale tries Chinese first (none here), then Japanese: the craft font.
        for locale in [(|| Some("en_US".into())) as fn() -> Option<String>, || Some("ja_JP".into()), || None] {
            let ctx = egui::Context::default();
            crate::theme::install_fonts_with(&ctx, craft_only(locale));
            assert!(render(&ctx, "日本語の文字"), "tofu in 日本語の文字");
            assert!(render(&ctx, "レイヤー 1"), "tofu in kana");
            let fonts = ctx.fonts(|f| f.definitions().clone());
            let name = format!("{FONT_PREFIX}-0");
            assert!(fonts.font_data.get(&name).is_some_and(|d| std::ptr::eq(d.font.as_ref().as_ptr(), ui_font.bytes.as_ptr())), "BIZ UDPGothic Regular first");
            for (fam, stack) in &fonts.families {
                assert_eq!(stack.last(), Some(&name), "{fam:?}: appended last so Latin keeps Inter");
                assert!(stack.first().is_some_and(|f| f != &name), "{fam:?}");
            }
        }
    }

    #[test]
    fn ui_works_without_craft_fonts() {
        // The pre-craft-fonts behaviour: no embedded fonts, no system fonts. Latin renders,
        // Japanese is tried (and stays missing) without panicking, and the loader exhausts.
        let ctx = egui::Context::default();
        crate::theme::install_fonts_with(
            &ctx,
            Sources { locale: || None, files: |_| vec![], last_resort: Vec::new, scripts: |_| Vec::new(), embedded: no_embedded },
        );
        assert!(render(&ctx, "Layer 1 – café"));
        assert!(!render(&ctx, "日本語の文字"));
        let n = ctx.fonts(|f| f.definitions().font_data.keys().filter(|k| k.starts_with(FONT_PREFIX)).count());
        assert_eq!(n, 0);
        if craft_fonts::CRAFT_FONTS.is_empty() {
            assert!(craft_embedded(CjkScript::Japanese).is_empty());
        }
    }

    /// With the real system fonts, every CJK sample renders (skipped per script when the
    /// machine has no font for it, e.g. CI Linux without Noto CJK).
    #[test]
    fn system_fonts_cover_cjk_samples() {
        let available = |s: CjkScript| cjk::font_files(s).iter().any(|f| f.path.is_file());
        let samples = [
            ("图层", CjkScript::SimplifiedChinese),
            ("圖層", CjkScript::TraditionalChinese),
            ("카드 배경", CjkScript::Korean),
            ("レイヤー", CjkScript::Japanese),
        ];
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        for (text, script) in samples {
            if !available(script) {
                eprintln!("skipping {text}: no {script:?} system font");
                continue;
            }
            assert!(render(&ctx, text), "{text} still has missing glyphs");
        }
        // All at once in a fresh context too (several scripts in one frame).
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let all: String = samples.iter().filter(|(_, s)| available(*s)).map(|(t, _)| *t).collect::<Vec<_>>().join(" ");
        assert!(render(&ctx, &all), "{all}");
    }
}
