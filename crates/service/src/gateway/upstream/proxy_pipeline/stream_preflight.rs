use serde_json::Value;
use std::time::Duration;

use super::super::response::GatewayStreamPrefetchTerminal;
use super::super::GatewayUpstreamResponse;

#[path = "stream_overload.rs"]
mod stream_overload;

const STREAM_PREFLIGHT_MAX_BYTES: usize = 64 * 1024;
// Keep response headers below the default 15-second SSE keepalive window. A longer
// transparent failover window requires moving candidate coordination into the body.
const STREAM_PREFLIGHT_WALL_CLOCK_TIMEOUT: Duration = Duration::from_secs(10);
const USAGE_LIMIT_NOTICE_PREFIXES: [&str; 6] = [
    "you've hit your usage limit",
    "you have hit your usage limit",
    "the usage limit has been reached",
    "usage limit has been reached",
    "quota exceeded",
    "usage exhausted",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum PrefixDecision {
    NeedMore,
    Deliver,
    Failover(String),
    RetryUsageNotice(String),
    Overload,
}

pub(in super::super) enum StreamPreflightOutcome {
    Ready(GatewayUpstreamResponse),
    Failover(String),
    StatusFailover { status_code: u16, message: String },
    RetryUsageNotice(String),
    TransportFailover(String),
    OverloadFailover {
        message: String,
        retry_after: Option<Duration>,
        response: GatewayUpstreamResponse,
    },
}

fn should_prefetch_actionable_error_body(status_code: u16) -> bool {
    matches!(status_code, 401 | 403 | 429)
}

fn is_actionable_gateway_error(message: &str) -> bool {
    crate::account_status::usage_limit_reason_from_message(message).is_some()
        || crate::account_status::deactivation_reason_from_message(message).is_some()
}

fn actionable_message_from_error_value(error: &Value) -> Option<String> {
    if let Some(message) = error.as_str() {
        return is_actionable_gateway_error(message).then(|| message.to_string());
    }

    let error = error.as_object()?;
    ["message", "code", "type"]
        .into_iter()
        .filter_map(|key| error.get(key).and_then(Value::as_str))
        .find(|message| is_actionable_gateway_error(message))
        .map(str::to_string)
}

fn actionable_message_from_explicit_error(value: &Value) -> Option<String> {
    let top_level_error = value.get("error");
    let response = value.get("response");
    let response_error = response.and_then(|response| response.get("error"));
    let status_details_error = response
        .and_then(|response| response.get("status_details"))
        .and_then(|details| details.get("error"));

    [top_level_error, response_error, status_details_error]
        .into_iter()
        .flatten()
        .find_map(actionable_message_from_error_value)
}

fn is_error_event(event_type: &str) -> bool {
    matches!(
        event_type.trim().to_ascii_lowercase().as_str(),
        "error" | "response.failed" | "response.incomplete"
    )
}

fn actionable_message_from_error_event(value: &Value) -> Option<String> {
    actionable_message_from_explicit_error(value).or_else(|| {
        ["message", "code", "type"]
            .into_iter()
            .filter_map(|key| value.get(key).and_then(Value::as_str))
            .find(|message| is_actionable_gateway_error(message))
            .map(str::to_string)
    })
}

fn actionable_message_from_error_body(body: &[u8]) -> Option<String> {
    let parsed = serde_json::from_slice::<Value>(body).ok();
    if let Some(message) = parsed
        .as_ref()
        .and_then(actionable_message_from_error_event)
    {
        return Some(message);
    }

    let text = std::str::from_utf8(body).ok()?.trim();
    (!text.is_empty() && is_actionable_gateway_error(text)).then(|| text.to_string())
}

fn summarize_non_200_status_failover(status_code: u16, body: Option<&[u8]>) -> String {
    let body_hint = body
        .and_then(|body| crate::gateway::summarize_upstream_error_hint_from_body(status_code, body))
        .map(|hint| format!(" body={hint}"))
        .unwrap_or_default();
    format!("upstream non-200 status={status_code}{body_hint}")
}

fn is_strong_usage_limit_delta(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    let looks_like_notice = USAGE_LIMIT_NOTICE_PREFIXES.iter().any(|prefix| {
        normalized.strip_prefix(prefix).is_some_and(|suffix| {
            suffix.is_empty()
                || suffix
                    .chars()
                    .next()
                    .is_some_and(|ch| matches!(ch, '.' | '!' | ':' | ',' | '\n' | '\r'))
        })
    }) || matches!(
        normalized.as_str(),
        "usage_limit_reached"
            | "usage_limit_exceeded"
            | "usage_limit_exhausted"
            | "insufficient_quota"
    );

    looks_like_notice && crate::account_status::usage_limit_reason_from_message(message).is_some()
}

fn is_possible_usage_limit_delta_prefix(message: &str) -> bool {
    let normalized = message.trim_start().to_ascii_lowercase();
    !normalized.is_empty()
        && USAGE_LIMIT_NOTICE_PREFIXES
            .iter()
            .chain(
                [
                    "usage_limit_reached",
                    "usage_limit_exceeded",
                    "usage_limit_exhausted",
                    "insufficient_quota",
                ]
                .iter(),
            )
            .any(|notice| notice.starts_with(normalized.as_str()))
}

fn normalized_frames(prefix: &[u8], include_incomplete_frame: bool) -> (Vec<String>, bool) {
    let normalized = String::from_utf8_lossy(prefix).replace("\r\n", "\n");
    let has_incomplete_trailing_frame = include_incomplete_frame && !normalized.ends_with("\n\n");
    let mut parts = normalized.split("\n\n").collect::<Vec<_>>();
    if !include_incomplete_frame && !normalized.ends_with("\n\n") {
        let _ = parts.pop();
    }
    (
        parts.into_iter().map(str::to_string).collect(),
        has_incomplete_trailing_frame,
    )
}

fn frame_event_and_data(frame: &str) -> (Option<String>, Option<String>) {
    let mut event_type = None;
    let mut data = String::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("event:") {
            event_type = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        }
    }
    let data = (!data.is_empty()).then_some(data);
    (event_type, data)
}

