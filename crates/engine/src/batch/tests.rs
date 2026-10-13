use serde_json::{Value, json};

use super::*;
use crate::Session;
use crate::file_cmds::{join, save_doc};
use crate::jobs::{JobCtx, Started};

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("photocraft-batch-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.to_string_lossy().into_owned()
}

fn png(dir: &str, name: &str, color: &str) -> String {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 8, "height": 6, "background": color})).unwrap();
    let path = join(dir, name);
    std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
    save_doc(&s.active().unwrap().doc, &path, None).unwrap();
    path
}

const DATE: Date = Date { year: 2026, month: 10, day: 12 };

fn name(template: &str, start: u64, stem: &str, ext: &str, index: u64) -> String {
    Naming::parse(template, start).unwrap().file_name(stem, ext, index, DATE)
}

#[test]
fn naming_tokens() {
    assert_eq!(name("{name}.{ext}", 1, "Photo", "jpg", 0), "Photo.jpg");
    assert_eq!(name("{NAME}_{Name}.{EXT}", 1, "my Photo", "JPG", 0), "MY PHOTO_My photo.JPG");
    assert_eq!(name("{name}-{serial}.{ext}", 1, "a", "png", 0), "a-1.png");
    assert_eq!(name("{name}-{serial:3}.{ext}", 7, "a", "png", 2), "a-009.png");
    assert_eq!(name("{serial:2}", 1, "a", "png", 150), "151", "a serial longer than its width keeps every digit");
    assert_eq!(name("{letter}{LETTER}", 1, "a", "png", 0), "aA");
    assert_eq!(name("{letter}", 1, "a", "png", 25), "z");
    assert_eq!(name("{letter}", 1, "a", "png", 26), "aa");
    assert_eq!(name("{LETTER}", 1, "a", "png", 27), "AB");
    assert_eq!(
        name("{date:mmddyy} {date:mmdd} {date:yyyymmdd} {date:yymmdd} {date:yyddmm} {date:ddmmyy}", 1, "a", "png", 0),
        "101226 1012 20261012 261012 261210 121026"
    );
    assert_eq!(name("final copy.{ext}", 1, "a", "png", 0), "final copy.png", "literal text");
    assert_eq!(Naming::plain("-x").file_name("a", "PNG", 0, DATE), "a-x.png");
    assert_eq!(Naming::plain("").file_name("a", "jpg", 0, DATE), "a.jpg");
    assert_eq!(Naming::parse(Naming::DEFAULT, 1).unwrap(), Naming::plain(""), "the default template is the plain naming");
}

#[test]
fn naming_rejects_bad_templates() {
    for (t, why) in [
        ("{name", "unclosed"),
        ("{name}}", "stray"),
        ("{nope}.{ext}", "unknown token"),
        ("{serial:5}", "unknown token"),
        ("{serial:0}", "unknown token"),
        ("{date:yyyy}", "unknown token"),
        ("", "empty"),
        ("../{name}.{ext}", "separators"),
        ("sub/{name}", "separators"),
        ("{name}\\x", "separators"),
    ] {
        let e = Naming::parse(t, 1).unwrap_err();
        assert!(e.contains(why), "{t:?}: {e}");
    }
    assert_eq!(name("v1.2.{ext}", 1, "a", "png", 0), "v1.2.png", "dots inside a name are fine");
}

#[test]
fn options_from_params() {
    let o = Options::from_params(&json!({}), "x").unwrap();
    assert_eq!(o, Options::plain(""));
    let o =
        Options::from_params(&json!({"naming": " {name}-{serial:2}.{ext} ", "startSerial": 5, "stopOnError": true, "errorLog": "/tmp/log.txt"}), "x").unwrap();
    assert!(o.stop_on_error && o.error_log.as_deref() == Some("/tmp/log.txt"));
    assert_eq!(o.naming.file_name("a", "png", 0, DATE), "a-05.png");
    let e = Options::from_params(&json!({"naming": "{bad}"}), "x").unwrap_err();
    assert!(e.to_string().contains("naming"), "{e}");
    assert!(Options::from_params(&json!({"startSerial": -1}), "x").is_err());
    assert!(Options::from_params(&json!({"startSerial": 1.5}), "x").is_err());
    assert_eq!(Options::from_params(&json!({"naming": "", "errorLog": "  "}), "x").unwrap(), Options::plain(""), "empty strings mean the defaults");
}

