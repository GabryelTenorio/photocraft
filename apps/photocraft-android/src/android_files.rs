//! Android document-provider adapter. No filesystem assumptions or unsafe handle conversion.
use jni::{
    JNIEnv, JavaVM,
    objects::{GlobalRef, JObject, JString, JValue},
};
use photocraft_ui_egui::{FileDialogAnswer, FileDialogReply, FileDialogRequest, Services};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub(crate) struct Bridge {
    pub(crate) vm: Arc<JavaVM>,
    pub(crate) activity: GlobalRef,
}
impl Bridge {
    pub(crate) fn with<T>(&self, f: impl FnOnce(&mut JNIEnv, &JObject) -> Result<T, String>) -> Result<T, String> {
        let mut env = self.vm.attach_current_thread().map_err(|e| e.to_string())?;
        let result = env.with_local_frame::<_, _, jni::errors::Error>(32, |env| Ok(f(env, self.activity.as_obj()))).map_err(err)?;
        // A provider error must not leave a pending Java exception poisoning future JNI calls.
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        result
    }
    fn dialog(&self, request: FileDialogRequest, reply: FileDialogReply, targets: &Mutex<HashMap<String, String>>) -> Result<(), String> {
        if matches!(request, FileDialogRequest::Folder { .. }) {
            reply.send(Some(FileDialogAnswer::Unavailable("Folder workflows are not supported on Android yet. Open individual files instead.".into())));
            return Ok(());
        }
        self.with(|env, act| {
            match &request {
                FileDialogRequest::Open { multiple, .. } => {
                    // PSD, PSB and native projects are frequently application/octet-stream.
                    let mime = env.new_string("*/*").map_err(err)?;
                    env.call_method(act, "openFile", "(Ljava/lang/String;Z)V", &[JValue::Object(&mime), JValue::Bool(u8::from(*multiple))]).map_err(err)?;
                }
                FileDialogRequest::Folder { .. } => return Err("Folder workflows are not supported on Android yet".into()),
                FileDialogRequest::Save { suggested } => {
                    let name = env.new_string(photocraft_ui_egui::file_open::display_name(suggested)).map_err(err)?;
                    env.call_method(act, "saveFile", "(Ljava/lang/String;)V", &[JValue::Object(&name)]).map_err(err)?;
                }
            }
            Ok(())
        })?;
        // No timeout: a user can leave a picker open while using another application.
        let result = loop {
            let result = self.with(|env, act| {
                let obj = env.call_method(act, "pollResult", "()Ljava/lang/String;", &[]).map_err(err)?.l().map_err(err)?;
                if obj.is_null() {
                    return Ok(None);
                }
                let text: String = env.get_string(&JString::from(obj)).map_err(err)?.into();
                Ok(Some(text))
            })?;
            if let Some(result) = result {
                break result;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let result: serde_json::Value = serde_json::from_str(&result).map_err(|e| e.to_string())?;
        if let Some(error) = result.get("error").and_then(|v| v.as_str()) {
            return Err(error.into());
        }
        let files = result.get("files").and_then(|v| v.as_array()).ok_or("Invalid picker result")?;
        if files.is_empty() {
            reply.send(None);
            return Ok(());
        }
        match request {
            FileDialogRequest::Folder { .. } => return Err("Unexpected folder picker result".into()),
            FileDialogRequest::Open { .. } => {
                let mut opened = Vec::new();
                for file in files {
                    let name = file.get("name").and_then(|v| v.as_str()).ok_or("Missing document name")?;
                    let uri = file.get("uri").and_then(|v| v.as_str()).ok_or("Missing document URI")?;
                    opened.push((name.to_owned(), self.read(uri)?));
                }
                reply.send(Some(FileDialogAnswer::ContentsMany(opened)));
            }
            FileDialogRequest::Save { .. } => {
                let file = files.first().ok_or("Missing save destination")?;
                let name = file.get("name").and_then(|v| v.as_str()).ok_or("Missing document name")?;
                let uri = file.get("uri").and_then(|v| v.as_str()).ok_or("Missing document URI")?;
                let mut targets = targets.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                // Unique identity per URI: two providers can return the same display name.
                let key = format!("android-saf/{}/{name}", targets.len());
                let directory = format!("android-saf/{}", targets.len());
                targets.insert(directory, uri.to_owned());
                reply.send(Some(FileDialogAnswer::SaveTo(key)));
            }
        }
        Ok(())
    }
    pub(crate) fn read(&self, uri: &str) -> Result<Vec<u8>, String> {
        self.with(|env, act| {
            let uri = env.new_string(uri).map_err(err)?;
            let stream =
                env.call_method(act, "openRead", "(Ljava/lang/String;)Ljava/io/InputStream;", &[JValue::Object(&uri)]).map_err(err)?.l().map_err(err)?;
            if stream.is_null() {
                return Err("Provider returned no input stream".into());
            }
            let result: Result<Vec<u8>, String> = (|| {
                let buf = env.new_byte_array(32768).map_err(err)?;
                let mut bytes = Vec::new();
                loop {
                    let n = env.call_method(&stream, "read", "([B)I", &[JValue::Object(&buf)]).map_err(err)?.i().map_err(err)?;
                    if n == -1 {
                        break;
                    }
                    if !(1..=32768).contains(&n) {
                        return Err("Invalid provider read length".into());
                    }
                    let count = n as usize;
                    if bytes.len().saturating_add(count) > 512 * 1024 * 1024 {
                        return Err("Document exceeds Android's 512 MiB import limit".into());
                    }
                    bytes.try_reserve(count).map_err(|e| e.to_string())?;
                    let mut chunk = vec![0_i8; count];
                    env.get_byte_array_region(&buf, 0, &mut chunk).map_err(err)?;
                    bytes.extend(chunk.into_iter().map(|b| b as u8));
                }
                Ok(bytes)
            })();
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            let close = env.call_method(&stream, "close", "()V", &[]).map_err(err);
            let bytes = result?;
            close?;
            Ok(bytes)
        })
    }
    fn write(&self, uri: &str, bytes: &[u8]) -> Result<(), String> {
        self.with(|env, act| {
            let uri = env.new_string(uri).map_err(err)?;
            let stream =
                env.call_method(act, "openWrite", "(Ljava/lang/String;)Ljava/io/OutputStream;", &[JValue::Object(&uri)]).map_err(err)?.l().map_err(err)?;
            if stream.is_null() {
                return Err("Provider returned no output stream".into());
            }
            let result: Result<(), String> = (|| {
                for chunk in bytes.chunks(32768) {
                    let array = env.byte_array_from_slice(chunk).map_err(err)?;
                    let array = env.auto_local(array);
                    env.call_method(&stream, "write", "([B)V", &[JValue::Object(array.as_ref())]).map_err(err)?;
                }
                env.call_method(&stream, "flush", "()V", &[]).map_err(err)?;
                Ok(())
            })();
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            let close = env.call_method(&stream, "close", "()V", &[]).map_err(err);
            result?;
            close?;
            Ok(())
        })
    }
}
pub(crate) fn err(e: jni::errors::Error) -> String {
    format!("Android file access: {e}")
}

/// The only editor service seam needed by the Android file provider.
pub(crate) fn install(services: &mut Services, bridge: &Bridge, ctx: &egui::Context, errors: &Arc<Mutex<Vec<String>>>) {
    let targets = Arc::new(Mutex::new(HashMap::new()));
    let dialog_bridge = bridge.clone();
    let dialog_targets = targets.clone();
    let dialog_errors = errors.clone();
    let ctx = ctx.clone();
    services.file_dialog = Some(Box::new(move |request, _, reply| {
        let bridge = dialog_bridge.clone();
        let targets = dialog_targets.clone();
        let errors = dialog_errors.clone();
        let ctx = ctx.clone();
        if let Err(e) = std::thread::Builder::new().name("saf-dialog".into()).spawn(move || {
            if let Err(e) = bridge.dialog(request, reply, &targets) {
                errors.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(e);
                ctx.request_repaint();
            }
        }) {
            dialog_errors.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(e.to_string());
        }
    }));
    let writer = bridge.clone();
    services.write = Some(Box::new(move |name, bytes| {
        let uri = targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name.rsplit_once('/').map_or(name, |(dir, _)| dir))
            .cloned()
            .ok_or("Choose a save destination first")?;
        writer.write(&uri, bytes)
    }));
}
