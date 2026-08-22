//! Test fixture recording mechanism
//!
//! Saves events to files in JSONL format

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use agen::llm_client::{LlmClient, Request};
use futures::StreamExt;

/// Recorded event
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct RecordedEvent {
    pub elapsed_ms: u64,
    pub event_type: String,
    pub data: String,
}

/// Session metadata
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SessionMetadata {
    pub timestamp: u64,
    pub model: String,
    pub description: String,
}

/// Save event sequence to file
pub fn save_fixture(
    path: impl AsRef<Path>,
    metadata: &SessionMetadata,
    events: &[RecordedEvent],
) -> std::io::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    writeln!(writer, "{}", serde_json::to_string(metadata)?)?;
    for event in events {
        writeln!(writer, "{}", serde_json::to_string(event)?)?;
    }
    writer.flush()?;
    Ok(())
}

/// Send request and record events
pub async fn record_request<C: LlmClient>(
    client: &C,
    request: Request,
    description: &str,
    output_name: &str,
    subdir: &str, // e.g. "anthropic", "openai"
    model: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    println!("\n📝 Recording: {}", description);

    let start_time = Instant::now();
    let mut events: Vec<RecordedEvent> = Vec::new();

    let mut stream = client.stream(request).await?;

    while let Some(result) = stream.next().await {
        let elapsed = start_time.elapsed().as_millis() as u64;
        match result {
            Ok(event) => {
                let event_json = serde_json::to_string(&event)?;
                println!("  [{:>6}ms] {:?}", elapsed, event);
                events.push(RecordedEvent {
                    elapsed_ms: elapsed,
                    event_type: format!("{:?}", std::mem::discriminant(&event)),
                    data: event_json,
                });
            }
            Err(e) => {
                eprintln!("  Error: {}", e);
                break;
            }
        }
    }

    // Save
    let fixtures_dir = Path::new("engine/tests/fixtures").join(subdir);
    fs::create_dir_all(&fixtures_dir)?;

    let filepath = fixtures_dir.join(format!("{}.jsonl", output_name));

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let metadata = SessionMetadata {
        timestamp,
        model: model.to_string(),
        description: description.to_string(),
    };

    save_fixture(&filepath, &metadata, &events)?;

    let event_count = events.len();
    println!("  💾 Saved: {}", filepath.display());
    println!("  📊 {} events recorded", event_count);

    Ok(event_count)
}
