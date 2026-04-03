//! Anthropic fixture-based integration tests

mod common;

#[test]
fn test_fixture_events_deserialize() {
    common::assert_events_deserialize("anthropic");
}

#[test]
fn test_fixture_event_sequence() {
    common::assert_event_sequence("anthropic");
}

#[test]
fn test_fixture_usage_tokens() {
    common::assert_usage_tokens("anthropic");
}

#[test]
fn test_fixture_with_timeline() {
    common::assert_timeline_integration("anthropic");
}
