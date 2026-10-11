//! Data-set exports must authorize stored replacement reads before producing any variation.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use photocraft_automation::{AuthorizedWorkspace, Headless};
use photocraft_engine::Session;
use serde_json::json;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT_SCRATCH_ID: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "photocraft-data-set-authority-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            NEXT_SCRATCH_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn blue_replacement(&self) -> PathBuf {
        let outside = self.0.join("outside.png");
        let mut image = Session::new();
        image.execute("file.new", json!({"width":2,"height":2,"background":"#0000ff"})).unwrap();
        let bytes = photocraft_io::export(&image.active().unwrap().doc, "png", &Default::default()).unwrap().bytes;
        std::fs::write(&outside, bytes).unwrap();
        outside
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn template(mut headless: Headless, replacement: &Path) -> Headless {
    headless.command_run("file.new", json!({"width":2,"height":2,"background":"white"})).unwrap();
    let layer = headless.session.active().unwrap().doc.layers[0].id.0;
    headless
        .command_run(
            "image.variables.define",
            json!({"defs":[
                {"name":"show","layer":layer,"type":"visibility"},
                {"name":"photo","layer":layer,"type":"pixelReplacement"}
            ]}),
        )
        .unwrap();
    headless
        .command_run(
            "image.variables.dataSets",
            json!({"dataSets":[
                {"name":"plain","values":[{"variable":"show","kind":"visibility","value":false}]},
                {"name":"pixels","values":[{"variable":"photo","kind":"pixels","value":replacement.to_str().unwrap()}]}
            ]}),
        )
        .unwrap();
    headless
}

#[test]
fn workspace_export_denies_ambient_pixel_reads_before_output_and_allows_nonpixel_subset() {
    let scratch = Scratch::new();
    let replacement = scratch.blue_replacement();
    let source_bytes = std::fs::read(&replacement).unwrap();
    for read_authority in [false, true] {
        let write_root = scratch.0.join(if read_authority { "read-write" } else { "write-only" });
        std::fs::create_dir(&write_root).unwrap();
        let workspace = AuthorizedWorkspace::new(read_authority.then_some(write_root.as_path()), Some(&write_root)).unwrap();
        let mut headless = template(Headless::with_workspace(workspace), &replacement);
        let before = headless.session.active().unwrap().doc.clone();
        let revision = headless.session.active().unwrap().revision;
        let history = headless.session.active().unwrap().history.entries();
        let past = headless.session.active().unwrap().history.past_len();
        let active = headless.session.active().unwrap().active_layer;

        // The first row is allowed, but a forbidden read in the later row rejects the whole batch.
        let error = headless.export_data_sets(None, "blocked", &json!({"format":"png"})).unwrap_err().to_string();
        assert!(error.contains("image.applyDataSet") && error.contains("ambient filesystem"), "{error}");
        assert_eq!(std::fs::read_dir(write_root.join("blocked")).unwrap().count(), 0);
        let after = headless.session.active().unwrap();
        assert!(Arc::ptr_eq(&before, &after.doc));
        assert_eq!(after.revision, revision);
        assert_eq!(after.history.entries(), history);
        assert_eq!(after.history.past_len(), past);
        assert_eq!(after.active_layer, active);
        assert_eq!(std::fs::read(&replacement).unwrap(), source_bytes);

        // Only the selected rows are checked: a visibility-only export needs no read authority.
        let result = headless.export_data_sets(None, "safe", &json!({"format":"png","dataSets":["plain"]})).unwrap();
        assert_eq!(result["count"], 1);
        let output = write_root.join("safe/plain.png");
        assert_eq!(std::fs::read_dir(write_root.join("safe")).unwrap().count(), 1);
        let exported = photocraft_io::import("plain.png", &std::fs::read(output).unwrap()).unwrap().document;
        assert_eq!(photocraft_compose::render(&exported, exported.bounds()).px[0], [0.0; 4]);
        let after = headless.session.active().unwrap();
        assert!(Arc::ptr_eq(&before, &after.doc));
        assert_eq!(after.revision, revision);
        assert_eq!(after.history.entries(), history);
        assert_eq!(after.history.past_len(), past);
        assert_eq!(after.active_layer, active);
        drop(headless);
    }
}

#[test]
fn trusted_local_export_keeps_pixel_replacement_behavior() {
    let scratch = Scratch::new();
    let replacement = scratch.blue_replacement();
    let output = scratch.0.join("trusted");
    let mut headless = template(Headless::trusted_local(), &replacement);
    let before = headless.session.active().unwrap().doc.clone();
    let revision = headless.session.active().unwrap().revision;
    let past = headless.session.active().unwrap().history.past_len();
    let result = headless.export_data_sets(None, output.to_str().unwrap(), &json!({"format":"png","dataSets":["pixels"]})).unwrap();
    assert_eq!(result["count"], 1);
    let exported = photocraft_io::import("pixels.png", &std::fs::read(output.join("pixels.png")).unwrap()).unwrap().document;
    assert_eq!(photocraft_compose::render(&exported, exported.bounds()).px[0], [0.0, 0.0, 1.0, 1.0]);
    assert!(Arc::ptr_eq(&before, &headless.session.active().unwrap().doc));
    assert_eq!(headless.session.active().unwrap().revision, revision);
    assert_eq!(headless.session.active().unwrap().history.past_len(), past);
    drop(headless);
}
