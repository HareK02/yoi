//! Worker Protocol example: control a Worker via WorkerHandle and stream events.
//!
//! ```bash
//! echo "ANTHROPIC_API_KEY=your-key" > .env
//! cargo run -p worker --example worker_protocol
//! ```

use session_store::FsStore;
use session_store::{CombinedStore, FsWorkerStore};
use worker::{Event, Method, WorkerController};

fn manifest_toml(pwd: &std::path::Path) -> String {
    let pwd = pwd.display();
    format!(
        r#"
[worker]
name = "protocol-demo"
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

    // All manifest paths must be absolute — see the worker-factory ticket.
    let pwd = std::env::current_dir()?;
    let toml = manifest_toml(&pwd);
    let tmp = tempfile::tempdir()?;
    let store = CombinedStore::new(
        FsStore::new(tmp.path().join("sessions"))?,
        FsWorkerStore::new(tmp.path().join("pods"))?,
    );
    let worker = worker::Worker::from_manifest_toml(&toml, store).await?;

    let runtime_tmp = tempfile::tempdir()?;
    let bash_output_dir = runtime_tmp.path().join("bash-output");
    let (handle, _shutdown_rx) =
        WorkerController::spawn(worker, runtime_tmp.path(), &bash_output_dir).await?;

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
                    cache_read_input_tokens,
                } => {
                    println!(
                        "[usage] in={} (cache_read={}) out={}",
                        input_tokens.unwrap_or(0),
                        cache_read_input_tokens.unwrap_or(0),
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
        .send(Method::run_text("What is the capital of France?"))
        .await?;

    // Wait for completion
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    println!(
        "\n[shared_state] final: {}",
        handle.shared_state.status_json()
    );
    println!("[session log] {} entries", handle.sink.len());

    drop(handle);
    let _ = listener.await;

    Ok(())
}
