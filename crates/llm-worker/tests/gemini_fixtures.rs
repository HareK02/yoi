//! Gemini fixture-based integration tests

mod common;

#[test]
fn test_fixture_events_deserialize() {
    common::assert_events_deserialize("gemini");
}

#[test]
fn test_fixture_event_sequence() {
    common::assert_event_sequence("gemini");
}

#[test]
fn test_fixture_usage_tokens() {
    common::assert_usage_tokens("gemini");
}

#[test]
fn test_fixture_with_timeline() {
    common::assert_timeline_integration("gemini");
}
