//! Compile-pass coverage for generated v2 bindings and the public adapter macro.
use yoi_plugin_pdk::wit_bindgen;

wit_bindgen::generate!({
    world: "tool-plugin",
    path: "../../resources/plugin/wit",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});

fn handle(
    ctx: yoi_plugin_pdk::ToolContext,
    input: serde_json::Value,
) -> yoi_plugin_pdk::Result<yoi_plugin_pdk::ToolOutput> {
    if ctx.tool_name() != "probe" {
        return Err(yoi_plugin_pdk::ToolError::unsupported_tool(ctx.tool_name()));
    }
    yoi_plugin_pdk::ToolOutput::json("probe ok", input)
}

yoi_plugin_pdk::export_component_tool!(Probe, handle);

fn invocation(input_json: &str) -> exports::yoi::plugin::tool::Invocation {
    exports::yoi::plugin::tool::Invocation {
        invocation_id: "call-1".into(),
        tool_name: "probe".into(),
        input_json: input_json.into(),
        config_json: "{}".into(),
        remaining_ms: 1000,
    }
}

#[test]
fn generated_adapter_returns_record_and_keeps_business_error_in_error_arm() {
    use exports::yoi::plugin::tool::Guest;
    let output = Probe::invoke(invocation(r#"{"ok":true}"#)).unwrap();
    assert_eq!(output.summary, "probe ok");
    assert_eq!(output.content, None);
    assert_eq!(output.structured_json.as_deref(), Some(r#"{"ok":true}"#));
    assert_eq!(
        Probe::invoke(invocation("broken")).unwrap_err().code,
        "invalid_input"
    );
    let mut input = invocation("{}");
    input.tool_name = "unknown".into();
    assert_eq!(Probe::invoke(input).unwrap_err().code, "unsupported_tool");
}