#[test]
fn recursive_listing_mirrors_folders() {
    let dir = tmp("list");
    png(&dir, "b.png", "#ffffff");
    png(&dir, "a.png", "#ffffff");
    png(&dir, "sub/z.png", "#ffffff");
    png(&dir, "sub/inner/y.png", "#ffffff");
    png(&dir, "zsub/x.png", "#ffffff");
    std::fs::write(join(&dir, "sub/notes.txt"), "no").unwrap();
    let flat = inputs(&json!({"input": dir}), "x").unwrap();
    assert_eq!(flat.iter().map(|i| crate::file_cmds::file_name(&i.path)).collect::<Vec<_>>(), ["a.png", "b.png"], "without the flag, only the top folder");
    let all = inputs(&json!({"input": dir, "includeSubfolders": true}), "x").unwrap();
    let got: Vec<(String, &str)> = all.iter().map(|i| (crate::file_cmds::file_name(&i.path), i.subdir.as_str())).collect();
    assert_eq!(
        got,
        [("a.png".to_string(), ""), ("b.png".into(), ""), ("z.png".into(), "sub"), ("y.png".into(), "sub/inner"), ("x.png".into(), "zsub")],
        "each folder's files, then its subfolders, sorted by name"
    );
    assert!(list_images_recursive(&join(&dir, "missing")).is_err());
}

#[test]
fn batch_into_subfolders_with_a_naming_template() {
    let dir = tmp("batch");
    let input = join(&dir, "in");
    png(&input, "a.png", "#ff0000");
    png(&input, "sub/a.png", "#00ff00");
    let out = join(&dir, "out");
    let mut s = Session::new();
    let r = s
        .execute(
            "file.automate.batch",
            json!({"steps": [["image.adjustments.invert", {}]], "input": input, "includeSubfolders": true, "output": out, "format": "png", "naming": "{name}_{serial:2}.{ext}", "startSerial": 10}),
        )
        .unwrap();
    let files: Vec<&str> = r["files"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(files, [join(&out, "a_10.png"), join(&out, "sub/a_11.png")], "{r}");
    assert!(r["errors"].as_array().unwrap().is_empty() && r.get("stopped").is_none(), "{r}");
    for f in files {
        assert!(std::path::Path::new(f).is_file(), "{f} written");
    }
    // Without the mirrored folders the two a.png would collide: the second is reported.
    let r = s
        .execute(
            "file.automate.batch",
            json!({"steps": [], "input": [join(&input, "a.png"), join(&input, "sub/a.png")], "output": join(&dir, "flat"), "format": "png"}),
        )
        .unwrap();
    assert_eq!(r["files"].as_array().unwrap().len(), 1);
    assert_eq!(r["errors"].as_array().unwrap().len(), 1, "{r}");
}

#[test]
fn stop_on_error_skips_the_rest_and_logs() {
    let dir = tmp("stop");
    let input = join(&dir, "in");
    png(&input, "a.png", "#ff0000");
    std::fs::write(join(&input, "b.png"), b"not a png").unwrap();
    png(&input, "c.png", "#0000ff");
    let out = join(&dir, "out");
    let log = join(&dir, "errors.txt");
    let mut s = Session::new();
    let p = json!({"steps": [], "input": input, "output": out, "format": "png", "errorLog": log});
    let r = s.execute("file.automate.batch", p.clone()).unwrap();
    assert_eq!((r["files"].as_array().unwrap().len(), r["errors"].as_array().unwrap().len()), (2, 1), "every file is tried: {r}");
    assert!(r.get("stopped").is_none());
    assert_eq!(r["errorLog"], json!(log));
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.starts_with("PhotoCraft Batch: 1 error(s)") && text.contains("b.png: "), "{text}");

    let mut p = p;
    p["stopOnError"] = json!(true);
    p["output"] = json!(join(&dir, "out2"));
    let r = s.execute("file.automate.batch", p).unwrap();
    assert_eq!((r["files"].as_array().unwrap().len(), r["errors"].as_array().unwrap().len()), (1, 1), "{r}");
    // Folder listings carry the platform's separators (`\` on Windows): compare the file names.
    let name = |v: &Value| crate::file_cmds::file_name(v.as_str().unwrap());
    assert_eq!(name(&r["stopped"]), "b.png", "{r}");
    assert_eq!(r["skipped"].as_array().unwrap().iter().map(name).collect::<Vec<_>>(), ["c.png"], "{r}");
    assert!(!std::path::Path::new(&join(&dir, "out2/c.png")).exists(), "c was not processed");

    // No errors: no log is written (an old one is not touched either).
    std::fs::remove_file(&log).unwrap();
    let r = s
        .execute("file.automate.batch", json!({"steps": [], "input": [join(&input, "a.png")], "output": join(&dir, "out3"), "format": "png", "errorLog": log}))
        .unwrap();
    assert!(r.get("errorLog").is_none() && !std::path::Path::new(&log).exists(), "{r}");
}

