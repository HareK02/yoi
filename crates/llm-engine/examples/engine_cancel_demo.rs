//! Engine cancellation demo
//!
//! Example of cancelling from another thread during streaming

use llm_engine::llm_client::scheme::{Scheme, anthropic::AnthropicScheme};
use llm_engine::llm_client::transport::{HttpTransport, ResolvedAuth};
use llm_engine::{Engine, EngineResult};
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
    let cap = scheme.default_capability();
    let base_url = scheme.default_base_url().to_string();
    let client = HttpTransport::new(scheme, model, base_url, ResolvedAuth::ApiKey(api_key), cap);
    let engine = Engine::new(client);

    println!("🚀 Starting Engine...");
    println!("💡 Will cancel after 2 seconds\n");

    // Get cancel sender before run (Mutable state)
    let cancel_tx = engine.cancel_sender();

    // Task: Cancel after 2 seconds
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        println!("\n🛑 Cancelling engine...");
        let _ = cancel_tx.send(()).await;
    });

    println!("📡 Sending request to LLM...");

    match engine.run("Tell me a very long story about a brave knight. Make it as detailed as possible with many paragraphs.").await {
        Ok(out) => match out.result {
            EngineResult::Finished => println!("✅ Task completed normally"),
            EngineResult::Paused => println!("⏸️  Task paused"),
            EngineResult::LimitReached => println!("🔒 Turn limit reached"),
            EngineResult::Yielded => println!("↩️  Task yielded"),
        },
        Err(e) => {
            println!("❌ Task error: {}", e);
        }
    }

    println!("\n✨ Demo complete!");

    Ok(())
}
