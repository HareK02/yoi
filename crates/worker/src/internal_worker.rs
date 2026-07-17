//! Reusable runner for Worker-internal LLM jobs.
//!
//! Internal workers are isolated model/tool runs owned by the foreground
//! [`Worker`](crate::Worker). They do not append their harness prompt/input to
//! the foreground session history. Callers provide a deliberately limited tool
//! surface and decide how to apply the result.

use std::sync::{Arc, Mutex};

use llm_engine::llm_client::client::LlmClient;
use llm_engine::llm_client::event::UsageEvent;
use llm_engine::tool::ToolDefinition;
use llm_engine::{Engine, EngineError};

/// Specification for a single internal worker run.
pub(crate) struct InternalWorkerSpec {
    /// Stable slug for audit/debug/future persistence metadata.
    pub slug: &'static str,
    /// System prompt for the isolated engine.
    pub system_prompt: String,
    /// Initial user input for the isolated engine.
    pub input: String,
    /// Client used by the internal engine.
    pub client: Box<dyn LlmClient>,
    /// Optional prompt-cache key.
    pub cache_key: Option<String>,
    /// Optional turn limit for the internal engine.
    pub max_turns: Option<u32>,
    /// Deliberately limited tool surface for this internal run.
    pub tools: Vec<ToolDefinition>,
}

/// Result metadata for an internal worker run.
#[derive(Debug, Clone, Default)]
pub(crate) struct InternalWorkerRunResult {
    pub slug: &'static str,
    /// Last usage event observed for this internal run.
    pub usage: Option<UsageEvent>,
}

/// Error metadata for an internal worker run.
#[derive(Debug)]
pub(crate) struct InternalWorkerRunError {
    pub slug: &'static str,
    pub source: EngineError,
    /// Last usage event observed before the failure, if any.
    pub usage: Option<UsageEvent>,
}

/// Run an isolated internal worker engine.
pub(crate) async fn run_internal_worker(
    spec: InternalWorkerSpec,
) -> Result<InternalWorkerRunResult, InternalWorkerRunError> {
    let InternalWorkerSpec {
        slug,
        system_prompt,
        input,
        client,
        cache_key,
        max_turns,
        tools,
    } = spec;

    let mut worker = Engine::new(client).system_prompt(system_prompt);
    worker.set_cache_key(cache_key);
    worker.set_max_turns(max_turns);
    worker.register_tools(tools);

    let usage_capture = Arc::new(Mutex::new(None));
    let usage_capture_for_worker = usage_capture.clone();
    worker.on_usage(move |event| {
        *usage_capture_for_worker
            .lock()
            .expect("internal worker usage capture poisoned") = Some(event.clone());
    });

    let run_result = worker.run(input).await;

    let usage = usage_capture
        .lock()
        .expect("internal worker usage capture poisoned")
        .clone();

    match run_result {
        Ok(_output) => Ok(InternalWorkerRunResult { slug, usage }),
        Err(source) => Err(InternalWorkerRunError {
            slug,
            source,
            usage,
        }),
    }
}
