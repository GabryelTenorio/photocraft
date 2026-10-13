//! Camera Raw Settings ▸ Save/Load as an Adobe Camera Raw `.xmp` sidecar (issue #4274), so a
//! look can be reused on other images. The mapping lives in [`photocraft_algo::camera_raw_xmp`].
//!
//! Both commands are document-independent: the dialog passes its live settings to
//! `filter.cameraRaw.exportSettings`, and applies what `filter.cameraRaw.importSettings`
//! returns. As with the other file commands an agent supplies `data` (base64) instead of a
//! `path` when it has no filesystem access.

use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "filter.cameraRaw.exportSettings";
    let settings: photocraft_algo::camera_raw::CameraRaw = match p.get("settings") {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| bad(CMD, format!("bad `settings`: {e}")))?,
        None => Default::default(),
    };
    settings.validate().map_err(|e| bad(CMD, e))?;
    let xmp = photocraft_algo::camera_raw_xmp::to_xmp(&settings);
    let bytes = xmp.into_bytes();
    let mut out = json!({"bytes": bytes.len()});
    match p.get("path").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        Some(path) => {
            crate::file_cmds::write_file(path, &bytes)?;
            out["path"] = json!(path);
        }
        None => out["data"] = json!(photocraft_paint::tile::b64_encode(&bytes)),
    }
    let _ = s;
    Ok(out)
}

fn import(_s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "filter.cameraRaw.importSettings";
    let (bytes, _name) = crate::preset_import_cmds::file_bytes(p, CMD, "an .xmp settings file")?;
    let text = std::str::from_utf8(&bytes).map_err(|_| bad(CMD, "the .xmp file is not UTF-8 text"))?;
    let settings = photocraft_algo::camera_raw_xmp::from_xmp(text).map_err(|e| bad(CMD, e))?;
    Ok(json!({"settings": serde_json::to_value(settings).map_err(|e| EngineError::Other(e.to_string()))?}))
}

/// Camera Raw settings command specs.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "filter.cameraRaw.exportSettings",
            label: "Save Camera Raw Settings…",
            menu: &[],
            shortcut: None,
            params: r##"{"settings":Camera Raw settings,"path":".xmp file"? (else the bytes come back as base64 `data`)} → {bytes,path|data}"##,
            enabled: always,
            run: export,
            journal: true,
        },
        CommandSpec {
            id: "filter.cameraRaw.importSettings",
            label: "Load Camera Raw Settings…",
            menu: &[],
            shortcut: None,
            params: r##"{"path":".xmp file"?,"data":base64 bytes?} → {settings}"##,
            enabled: always,
            run: import,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
        s.execute(id, p)
    }

    #[test]
    fn export_then_import_round_trips() {
        let mut s = Session::default();
        let settings = photocraft_algo::camera_raw::CameraRaw {
            exposure: 0.75,
            contrast: -20.0,
            vibrance: 30.0,
            grade_shadows: photocraft_algo::camera_raw::Wheel { hue: 200.0, sat: 15.0, lum: -5.0 },
            point_curve: vec![[0.0, 0.0], [128.0, 150.0], [255.0, 255.0]],
            ..Default::default()
        };
        let value = serde_json::to_value(&settings).unwrap();

        let out = run(&mut s, "filter.cameraRaw.exportSettings", json!({"settings": value})).unwrap();
        let data = out["data"].as_str().expect("base64 data").to_string();
        assert!(out["bytes"].as_u64().unwrap() > 0);

        let back = run(&mut s, "filter.cameraRaw.importSettings", json!({"data": data})).unwrap();
        let got: photocraft_algo::camera_raw::CameraRaw = serde_json::from_value(back["settings"].clone()).unwrap();
        assert_eq!(got.exposure, 0.75);
        assert_eq!(got.contrast, -20.0);
        assert_eq!(got.vibrance, 30.0);
        assert_eq!(got.grade_shadows, settings.grade_shadows);
        assert_eq!(got.point_curve, settings.point_curve);
    }

    #[test]
    fn import_rejects_garbage_and_params_never_panic() {
        let mut s = Session::default();
        // Not an XMP packet.
        let data = photocraft_paint::tile::b64_encode(b"hello, not xmp");
        assert!(run(&mut s, "filter.cameraRaw.importSettings", json!({"data": data})).is_err());
        // Missing path/data, bad types, bad settings: all errors, never a panic.
        for p in [json!({}), json!({"data": 123}), json!({"path": ""})] {
            assert!(run(&mut s, "filter.cameraRaw.importSettings", p).is_err());
        }
        assert!(run(&mut s, "filter.cameraRaw.exportSettings", json!({"settings": {"exposure": "no"}})).is_err());
    }
}
