//! A .pcraft directory save must never touch files reached through symlinks.
//! Ordinary ZIP saves are unaffected.

mod common;

use common::temp_dir;
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::Document;
use photocraft_format::{PcraftWriter, SaveOptions};
use photocraft_geom::Size;

fn empty_doc() -> Document {
    Document::new("safe", Size::new(1, 1), ColorMode::Rgb, SampleType::U8)
}

#[cfg(unix)]
#[test]
fn linked_object_and_preview_directories_cannot_touch_external_files() {
    use std::os::unix::fs::symlink;

    for sub in ["tiles", "blobs", "composite"] {
        let root = temp_dir(sub);
        let bundle = root.join("project.pcraft");
        let outside = root.join("outside");
        std::fs::create_dir(&bundle).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let filename = if sub == "composite" { "preview.png".to_string() } else { format!("{}.zst", "a".repeat(64)) };
        let marker = outside.join(filename);
        std::fs::write(&marker, b"external content must survive").unwrap();
        symlink(&outside, bundle.join(sub)).unwrap();

        let result = PcraftWriter::new().save_dir(&empty_doc(), &bundle, &SaveOptions::default());
        assert!(result.is_err(), "linked {sub} directory must be rejected");
        assert_eq!(std::fs::read(&marker).unwrap(), b"external content must survive");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn linked_manifest_and_preview_files_cannot_overwrite_external_files() {
    use std::os::unix::fs::symlink;

    for file in ["manifest.json", "thumb.png", "preview.png"] {
        let root = temp_dir(file);
        let bundle = root.join("project.pcraft");
        std::fs::create_dir(&bundle).unwrap();
        std::fs::create_dir(bundle.join("composite")).unwrap();
        let outside = root.join("external");
        std::fs::write(&outside, b"outside data").unwrap();
        let dest = if file == "preview.png" { bundle.join("composite").join(file) } else { bundle.join(file) };
        symlink(&outside, dest).unwrap();

        let result = PcraftWriter::new().save_dir(&empty_doc(), &bundle, &SaveOptions::default());
        assert!(result.is_err(), "linked {file} must be rejected");
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside data");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn linked_existing_tile_cannot_overwrite_external_data() {
    use std::os::unix::fs::symlink;

    let root = temp_dir("tile-link");
    let bundle = root.join("project.pcraft");
    let doc = common::rich_doc(ColorMode::Rgb, SampleType::U8);
    let mut writer = PcraftWriter::new();
    writer.save_dir(&doc, &bundle, &SaveOptions::default()).unwrap();
    let entry = std::fs::read_dir(bundle.join("tiles")).unwrap().next().unwrap().unwrap().path();
    let outside = root.join("outside.zst");
    std::fs::write(&outside, b"external content must survive").unwrap();
    std::fs::remove_file(&entry).unwrap();
    symlink(&outside, &entry).unwrap();

    let result = writer.save_dir(&doc, &bundle, &SaveOptions::default());
    assert!(result.is_err(), "linked content-addressed tile must be rejected");
    assert_eq!(std::fs::read(&outside).unwrap(), b"external content must survive");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn linked_bundle_root_is_allowed() {
    use std::os::unix::fs::symlink;

    let root = temp_dir("root-link");
    let outside = root.join("elsewhere");
    let bundle = root.join("project.pcraft");
    std::fs::create_dir(&outside).unwrap();
    symlink(&outside, &bundle).unwrap();
    PcraftWriter::new().save_dir(&empty_doc(), &bundle, &SaveOptions::default()).unwrap();
    assert!(outside.join("manifest.json").is_file());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_cleanup_preserves_unrelated_zst_files() {
    let root = temp_dir("unrelated");
    let bundle = root.join("project.pcraft");
    let mut writer = PcraftWriter::new();
    writer.save_dir(&empty_doc(), &bundle, &SaveOptions::default()).unwrap();
    let unrelated = bundle.join("tiles").join("personal-notes.zst");
    std::fs::write(&unrelated, b"not a content-addressed tile").unwrap();
    writer.save_dir(&empty_doc(), &bundle, &SaveOptions::default()).unwrap();
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"not a content-addressed tile");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn cleanup_unlinks_stale_symlink_without_touching_external_target() {
    use std::os::unix::fs::symlink;

    let root = temp_dir("gc-symlink");
    let bundle = root.join("project.pcraft");
    let mut writer = PcraftWriter::new();
    writer.save_dir(&empty_doc(), &bundle, &SaveOptions::default()).unwrap();
    let external = root.join("external.zst");
    std::fs::write(&external, b"outside data").unwrap();
    let stale = bundle.join("tiles").join(format!("{}.zst", "b".repeat(64)));
    symlink(&external, &stale).unwrap();

    writer.save_dir(&empty_doc(), &bundle, &SaveOptions::default()).unwrap();
    assert!(!std::fs::symlink_metadata(&stale).is_ok());
    assert_eq!(std::fs::read(&external).unwrap(), b"outside data");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_linked_bundle_subdirectory_is_rejected() {
    use std::os::windows::fs::symlink_dir;

    let root = temp_dir("windows-subdir-link");
    let bundle = root.join("project.pcraft");
    let external = root.join("external");
    std::fs::create_dir(&bundle).unwrap();
    std::fs::create_dir(&external).unwrap();
    let marker = external.join("marker.txt");
    std::fs::write(&marker, b"untouched").unwrap();
    if let Err(e) = symlink_dir(&external, bundle.join("tiles")) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            std::fs::remove_dir_all(root).unwrap();
            return; // Windows without Developer Mode or symlink privileges.
        }
        panic!("could not create test symlink: {e}");
    }

    assert!(PcraftWriter::new().save_dir(&empty_doc(), &bundle, &SaveOptions::default()).is_err());
    assert_eq!(std::fs::read(&marker).unwrap(), b"untouched");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_linked_bundle_root_is_allowed() {
    use std::os::windows::fs::symlink_dir;

    let root = temp_dir("windows-root-link");
    let target = root.join("actual-project");
    let link = root.join("project.pcraft");
    std::fs::create_dir(&target).unwrap();
    if let Err(e) = symlink_dir(&target, &link) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            std::fs::remove_dir_all(root).unwrap();
            return;
        }
        panic!("could not create test symlink: {e}");
    }
    PcraftWriter::new().save_dir(&empty_doc(), &link, &SaveOptions::default()).unwrap();
    assert!(target.join("manifest.json").is_file());
    std::fs::remove_dir_all(root).unwrap();
}
