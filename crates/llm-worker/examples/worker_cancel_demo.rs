//! Worker cancellation demo
//!
//! Example of cancelling from another thread during streaming

use llm_worker::llm_client::capability::{
    CacheStrategy, ModelCapability, StructuredOutput, ToolCallingSupport,
};
use llm_worker::llm_client::scheme::{Scheme, anthropic::AnthropicScheme};
use llm_worker::llm_client::transport::{HttpTransport, ResolvedAuth};
use llm_worker::{Worker, WorkerResult};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env file
    dotenv::dotenv().ok();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let api_key =
        std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY environment variable not set");

    let scheme = AnthropicScheme::new();
    let model = "claude-sonnet-4-20250514".to_string();
    let cap = scheme.capability_for(&model).unwrap_or(ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: false,
        prompt_caching: CacheStrategy::Auto,
    });
    let base_url = scheme.default_base_url().to_string();
    let client = HttpTransport::new(scheme, model, base_url, ResolvedAuth::ApiKey(api_key), cap);
    let worker = Worker::new(client);

    println!("🚀 Starting Worker...");
    println!("💡 Will cancel after 2 seconds\n");

    // Get cancel sender before run (Mutable state)
    let cancel_tx = worker.cancel_sender();

    // Task: Cancel after 2 seconds
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        println!("\n🛑 Cancelling worker...");
        let _ = cancel_tx.send(()).await;
    });

    println!("📡 Sending request to LLM...");

    match worker.run("Tell me a very long story about a brave knight. Make it as detailed as possible with many paragraphs.").await {
        Ok(out) => match out.result {
            WorkerResult::Finished => println!("✅ Task completed normally"),
            WorkerResult::Paused => println!("⏸️  Task paused"),
            WorkerResult::LimitReached => println!("🔒 Turn limit reached"),
            WorkerResult::Yielded => println!("↩️  Task yielded"),
        },
        Err(e) => {
            println!("❌ Task error: {}", e);
        }
    }

    println!("\n✨ Demo complete!");

    Ok(())
}
