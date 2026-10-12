//! Load a Photoshop `.atn` action set into the session.
//!
//! Only events PhotoCraft can play are kept. Everything else is reported and left out of the
//! action. This does not install a built-in action; it only reads the file the caller passed.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use photocraft_psd::atn::{ActionCommand, ActionFile};
use photocraft_psd::descriptor::{Descriptor, ReferenceItem, Value as DescValue};

use crate::actions_cmds::{self, Action, PLAY_SHORTCUT_PREFIX, color_from_index};
use crate::{EngineError, Result, Session};

const CMD: &str = "actions.importAtn";
const MAX_BYTES: usize = 8_000_000;
const MAX_BASE64: usize = 12_000_000;
const MAX_SET_CHARS: usize = 256;
const MAX_SETS: usize = 64;

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: CMD.into(), msg: msg.into() }
}

fn id_text(id: &photocraft_psd::descriptor::Id) -> String {
    String::from_utf8_lossy(id.as_bytes()).into_owned()
}

fn key_is(id: &photocraft_psd::descriptor::Id, names: &[&str]) -> bool {
    let text = id_text(id);
    let trimmed = text.trim();
    names.iter().any(|name| text == *name || trimmed.eq_ignore_ascii_case(name))
}

fn event_is(event: &str, names: &[&str]) -> bool {
    let trimmed = event.trim();
    names.iter().any(|name| event == *name || trimmed.eq_ignore_ascii_case(name))
}

fn find<'a>(desc: &'a Descriptor, keys: &[&str]) -> Option<&'a DescValue> {
    desc.items.iter().find(|(key, _)| key_is(key, keys)).map(|(_, value)| value)
}

fn number(value: &DescValue) -> Option<f64> {
    match value {
        DescValue::UnitFloat { value, .. } => value.is_finite().then_some(*value),
        DescValue::Double(value) => value.is_finite().then_some(*value),
        DescValue::Integer(value) => Some(f64::from(*value)),
        DescValue::LargeInteger(value) => Some(*value as f64),
        _ => None,
    }
}

fn text(value: &DescValue) -> Option<String> {
    match value {
        DescValue::Text(units) => Some(units.to_string_lossy()),
        _ => None,
    }
}

fn flag(value: &DescValue) -> Option<bool> {
    match value {
        DescValue::Boolean(on) => Some(*on),
        _ => None,
    }
}

fn object(value: &DescValue) -> Option<&Descriptor> {
    match value {
        DescValue::Descriptor(desc) | DescValue::GlobalObject(desc) => Some(desc),
        _ => None,
    }
}

fn class_is(desc: &Descriptor, names: &[&str]) -> bool {
    event_is(&id_text(&desc.class_id), names)
}

fn walk(value: &DescValue, visit: &mut impl FnMut(&ReferenceItem)) {
    match value {
        DescValue::Reference(items) => {
            for item in items {
                visit(item);
            }
        }
        DescValue::Descriptor(desc) | DescValue::GlobalObject(desc) => {
            for (_, child) in &desc.items {
                walk(child, visit);
            }
        }
        DescValue::List(items) => {
            for child in items {
                walk(child, visit);
            }
        }
        _ => {}
    }
}

fn mentions(desc: &Descriptor, needle: &[&str]) -> bool {
    let mut found = false;
    for (_, value) in &desc.items {
        walk(value, &mut |item| {
            let hit = match item {
                ReferenceItem::Property { key, .. } => event_is(&id_text(key), needle),
                ReferenceItem::Enumerated { value, type_id, .. } => event_is(&id_text(value), needle) || event_is(&id_text(type_id), needle),
                ReferenceItem::Class(class) | ReferenceItem::Name { class, .. } | ReferenceItem::Offset { class, .. } => {
                    event_is(&id_text(&class.class_id), needle)
                }
                _ => false,
            };
            found |= hit;
        });
    }
    found
}

fn channel_byte(c: f64) -> u8 {
    let c = if (0.0..=1.0).contains(&c) { c * 255.0 } else { c };
    let c = c.round().clamp(0.0, 255.0);
    u8::try_from(c as u16).unwrap_or(0)
}

fn rgb_hex(desc: &Descriptor) -> Option<String> {
    let color = find(desc, &["Clr ", "color"]).and_then(object)?;
    let channel = |keys: &[&str]| find(color, keys).and_then(number);
    let (r, g, b) = (channel(&["Rd  ", "red"])?, channel(&["Grn ", "green"])?, channel(&["Bl  ", "blue"])?);
    Some(format!("#{:02x}{:02x}{:02x}", channel_byte(r), channel_byte(g), channel_byte(b)))
}