#[test]
fn a_cancelled_run_stops_and_still_logs() {
    let dir = tmp("cancel");
    let input = join(&dir, "in");
    let a = png(&input, "a.png", "#ff0000");
    let log = join(&dir, "errors.txt");
    let ctx = JobCtx::new();
    ctx.cancel();
    let options = Options { naming: Naming::plain(""), stop_on_error: false, error_log: Some(log.clone()) };
    let r = crate::file_cmds::process_files(&[Input::file(&a)], &join(&dir, "out"), "png", Default::default(), &options, Some(&ctx), &|_| Ok(()));
    assert!(matches!(r, Err(crate::EngineError::Cancelled)), "{r:?}");
    assert!(!std::path::Path::new(&join(&dir, "out/a.png")).exists(), "nothing was written after the cancel");
}

#[test]
fn batch_runs_as_a_background_job_with_progress() {
    let dir = tmp("job");
    let input = join(&dir, "in");
    png(&input, "a.png", "#ff0000");
    png(&input, "b.png", "#00ff00");
    let out = join(&dir, "out");
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 4, "height": 4})).unwrap();
    let r = s.start("file.automate.batch", json!({"steps": [["image.adjustments.invert", {}]], "input": input, "output": out, "format": "png"})).unwrap();
    let Started::Job(id) = r else { panic!("expected a job: {r:?}") };
    let v = s.wait_job(id).unwrap();
    assert_eq!(v["files"].as_array().unwrap().len(), 2, "{v}");
    assert!(s.active().is_some_and(|d| d.doc.size.width == 4), "the open document is untouched");
}

#[test]
fn source_open_runs_the_steps_on_every_open_document() {
    let dir = tmp("open");
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 8, "height": 6, "background": "white", "name": "First"})).unwrap();
    s.execute("file.new", json!({"width": 8, "height": 6, "background": "white", "name": "Second"})).unwrap();
    s.set_active(0);
    let out = join(&dir, "out");
    let r = s
        .execute(
            "file.automate.batch",
            json!({"steps": [["image.adjustments.invert", {}]], "source": "open", "output": out, "format": "png", "naming": "{serial:2}-{name}.{ext}"}),
        )
        .unwrap();
    assert_eq!(r["documents"], 2);
    let files: Vec<&str> = r["files"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(files, [join(&out, "01-First.png"), join(&out, "02-Second.png")], "{r}");
    assert_eq!(s.active_index(), Some(0), "the active document is restored");
    for d in s.documents() {
        let px = photocraft_compose::render(&d.doc, photocraft_geom::Rect::new(1, 1, 2, 2)).px[0];
        assert!(px[0] < 0.01, "both documents were inverted: {px:?}");
        assert!(d.history.can_undo(), "as undoable steps");
    }
    // Destination None: no output, the documents just change.
    let r = s.execute("file.automate.batch", json!({"steps": [["image.adjustments.invert", {}]], "source": "open"})).unwrap();
    assert!(r["files"].as_array().unwrap().is_empty() && r["errors"].as_array().unwrap().is_empty(), "{r}");
    // "same" on an untitled document saves a PSD.
    let r = s.execute("file.automate.batch", json!({"steps": [], "source": "open", "output": join(&dir, "same")})).unwrap();
    assert!(r["files"][0].as_str().unwrap().ends_with("First.psd"), "{r}");
    // A failing step with stop on error.
    let r = s
        .execute(
            "file.automate.batch",
            json!({"steps": [["layer.delete", {"layer": 999_999}]], "source": "open", "stopOnError": true, "errorLog": join(&dir, "log.txt")}),
        )
        .unwrap();
    assert_eq!(r["errors"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["stopped"], "First");
    assert_eq!(r["skipped"], json!(["Second"]));
    assert!(std::fs::read_to_string(join(&dir, "log.txt")).unwrap().contains("First: "));
    assert!(s.execute("file.automate.batch", json!({"steps": [], "source": "nope"})).is_err());
    let mut empty = Session::new();
    assert!(empty.execute("file.automate.batch", json!({"steps": [], "source": "open"})).is_err());
}
