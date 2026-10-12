use super::*;
use crate::craft_codec as codec;
use photocraft_ops::{HistoryState, LayerTarget};
use serde_json::json;
impl Session {
    pub fn craft_signature(&self, ui: &Value) -> String {
        json!([
            ui,
            self.active,
            self.docs
                .iter()
                .map(|d| json!([
                    d.doc.id,
                    d.revision,
                    d.saved_revision,
                    d.active_layer,
                    d.selected_layers,
                    d.layer_anchor,
                    d.channel_view,
                    d.isolated_layers,
                    d.fx_collapsed,
                    d.show_only,
                    d.path,
                    d.source_read_only
                ]))
                .collect::<Vec<_>>(),
            self.tools.foreground,
            self.tools.background,
            &self.tools.brush,
            self.prefs.rev()
        ])
        .to_string()
    }
    pub fn craft_capture(&self, ui: Value) -> std::result::Result<Vec<u8>, String> {
        let mut blobs = Vec::new();
        let mut ids = std::collections::HashMap::new();
        let mut put = |doc: &Arc<Document>| -> std::result::Result<usize, String> {
            let key = Arc::as_ptr(doc) as usize;
            if let Some(id) = ids.get(&key) {
                return Ok(*id);
            }
            let bytes = photocraft_format::save_to_bytes(doc, &Default::default()).map_err(|e| e.to_string())?;
            let id = codec::push(&mut blobs, bytes)?;
            ids.insert(key, id);
            Ok(id)
        };
        let mut docs = Vec::new();
        for d in &self.docs {
            let current = put(&d.doc)?;
            let (undo, redo, label, layers) = d.history.craft_parts();
            let mut stack = |states: Vec<HistoryState>| -> std::result::Result<Vec<Value>, String> {
                states.iter().map(|h| Ok(json!([h.label, put(&h.doc)?, h.layers.active, h.layers.selected]))).collect()
            };
            docs.push(json!({"blob":current,"path":d.path,"revision":d.revision,"saved":d.saved_revision,"active":d.active_layer,"selected":d.selected_layers,"anchor":d.layer_anchor,"readonly":d.source_read_only,"undo":stack(undo)?,"redo":stack(redo)?,"label":label,"layers":[layers.active,layers.selected],"maxStates":d.history.max_states,"maxBytes":d.history.max_bytes,"channel":d.channel_view,"isolated":d.isolated_layers,"fx":d.fx_collapsed,"showOnly":d.show_only}));
        }
        codec::pack(
            &json!({"app":"photocraft","schema":1,"ui":ui,"docs":docs,"active":self.active,"foreground":self.tools.foreground,"background":self.tools.background,"brush":self.tools.brush,"prefs":self.prefs()}),
            &blobs,
        )
    }
    pub fn craft_restore(&mut self, bytes: &[u8]) -> std::result::Result<Value, String> {
        let (v, blobs) = codec::unpack(bytes)?;
        if v["app"] != "photocraft" || v["schema"] != 1 {
            return Err("Incompatible PhotoCraft session".into());
        }
        let mut decoded = std::collections::HashMap::<usize, Arc<Document>>::new();
        let mut get = |id: usize| -> std::result::Result<Arc<Document>, String> {
            if let Some(d) = decoded.get(&id) {
                return Ok(d.clone());
            }
            let d = Arc::new(photocraft_format::load_from_bytes(codec::blob(&blobs, id)?).map_err(|e| e.to_string())?);
            decoded.insert(id, d.clone());
            Ok(d)
        };
        let mut docs = Vec::new();
        for m in v["docs"].as_array().ok_or("Missing documents")? {
            let doc = get(codec::decode(&m["blob"])?)?;
            let mut d = DocState::new((*doc).clone(), codec::decode(&m["path"])?);
            d.doc = doc;
            d.revision = codec::decode(&m["revision"])?;
            d.saved_revision = codec::decode(&m["saved"])?;
            d.active_layer = codec::decode(&m["active"])?;
            d.selected_layers = codec::decode(&m["selected"])?;
            d.layer_anchor = codec::decode(&m["anchor"])?;
            d.source_read_only = codec::decode(&m["readonly"])?;
            let mut stack = |value: &Value| -> std::result::Result<Vec<HistoryState>, String> {
                value
                    .as_array()
                    .ok_or("Missing history")?
                    .iter()
                    .map(|h| {
                        Ok(HistoryState {
                            label: codec::decode(&h[0])?,
                            doc: get(codec::decode(&h[1])?)?,
                            layers: LayerTarget { active: codec::decode(&h[2])?, selected: codec::decode(&h[3])? },
                        })
                    })
                    .collect()
            };
            d.history.craft_restore(
                stack(&m["undo"])?,
                stack(&m["redo"])?,
                codec::decode(&m["label"])?,
                LayerTarget { active: codec::decode(&m["layers"][0])?, selected: codec::decode(&m["layers"][1])? },
            );
            d.history.max_states = codec::decode(&m["maxStates"])?;
            d.history.max_bytes = codec::decode(&m["maxBytes"])?;
            d.channel_view = codec::decode(&m["channel"])?;
            d.isolated_layers = codec::decode(&m["isolated"])?;
            d.fx_collapsed = codec::decode(&m["fx"])?;
            d.show_only = codec::decode(&m["showOnly"])?;
            docs.push(d);
        }
        let active: Option<usize> = codec::decode(&v["active"])?;
        if active.is_some_and(|i| i >= docs.len()) {
            return Err("Invalid active tab".into());
        }
        let foreground = codec::decode(&v["foreground"])?;
        let background = codec::decode(&v["background"])?;
        let brush = codec::decode(&v["brush"])?;
        let prefs = codec::decode(&v["prefs"])?;
        self.prefs.edit(|p| *p = prefs);
        self.docs = docs;
        self.active = active;
        self.tools.foreground = foreground;
        self.tools.background = background;
        self.tools.brush = brush;
        if v["app"] != "photocraft" || v["schema"] != 1 {
            return Err("Incompatible PhotoCraft session".into());
        }
        Ok(v["ui"].clone())
    }
}

impl Session {
    pub fn craft_ui(bytes: &[u8]) -> std::result::Result<Value, String> {
        let (v, _) = crate::craft_codec::unpack(bytes)?;
        if v["app"] != "photocraft" || v["schema"] != 1 {
            return Err("Incompatible PhotoCraft session".into());
        }
        Ok(v["ui"].clone())
    }
}
