//! Directory cleanup only owns content-addressed objects; unrelated files survive.

mod common;

use common::temp_dir;
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::Document;
use photocraft_format::{PcraftWriter, SaveOptions};
use photocraft_geom::Size;

fn empty_doc() -> Document {
    Document::new("clean", Size::new(1, 1), ColorMode::Rgb, SampleType::U8)
}

#[test]
fn directory_cleanup_preserves_unrelated_zst_files() {
    let root = temp_dir("unrelated-object");
    let bundle = root.join("project.pcraft");
    let mut writer = PcraftWriter::new();
    writer.save_dir(&empty_doc(), &bundle, &SaveOptions::default()).unwrap();

    let unrelated = bundle.join("tiles/personal-notes.zst");
    let orphan = bundle.join("blobs").join(format!("{}.zst", "b".repeat(64)));
    std::fs::write(&unrelated, b"unrelated file").unwrap();
    std::fs::write(&orphan, b"unused content-addressed object").unwrap();

    let stats = writer.save_dir(&empty_doc(), &bundle, &SaveOptions::default()).unwrap();
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"unrelated file");
    assert!(!orphan.exists(), "a valid but unreferenced object is removed");
    assert_eq!(stats.objects_removed, 1);
    std::fs::remove_dir_all(root).unwrap();
}
