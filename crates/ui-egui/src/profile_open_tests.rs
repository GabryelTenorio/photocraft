//! Files opened together whose embedded profile doesn't match the working space (File › Open with
//! several selected, Ask When Opening on). Checked in Photoshop 27.11: one Embedded Profile
//! Mismatch prompt at a time, naming its file, before that file opens; OK opens it and asks about
//! the next; Cancel opens neither it nor the files still to come.

use crate::{PhotocraftApp, Services};
use egui::{Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use photocraft_doc::{Color, ColorMode, Document, SampleType, Size};
use photocraft_engine::Session;
use serde_json::Value;

const USE: &str = "Use the embedded profile (instead of the working space)";
const CONVERT: &str = "Convert document's colors to the working space";
const DISCARD: &str = "Discard the embedded profile (don't color manage)";

/// A PNG tagged with Adobe RGB, which doesn't match the sRGB working space.
fn adobe_png() -> Vec<u8> {
    let mut d = Document::with_background("t", Size::new(8, 8), ColorMode::Rgb, SampleType::U8, Color::rgba(0.2, 0.6, 0.9, 1.0));
    d.icc_profile = Some(photocraft_cms::Builtin::from_id("adobe-rgb-compat").unwrap().profile().to_bytes());
    photocraft_io::export(&d, "t.png", &Default::default()).unwrap().bytes
}

fn harness(background: bool) -> Harness<'static, PhotocraftApp> {
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_max_steps(64).build_eframe(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        PhotocraftApp::new(
            Session::new(),
            Services {
                import: Some(Box::new(|name, bytes, depth| {
                    photocraft_io::import_with_svg_group_depth(name, bytes, depth).map(|r| (r.document, r.warnings)).map_err(|e| e.to_string())
                })),
                ..Default::default()
            },
        )
    });
    h.state_mut().background_jobs = background;
    h.run_steps(3);
    h.state_mut().run("edit.colorSettings", serde_json::json!({"askOnMismatch": true, "policyRgb": "preserve"})).unwrap();
    h
}

/// Open a.png, b.png and c.png together and let every read finish.
fn open_three(h: &mut Harness<'static, PhotocraftApp>) {
    let png = adobe_png();
    for name in ["a.png", "b.png", "c.png"] {
        h.state_mut().open_file(&format!("/pics/{name}"), &png).unwrap();
    }
    settle(h);
}

/// Run frames until no open is still reading.
fn settle(h: &mut Harness<'static, PhotocraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if h.state().jobs.opens.is_empty() && h.state().session.jobs().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(6);
}

fn docs(h: &Harness<'static, PhotocraftApp>) -> Vec<String> {
    h.state().session.documents().iter().map(|d| d.doc.name.clone()).collect()
}

/// The one prompt up: its message.
fn prompt(h: &Harness<'static, PhotocraftApp>) -> String {
    let d = &h.state().ui.dialogs;
    assert_eq!(d.len(), 1, "one prompt at a time");
    d[0].fields.get("message").and_then(Value::as_str).unwrap_or_default().to_string()
}

/// A real click (pointer events, hit-tested like a mouse).
fn click(h: &mut Harness<'static, PhotocraftApp>, label: &str) {
    let at: Pos2 = h.query_all_by_label(label).find(|n| !n.accesskit_node().is_disabled()).map(|n| n.rect().center()).unwrap();
    h.event(egui::Event::PointerMoved(at));
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    settle(h);
}

fn answer(h: &mut Harness<'static, PhotocraftApp>, choice: &str) {
    click(h, choice);
    click(h, "OK");
}

fn profile(h: &Harness<'static, PhotocraftApp>, name: &str) -> Option<String> {
    let st = h.state().session.documents().iter().find(|d| d.doc.name == name).unwrap();
    st.doc.icc_profile.as_ref().map(|b| photocraft_engine::color_cmds::profile_from_bytes(b).unwrap().description.clone())
}

#[test]
fn files_opened_together_ask_one_at_a_time_before_each_opens() {
    for background in [false, true] {
        let mut h = harness(background);
        open_three(&mut h);
        for (name, opened) in [("a.png", vec![]), ("b.png", vec!["a.png"]), ("c.png", vec!["a.png", "b.png"])] {
            assert_eq!(docs(&h), opened, "background {background}: {name} opens only after its prompt");
            let msg = prompt(&h);
            assert!(
                msg.starts_with(&format!("The document “{name}” has an embedded color profile that does not match the current RGB working space.")),
                "background {background}: {msg}"
            );
            answer(&mut h, USE);
        }
        assert!(h.state().ui.dialogs.is_empty());
        assert_eq!(docs(&h), ["a.png", "b.png", "c.png"], "background {background}");
        assert_eq!(h.state().session.active().map(|d| d.doc.name.as_str()), Some("c.png"));
    }
}

