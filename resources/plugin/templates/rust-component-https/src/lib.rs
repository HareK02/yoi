use serde::{Deserialize, Serialize};
use yoi_plugin_pdk::wit_bindgen;
use yoi_plugin_pdk::{ToolContext, ToolError, ToolOutput};

wit_bindgen::generate!({
    world: "https-tool-plugin",
    path: "wit",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {}

#[derive(Serialize)]
struct Output {
    status: u16,
    body_bytes: usize,
}

fn handle_status(ctx: ToolContext, _input: Input) -> Result<ToolOutput, ToolError> {
    use yoi::host::{connections, http};
    if ctx.tool_name() != "example_status" {
        return Err(ToolError::unsupported_tool(ctx.tool_name()));
    }
    // This resource is call-local; the slot has no URL or credential value.
    let target = connections::open("api").map_err(host_failure)?;
    let response = http::send(
        &target,
        &http::Request {
            method: http::Method::Get,
            path: "/status".into(),
            query: Vec::new(),
            headers: Vec::new(),
            body: Vec::new(),
            timeout_ms: ctx.remaining_ms().min(15_000),
            // Stable within this logical invocation. Never automatically retry a failure.
            effect_key: "status-v1".into(),
        },
    )
    .map_err(host_failure)?;
    // HTTP 4xx/5xx remain responses, not transport failures. Do not log raw bodies.
    ToolOutput::json(
        "HTTP response observed",
        Output {
            status: response.status,
            body_bytes: response.body.len(),
        },
    )
}

fn host_failure(error: yoi::host::errors::HostError) -> ToolError {
    // Preserve the effect observation for the caller without claiming rollback or retry safety.
    ToolError::failed(format!(
        "Host {:?}; effect {:?}; diagnostic {}",
        error.code, error.effect, error.diagnostic_id
    ))
}

yoi_plugin_pdk::export_component_tool!(Plugin, handle_status);
