use llm_worker::Worker;
use llm_worker::llm_client::providers::ollama::OllamaClient;
use std::sync::Arc;

fn main() {
    let client = OllamaClient::new("dummy-model");
    let worker = Worker::new(client);
    let handle = worker.tool_server_handle();
    let def: llm_worker::tool::ToolDefinition = Arc::new(|| panic!("unused"));
    let _ = handle.register_tool(def);
}
