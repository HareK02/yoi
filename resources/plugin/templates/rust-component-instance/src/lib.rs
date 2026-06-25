use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use yoi_plugin_pdk::wit_bindgen;
use yoi_plugin_pdk::{
    export_plugin_instance, Plugin, PluginIngressEvent, PluginStatus, ServiceOutput, ToolError,
    ToolOutput,
};

wit_bindgen::generate!({
    world: "instance",
    path: "../../../../resources/plugin/wit",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});

#[derive(Default)]
struct ExamplePlugin {
    count: u64,
}

#[derive(Deserialize)]
struct EchoInput {
    text: String,
}

#[derive(Serialize)]
struct EchoOutput {
    text: String,
    count: u64,
}

impl Plugin for ExamplePlugin {
    fn start(config: Value) -> Result<Self, ToolError> {
        Ok(Self {
            count: config.get("start_count").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    fn handle_tool(&mut self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        if name != "example_echo" {
            return Err(ToolError::invalid_input(format!("unknown tool: {name}")));
        }
        let input: EchoInput =
            serde_json::from_value(input).map_err(|err| ToolError::invalid_input(err.to_string()))?;
        self.count += 1;
        ToolOutput::json(
            format!("echoed {} bytes", input.text.len()),
            EchoOutput {
                text: input.text,
                count: self.count,
            },
        )
    }

    fn handle_ingress(
        &mut self,
        name: &str,
        event: PluginIngressEvent,
    ) -> Result<ServiceOutput, ToolError> {
        if name != "example_ws" {
            return Ok(ServiceOutput::accepted(json!({ "ignored": name }))?);
        }

        let Some(text) = event.websocket_text() else {
            return Ok(ServiceOutput::accepted(json!({
                "accepted": true,
                "kind": event.kind,
            }))?);
        };

        self.count += 1;
        ServiceOutput::websocket_send(
            &event,
            format!("example-reply-{}", self.count),
            event.source.strip_prefix("websocket:").unwrap_or(&event.source),
            format!("echo({}): {text}", self.count),
        )
    }

    fn status(&self) -> Result<PluginStatus, ToolError> {
        Ok(PluginStatus::ready(json!({ "count": self.count })))
    }
}

export_plugin_instance!(ExamplePluginComponent, ExamplePlugin);
