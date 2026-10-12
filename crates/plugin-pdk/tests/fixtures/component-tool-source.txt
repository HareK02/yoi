//! Minimal Component Model Tool plugin authoring sketch using `yoi-plugin-pdk`.
//!
//! Build this as a `wasm32-unknown-unknown` cdylib with Component Model tooling
//! and package the adapted component as `plugin.component.wasm`.

use serde::{Deserialize, Serialize};
use yoi_plugin_pdk::wit_bindgen;
use yoi_plugin_pdk::{ToolContext, ToolError, ToolOutput};

wit_bindgen::generate!({
    world: "tool",
    path: "../../../resources/plugin/wit",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});

#[derive(Debug, Deserialize)]
struct EchoInput {
    text: String,
}

#[derive(Debug, Serialize)]
struct EchoOutput<'a> {
    tool: &'a str,
    text: String,
}

fn handle_echo(ctx: ToolContext, input: EchoInput) -> Result<ToolOutput, ToolError> {
    ToolOutput::json(
        format!("{} ok", ctx.tool_name()),
        EchoOutput {
            tool: ctx.tool_name(),
            text: input.text,
        },
    )
}

yoi_plugin_pdk::export_component_tool!(Plugin, handle_echo);
