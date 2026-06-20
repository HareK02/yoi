use yoi_plugin_pdk::wit_bindgen;

wit_bindgen::generate!({
    world: "tool",
    path: "../../resources/plugin/wit",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});

struct Probe;

impl Guest for Probe {
    fn call(tool_name: String, input_json: String) -> String {
        yoi_plugin_pdk::run_json_tool(&tool_name, &input_json, |_ctx, input: serde_json::Value| {
            yoi_plugin_pdk::ToolOutput::json("probe ok", input)
        })
    }
}

#[test]
fn wit_bindgen_generates_current_tool_world() {
    let output = <Probe as Guest>::call("probe".to_string(), r#"{"ok":true}"#.to_string());
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["summary"], "probe ok");
}
