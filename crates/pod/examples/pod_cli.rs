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
use pod_store::{CombinedStore, FsPodStore};
use session_store::FsStore;

fn manifest_toml(pwd: &std::path::Path) -> String {
    let pwd = pwd.display();
    format!(
        r#"
[pod]
name = "hello-pod"
pwd = "{pwd}"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[worker]
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
    //    All paths in a manifest must be absolute — see the pod-factory ticket.
    let pwd = std::env::current_dir()?;
    let toml = manifest_toml(&pwd);

    // 2. Create a persistent store (temp dir for demo)
    let tmp = tempfile::tempdir()?;
    let store = CombinedStore::new(
        FsStore::new(tmp.path().join("sessions"))?,
        FsPodStore::new(tmp.path().join("pods"))?,
    );

    // 3. Build the Pod from the single-layer manifest TOML
    let mut pod = Pod::from_manifest_toml(&toml, store).await?;
    let manifest: &PodManifest = pod.manifest();
    println!("Pod: {}", manifest.pod.name);
    println!("Segment: {}", pod.segment_id());

    // 4. Run a prompt
    let result = pod.run_text("What is the capital of France?").await?;
    match result {
        PodRunResult::Finished => println!("(finished)"),
        PodRunResult::Paused => println!("(paused)"),
        PodRunResult::LimitReached => println!("(turn limit reached)"),
        PodRunResult::RolledBack => println!("(empty turn rolled back)"),
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
    println!("\nSegment ID: {}", pod.segment_id());

    Ok(())
}
