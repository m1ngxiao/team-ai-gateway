//! Narrow overload recognition and the metadata boundary for transparent retry.
use serde_json::Value;
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, RETRY_AFTER};

pub(super) const MAX_RETRY_AFTER: Duration = Duration::from_secs(2);

/// Invalid or ambiguous retry instructions must not turn into an immediate retry.
pub(super) fn retry_after_delay(
    headers: &HeaderMap,
    now: DateTime<Utc>,
) -> Result<Option<Duration>, ()> {
    let mut values = headers.get_all(RETRY_AFTER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(());
    }
    let value = value.to_str().map_err(|_| ())?.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .map(Duration::from_secs)
            .map(Some)
            .map_err(|_| ());
    }
    let date = DateTime::parse_from_rfc2822(value).map_err(|_| ())?;
    Ok(Some(
        date.signed_duration_since(now)
            .to_std()
            .unwrap_or(Duration::ZERO),
    ))
}

pub(super) fn is_structured_overload(value: &Value, event_type: &str) -> bool {
    // An error can include an output snapshot even without preceding delta
    // frames. A tool call or already generated content must never be replayed.
    if !empty_array_or_absent(value.pointer("/response/output"))
        || !empty_array_or_absent(value.get("output"))
    {
        return false;
    }
    [
        value.get("error"),
        value.pointer("/response/error"),
        value.pointer("/response/status_details/error"),
        // The Responses `error` event can put code/message at the top level.
        (event_type == "error").then_some(value),
    ]
    .into_iter()
    .flatten()
    .any(|error| error.get("code").and_then(Value::as_str) == Some("server_is_overloaded"))
}

fn empty_array_or_absent(value: Option<&Value>) -> bool {
    value.is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
}

pub(super) fn is_retry_safe_metadata(event_type: &str, value: Option<&Value>) -> bool {
    let Some(value) = value.filter(|value| value.is_object()) else {
        return false;
    };
    match event_type {
        "response.created" | "response.in_progress" | "response.queued" | "ping" => {
            empty_array_or_absent(value.pointer("/response/output"))
        }
        "response.output_item.added" => {
            value.pointer("/item/type").and_then(Value::as_str) == Some("message")
                && value
                    .pointer("/item/content")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
        }
        "response.content_part.added" | "response.reasoning_summary_part.added" => {
            matches!(
                value.pointer("/part/type").and_then(Value::as_str),
                Some("output_text" | "summary_text")
            ) && value.pointer("/part/text").and_then(Value::as_str) == Some("")
                && empty_array_or_absent(value.pointer("/part/annotations"))
        }
        _ => false,
    }
}

pub(super) fn is_content_boundary(event_type: &str, value: Option<&Value>) -> bool {
    matches!(
        event_type,
        "response.created"
            | "response.in_progress"
            | "response.queued"
            | "response.output_item.added"
            | "response.content_part.added"
            | "response.reasoning_summary_part.added"
    ) && !is_retry_safe_metadata(event_type, value)
}