fn dialog_on(command: &ActionCommand) -> bool {
    match command.dialog_options {
        1 => true,
        2 => false,
        _ => command.with_dialog,
    }
}

/// A step PhotoCraft can play, or the event name that was left out.
fn map_command(command: &ActionCommand) -> std::result::Result<(String, Value, bool), String> {
    let desc = command.descriptor.as_ref();
    let event = command.event.as_str();
    let mapped = if event_is(event, &["setd", "set"]) {
        desc.is_some_and(|d| mentions(d, &["Trsp", "transparencyEnum", "transparency"]))
            .then(|| ("select.loadSelection".to_string(), json!({"channel": "transparency"})))
    } else if event_is(event, &["Cntc", "contract"]) {
        Some(modify(desc, "select.modify.contract"))
    } else if event_is(event, &["Expn", "expand"]) {
        Some(modify(desc, "select.modify.expand"))
    } else if event_is(event, &["Fthr", "feather"]) {
        Some(modify(desc, "select.modify.feather"))
    } else if event_is(event, &["Invs", "inverse"]) {
        Some(("select.inverse".to_string(), json!({})))
    } else if event_is(event, &["Mk  ", "Mk", "make"]) {
        desc.and_then(map_make)
    } else {
        None
    };
    match mapped {
        Some((id, params)) => Ok((id, params, dialog_on(command))),
        None => {
            let name = if command.dictionary_name.is_empty() { event.trim().to_string() } else { command.dictionary_name.clone() };
            Err(name)
        }
    }
}

fn modify(desc: Option<&Descriptor>, id: &str) -> (String, Value) {
    let radius = desc.and_then(|d| find(d, &["By  ", "by", "Rds "]).and_then(number)).unwrap_or(1.0);
    let bounds = desc.and_then(|d| find(d, &["selectionModifyEffectAtCanvasBounds", "AtCb"]).and_then(flag)).unwrap_or(false);
    (id.to_string(), json!({"radius": radius, "applyAtCanvasBounds": bounds}))
}

fn map_make(desc: &Descriptor) -> Option<(String, Value)> {
    let new = find(desc, &["Nw  ", "new", "using"]).and_then(object)?;
    if class_is(new, &["SCch", "spotColorChannel", "SptC"]) {
        let mut params = json!({"fromSelection": true});
        if let Some(name) = find(new, &["Nm  ", "name"]).and_then(text) {
            params["name"] = json!(name);
        }
        if let Some(color) = rgb_hex(new) {
            params["color"] = json!(color);
        }
        if let Some(solidity) = find(new, &["Opct", "solidity", "Scld"]).and_then(number) {
            params["solidity"] = json!(solidity);
        }
        return Some(("channel.newSpot".to_string(), params));
    }
    if class_is(new, &["Lyr ", "layer"]) {
        let mut params = json!({});
        if let Some(name) = find(new, &["Nm  ", "name"]).and_then(text) {
            params["name"] = json!(name);
        }
        return Some(("layer.new.layer".to_string(), params));
    }
    None
}

fn unique_name(names: &[String], name: &str) -> Result<String> {
    let base = if name.is_empty() { "Action".to_string() } else { name.to_string() };
    if !names.iter().any(|have| have == &base) {
        return Ok(base);
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base} {n}");
        if !names.iter().any(|have| have == &candidate) {
            return Ok(candidate);
        }
        n = n.checked_add(1).ok_or_else(|| bad("could not allocate a unique action name"))?;
        if n > 100_000 {
            return Err(bad("could not allocate a unique action name"));
        }
    }
}

fn shortcut_of(key: u16, shift: bool, command: bool) -> String {
    if !(1..=12).contains(&key) {
        return String::new();
    }
    let key = format!("F{key}");
    match (command, shift) {
        (true, true) => format!("Ctrl+Shift+{key}"),
        (true, false) => format!("Ctrl+{key}"),
        (false, true) => format!("Shift+{key}"),
        (false, false) => key,
    }
}

struct Built {
    action: Action,
    shortcut: String,
}

