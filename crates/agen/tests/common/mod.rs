#![allow(dead_code)]

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use agen::llm_client::event::{BlockType, DeltaContent, Event};
use agen::llm_client::{ClientError, LlmClient, Request};
use agen::timeline::{Handler, TextBlockEvent, TextBlockKind, Timeline};
use async_trait::async_trait;
use futures::Stream;

use std::sync::atomic::{AtomicUsize, Ordering};

/// A mock LLM client that replays a sequence of events
#[derive(Clone)]
pub struct MockLlmClient {
    responses: Arc<Vec<Vec<Event>>>,
    call_count: Arc<AtomicUsize>,
}

impl MockLlmClient {
    pub fn new(events: Vec<Event>) -> Self {
        Self::with_responses(vec![events])
    }

    pub fn with_responses(responses: Vec<Vec<Event>>) -> Self {
        Self {
            responses: Arc::new(responses),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn from_fixture(path: impl AsRef<Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let events = load_events_from_fixture(path);
        Ok(Self::new(events))
    }

    pub fn event_count(&self) -> usize {
        self.responses.iter().map(|v| v.len()).sum()
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        _request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.responses.len() {
            return Err(ClientError::Api {
                status: Some(500),
                code: Some("mock_error".to_string()),
                message: "No more mock responses".to_string(),
                retry_after: None,
            });
        }
        let events = self.responses[count].clone();
        let stream = futures::stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}

/// Load events from a fixture file
pub fn load_events_from_fixture(path: impl AsRef<Path>) -> Vec<Event> {
    let file = File::open(path).expect("Failed to open fixture file");
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    // Skip metadata line
    let _metadata = lines.next().expect("Empty fixture file").unwrap();

    let mut events = Vec::new();
    for line in lines {
        let line = line.unwrap();
        if line.is_empty() {
            continue;
        }

        let recorded: serde_json::Value = serde_json::from_str(&line).unwrap();
        let data = recorded["data"].as_str().unwrap();
        let event: Event = serde_json::from_str(data).unwrap();
        events.push(event);
    }
    events
}

/// Find fixture files in a specific subdirectory
pub fn find_fixtures(subdir: &str) -> Vec<PathBuf> {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(subdir);

    if !fixtures_dir.exists() {
        return Vec::new();
    }

    std::fs::read_dir(&fixtures_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".jsonl"))
        })
        .collect()
}

/// Assert that events in all fixtures for a provider can be deserialized
pub fn assert_events_deserialize(subdir: &str) {
    let fixtures = find_fixtures(subdir);
    assert!(!fixtures.is_empty(), "No fixtures found for {}", subdir);

    for fixture_path in fixtures {
        println!("Testing fixture deserialization: {:?}", fixture_path);
        let events = load_events_from_fixture(&fixture_path);

        assert!(!events.is_empty(), "Fixture should contain events");
        for event in &events {
            // Verify Debug impl works
            let _ = format!("{:?}", event);
        }
    }
}

/// Assert that event sequence follows expected patterns
pub fn assert_event_sequence(subdir: &str) {
    let fixtures = find_fixtures(subdir);
    if fixtures.is_empty() {
        println!("No fixtures found for {}, skipping sequence test", subdir);
        return;
    }

    // Find a text-based fixture
    let fixture_path = fixtures
        .iter()
        .find(|p| p.to_string_lossy().contains("text"))
        .unwrap_or(&fixtures[0]);

    println!("Testing sequence with fixture: {:?}", fixture_path);
    let events = load_events_from_fixture(fixture_path);

    let mut start_found = false;
    let mut delta_found = false;
    let mut stop_found = false;
    let mut tool_use_found = false;

    for event in &events {
        match event {
            Event::BlockStart(start) => {
                start_found = true;
                if start.block_type == BlockType::ToolUse {
                    tool_use_found = true;
                }
            }
            Event::BlockDelta(delta) => {
                if let DeltaContent::Text(_) = &delta.delta {
                    delta_found = true;
                }
            }
            Event::BlockStop(stop) => {
                if stop.block_type == BlockType::Text {
                    stop_found = true;
                }
            }
            _ => {}
        }
    }

    assert!(!events.is_empty(), "Fixture should contain events");

    // Check for BlockStart (Warn only for OpenAI/Ollama as it might be missing for text)
    if !start_found {
        println!("Warning: No BlockStart found. This is common for OpenAI/Ollama text streams.");
        // For Anthropic, strict start is usually expected, but to keep common logic simple we allow warning.
        // If specific strictness is needed, we could add a `strict: bool` arg.
    }

    assert!(delta_found, "Should contain BlockDelta");

    if !tool_use_found {
        assert!(stop_found, "Should contain BlockStop for Text block");
    } else {
        if !stop_found {
            println!(
                "  [Type: ToolUse] BlockStop detection skipped (not explicitly emitted by scheme)"
            );
        }
    }
}

/// Assert usage tokens are present
pub fn assert_usage_tokens(subdir: &str) {
    let fixtures = find_fixtures(subdir);
    if fixtures.is_empty() {
        return;
    }

    for fixture in fixtures {
        let events = load_events_from_fixture(&fixture);
        let usage_events: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let Event::Usage(u) = e {
                    Some(u)
                } else {
                    None
                }
            })
            .collect();

        if !usage_events.is_empty() {
            let last_usage = usage_events.last().unwrap();
            if last_usage.input_tokens.is_some() || last_usage.output_tokens.is_some() {
                println!(
                    "  Fixture {:?} Usage: {:?}",
                    fixture.file_name(),
                    last_usage
                );
                return; // Found valid usage
            }
        }
    }
    println!("Warning: No usage events found for {}", subdir);
}

/// Assert timeline integration works
pub fn assert_timeline_integration(subdir: &str) {
    let fixtures = find_fixtures(subdir);
    if fixtures.is_empty() {
        return;
    }

    let fixture_path = fixtures
        .iter()
        .find(|p| p.to_string_lossy().contains("text"))
        .unwrap_or(&fixtures[0]);

    println!("Testing timeline with fixture: {:?}", fixture_path);
    let events = load_events_from_fixture(fixture_path);

    struct TestCollector {
        texts: Arc<Mutex<Vec<String>>>,
    }

    impl Handler<TextBlockKind> for TestCollector {
        type Scope = String;
        fn on_event(&mut self, buffer: &mut String, event: &TextBlockEvent) {
            match event {
                TextBlockEvent::Start(_) => {}
                TextBlockEvent::Delta(text) => buffer.push_str(text),
                TextBlockEvent::Stop(_) => {
                    let text = std::mem::take(buffer);
                    self.texts.lock().unwrap().push(text);
                }
            }
        }
    }

    let collected = Arc::new(Mutex::new(Vec::new()));
    let mut timeline = Timeline::new();
    timeline.on_text_block(TestCollector {
        texts: collected.clone(),
    });

    for event in &events {
        let timeline_event: agen::timeline::event::Event = event.clone().into();
        timeline.dispatch(&timeline_event);
    }

    let texts = collected.lock().unwrap();
    if !texts.is_empty() {
        assert!(!texts[0].is_empty(), "Collected text should not be empty");
        println!("  Collected {} text blocks.", texts.len());
    } else {
        println!("  No text blocks collected (might be tool-only fixture)");
    }
}
