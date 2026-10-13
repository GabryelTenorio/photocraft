//! Advertised tool schemas type scalar arguments as a single JSON type: picky clients
//! coerce scalars on plain-typed properties but leave them as strings on union types.

mod common;

use common::{call, connect, json_of, text};
use photocraft_automation::PhotocraftMcp;
use serde_json::{Value, json};

fn assert_single_type(tool: &str, prop: &str, schema: &Value, ty: &str) {
    assert_eq!(schema["type"], json!(ty), "{tool}.{prop} must be a plain {ty}: {schema}");
    assert!(schema.get("anyOf").is_none(), "{tool}.{prop} must not be a union: {schema}");
}

#[tokio::test(flavor = "multi_thread")]
async fn scalar_arguments_advertise_a_single_json_type() {
    let client = connect(PhotocraftMcp::headless()).await;
    let tools = client.list_all_tools().await.unwrap();
    let schema = |name: &str| {
        let tool = tools.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("{name} is listed"));
        Value::Object((*tool.input_schema).clone())
    };
    for (tool, prop, ty) in [
        ("doc_new", "width", "integer"),
        ("doc_new", "height", "integer"),
        ("doc_new", "depth", "integer"),
        ("doc_place", "index", "integer"),
        ("doc_place", "scale", "number"),
        ("doc_place", "fit", "boolean"),
        ("doc_export_layers", "index", "integer"),
        ("doc_export_layers", "visible_only", "boolean"),
        ("doc_export_layers", "quality", "number"),
        ("doc_export_data_sets", "index", "integer"),
        ("doc_import_data_sets", "index", "integer"),
        ("doc_save", "quality", "integer"),
        ("doc_save", "tgaBits", "integer"),
        ("doc_save", "dpxBits", "integer"),
        ("doc_save", "index", "integer"),
        ("doc_inspect", "index", "integer"),
        ("doc_render_preview", "index", "integer"),
        ("doc_render_preview", "max_side", "integer"),
        ("ui_screenshot", "max_side", "integer"),
        ("command_list", "enabled_only", "boolean"),
        ("command_run", "wait", "boolean"),
        ("jobs_cancel", "job", "integer"),
        ("command_batch", "stop_on_error", "boolean"),
    ] {
        let prop_schema = &schema(tool)["properties"][prop];
        assert_single_type(tool, prop, prop_schema, ty);
    }
    // The nested step and event objects carry the same scalar arguments.
    let step = &schema("command_batch")["properties"]["steps"]["items"];
    assert_single_type("command_batch.steps", "wait", &step["properties"]["wait"], "boolean");
    let event = &schema("ui_pointer")["properties"]["events"]["items"];
    for (prop, ty) in [("pressure", "number"), ("tiltX", "number"), ("tiltY", "number"), ("rotation", "number")] {
        assert_single_type("ui_pointer.events", prop, &event["properties"][prop], ty);
    }
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn explicit_null_stays_accepted_and_string_scalars_stay_rejected() {
    let client = connect(PhotocraftMcp::headless()).await;
    // An explicit null selects the default even though the schema types the field as non-nullable.
    json_of(&call(&client, "doc_new", json!({"width": null, "height": null})).await);
    let doc = json_of(&call(&client, "doc_inspect", json!({})).await);
    assert_eq!((doc["width"].as_u64(), doc["height"].as_u64()), (Some(1920), Some(1080)), "{doc}");
    let batch = json_of(&call(&client, "command_batch", json!({"steps": [{"id": "command.list"}], "stop_on_error": null})).await);
    assert_eq!(batch["completed"], 1, "{batch}");
    // A string scalar never reaches the engine: the server's own rejection names the type
    // mismatch, unlike a client-side "must be integer".
    let r = call(&client, "doc_new", json!({"width": "64", "height": 48})).await;
    assert_eq!(r.is_error, Some(true), "{}", text(&r));
    assert!(text(&r).contains("invalid type: string"), "{}", text(&r));
    client.cancel().await.unwrap();
}