fn is_usage_notice_followup_event(event_type: &str) -> bool {
    matches!(
        event_type.trim().to_ascii_lowercase().as_str(),
        "response.output_text.done" | "response.output_item.done" | "response.content_part.done"
    )
}

fn is_usage_notice_terminal_event(event_type: &str) -> bool {
    matches!(
        event_type.trim().to_ascii_lowercase().as_str(),
        "error" | "response.failed" | "response.incomplete"
    )
}

fn is_sse_stream_response(response: &GatewayUpstreamResponse) -> bool {
    matches!(response, GatewayUpstreamResponse::Stream(_))
        && response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SniffedFraming {
    NeedMore,
    Sse(usize),
    Other,
}

/// Only recognized SSE field prefixes qualify an incorrectly labelled body for
/// the capacity recognizer. A JSON object/array or ordinary text is committed
/// unchanged, including when it happens to contain an overload error string.
fn sniff_sse_framing(prefix: &[u8]) -> SniffedFraming {
    let skip_space = |bytes: &[u8]| bytes.iter().take_while(|byte| byte.is_ascii_whitespace()).count();
    let mut start = skip_space(prefix);
    let remaining = &prefix[start..];
    const BOM: &[u8] = &[0xef, 0xbb, 0xbf];
    if remaining.is_empty() { return SniffedFraming::NeedMore; }
    if remaining.starts_with(BOM) {
        start += BOM.len();
        start += skip_space(&prefix[start..]);
    } else if BOM.starts_with(remaining) {
        return SniffedFraming::NeedMore;
    }
    let remaining = &prefix[start..];
    if remaining.is_empty() { return SniffedFraming::NeedMore; }
    const FIELDS: [&[u8]; 5] = [b"data:", b"event:", b"id:", b"retry:", b":"];
    // Qualify the whole observed prefix, not only its first line: a comment
    // followed by ordinary body text must not hide that text from the existing
    // SSE classifier. Unknown fields are conservatively delivered in this new
    // dynamic path, even though a general SSE client may ignore them.
    let mut line_start = 0;
    for (index, byte) in remaining.iter().enumerate() {
        if *byte != b'\n' { continue; }
        let line = &remaining[line_start..index];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if !line.is_empty() && !FIELDS.iter().any(|field| line.starts_with(field)) {
            return SniffedFraming::Other;
        }
        line_start = index + 1;
    }
    let tail = &remaining[line_start..];
    let tail = tail.strip_suffix(b"\r").unwrap_or(tail);
    if tail.is_empty() || FIELDS.iter().any(|field| tail.starts_with(field)) {
        return SniffedFraming::Sse(start);
    }
    if FIELDS.iter().any(|field| field.starts_with(tail)) { SniffedFraming::NeedMore }
    else { SniffedFraming::Other }
}

fn classify_preflight_prefix(
    prefix: &[u8],
    include_incomplete_frame: bool,
    allow_overload_failover: bool,
    sniff_framing: bool,
) -> PrefixDecision {
    if !sniff_framing {
        return classify_prefix(prefix, include_incomplete_frame, allow_overload_failover);
    }
    match sniff_sse_framing(prefix) {
        SniffedFraming::NeedMore => PrefixDecision::NeedMore,
        SniffedFraming::Other => PrefixDecision::Deliver,
        SniffedFraming::Sse(start) => classify_prefix(
            &prefix[start..], include_incomplete_frame, allow_overload_failover,
        ),
    }
}

fn classify_prefix(
    prefix: &[u8],
    include_incomplete_frame: bool,
    allow_overload_failover: bool,
) -> PrefixDecision {
    let (frames, has_incomplete_trailing_frame) =
        normalized_frames(prefix, include_incomplete_frame);
    if frames.is_empty() {
        return PrefixDecision::NeedMore;
    }

    let frame_count = frames.len();
    let mut pending_usage_notice = None;
    let mut usage_notice_candidate = String::new();
    for (index, frame) in frames.into_iter().enumerate() {
        let (declared_event_type, data) = frame_event_and_data(frame.as_str());
        let Some(data) = data else {
            continue;
        };
        if data.trim() == "[DONE]" {
            return pending_usage_notice
                .map(PrefixDecision::RetryUsageNotice)
                .unwrap_or(PrefixDecision::Deliver);
        }

        let parsed = serde_json::from_str::<Value>(data.as_str()).ok();
        let is_incomplete_trailing_frame =
            has_incomplete_trailing_frame && index + 1 == frame_count;
        if is_incomplete_trailing_frame
            && parsed.is_none()
            && data
                .trim_start()
                .as_bytes()
                .first()
                .is_some_and(|byte| matches!(*byte, b'{' | b'['))
        {
            return PrefixDecision::NeedMore;
        }
        let payload_event_type = parsed
            .as_ref()
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let event_type = payload_event_type.clone().or(declared_event_type.clone());

        // A retry must not consume tool starts or populated content disguised as
        // an item/part-added event. Only genuinely empty metadata may be held.
        if event_type.as_deref().is_some_and(|kind| {
            stream_overload::is_content_boundary(kind, parsed.as_ref())
        }) {
            return PrefixDecision::Deliver;
        }

        if allow_overload_failover
            && event_type.as_deref().is_some_and(is_error_event)
            && parsed
                .as_ref()
                .is_some_and(|value| {
                    stream_overload::is_structured_overload(
                        value,
                        event_type.as_deref().unwrap_or_default(),
                    )
                })
        {
            // The existing quota-notice recognizer can temporarily hold a text
            // delta. That exception must not expand the new overload replay
            // path to responses which have already produced any text.
            if !usage_notice_candidate.is_empty() {
                return PrefixDecision::Deliver;
            }
            return PrefixDecision::Overload;
        }

        if let Some(message) = parsed
            .as_ref()
            .and_then(actionable_message_from_explicit_error)
        {
            return PrefixDecision::Failover(message);
        }

        let has_error_event = declared_event_type.as_deref().is_some_and(is_error_event)
            || payload_event_type.as_deref().is_some_and(is_error_event);
        if has_error_event {
            let message = match parsed.as_ref() {
                Some(value) => actionable_message_from_error_event(value),
                None => is_actionable_gateway_error(data.as_str()).then(|| data.clone()),
            };
            if let Some(message) = message {
                return PrefixDecision::Failover(message);
            }
        }

        if event_type.as_deref() == Some("response.output_text.delta") {
            if let Some(message) = parsed
                .as_ref()
                .and_then(|value| value.get("delta"))
                .and_then(Value::as_str)
            {
                usage_notice_candidate.push_str(message);
                if is_strong_usage_limit_delta(usage_notice_candidate.as_str()) {
                    pending_usage_notice = Some(usage_notice_candidate.clone());
                    continue;
                }
                if pending_usage_notice.is_none()
                    && is_possible_usage_limit_delta_prefix(usage_notice_candidate.as_str())
                {
                    continue;
                }
            }
        }

        match event_type.as_deref() {
            Some(event_type)
                if stream_overload::is_retry_safe_metadata(event_type, parsed.as_ref()) => {}
            Some(event_type)
                if pending_usage_notice.is_some() && is_usage_notice_terminal_event(event_type) =>
            {
                return PrefixDecision::RetryUsageNotice(
                    pending_usage_notice.expect("pending usage notice checked above"),
                );
            }
            Some(event_type)
                if pending_usage_notice.is_some() && is_usage_notice_followup_event(event_type) => {
            }
            // Any non-metadata event means the upstream has begun producing a real response.
            // At that point retrying another account could duplicate visible output/tool work.
            Some(_) | None => return PrefixDecision::Deliver,
        }
    }

    if include_incomplete_frame {
        if let Some(message) = pending_usage_notice {
            return PrefixDecision::RetryUsageNotice(message);
        }
    }
    PrefixDecision::NeedMore
}

pub(super) fn retry_after_for_health(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    stream_overload::retry_after_delay(headers, chrono::Utc::now()).ok().flatten()
}

pub(in super::super) fn preflight_stream_response(
    response: GatewayUpstreamResponse,
    request_path: &str,
    upstream_is_stream: bool,
    has_more_candidates: bool,
) -> StreamPreflightOutcome {
    preflight_stream_response_with_overload_failover(
        response,
        request_path,
        upstream_is_stream,
        has_more_candidates,
        false,
    )
}

/// The candidate coordinator grants this permission only while a single backup
/// account is still within its request budget. Otherwise preserve the response.
pub(in super::super) fn preflight_stream_response_with_overload_failover(
    response: GatewayUpstreamResponse,
    request_path: &str,
    upstream_is_stream: bool,
    has_more_candidates: bool,
    allow_overload_failover: bool,
) -> StreamPreflightOutcome {
    preflight_stream_response_with_dynamic_failover(response, request_path, upstream_is_stream,
        has_more_candidates, allow_overload_failover, false)
}

pub(in super::super) fn preflight_stream_response_with_dynamic_failover(
    response: GatewayUpstreamResponse,
    request_path: &str,
    upstream_is_stream: bool,
    has_more_candidates: bool,
    allow_overload_failover: bool,
    dynamic_selection: bool,
) -> StreamPreflightOutcome {
    preflight_stream_response_with_dynamic_timeouts(
        response, request_path, upstream_is_stream, has_more_candidates,
        crate::gateway::upstream_stream_timeout(), Some(STREAM_PREFLIGHT_WALL_CLOCK_TIMEOUT),
        allow_overload_failover && (has_more_candidates || dynamic_selection),
        dynamic_selection,
    )
}

#[cfg(test)]
fn preflight_stream_response_with_idle_timeout(
    response: GatewayUpstreamResponse,
    request_path: &str,
    upstream_is_stream: bool,
    has_more_candidates: bool,
    idle_timeout: Option<std::time::Duration>,
) -> StreamPreflightOutcome {
    preflight_stream_response_with_timeouts(
        response,
        request_path,
        upstream_is_stream,
        has_more_candidates,
        idle_timeout,
        Some(STREAM_PREFLIGHT_WALL_CLOCK_TIMEOUT),
        false,
    )
}

fn preflight_stream_response_with_timeouts(
    response: GatewayUpstreamResponse,
    request_path: &str,
    upstream_is_stream: bool,
    has_more_candidates: bool,
    idle_timeout: Option<Duration>,
    wall_clock_timeout: Option<Duration>,
    allow_overload_failover: bool,
) -> StreamPreflightOutcome {
    preflight_stream_response_with_dynamic_timeouts(
        response, request_path, upstream_is_stream, has_more_candidates,
        idle_timeout, wall_clock_timeout, allow_overload_failover, false,
    )
}

fn preflight_stream_response_with_dynamic_timeouts(
    response: GatewayUpstreamResponse,
    request_path: &str,
    upstream_is_stream: bool,
    has_more_candidates: bool,
    idle_timeout: Option<Duration>,
    wall_clock_timeout: Option<Duration>,
    allow_overload_failover: bool,
    dynamic_selection: bool,
) -> StreamPreflightOutcome {
    let status_code = response.status().as_u16();
    if has_more_candidates && !(200..=299).contains(&status_code) {
        if should_prefetch_actionable_error_body(status_code) {
            return match response.into_buffered() {
                Ok((body, _response)) => actionable_message_from_error_body(body.as_ref())
                    .map(StreamPreflightOutcome::Failover)
                    .unwrap_or_else(|| StreamPreflightOutcome::StatusFailover {
                        status_code,
                        message: summarize_non_200_status_failover(
                            status_code,
                            Some(body.as_ref()),
                        ),
                    }),
                Err(err) => StreamPreflightOutcome::StatusFailover {
                    status_code,
                    message: format!(
                        "upstream non-200 status={status_code}; read response body failed: {err}"
                    ),
                },
            };
        }
        return StreamPreflightOutcome::StatusFailover {
            status_code,
            message: summarize_non_200_status_failover(status_code, None),
        };
    }

    if !upstream_is_stream
        || (!has_more_candidates && !allow_overload_failover)
        || !request_path.starts_with("/v1/responses")
        || status_code >= 400
    {
        return StreamPreflightOutcome::Ready(response);
    }

    let retry_after = stream_overload::retry_after_delay(response.headers(), chrono::Utc::now());
    let allow_overload_failover = allow_overload_failover
        && retry_after
            .as_ref()
            .is_ok_and(|delay| delay.is_none_or(|delay| delay <= stream_overload::MAX_RETRY_AFTER));
    let has_sse_header = is_sse_stream_response(&response);
    let sniff_framing = dynamic_selection && allow_overload_failover;
    if !has_sse_header && !sniff_framing {
        return StreamPreflightOutcome::Ready(response);
    }

    let (prefix, response, terminal) = response.prefetch_stream_prefix(
        STREAM_PREFLIGHT_MAX_BYTES,
        idle_timeout,
        wall_clock_timeout,
        |prefix| {
            !matches!(
                classify_preflight_prefix(prefix, false, allow_overload_failover, sniff_framing),
                PrefixDecision::NeedMore
            )
        },
    );
    let include_incomplete_frame = matches!(
        terminal,
        GatewayStreamPrefetchTerminal::Eof
            | GatewayStreamPrefetchTerminal::Error(_)
            | GatewayStreamPrefetchTerminal::Disconnected
    );
    let decision = classify_preflight_prefix(prefix.as_ref(), include_incomplete_frame, allow_overload_failover, sniff_framing);
    if sniff_framing && !matches!(sniff_sse_framing(prefix.as_ref()), SniffedFraming::Sse(_)) {
        return StreamPreflightOutcome::Ready(response);
    }
    // Missing/mislabelled content types previously committed immediately. The
    // new compatibility path adds only proven safe capacity failover, never a
    // new transport/quota retry for JSON, plain text, partial framing or EOF.
    if !has_sse_header && !matches!(decision, PrefixDecision::Overload) {
        return StreamPreflightOutcome::Ready(response);
    }
    if !has_more_candidates && !matches!(decision, PrefixDecision::Overload) {
        return StreamPreflightOutcome::Ready(response);
    }
    match decision {
        PrefixDecision::Failover(message) => StreamPreflightOutcome::Failover(message),
        PrefixDecision::RetryUsageNotice(message) => {
            StreamPreflightOutcome::RetryUsageNotice(message)
        }
        PrefixDecision::Overload => StreamPreflightOutcome::OverloadFailover {
            // Do not record arbitrary upstream error text as part of retry logs.
            message: "upstream model temporarily overloaded (code=server_is_overloaded)".to_string(),
            retry_after: retry_after.ok().flatten(),
            response,
        },
        PrefixDecision::Deliver => StreamPreflightOutcome::Ready(response),
        PrefixDecision::NeedMore => match terminal {
            GatewayStreamPrefetchTerminal::Open => StreamPreflightOutcome::TransportFailover(
                "upstream response stream preflight stopped before producing deliverable content"
                    .to_string(),
            ),
            GatewayStreamPrefetchTerminal::PrefixLimit => {
                // Large response.created metadata can legitimately exceed the
                // classification buffer (for example with a large tools list).
                // Commit and replay it instead of exhausting every account with
                // the same request-shaped prefix.
                StreamPreflightOutcome::Ready(response)
            }
            GatewayStreamPrefetchTerminal::IdleTimeout => {
                StreamPreflightOutcome::TransportFailover(
                    "upstream response stream idle timeout before producing deliverable content"
                        .to_string(),
                )
            }
            GatewayStreamPrefetchTerminal::WallClockTimeout => {
                // A normal response may have a slow first semantic event. Commit the
                // buffered stream instead of treating the account as unhealthy and
                // delaying downstream response headers for the full idle timeout.
                StreamPreflightOutcome::Ready(response)
            }
            GatewayStreamPrefetchTerminal::Eof => StreamPreflightOutcome::TransportFailover(
                "upstream response stream ended before producing deliverable content".to_string(),
            ),
            GatewayStreamPrefetchTerminal::Error(err) => {
                StreamPreflightOutcome::TransportFailover(format!(
                    "upstream response stream failed before producing deliverable content: {err}"
                ))
            }
            GatewayStreamPrefetchTerminal::Disconnected => {
                StreamPreflightOutcome::TransportFailover(
                    "upstream response stream disconnected before producing deliverable content"
                        .to_string(),
                )
            }
        },
    }
}

#[cfg(test)]
#[path = "stream_preflight_tests.rs"]
mod tests;
