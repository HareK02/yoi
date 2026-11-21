//! Responses owns error-code semantics; diagnostics and headers alone never
//! authorize replay. Nested error headers take precedence over event headers.

use crate::llm_client::event::RetryAdvice;
use serde_json::Value;
use std::time::{Duration, SystemTime};

pub(super) fn classify(
    code: Option<&str>,
    error_headers: Option<&Value>,
    event_headers: Option<&Value>,
) -> Option<RetryAdvice> {
    // The service_unavailable_error type is not sufficient: require the explicit
    // overload code observed in Responses error and response.failed events.
    if code != Some("server_is_overloaded") {
        return None;
    }
    let now = SystemTime::now();
    Some(RetryAdvice {
        retry_after: retry_after(error_headers, now).or_else(|| retry_after(event_headers, now)),
    })
}

fn retry_after(headers: Option<&Value>, now: SystemTime) -> Option<Duration> {
    headers?
        .as_object()?
        .iter()
        .filter_map(|(name, value)| {
            let milliseconds = name.eq_ignore_ascii_case("retry-after-ms");
            if !milliseconds && !name.eq_ignore_ascii_case("retry-after") {
                return None;
            }
            let value = match value {
                Value::String(value) => value.clone(),
                Value::Number(value) => value.to_string(),
                _ => return None,
            };
            let value = value.trim();
            if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                // A valid but unrepresentably long delay must exhaust the budget,
                // not fall back to a shorter local backoff.
                return Some(match value.parse::<u64>() {
                    Ok(value) if milliseconds => Duration::from_millis(value),
                    Ok(value) => Duration::from_secs(value),
                    Err(_) => Duration::MAX,
                });
            }
            if milliseconds {
                return None;
            }
            httpdate::parse_http_date(value)
                .ok()
                .map(|date| date.duration_since(now).unwrap_or_default())
        })
        // When both units are supplied, never retry earlier than either advice.
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retry_headers_preserve_units_dates_and_long_delays() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784111777);
        for (headers, expected) in [
            (
                json!({"Retry-After": "323", "retry-after-ms": "323000"}),
                Some(Duration::from_secs(323)),
            ),
            (
                json!({"RETRY-AFTER-MS": 125}),
                Some(Duration::from_millis(125)),
            ),
            (json!({"retry-after": " 2 "}), Some(Duration::from_secs(2))),
            (
                json!({"retry-after": "Sun, 06 Nov 1994 08:49:40 GMT"}),
                Some(Duration::from_secs(3)),
            ),
            (
                json!({"retry-after": "Sun, 06 Nov 1994 08:49:36 GMT"}),
                Some(Duration::ZERO),
            ),
            (
                json!({"retry-after-ms": "invalid", "retry-after": 3}),
                Some(Duration::from_secs(3)),
            ),
            (
                json!({"retry-after": "2", "retry-after-ms": "3000"}),
                Some(Duration::from_secs(3)),
            ),
            (
                json!({"retry-after": "9999999999999999999999999"}),
                Some(Duration::MAX),
            ),
            (
                json!({"retry-after-ms": "9999999999999999999999999"}),
                Some(Duration::MAX),
            ),
            (json!({"retry-after": "-1", "retry-after-ms": false}), None),
            (json!({"retry-after": []}), None),
            (json!("retry-after: 3"), None),
        ] {
            assert_eq!(retry_after(Some(&headers), now), expected, "{headers}");
        }
    }
}
