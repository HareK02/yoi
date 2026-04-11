//! Minimal example: Pod running a single prompt with persistence.
//!
//! Demonstrates the core insomnia abstraction — a TOML manifest drives
//! provider selection, model config, and system prompt, while FsStore
//! persists the session to disk automatically.
//!
//! ## Usage
//!
//! ```bash
//! echo "ANTHROPIC_API_KEY=your-key" > .env
//! cargo run -p pod --example pod_cli
//! ```

use pod::{Pod, PodManifest, PodRunResult};
use session_store::FsStore;

const MANIFEST_TOML: &str = r#"
[pod]
name = "hello-pod"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]
system_prompt = "You are a concise assistant. Reply in one or two sentences."
max_tokens = 256
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    // 1. Parse the manifest
    let manifest = PodManifest::from_toml(MANIFEST_TOML)?;
    println!("Pod: {}", manifest.pod.name);

    // 2. Create a persistent store (temp dir for demo)
    let tmp = tempfile::tempdir()?;
    let store = FsStore::new(tmp.path()).await?;

    // 3. Build the Pod from manifest
    let mut pod = Pod::from_manifest(manifest, store, None, None).await?;
    println!("Session: {}", pod.session_id());

    // 4. Run a prompt
    let result = pod.run("What is the capital of France?").await?;
    match result {
        PodRunResult::Finished => println!("(finished)"),
        PodRunResult::Paused => println!("(paused)"),
        PodRunResult::LimitReached => println!("(turn limit reached)"),
    }

    // 5. Extract the assistant's reply from history
    let history = pod.worker().history();
    if let Some(text) = history
        .iter()
        .rev()
        .find(|item| item.is_assistant_message())
        .and_then(|item| item.as_text())
    {
        println!("\nAssistant: {text}");
    }

    // 6. Session ID for potential restore
    println!("\nSession ID: {}", pod.session_id());

    Ok(())
}
