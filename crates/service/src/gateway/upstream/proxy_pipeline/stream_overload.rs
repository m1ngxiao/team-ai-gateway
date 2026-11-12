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
    // frames. Empty reasoning/message placeholders are metadata, not output;
    // any real text, tool arguments, tool result, or encrypted reasoning is a
    // hard boundary and must never be replayed.
    if !content_free_output(value.pointer("/response/output"))
        || !content_free_output(value.get("output"))
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

fn empty_string_or_absent(value: Option<&Value>) -> bool {
    value.is_none_or(|value| value.as_str().is_none_or(|text| text.is_empty()))
}

/// True only when an output item carries no user-visible or replay-sensitive
/// payload. `reasoning` may appear before an overload with an empty summary;
/// that is a placeholder, not generated content. Encrypted reasoning is always
/// a hard boundary.
fn content_free_output_item(item: &Value) -> bool {
    let Some(object) = item.as_object() else {
        return false;
    };
    match object.get("type").and_then(Value::as_str) {
        Some("reasoning") => {
            empty_array_or_absent(object.get("summary"))
                && empty_array_or_absent(object.get("content"))
                && empty_string_or_absent(object.get("encrypted_content"))
        }
        Some("message") => empty_array_or_absent(object.get("content")),
        _ => false,
    }
}

fn content_free_output(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Array(items)) => items.iter().all(content_free_output_item),
        _ => false,
    }
}

pub(super) fn is_retry_safe_metadata(event_type: &str, value: Option<&Value>) -> bool {
    let Some(value) = value.filter(|value| value.is_object()) else {
        return false;
    };
    match event_type {
        "response.created" | "response.in_progress" | "response.queued" | "ping" => {
            content_free_output(value.pointer("/response/output"))
        }
        "response.output_item.added" => value
            .pointer("/item")
            .is_some_and(content_free_output_item),
        "response.content_part.added" | "response.reasoning_summary_part.added" => {
            matches!(
                value.pointer("/part/type").and_then(Value::as_str),
                Some("output_text" | "summary_text" | "reasoning_text")
            ) && value.pointer("/part/text").and_then(Value::as_str) == Some("")
                && empty_array_or_absent(value.pointer("/part/annotations"))
        }
        "response.output_text.delta"
        | "response.reasoning_summary_text.delta"
        | "response.reasoning_text.delta"
        | "response.function_call_arguments.delta"
        | "response.custom_tool_call_input.delta"
        | "response.refusal.delta" => empty_string_or_absent(value.get("delta")),
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
