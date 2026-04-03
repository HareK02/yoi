//! Worker cancellation demo
//!
//! Example of cancelling from another thread during streaming

use llm_worker::llm_client::providers::anthropic::AnthropicClient;
use llm_worker::{Worker, WorkerResult};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

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

    let client = AnthropicClient::new(&api_key, "claude-sonnet-4-20250514");
    let worker = Arc::new(Mutex::new(Worker::new(client)));

    println!("🚀 Starting Worker...");
    println!("💡 Will cancel after 2 seconds\n");

    // Get cancel sender first (without holding lock)
    let cancel_tx = {
        let w = worker.lock().await;
        w.cancel_sender()
    };

    // Task 1: Run Worker
    let worker_clone = worker.clone();
    let task = tokio::spawn(async move {
        let mut w = worker_clone.lock().await;
        println!("📡 Sending request to LLM...");

        match w.run("Tell me a very long story about a brave knight. Make it as detailed as possible with many paragraphs.").await {
            Ok(WorkerResult::Finished) => {
                println!("✅ Task completed normally");
            }
            Ok(WorkerResult::Paused) => {
                println!("⏸️  Task paused");
            }
            Err(e) => {
                println!("❌ Task error: {}", e);
            }
        }
    });

    // Task 2: Cancel after 2 seconds
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        println!("\n🛑 Cancelling worker...");
        let _ = cancel_tx.send(()).await;
    });

    // Wait for task completion
    task.await?;

    println!("\n✨ Demo complete!");

    Ok(())
}
