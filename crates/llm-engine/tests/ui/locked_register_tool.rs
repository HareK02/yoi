use llm_engine::Engine;
use llm_engine::llm_client::capability::{
    CacheStrategy, ModelCapability, StructuredOutput, ToolCallingSupport,
};
use llm_engine::llm_client::scheme::anthropic::AnthropicScheme;
use llm_engine::llm_client::transport::{HttpTransport, ResolvedAuth};
use std::sync::Arc;

fn main() {
    let cap = ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: false,
        prompt_caching: CacheStrategy::Auto,
    };
    let client = HttpTransport::new(
        AnthropicScheme::new(),
        "dummy-model".to_string(),
        "http://localhost:11434".to_string(),
        ResolvedAuth::None,
        cap,
    );
    let engine = Engine::new(client);
    let mut locked = engine.lock();
    let def: llm_engine::tool::ToolDefinition = Arc::new(|| panic!("unused"));
    let _ = locked.register_tool(def);
}