/// The prompt reads in the interface language: its sentence, the Embedded and Working lines and
/// the mode's name (French "RVB"). The file's name goes in as it is, braces and all.
#[test]
fn the_prompt_is_translated_and_still_names_its_file() {
    let png = adobe_png();
    for (code, sentence, embedded, working) in [
        (
            "en",
            "The document “shot {mode}.png” has an embedded color profile that does not match the current RGB working space.",
            "Embedded: Adobe RGB",
            "Working: sRGB",
        ),
        (
            "de",
            "Das Dokument „shot {mode}.png“ hat ein eingebettetes Farbprofil, das nicht zum aktuellen RGB-Arbeitsfarbraum passt.",
            "Eingebettet: Adobe RGB",
            "Arbeitsfarbraum: sRGB",
        ),
        (
            "fr",
            "Le document «NBSPshot {mode}.pngNBSP» a un profil colorimétrique incorporé qui ne correspond pas à l'espace de travail RVB actuel.",
            "IncorporéNBSP: Adobe RGB",
            "Espace de travailNBSP: sRGB",
        ),
        (
            "ja",
            "ドキュメント「shot {mode}.png」の埋め込みカラープロファイルは、現在の RGB 作業用スペースと一致しません。",
            "埋め込み: Adobe RGB",
            "作業用スペース: sRGB",
        ),
    ] {
        let services = Services {
            import: Some(Box::new(|name, bytes, depth| {
                photocraft_io::import_with_svg_group_depth(name, bytes, depth).map(|r| (r.document, r.warnings)).map_err(|e| e.to_string())
            })),
            ..Default::default()
        };
        let mut app = PhotocraftApp::new(Session::new(), services);
        app.run("edit.colorSettings", serde_json::json!({"askOnMismatch": true, "policyRgb": "preserve"})).unwrap();
        let lang = crate::i18n::Lang::from_code(code).unwrap();
        crate::i18n::with_language(lang, || app.open_file("/pics/shot {mode}.png", &png)).unwrap();
        assert!(app.session.documents().is_empty(), "{code}: held until answered");
        let message = app.ui.dialogs[0].fields.get("message").and_then(Value::as_str).unwrap().to_string();
        let lines: Vec<&str> = message.lines().collect();
        assert_eq!(lines.len(), 3, "{code}: {message}");
        assert_eq!(lines[0], sentence.replace("NBSP", "\u{a0}"), "{code}");
        assert!(lines[1].starts_with(&embedded.replace("NBSP", "\u{a0}")), "{code}: {}", lines[1]);
        assert!(lines[2].starts_with(&working.replace("NBSP", "\u{a0}")), "{code}: {}", lines[2]);
    }
}

#[test]
fn each_answer_applies_to_its_own_file() {
    for background in [false, true] {
        let mut h = harness(background);
        open_three(&mut h);
        answer(&mut h, CONVERT);
        answer(&mut h, DISCARD);
        answer(&mut h, USE);
        assert_eq!(docs(&h), ["a.png", "b.png", "c.png"]);
        assert_eq!(profile(&h, "a.png").as_deref(), Some("sRGB IEC61966-2.1"), "background {background}: a converted");
        assert_eq!(profile(&h, "b.png"), None, "background {background}: b discarded");
        assert!(profile(&h, "c.png").unwrap().starts_with("Adobe RGB"), "background {background}: c kept");
    }
}

#[test]
fn cancel_opens_neither_the_file_nor_the_ones_still_to_come() {
    for background in [false, true] {
        let mut h = harness(background);
        open_three(&mut h);
        answer(&mut h, USE);
        click(&mut h, "Cancel");
        settle(&mut h);
        assert!(h.state().ui.dialogs.is_empty(), "background {background}: no prompt left");
        assert_eq!(docs(&h), ["a.png"], "background {background}");
        // A later open asks again.
        h.state_mut().open_file("/pics/d.png", &adobe_png()).unwrap();
        settle(&mut h);
        assert!(prompt(&h).contains("“d.png”"));
    }
}
