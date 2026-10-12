#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::Document;
use photocraft_engine::Session;
use photocraft_geom::Size;
use photocraft_ops::LayerTarget;
use serde_json::json;
use std::sync::Arc;
#[test]
fn craft_session_tabs_and_both_history_stacks_round_trip() {
    let mut s = Session::new();
    s.add_document(Document::new("first", Size::new(8, 8), ColorMode::Rgb, SampleType::U8), None);
    for (name, dpi) in [("second", 144.0), ("third", 300.0)] {
        let d = s.active_mut().unwrap();
        let before = d.doc.clone();
        Arc::make_mut(&mut d.doc).resolution_dpi = dpi;
        d.history.record(name, before, LayerTarget::default());
        d.revision += 1;
    }
    assert!(s.undo());
    s.add_document(Document::new("other tab", Size::new(4, 4), ColorMode::Rgb, SampleType::U8), None);
    s.set_active(0);
    let bytes = s.craft_capture(json!({"zoom":2})).unwrap();
    let mut restored = Session::new();
    assert_eq!(restored.craft_restore(&bytes).unwrap(), json!({"zoom":2}));
    assert_eq!(restored.documents().len(), 2);
    assert_eq!(restored.active_index(), Some(0));
    assert_eq!(restored.active().unwrap().doc.resolution_dpi, 144.0);
    assert!(restored.redo());
    assert_eq!(restored.active().unwrap().doc.resolution_dpi, 300.0);
    assert!(restored.undo());
    assert!(restored.undo());
    assert_eq!(restored.active().unwrap().doc.resolution_dpi, 72.0);
    let before = restored.documents().len();
    assert!(restored.craft_restore(&bytes[..20]).is_err());
    assert_eq!(restored.documents().len(), before);
}

#[test]
fn recovery_inspection_rejects_wrong_app_and_malformed_envelopes() {
    for bytes in [b"".as_slice(), b"CRAFTS01", b"CRAFTS01\xff\xff\xff\xff"] {
        assert!(Session::craft_ui(bytes).is_err());
    }
    let meta = br#"{"app":"unrelated","schema":1,"ui":{}}"#;
    let mut bytes = b"CRAFTS01".to_vec();
    bytes.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    bytes.extend_from_slice(meta);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    assert!(Session::craft_ui(&bytes).is_err());
}