fn build(file: &ActionFile, names: &[String]) -> Result<(Vec<Built>, Vec<String>)> {
    let mut built = Vec::new();
    let mut skipped = Vec::new();
    let mut names = names.to_vec();
    for item in &file.actions {
        let mut steps = Vec::new();
        let mut disabled = BTreeSet::new();
        let mut dialogs = Vec::new();
        for (index, command) in item.commands.iter().enumerate() {
            match map_command(command) {
                Ok((id, params, dialog)) => {
                    if !command.enabled {
                        disabled.insert(steps.len());
                    }
                    dialogs.push(dialog);
                    steps.push((id, params));
                }
                Err(name) => skipped.push(format!("{} / {} — {name}", item.name, index.saturating_add(1))),
            }
        }
        let name = unique_name(&names, &item.name)?;
        let action = Action { name: name.clone(), steps, disabled, enabled: true, step_dialog: dialogs, color: color_from_index(item.color_index).to_string() };
        names.push(name);
        built.push(Built { shortcut: shortcut_of(item.function_key, item.shift, item.command), action });
    }
    Ok((built, skipped))
}

fn decoded_bytes(p: &Value) -> std::result::Result<Vec<u8>, EngineError> {
    if let Some(values) = p.get("bytes").and_then(Value::as_array) {
        if values.len() > MAX_BYTES {
            return Err(bad("action file is too large"));
        }
        let mut out = Vec::with_capacity(values.len());
        for value in values {
            let Some(n) = value.as_u64() else { return Err(bad("bytes must be integers 0..255")) };
            if n > 255 {
                return Err(bad("bytes must be integers 0..255"));
            }
            out.push(u8::try_from(n).map_err(|_| bad("bytes must be integers 0..255"))?);
        }
        return Ok(out);
    }
    let Some(text) = p.get("base64").and_then(Value::as_str) else {
        return Err(bad("needs \"base64\" or \"bytes\""));
    };
    if text.len() > MAX_BASE64 {
        return Err(bad("action file is too large"));
    }
    decode_base64(text).map_err(bad)
}

fn decode_base64(text: &str) -> std::result::Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !bytes.len().is_multiple_of(4) {
        return Err("base64 length is not a multiple of 4".into());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let &[a, b, c, d] = chunk else {
            return Err("base64 length is not a multiple of 4".into());
        };
        let pad = [c, d].iter().rev().take_while(|byte| **byte == b'=').count();
        if pad > 2 || (a == b'=' || b == b'=') {
            return Err("base64 padding is invalid".into());
        }
        let digit = |byte: u8| {
            if byte == b'=' { Ok(0) } else { val(byte).ok_or_else(|| "base64 contains a character that is not base64".to_string()) }
        };
        let acc = (u32::from(digit(a)?) << 18) | (u32::from(digit(b)?) << 12) | (u32::from(digit(c)?) << 6) | u32::from(digit(d)?);
        out.push((acc >> 16) as u8);
        if pad < 2 {
            out.push((acc >> 8) as u8);
        }
        if pad < 1 {
            out.push(acc as u8);
        }
    }
    Ok(out)
}

fn set_ok(s: &Session, name: &str, replace: bool) -> Result<()> {
    if name.is_empty() {
        return Ok(());
    }
    if name.chars().count() > MAX_SET_CHARS {
        return Err(bad(format!("folder name must be at most {MAX_SET_CHARS} characters")));
    }
    let exists = !replace && s.actions.sets.iter().any(|n| n == name);
    let sets_after = if replace { 0 } else { s.actions.sets.len() };
    if !exists && sets_after >= MAX_SETS {
        return Err(bad(format!("at most {MAX_SETS} folders")));
    }
    Ok(())
}

