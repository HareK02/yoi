//! Minimal example: Worker running a single prompt with persistence.
//!
//! Demonstrates the core yoi abstraction — a TOML manifest drives
//! provider selection, model config, and system prompt, while FsStore
//! persists the session to disk automatically.
//!
//! ## Usage
//!
//! ```bash
//! echo "ANTHROPIC_API_KEY=your-key" > .env
//! cargo run -p worker --example worker_cli
//! ```

use session_store::FsStore;
use session_store::{CombinedStore, FsWorkerStore};
use worker::{Worker, WorkerManifest, WorkerRunResult};

fn manifest_toml(pwd: &std::path::Path) -> String {
    let pwd = pwd.display();
    format!(
        r#"
[worker]
name = "hello-worker"
pwd = "{pwd}"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[engine]
system_prompt = "You are a concise assistant. Reply in one or two sentences."
max_tokens = 256

[[scope.allow]]
target = "{pwd}"
permission = "write"
"#
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    // 1. Build a manifest rooted at the current working directory.
    //    All paths in a manifest must be absolute — see the worker-factory ticket.
    let pwd = std::env::current_dir()?;
    let toml = manifest_toml(&pwd);

    // 2. Create a persistent store (temp dir for demo)
    let tmp = tempfile::tempdir()?;
    let store = CombinedStore::new(
        FsStore::new(tmp.path().join("sessions"))?,
        FsWorkerStore::new(tmp.path().join("pods"))?,
    );

    // 3. Build the Worker from the single-layer manifest TOML
    let mut worker = Worker::from_manifest_toml(&toml, store).await?;
    let manifest: &WorkerManifest = worker.manifest();
    println!("Worker: {}", manifest.worker.name);
    println!("Segment: {}", worker.segment_id());

    // 4. Run a prompt
    let result = worker.run_text("What is the capital of France?").await?;
    match result {
        WorkerRunResult::Finished => println!("(finished)"),
        WorkerRunResult::Paused => println!("(paused)"),
        WorkerRunResult::LimitReached => println!("(turn limit reached)"),
        WorkerRunResult::RolledBack => println!("(empty turn rolled back)"),
    }

    // 5. Extract the assistant's reply from history
    let history = worker.engine().history();
    if let Some(text) = history
        .iter()
        .rev()
        .find(|item| item.is_assistant_message())
        .and_then(|item| item.as_text())
    {
        println!("\nAssistant: {text}");
    }

    // 6. Session ID for potential restore
    println!("\nSegment ID: {}", worker.segment_id());

    Ok(())
}
