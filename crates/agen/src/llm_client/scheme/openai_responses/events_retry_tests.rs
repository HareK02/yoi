//! Wire-to-event retry classification, independent of Engine replay policy.
use super::*;
use serde_json::json;
use std::time::Duration;

fn error(event_type: &str, payload: Value) -> ErrorEvent {
    let events = parse_sse(event_type, &payload.to_string(), &mut Default::default()).unwrap();
    let Event::Error(error) = events.into_iter().next().unwrap() else {
        panic!("expected error event");
    };
    error
}

#[test]
fn responses_overload_forms_preserve_retry_advice_and_diagnostics() {
    let detail = json!({
        "code": "server_is_overloaded", "type": "service_unavailable_error",
        "message": "provider at capacity", "headers": {"retry-after": "323", "retry-after-ms": "323000"}
    });
    for (event_type, payload) in [
        ("error", detail.clone()),
        ("error", json!({"type":"error", "error":detail})),
        ("response.failed", json!({"response":{"error":detail}})),
    ] {
        let error = error(event_type, payload);
        assert_eq!(
            error.retry.unwrap().retry_after,
            Some(Duration::from_secs(323))
        );
        assert_eq!(error.code.as_deref(), Some("server_is_overloaded"));
        assert!(error.message.contains("provider at capacity"));
        assert!(error.message.contains("service_unavailable_error"));
        assert!(error.message.contains("retry-after"));
    }
}

#[test]
fn responses_retry_headers_use_nested_advice_then_event_advice_then_backoff() {
    for event_type in ["error", "response.failed"] {
        for (nested, top, expected) in [
            (
                json!({"Retry-After": 5}),
                json!({"Retry-After": 2}),
                Some(Duration::from_secs(5)),
            ),
            (
                json!({"Retry-After": "bad"}),
                json!({"Retry-After-ms": 200}),
                Some(Duration::from_millis(200)),
            ),
            (
                Value::Null,
                json!({"Retry-After": 3}),
                Some(Duration::from_secs(3)),
            ),
            (Value::Null, Value::Null, None),
        ] {
            let detail = json!({"code":"server_is_overloaded", "headers":nested});
            let payload = if event_type == "error" {
                json!({"error":detail, "headers":top})
            } else {
                json!({"response":{"error":detail}, "headers":top})
            };
            assert_eq!(
                error(event_type, payload).retry.unwrap().retry_after,
                expected
            );
        }
    }
}

#[test]
fn unknown_codes_types_messages_and_incomplete_responses_do_not_authorize_retry() {
    for code in [
        Value::Null,
        json!("upstream_overloaded"),
        json!("overloaded_error"),
        json!("authentication_error"),
        json!("unknown"),
    ] {
        let detail = json!({
            "code":code, "type":"service_unavailable_error",
            "message":"server_is_overloaded", "headers":{"retry-after":3},
            "other":{"code":"server_is_overloaded"}
        });
        for (event_type, payload) in [
            ("error", detail.clone()),
            ("error", json!({"error":detail})),
            ("response.failed", json!({"response":{"error":detail}})),
        ] {
            assert_eq!(error(event_type, payload).retry, None);
        }
    }
    for (event_type, payload) in [
        (
            "response.incomplete",
            json!({"response":{"error":{"code":"server_is_overloaded"}}}),
        ),
        ("error", json!({"error":{"type":"server_is_overloaded"}})),
        (
            "error",
            json!({"error":{"code":"authentication_error"}, "code":"server_is_overloaded"}),
        ),
    ] {
        assert_eq!(error(event_type, payload).retry, None);
    }
}