/// Replace or append the actions in a `.atn` payload.
pub(crate) fn import_atn(s: &mut Session, p: &Value) -> Result<Value> {
    if s.actions.playing > 0 || s.actions.recording.is_some() {
        return Err(bad("stop playback and recording before loading actions"));
    }
    let bytes = decoded_bytes(p)?;
    let file = photocraft_psd::atn::parse(&bytes).map_err(|e| bad(e.to_string()))?;
    let replace = p.get("replace").and_then(Value::as_bool).unwrap_or(false);
    set_ok(s, &file.name, replace)?;
    let existing: Vec<String> = if replace { Vec::new() } else { s.actions.list.iter().map(|a| a.name.clone()).collect() };
    let (built, skipped) = build(&file, &existing)?;
    if replace {
        s.actions.list.clear();
        s.actions.sets.clear();
        s.actions.folders.clear();
    }
    let set = file.name.clone();
    s.actions.align_folders();
    let mut shortcuts = Vec::new();
    for item in built {
        if !item.shortcut.is_empty() {
            let id = format!("{PLAY_SHORTCUT_PREFIX}{}", item.action.name);
            let shortcut = item.shortcut.clone();
            s.edit_prefs(|prefs| {
                prefs.shortcuts.insert(id, shortcut);
            });
            shortcuts.push(json!({"action": item.action.name, "shortcut": item.shortcut}));
        }
        s.actions.list.push(item.action);
        s.actions.folders.push(set.clone());
    }
    s.actions.align_folders();
    actions_cmds::ensure_set(&mut s.actions, &set)?;
    s.actions.touch();
    Ok(json!({"set": set, "skipped": skipped, "shortcuts": shortcuts, "actions": s.actions.list.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_psd::descriptor::{Descriptor, Value as DescValue};

    fn put_u16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    fn put_u32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    fn put_i32(out: &mut Vec<u8>, v: i32) {
        out.extend_from_slice(&v.to_be_bytes());
    }

    fn put_unicode(out: &mut Vec<u8>, text: &str) {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        put_u32(out, units.len() as u32);
        for unit in units {
            put_u16(out, unit);
        }
    }

    fn put_bytes(out: &mut Vec<u8>, text: &str) {
        put_u32(out, text.len() as u32);
        out.extend_from_slice(text.as_bytes());
    }

    fn command(out: &mut Vec<u8>, event: &[u8; 4], enabled: bool, dialog: bool, desc: Option<&Descriptor>) {
        out.push(1);
        out.push(u8::from(enabled));
        out.push(u8::from(dialog));
        out.push(0);
        out.extend_from_slice(b"long");
        out.extend_from_slice(event);
        put_bytes(out, "");
        match desc {
            Some(desc) => {
                put_i32(out, -1);
                out.extend_from_slice(&desc.to_bytes());
            }
            None => put_i32(out, 0),
        }
    }

    /// Transparency selection, a 1 px contract, a red spot at solidity 60, and a Gaussian Blur
    /// event that is reported and left out.
    fn spot_atn() -> Vec<u8> {
        let set = Descriptor::new("null")
            .with(
                "null",
                DescValue::Reference(vec![photocraft_psd::descriptor::ReferenceItem::Property {
                    class: photocraft_psd::descriptor::Class { name: Default::default(), class_id: photocraft_psd::descriptor::Id::new("Chnl") },
                    key: photocraft_psd::descriptor::Id::new("fsel"),
                }]),
            )
            .with(
                "T   ",
                DescValue::Reference(vec![photocraft_psd::descriptor::ReferenceItem::Enumerated {
                    class: photocraft_psd::descriptor::Class { name: Default::default(), class_id: photocraft_psd::descriptor::Id::new("Chnl") },
                    type_id: photocraft_psd::descriptor::Id::new("Chnl"),
                    value: photocraft_psd::descriptor::Id::new("Trsp"),
                }]),
            );
        let contract = Descriptor::new("null")
            .with("By  ", DescValue::UnitFloat { unit: *b"#Pxl", value: 1.0 })
            .with("selectionModifyEffectAtCanvasBounds", DescValue::Boolean(false));
        let spot = Descriptor::new("null").with(
            "Nw  ",
            DescValue::Descriptor(
                Descriptor::new("SCch")
                    .with("Nm  ", DescValue::Text(photocraft_psd::descriptor::UnicodeString::new_nul("Spot Color 1")))
                    .with(
                        "Clr ",
                        DescValue::Descriptor(
                            Descriptor::new("RGBC")
                                .with("Rd  ", DescValue::Double(255.0))
                                .with("Grn ", DescValue::Double(0.0))
                                .with("Bl  ", DescValue::Double(0.0)),
                        ),
                    )
                    .with("Opct", DescValue::Integer(60)),
            ),
        );
        let mut out = Vec::new();
        put_u32(&mut out, 16);
        put_unicode(&mut out, "Shop");
        out.push(1);
        put_u32(&mut out, 1);
        put_u16(&mut out, 3);
        out.push(0);
        out.push(1);
        put_u16(&mut out, 5);
        put_unicode(&mut out, "Spot");
        out.push(1);
        put_u32(&mut out, 4);
        command(&mut out, b"setd", true, false, Some(&set));
        command(&mut out, b"Cntc", true, false, Some(&contract));
        command(&mut out, b"Mk  ", false, true, Some(&spot));
        command(&mut out, b"GsnB", true, false, None);
        out
    }

    fn layer_atn() -> Vec<u8> {
        let layer = Descriptor::new("null").with(
            "Nw  ",
            DescValue::Descriptor(Descriptor::new("Lyr ").with("Nm  ", DescValue::Text(photocraft_psd::descriptor::UnicodeString::new_nul("Ink")))),
        );
        let mut out = Vec::new();
        put_u32(&mut out, 16);
        put_unicode(&mut out, "Shop");
        out.push(1);
        put_u32(&mut out, 1);
        put_u16(&mut out, 0);
        out.push(0);
        out.push(0);
        put_u16(&mut out, 0);
        put_unicode(&mut out, "Ink");
        out.push(0);
        put_u32(&mut out, 1);
        command(&mut out, b"Mk  ", true, false, Some(&layer));
        out
    }

    fn as_bytes(raw: &[u8]) -> Value {
        json!(raw.iter().map(|b| json!(b)).collect::<Vec<_>>())
    }

    #[test]
    fn spot_atn_keeps_supported_steps_and_reports_the_blur() {
        let mut s = Session::new();
        let v = s.execute("actions.importAtn", json!({"bytes": as_bytes(&spot_atn())})).unwrap();
        assert_eq!(v["set"], "Shop");
        assert_eq!(v["skipped"], json!(["Spot / 4 — GsnB"]));
        assert_eq!(v["shortcuts"][0]["shortcut"], "Ctrl+F3");
        assert_eq!(s.prefs().shortcuts.get(&format!("{PLAY_SHORTCUT_PREFIX}Spot")).map(String::as_str), Some("Ctrl+F3"));
        assert_eq!(s.actions.folder(0), "Shop");
        assert!(s.actions.sets.iter().any(|n| n == "Shop"));
        let action = &s.actions.list[0];
        assert_eq!(action.name, "Spot");
        assert_eq!(action.color, "blue");
        assert!(action.enabled);
        assert_eq!(action.steps.iter().map(|step| step.0.as_str()).collect::<Vec<_>>(), ["select.loadSelection", "select.modify.contract", "channel.newSpot"]);
        assert_eq!(action.steps[0].1["channel"], "transparency");
        assert_eq!(action.steps[1].1["radius"].as_f64(), Some(1.0));
        assert_eq!(action.steps[1].1["applyAtCanvasBounds"], false);
        assert_eq!(action.steps[2].1["name"], "Spot Color 1");
        assert_eq!(action.steps[2].1["color"], "#ff0000");
        assert_eq!(action.steps[2].1["solidity"].as_f64(), Some(60.0));
        assert!(!action.step_enabled(2), "the Make step was unchecked in the file");
        assert!(action.step_dialog_on(2));
        assert!(s.execute("actions.importAtn", json!({"base64": "@@@@"})).is_err());
        assert!(s.execute("actions.importAtn", json!({})).is_err());
        assert!(s.execute("actions.importAtn", json!({"bytes": [0, 0, 0, 16]})).is_err());
        assert!(s.execute("actions.importAtn", json!({"bytes": [256]})).is_err());
        assert!(s.execute("actions.importAtn", json!({"bytes": "nope"})).is_err());
    }

    #[test]
    fn imported_layer_step_plays_and_replace_drops_the_previous_list() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
        s.execute("actions.importAtn", json!({"bytes": as_bytes(&layer_atn())})).unwrap();
        let before = s.active().unwrap().doc.layers.len();
        let played = s.execute("actions.play", json!({"action": "Ink"})).unwrap();
        assert_eq!(played["ran"], 1);
        assert_eq!(s.active().unwrap().doc.layers.len(), before + 1);
        assert_eq!(s.active().unwrap().doc.layers.last().unwrap().name, "Ink");

        s.execute("actions.record", json!({"name": "Busy"})).unwrap();
        assert!(s.execute("actions.importAtn", json!({"bytes": as_bytes(&layer_atn())})).is_err());
        s.execute("actions.stop", json!({})).unwrap();

        let again = s.execute("actions.importAtn", json!({"bytes": as_bytes(&layer_atn()), "replace": true})).unwrap();
        assert_eq!(again["actions"], 1);
        assert_eq!(s.actions.list.len(), 1);
        assert_eq!(s.actions.list[0].name, "Ink");
        assert_eq!(s.actions.folder(0), "Shop");
    }
}
