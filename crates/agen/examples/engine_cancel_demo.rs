//! Engine cancellation demo
//!
//! Example of cancelling from another thread during streaming

use agen::llm_client::scheme::{Scheme, anthropic::AnthropicScheme};
use agen::llm_client::transport::{HttpTransport, ResolvedAuth};
use agen::{Engine, EngineRunExit, RunInterruptionReason};
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
    let mut history = agen::History::new();

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

    let output = engine.run(&mut history, "Tell me a very long story about a brave knight. Make it as detailed as possible with many paragraphs.").await;
    match output.result {
        EngineRunExit::Finished => println!("✅ Task completed normally"),
        EngineRunExit::Paused => println!("⏸️  Task paused"),
        EngineRunExit::Yielded => println!("↩️  Task yielded"),
        EngineRunExit::Interrupted(RunInterruptionReason::LimitReached) => {
            println!("🔒 Turn limit reached")
        }
        EngineRunExit::Interrupted(reason) => println!("❌ Task interrupted: {reason:?}"),
    }

    println!("\n✨ Demo complete!");

    Ok(())
}
