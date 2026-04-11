//! Pod Protocol example: control a Pod via PodHandle and stream events.
//!
//! ```bash
//! echo "ANTHROPIC_API_KEY=your-key" > .env
//! cargo run -p pod --example pod_protocol
//! ```

use pod::{Event, Method, PodController, PodManifest};
use llm_worker_persistence::FsStore;

const MANIFEST_TOML: &str = r#"
[pod]
name = "protocol-demo"

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

    let manifest = PodManifest::from_toml(MANIFEST_TOML)?;
    let tmp = tempfile::tempdir()?;
    let store = FsStore::new(tmp.path()).await?;
    let pod = pod::Pod::from_manifest(manifest, store, None, None).await?;

    let runtime_tmp = tempfile::tempdir()?;
    let handle = PodController::spawn(pod, runtime_tmp.path()).await?;

    // Check initial status via shared state
    println!("[shared_state] {}", handle.shared_state.status_json());

    // Check runtime directory files
    println!("[runtime_dir] {:?}", handle.runtime_dir.path());

    // Spawn event listener
    let mut rx = handle.subscribe();
    let shared = handle.shared_state.clone();
    let listener = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            match &event {
                Event::TurnStart { turn } => {
                    println!("[turn {turn}] start");
                }
                Event::TextDelta { text } => {
                    print!("{text}");
                }
                Event::TextDone { .. } => {
                    println!();
                }
                Event::TurnEnd { turn, result } => {
                    println!("[turn {turn}] end ({result:?})");
                    println!("[shared_state] {}", shared.status_json());
                }
                Event::ToolCallStart { name, .. } => {
                    println!("[tool] {name}");
                }
                Event::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    println!(
                        "[usage] in={} out={}",
                        input_tokens.unwrap_or(0),
                        output_tokens.unwrap_or(0)
                    );
                }
                Event::Error { code, message } => {
                    println!("[error] {code:?}: {message}");
                }
                _ => {}
            }
        }
    });

    // Send a run method
    handle
        .send(Method::Run {
            input: "What is the capital of France?".into(),
        })
        .await?;

    // Wait for completion
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    println!("\n[shared_state] final: {}", handle.shared_state.status_json());
    println!("[history] {} bytes", handle.shared_state.history_json().len());

    drop(handle);
    let _ = listener.await;

    Ok(())
}
