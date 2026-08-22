//! Ollama fixture-based integration tests

mod common;

#[test]
fn test_fixture_events_deserialize() {
    common::assert_events_deserialize("ollama");
}

#[test]
fn test_fixture_event_sequence() {
    common::assert_event_sequence("ollama");
}

#[test]
fn test_fixture_usage_tokens() {
    common::assert_usage_tokens("ollama");
}

#[test]
fn test_fixture_with_timeline() {
    common::assert_timeline_integration("ollama");
}
