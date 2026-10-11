//! A directory `.pcraft` bundle owns only what lies beneath its root. A symlinked `tiles/`,
//! `blobs/`, manifest or object must not carry the save's writes or stale-object removal
//! elsewhere.

#![cfg(unix)]

mod common;
use common::*;
use photocraft_color::{ColorMode, SampleType};
use photocraft_format::*;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

/// A fresh scratch directory for one test.
fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("photocraft-bundle-symlinks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Saves into `bundle` and asserts the save was refused.
fn save_refused(bundle: &Path) {
    let doc = rich_doc(ColorMode::Rgb, SampleType::U8);
    let err = PcraftWriter::new().save_dir(&doc, bundle, &SaveOptions::default()).unwrap_err();
    assert!(err.to_string().contains("symbolic link"), "{err}");
}

#[test]
fn a_symlinked_object_directory_refuses_the_save_and_leaves_its_target_alone() {
    let dir = tmp("tiles");
    let (bundle, outside) = (dir.join("doc.pcraft"), dir.join("outside"));
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("sentinel.zst"), b"not ours").unwrap();
    symlink(&outside, bundle.join("tiles")).unwrap();

    save_refused(&bundle);

    assert_eq!(std::fs::read(outside.join("sentinel.zst")).unwrap(), b"not ours");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1, "nothing was written beside the sentinel");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_symlinked_manifest_or_object_refuses_the_save_and_keeps_its_target() {
    let dir = tmp("members");
    let doc = rich_doc(ColorMode::Rgb, SampleType::U8);
    let bundle = dir.join("doc.pcraft");
    PcraftWriter::new().save_dir(&doc, &bundle, &SaveOptions::default()).unwrap();
    let victim = dir.join("victim");
    std::fs::write(&victim, b"keep").unwrap();

    let manifest = bundle.join("manifest.json");
    std::fs::remove_file(&manifest).unwrap();
    symlink(&victim, &manifest).unwrap();
    save_refused(&bundle);
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    std::fs::remove_file(&manifest).unwrap();

    let object = std::fs::read_dir(bundle.join("tiles")).unwrap().next().unwrap().unwrap().path();
    std::fs::remove_file(&object).unwrap();
    symlink(&victim, &object).unwrap();
    save_refused(&bundle);
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    std::fs::remove_dir_all(dir).unwrap();
}
