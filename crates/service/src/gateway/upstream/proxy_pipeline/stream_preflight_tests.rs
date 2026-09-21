use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::gateway::upstream::{GatewayByteStream, GatewayByteStreamItem, GatewayStreamResponse};

fn classify_prefix(prefix: &[u8], include_incomplete_frame: bool) -> PrefixDecision {
    super::classify_prefix(prefix, include_incomplete_frame, false)
}

fn stream_response(body: &'static str) -> GatewayUpstreamResponse {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_bytes(Bytes::from_static(body.as_bytes())),
    ))
}

fn json_stream_response(body: &'static str) -> GatewayUpstreamResponse {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_bytes(Bytes::from_static(body.as_bytes())),
    ))
}

fn json_stream_response_with_status(
    status: reqwest::StatusCode,
    body: &'static str,
) -> GatewayUpstreamResponse {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        status,
        headers,
        GatewayByteStream::from_bytes(Bytes::from_static(body.as_bytes())),
    ))
}

fn stream_response_from_items(items: Vec<GatewayByteStreamItem>) -> GatewayUpstreamResponse {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    let (tx, rx) = mpsc::sync_channel(items.len().max(1));
    for item in items {
        tx.send(item).expect("queue upstream item");
    }
    drop(tx);
    GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_receiver(rx),
    ))
}

#[test]
fn prefix_waits_through_metadata_only_frames() {
    let prefix = b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n";
    assert_eq!(classify_prefix(prefix, false), PrefixDecision::NeedMore);
}

#[test]
fn prefix_waits_for_terminal_confirmation_of_usage_notice_output() {
    let prefix = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\",\"content\":[]}}\n\n",
        "data: {\"type\":\"response.content_part.added\",\"part\":{\"type\":\"output_text\",\"text\":\"\"}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"You've hit your usage limit. Try again later.\"}\n\n"
    );
    assert_eq!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::NeedMore
    );

    let terminated = format!("{prefix}data: [DONE]\n\n");
    assert!(matches!(
        classify_prefix(terminated.as_bytes(), false),
        PrefixDecision::RetryUsageNotice(message) if message.contains("usage limit")
    ));
}

#[test]
fn prefix_accumulates_usage_notice_split_across_deltas() {
    let prefix = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"You've hit your \"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"usage limit. Try again later.\"}\n\n",
        "data: [DONE]\n\n"
    );
    assert!(matches!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::RetryUsageNotice(message) if message.contains("usage limit")
    ));
}

#[test]
fn prefix_retries_usage_notice_confirmed_by_incomplete_terminal() {
    let prefix = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"You've hit your usage limit. Try again later.\"}\n\n",
        "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_limited\",\"status\":\"incomplete\"}}\n\n"
    );
    assert!(matches!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::RetryUsageNotice(message) if message.contains("usage limit")
    ));
}

#[test]
fn prefix_retries_usage_notice_confirmed_by_bare_terminal_events() {
    for terminal_event in ["response.incomplete", "response.failed"] {
        let prefix = format!(
            "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"You've hit your usage limit. Try again later.\"}}\n\ndata: {{\"type\":\"{terminal_event}\"}}\n\n"
        );
        assert!(matches!(
            classify_prefix(prefix.as_bytes(), false),
            PrefixDecision::RetryUsageNotice(message) if message.contains("usage limit")
        ));
    }
}

#[test]
fn prefix_delivers_when_a_possible_usage_notice_prefix_diverges() {
    let prefix = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"You've hit your \"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"stride goal for today.\"}\n\n"
    );
    assert_eq!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::Deliver
    );
}

#[test]
fn prefix_detects_usage_limit_in_explicit_error_fields() {
    let prefix = concat!(
        "event: response.failed\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"usage_limit_reached\"}}}\n\n"
    );
    assert_eq!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::Failover("usage_limit_reached".to_string())
    );
}

#[test]
fn prefix_detects_deactivation_in_explicit_error_event() {
    let prefix = concat!(
        "event: error\n",
        "data: {\"message\":\"workspace_deactivated\"}\n\n"
    );
    assert_eq!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::Failover("workspace_deactivated".to_string())
    );
}

#[test]
fn terminal_prefix_detects_error_frame_without_trailing_separator() {
    let prefix = concat!(
        "event: response.failed\n",
        "data: {\"type\":\"response.failed\",\"error\":{\"message\":\"You've hit your usage limit.\"}}"
    );
    assert_eq!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::NeedMore
    );
    assert!(matches!(
        classify_prefix(prefix.as_bytes(), true),
        PrefixDecision::Failover(message) if message.contains("usage limit")
    ));
}

#[test]
fn prefix_ignores_actionable_words_in_metadata_strings() {
    let prefix = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"metadata\":{\"prompt\":\"Explain You've hit your usage limit and deactivated\"}}}\n\n"
    );
    assert_eq!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::NeedMore
    );
}

#[test]
fn prefix_does_not_scan_arbitrary_strings_inside_error_event() {
    let prefix = concat!(
        "event: error\n",
        "data: {\"type\":\"error\",\"context\":{\"prompt\":\"Explain You've hit your usage limit and deactivated\"}}\n\n"
    );
    assert_eq!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::Deliver
    );
}

#[test]
fn prefix_delivers_usage_limit_words_in_normal_output() {
    let prefix = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"The usage limit has been reached is an English error message. An account may also be deactivated.\"}\n\n"
    );
    assert_eq!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::Deliver
    );
}

#[test]
fn prefix_delivers_deactivation_notice_in_normal_output() {
    let prefix = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Your account has been deactivated.\"}\n\n"
    );
    assert_eq!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::Deliver
    );
}

#[test]
fn prefix_delivers_exact_usage_notice_when_response_completes_normally() {
    let prefix = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"You've hit your usage limit. Try again later.\"}\n\n",
        "data: {\"type\":\"response.output_text.done\",\"text\":\"You've hit your usage limit. Try again later.\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_normal\",\"status\":\"completed\"}}\n\n",
        "data: [DONE]\n\n"
    );
    assert_eq!(
        classify_prefix(prefix.as_bytes(), false),
        PrefixDecision::Deliver
    );
}

#[test]
fn prefix_commits_normal_output() {
    let prefix = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n";
    assert_eq!(classify_prefix(prefix, false), PrefixDecision::Deliver);
}

#[test]
fn preflight_replays_normal_prefix_without_loss() {
    let body = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
        "data: [DONE]\n\n"
    );
    let outcome = preflight_stream_response(stream_response(body), "/v1/responses", true, true);
    let StreamPreflightOutcome::Ready(response) = outcome else {
        panic!("normal output must be delivered");
    };
    let (replayed, _) = response.into_buffered().expect("buffer replayed response");
    assert_eq!(replayed.as_ref(), body.as_bytes());
}

#[test]
fn preflight_leaves_successful_json_response_untouched() {
    let body = r#"{"id":"resp_json","status":"completed","output":[]}"#;
    let outcome =
        preflight_stream_response(json_stream_response(body), "/v1/responses", true, true);
    let StreamPreflightOutcome::Ready(response) = outcome else {
        panic!("successful JSON must bypass SSE preflight");
    };
    let (replayed, _) = response.into_buffered().expect("buffer JSON response");
    assert_eq!(replayed.as_ref(), body.as_bytes());
}

#[test]
fn preflight_fails_over_on_json_usage_limit_error_response() {
    let body = r#"{"error":{"message":"The usage limit has been reached.","type":"usage_limit_reached","code":"usage_limit_reached"}}"#;
    let outcome = preflight_stream_response(
        json_stream_response_with_status(reqwest::StatusCode::TOO_MANY_REQUESTS, body),
        "/v1/responses",
        false,
        true,
    );
    assert!(matches!(
        outcome,
        StreamPreflightOutcome::Failover(message) if message.contains("usage limit")
            || message == "usage_limit_reached"
    ));
}

#[test]
fn preflight_fails_over_on_any_non_2xx_when_more_candidates_exist() {
    let body = r#"{"error":{"message":"upstream temporarily unavailable"}}"#;
    let outcome = preflight_stream_response(
        json_stream_response_with_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR, body),
        "/v1/responses",
        false,
        true,
    );
    assert!(matches!(
        outcome,
        StreamPreflightOutcome::StatusFailover {
            status_code: 500,
            message,
        } if message.contains("status=500")
    ));
}

#[test]
fn preflight_delivers_2xx_success_status_when_more_candidates_exist() {
    let body = r#"{"id":"created_elsewhere"}"#;
    let outcome = preflight_stream_response(
        json_stream_response_with_status(reqwest::StatusCode::CREATED, body),
        "/v1/responses",
        false,
        true,
    );
    let StreamPreflightOutcome::Ready(response) = outcome else {
        panic!("2xx response must be delivered");
    };
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let (replayed, _) = response.into_buffered().expect("buffer JSON response");
    assert_eq!(replayed.as_ref(), body.as_bytes());
}

#[test]
fn preflight_delivers_json_usage_limit_error_when_no_more_candidates() {
    let body =
        r#"{"error":{"message":"The usage limit has been reached.","type":"usage_limit_reached"}}"#;
    let outcome = preflight_stream_response(
        json_stream_response_with_status(reqwest::StatusCode::TOO_MANY_REQUESTS, body),
        "/v1/responses",
        false,
        false,
    );
    let StreamPreflightOutcome::Ready(response) = outcome else {
        panic!("last candidate error must be delivered");
    };
    assert_eq!(response.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    let (replayed, _) = response.into_buffered().expect("buffer JSON response");
    assert_eq!(replayed.as_ref(), body.as_bytes());
}

#[test]
fn preflight_suppresses_actionable_usage_limit() {
    let body = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"You've hit your usage limit. Try again later.\"}\n\n",
        "data: [DONE]\n\n"
    );
    assert!(matches!(
        preflight_stream_response(stream_response(body), "/v1/responses", true, true),
        StreamPreflightOutcome::RetryUsageNotice(message) if message.contains("usage limit")
    ));
}

#[test]
fn preflight_retry_cancels_the_discarded_upstream_producer() {
    let body = Bytes::from_static(
        concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"You've hit your usage limit. Try again later.\"}\n\n",
            "data: [DONE]\n\n"
        )
        .as_bytes(),
    );
    let (tx, rx) = mpsc::sync_channel(2);
    tx.send(GatewayByteStreamItem::Chunk(body))
        .expect("queue quota notice");
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_receiver_with_cancel(rx, Some(cancel_tx)),
    ));

    assert!(matches!(
        preflight_stream_response(response, "/v1/responses", true, true),
        StreamPreflightOutcome::RetryUsageNotice(_)
    ));
    assert_eq!(cancel_rx.try_recv(), Ok(()));
}

#[test]
fn preflight_waits_beyond_legacy_two_second_window_for_quota_notice() {
    let metadata = Bytes::from_static(
        b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_delayed\"}}\n\n",
    );
    let quota_notice = Bytes::from_static(
        concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"You've hit your usage limit. Try again later.\"}\n\n",
            "data: [DONE]\n\n"
        )
        .as_bytes(),
    );
    let (tx, rx) = mpsc::sync_channel(2);
    tx.send(GatewayByteStreamItem::Chunk(metadata))
        .expect("queue response metadata");
    let producer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2_100));
        tx.send(GatewayByteStreamItem::Chunk(quota_notice))
            .expect("queue delayed quota notice");
    });
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_receiver(rx),
    ));

    assert!(matches!(
        preflight_stream_response_with_idle_timeout(
            response,
            "/v1/responses",
            true,
            true,
            Some(Duration::from_secs(5)),
        ),
        StreamPreflightOutcome::RetryUsageNotice(message) if message.contains("usage limit")
    ));
    producer.join().expect("join delayed quota producer");
}

#[test]
fn preflight_idle_before_deliverable_content_fails_over_and_cancels_upstream() {
    let metadata = Bytes::from_static(
        b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_idle\"}}\n\n",
    );
    let (tx, rx) = mpsc::sync_channel(1);
    tx.send(GatewayByteStreamItem::Chunk(metadata))
        .expect("queue response metadata");
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_receiver_with_cancel(rx, Some(cancel_tx)),
    ));

    assert!(matches!(
        preflight_stream_response_with_idle_timeout(
            response,
            "/v1/responses",
            true,
            true,
            Some(Duration::from_millis(25)),
        ),
        StreamPreflightOutcome::TransportFailover(message) if message.contains("idle timeout")
    ));
    assert_eq!(cancel_rx.try_recv(), Ok(()));
    drop(tx);
}

#[test]
fn preflight_wall_clock_cap_commits_slow_stream_without_losing_prefix() {
    let metadata = Bytes::from_static(
        b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_slow\"}}\n\n",
    );
    let output = Bytes::from_static(
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\ndata: [DONE]\n\n",
    );
    let expected = [metadata.as_ref(), output.as_ref()].concat();
    let (tx, rx) = mpsc::sync_channel(2);
    tx.send(GatewayByteStreamItem::Chunk(metadata))
        .expect("queue response metadata");
    let producer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        tx.send(GatewayByteStreamItem::Chunk(output))
            .expect("queue delayed output");
        tx.send(GatewayByteStreamItem::Eof)
            .expect("queue delayed output EOF");
    });
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_receiver_with_cancel(rx, Some(cancel_tx)),
    ));

    let started_at = std::time::Instant::now();
    let outcome = preflight_stream_response_with_timeouts(
        response,
        "/v1/responses",
        true,
        true,
        Some(Duration::from_secs(1)),
        Some(Duration::from_millis(25)),
        false,
    );
    assert!(started_at.elapsed() < Duration::from_millis(500));
    let StreamPreflightOutcome::Ready(response) = outcome else {
        panic!("wall-clock cap must commit a normal slow stream");
    };
    assert!(matches!(
        cancel_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    producer.join().expect("join delayed output producer");
    let (replayed, _) = response.into_buffered().expect("buffer replayed response");
    assert_eq!(replayed.as_ref(), expected.as_slice());
}

#[test]
fn preflight_prefix_limit_commits_large_metadata_without_losing_bytes() {
    let body = format!(
        "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_large\",\"metadata\":{{\"padding\":\"{}\"}}}}}}\n\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}}\n\ndata: [DONE]\n\n",
        "x".repeat(STREAM_PREFLIGHT_MAX_BYTES + 1024),
    );
    let response = stream_response_from_items(vec![
        GatewayByteStreamItem::Chunk(Bytes::copy_from_slice(body.as_bytes())),
        GatewayByteStreamItem::Eof,
    ]);

    let outcome = preflight_stream_response(response, "/v1/responses", true, true);
    let StreamPreflightOutcome::Ready(response) = outcome else {
        panic!("classification prefix limit must commit the original stream");
    };
    let (replayed, _) = response.into_buffered().expect("buffer replayed response");
    assert_eq!(replayed.as_ref(), body.as_bytes());
}

#[test]
fn preflight_fails_over_on_read_error_before_deliverable_content() {
    let metadata = Bytes::from_static(
        b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
    );
    let truncated_event =
        Bytes::from_static(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial");
    let response = stream_response_from_items(vec![
        GatewayByteStreamItem::Chunk(metadata),
        GatewayByteStreamItem::Chunk(truncated_event),
        GatewayByteStreamItem::Error("connection reset".to_string()),
    ]);

    assert!(matches!(
        preflight_stream_response(response, "/v1/responses", true, true),
        StreamPreflightOutcome::TransportFailover(message)
            if message.contains("connection reset")
    ));
}

#[test]
fn preflight_fails_over_when_producer_disconnects_after_metadata() {
    let metadata = Bytes::from_static(
        b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
    );
    let response = stream_response_from_items(vec![GatewayByteStreamItem::Chunk(metadata)]);

    assert!(matches!(
        preflight_stream_response(response, "/v1/responses", true, true),
        StreamPreflightOutcome::TransportFailover(message)
            if message.contains("disconnected")
    ));
}

const OVERLOAD_EVENT: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_overloaded\",\"output\":[]}}\n\n",
    "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"output\":[],\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"Selected model is at capacity.\"}}}\n\n"
);

fn assert_ready_preserves_body(outcome: StreamPreflightOutcome, expected: &[u8]) {
    let StreamPreflightOutcome::Ready(response) = outcome else {
        panic!("response must be delivered without transparent retry");
    };
    let (body, _) = response.into_buffered().expect("read preserved response");
    assert_eq!(body.as_ref(), expected);
}

fn response_with_retry_after(value: &str) -> GatewayUpstreamResponse {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(
        reqwest::header::RETRY_AFTER,
        HeaderValue::from_str(value).expect("valid header bytes"),
    );
    GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_bytes(Bytes::from_static(OVERLOAD_EVENT.as_bytes())),
    ))
}

#[test]
fn overload_preflight_recognizes_only_structured_error_codes() {
    let errors = [
        r#"{"type":"response.failed","response":{"error":{"code":"server_is_overloaded"}}}"#,
        r#"{"type":"response.failed","error":{"code":"server_is_overloaded"}}"#,
        r#"{"type":"response.incomplete","response":{"status_details":{"error":{"code":"server_is_overloaded"}}}}"#,
        r#"{"type":"error","code":"server_is_overloaded"}"#,
    ];
    for payload in errors {
        let frame = format!("data: {payload}\n\n");
        assert_eq!(
            super::classify_prefix(frame.as_bytes(), false, true),
            PrefixDecision::Overload,
            "structured shape: {payload}"
        );
    }
    assert_eq!(
        super::classify_prefix(
            b"event: error\ndata: {\"code\":\"server_is_overloaded\"}\n\n",
            false,
            true,
        ),
        PrefixDecision::Overload
    );
}

#[test]
fn overload_preflight_does_not_match_body_words_or_unstructured_messages() {
    for payload in [
        r#"{"type":"response.output_text.delta","delta":"server_is_overloaded"}"#,
        r#"{"type":"response.failed","response":{"error":{"message":"server_is_overloaded"}}}"#,
        r#"{"type":"response.failed","response":{"error":"server_is_overloaded"}}"#,
        r#"{"type":"response.failed","error":{"code":"other_error","message":"Selected model is at capacity."}}"#,
        r#"{"type":"response.failed","error":{"code":"SERVER_IS_OVERLOADED"}}"#,
        r#"{"type":"response.failed","error":{"code":" server_is_overloaded "}}"#,
    ] {
        let frame = format!("data: {payload}\n\n");
        assert_eq!(
            super::classify_prefix(frame.as_bytes(), false, true),
            PrefixDecision::Deliver,
            "must not broaden the exact error-code allowlist: {payload}"
        );
    }
    assert_eq!(
        super::classify_prefix(
            b"data: {\"type\":\"response.created\",\"response\":{\"metadata\":{\"error\":{\"code\":\"server_is_overloaded\"}}}}\n\n",
            false,
            true,
        ),
        PrefixDecision::NeedMore
    );
}

#[test]
fn overload_preflight_commits_tool_starts_before_later_error_in_same_chunk() {
    for tool_kind in [
        "function_call",
        "custom_tool_call",
        "web_search_call",
        "file_search_call",
        "computer_call",
        "code_interpreter_call",
        "mcp_call",
    ] {
        let body = format!(
            "data: {{\"type\":\"response.output_item.added\",\"item\":{{\"type\":\"{tool_kind}\",\"id\":\"tool_1\"}}}}\n\n{OVERLOAD_EVENT}"
        );
        assert_eq!(
            super::classify_prefix(body.as_bytes(), false, true),
            PrefixDecision::Deliver,
            "tool start must commit the stream: {tool_kind}"
        );
    }
}

#[test]
fn overload_preflight_commits_visible_content_and_reasoning_before_error() {
    for payload in [
        r#"{"type":"response.output_text.delta","delta":"hello"}"#,
        r#"{"type":"response.reasoning_summary_text.delta","delta":"thinking"}"#,
        r#"{"type":"response.reasoning_text.delta","delta":"thinking"}"#,
        r#"{"type":"response.function_call_arguments.delta","delta":"{}"}"#,
        r#"{"type":"response.custom_tool_call_input.delta","delta":"print(1)"}"#,
        r#"{"type":"response.output_item.added","item":{"type":"message","content":[{"type":"output_text","text":"hello"}]}}"#,
        r#"{"type":"response.content_part.added","part":{"type":"output_text","text":"hello"}}"#,
        r#"{"type":"response.reasoning_summary_part.added","part":{"type":"summary_text","text":"thinking"}}"#,
        r#"{"type":"response.created","response":{"output":[{"type":"function_call"}]}}"#,
    ] {
        let body = format!("data: {payload}\n\n{OVERLOAD_EVENT}");
        assert_eq!(
            super::classify_prefix(body.as_bytes(), false, true),
            PrefixDecision::Deliver,
            "content boundary: {payload}"
        );
    }
}

#[test]
fn overload_preflight_does_not_retry_failed_response_with_output_snapshot() {
    for output in [
        r#"[{"type":"function_call","name":"write_file","arguments":"{}"}]"#,
        r#"[{"type":"message","content":[{"type":"output_text","text":"hello"}]}]"#,
        r#"[{"type":"reasoning","summary":[]}]"#,
    ] {
        let frame = format!(
            "data: {{\"type\":\"response.failed\",\"response\":{{\"output\":{output},\"error\":{{\"code\":\"server_is_overloaded\"}}}}}}\n\n"
        );
        assert_eq!(
            super::classify_prefix(frame.as_bytes(), false, true),
            PrefixDecision::Deliver
        );
    }
}

#[test]
fn overload_preflight_holds_only_empty_item_and_part_metadata() {
    let body = format!(
        "data: {{\"type\":\"response.output_item.added\",\"item\":{{\"type\":\"message\",\"content\":[]}}}}\n\ndata: {{\"type\":\"response.content_part.added\",\"part\":{{\"type\":\"output_text\",\"text\":\"\"}}}}\n\n{OVERLOAD_EVENT}"
    );
    assert_eq!(
        super::classify_prefix(body.as_bytes(), false, true),
        PrefixDecision::Overload
    );
}

#[test]
fn overload_preflight_does_not_replay_held_quota_notice_text() {
    for text in ["You", "You've hit your usage limit. Try again later."] {
        let delta = serde_json::json!({"type": "response.output_text.delta", "delta": text});
        let body = format!("data: {delta}\n\n{OVERLOAD_EVENT}");
        assert_eq!(
            super::classify_prefix(body.as_bytes(), false, true),
            PrefixDecision::Deliver
        );
    }
}

#[test]
fn overload_preflight_commits_malformed_metadata_before_later_error() {
    for data in ["null", "[]", "\"metadata\""] {
        let body = format!("event: response.created\ndata: {data}\n\n{OVERLOAD_EVENT}");
        assert_eq!(
            super::classify_prefix(body.as_bytes(), false, true),
            PrefixDecision::Deliver
        );
    }
}

#[test]
fn overload_preflight_preserves_error_when_no_backup_or_permission() {
    for (has_more, allowed) in [(false, true), (true, false), (false, false)] {
        assert_ready_preserves_body(
            preflight_stream_response_with_overload_failover(
                stream_response(OVERLOAD_EVENT),
                "/v1/responses",
                true,
                has_more,
                allowed,
            ),
            OVERLOAD_EVENT.as_bytes(),
        );
    }
}

#[test]
fn overload_preflight_retains_original_response_until_coordinator_decides() {
    let (tx, rx) = mpsc::sync_channel(2);
    tx.send(GatewayByteStreamItem::Chunk(Bytes::from_static(
        OVERLOAD_EVENT.as_bytes(),
    )))
    .expect("queue overload");
    tx.send(GatewayByteStreamItem::Eof).expect("queue EOF");
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_receiver_with_cancel(rx, Some(cancel_tx)),
    ));
    let StreamPreflightOutcome::OverloadFailover {
        message,
        retry_after,
        response,
    } = preflight_stream_response_with_overload_failover(
        response,
        "/v1/responses",
        true,
        true,
        true,
    )
    else {
        panic!("expected structured overload decision");
    };
    assert!(message.contains("code=server_is_overloaded"));
    assert_eq!(retry_after, None);
    assert!(matches!(
        cancel_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    // If the coordinator's deadline expired during prefetch, the bytes remain
    // available for the original response instead of synthesizing a timeout.
    let (body, _) = response.into_buffered().expect("preserve overload bytes");
    assert_eq!(body.as_ref(), OVERLOAD_EVENT.as_bytes());
    drop(tx);
}

#[test]
fn overload_preflight_cancels_upstream_when_coordinator_drops_failed_response() {
    let (tx, rx) = mpsc::sync_channel(1);
    tx.send(GatewayByteStreamItem::Chunk(Bytes::from_static(
        OVERLOAD_EVENT.as_bytes(),
    )))
    .expect("queue overload");
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_receiver_with_cancel(rx, Some(cancel_tx)),
    ));
    let StreamPreflightOutcome::OverloadFailover { response, .. } =
        preflight_stream_response_with_overload_failover(
            response,
            "/v1/responses",
            true,
            true,
            true,
        )
    else {
        panic!("expected structured overload decision");
    };
    drop(response);
    assert_eq!(cancel_rx.try_recv(), Ok(()));
    drop(tx);
}

#[test]
fn overload_preflight_keeps_nonstream_and_chat_completions_unchanged() {
    for (path, is_stream) in [
        ("/v1/responses", false),
        ("/v1/chat/completions", true),
        ("/v1/chat/completions", false),
    ] {
        assert_ready_preserves_body(
            preflight_stream_response_with_overload_failover(
                stream_response(OVERLOAD_EVENT),
                path,
                is_stream,
                true,
                true,
            ),
            OVERLOAD_EVENT.as_bytes(),
        );
    }
    let json = r#"{"error":{"code":"server_is_overloaded"}}"#;
    assert_ready_preserves_body(
        preflight_stream_response_with_overload_failover(
            json_stream_response(json),
            "/v1/responses",
            true,
            true,
            true,
        ),
        json.as_bytes(),
    );
}

#[test]
fn overload_preflight_does_not_expand_existing_http_status_retry_policy() {
    for status in [
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let outcome = preflight_stream_response_with_overload_failover(
            json_stream_response_with_status(
                status,
                r#"{"error":{"code":"server_is_overloaded"}}"#,
            ),
            "/v1/responses",
            true,
            true,
            true,
        );
        assert!(matches!(
            outcome,
            StreamPreflightOutcome::StatusFailover { status_code, .. }
                if status_code == status.as_u16()
        ));
    }
}

#[test]
fn overload_preflight_obeys_short_retry_after_and_preserves_long_or_invalid() {
    for (header, seconds) in [("0", 0), ("1", 1), ("2", 2)] {
        let outcome = preflight_stream_response_with_overload_failover(
            response_with_retry_after(header),
            "/v1/responses",
            true,
            true,
            true,
        );
        assert!(matches!(
            outcome,
            StreamPreflightOutcome::OverloadFailover { retry_after: Some(delay), .. }
                if delay == Duration::from_secs(seconds)
        ));
    }
    for header in ["3", "120", "later", "-1", "1.5", "99999999999999999999999"] {
        assert_ready_preserves_body(
            preflight_stream_response_with_overload_failover(
                response_with_retry_after(header),
                "/v1/responses",
                true,
                true,
                true,
            ),
            OVERLOAD_EVENT.as_bytes(),
        );
    }
}

#[test]
fn overload_preflight_parses_retry_after_http_date_and_duplicate_headers() {
    use chrono::{DateTime, Utc};
    use reqwest::header::RETRY_AFTER;
    let now = DateTime::parse_from_rfc3339("2026-09-16T10:00:00Z")
        .expect("fixed UTC clock")
        .with_timezone(&Utc);
    for (value, seconds) in [
        ("Wed, 16 Sep 2026 10:00:02 GMT", 2),
        ("Wed, 16 Sep 2026 09:59:59 GMT", 0),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static(value));
        assert_eq!(
            stream_overload::retry_after_delay(&headers, now),
            Ok(Some(Duration::from_secs(seconds)))
        );
    }
    let mut headers = HeaderMap::new();
    headers.append(RETRY_AFTER, HeaderValue::from_static("0"));
    headers.append(RETRY_AFTER, HeaderValue::from_static("10"));
    assert_eq!(stream_overload::retry_after_delay(&headers, now), Err(()));
}

#[test]
fn overload_preflight_reassembles_fragmented_error_without_losing_fallback() {
    let split = OVERLOAD_EVENT.len() - 17;
    let response = stream_response_from_items(vec![
        GatewayByteStreamItem::Chunk(Bytes::copy_from_slice(
            &OVERLOAD_EVENT.as_bytes()[..split],
        )),
        GatewayByteStreamItem::Chunk(Bytes::copy_from_slice(
            &OVERLOAD_EVENT.as_bytes()[split..],
        )),
        GatewayByteStreamItem::Eof,
    ]);
    let StreamPreflightOutcome::OverloadFailover { response, .. } =
        preflight_stream_response_with_overload_failover(
            response,
            "/v1/responses",
            true,
            true,
            true,
        )
    else {
        panic!("fragmented structured error should be detected");
    };
    let (body, _) = response.into_buffered().expect("replay error prefix");
    assert_eq!(body.as_ref(), OVERLOAD_EVENT.as_bytes());
}

#[test]
fn overload_preflight_wall_clock_limit_commits_later_overload_without_retry() {
    let metadata = b"data: {\"type\":\"response.created\",\"response\":{\"output\":[]}}\n\n";
    let (tx, rx) = mpsc::sync_channel(3);
    tx.send(GatewayByteStreamItem::Chunk(Bytes::from_static(metadata)))
        .expect("queue metadata");
    let producer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        tx.send(GatewayByteStreamItem::Chunk(Bytes::from_static(
            OVERLOAD_EVENT.as_bytes(),
        )))
        .expect("queue delayed overload");
        tx.send(GatewayByteStreamItem::Eof).expect("queue EOF");
    });
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        headers,
        GatewayByteStream::from_receiver(rx),
    ));
    let outcome = preflight_stream_response_with_timeouts(
        response,
        "/v1/responses",
        true,
        true,
        Some(Duration::from_secs(1)),
        Some(Duration::from_millis(25)),
        true,
    );
    producer.join().expect("join delayed error producer");
    assert_ready_preserves_body(
        outcome,
        &[metadata.as_slice(), OVERLOAD_EVENT.as_bytes()].concat(),
    );
}

#[test]
fn overload_preflight_prefix_limit_commits_error_after_large_metadata() {
    let body = format!(
        "data: {{\"type\":\"response.created\",\"response\":{{\"metadata\":{{\"padding\":\"{}\"}}}}}}\n\n{OVERLOAD_EVENT}",
        "x".repeat(STREAM_PREFLIGHT_MAX_BYTES + 1024)
    );
    let response = stream_response_from_items(vec![
        GatewayByteStreamItem::Chunk(Bytes::copy_from_slice(body.as_bytes())),
        GatewayByteStreamItem::Eof,
    ]);
    assert_ready_preserves_body(
        preflight_stream_response_with_overload_failover(
            response,
            "/v1/responses",
            true,
            true,
            true,
        ),
        body.as_bytes(),
    );
}

#[test]
fn dynamic_preflight_can_refresh_an_initially_single_candidate_pool() {
    let outcome = preflight_stream_response_with_dynamic_failover(
        stream_response(OVERLOAD_EVENT), "/v1/responses", true, false, true, true,
    );
    let StreamPreflightOutcome::OverloadFailover { response, .. } = outcome else {
        panic!("dynamic coordinator must be able to refresh candidates");
    };
    let (body, _) = response.into_buffered().unwrap();
    assert_eq!(body.as_ref(), OVERLOAD_EVENT.as_bytes());
}

fn sniff_response(header: Option<&str>, body: &[u8], retry_after: Option<&str>) -> GatewayUpstreamResponse {
    let mut headers = HeaderMap::new();
    if let Some(header) = header {
        headers.insert(CONTENT_TYPE, HeaderValue::from_str(header).unwrap());
    }
    if let Some(value) = retry_after {
        headers.insert(reqwest::header::RETRY_AFTER, HeaderValue::from_str(value).unwrap());
    }
    GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK, headers,
        GatewayByteStream::from_bytes(Bytes::copy_from_slice(body)),
    ))
}

#[test]
fn dynamic_sniff_retries_headerless_and_mislabelled_sse_overload_preserving_bytes() {
    for header in [None, Some("application/json"), Some("application/octet-stream"), Some("Text/Event-Stream")] {
        let outcome = preflight_stream_response_with_dynamic_failover(
            sniff_response(header, OVERLOAD_EVENT.as_bytes(), None),
            "/v1/responses", true, false, true, true,
        );
        let StreamPreflightOutcome::OverloadFailover { response, .. } = outcome else {
            panic!("structured SSE overload must qualify: header={header:?}");
        };
        let (bytes, _) = response.into_buffered().unwrap();
        assert_eq!(bytes.as_ref(), OVERLOAD_EVENT.as_bytes());
    }
}

#[test]
fn dynamic_sniff_preserves_json_plain_text_and_partial_field_prefixes() {
    let bodies = [
        "{\"error\":{\"code\":\"server_is_overloaded\"}}",
        " [{\"type\":\"response.failed\",\"error\":{\"code\":\"server_is_overloaded\"}}]",
        "plain server_is_overloaded text", "eventual plaintext", "data without colon",
        "data", "eve", "re", "i", "\u{feff}", " \r\n\t", "",
    ];
    for header in [None, Some("application/json"), Some("text/event-stream")] {
        for body in bodies {
            assert_ready_preserves_body(preflight_stream_response_with_dynamic_failover(
                sniff_response(header, body.as_bytes(), None),
                "/v1/responses", true, true, true, true,
            ), body.as_bytes());
        }
    }
}

#[test]
fn dynamic_sniff_never_skips_plaintext_before_an_embedded_sse_error() {
    for prefix in ["plain output\n\n", "eventual text\n\n", "[not-json]\n\n", "data without colon\n\n",
                   ": comment\nordinary visible text\n\n", "id: synthetic\nordinary visible text\n\n"] {
        let body = format!("{prefix}{OVERLOAD_EVENT}");
        assert_ready_preserves_body(preflight_stream_response_with_dynamic_failover(
            sniff_response(None, body.as_bytes(), None),
            "/v1/responses", true, true, true, true,
        ), body.as_bytes());
    }
}

#[test]
fn dynamic_sniff_handles_bytewise_bom_whitespace_and_all_standard_field_prefixes() {
    for first_field in ["", ": comment\r\n", "id: synthetic\r\n", "retry: 1000\r\n", "event: response.failed\r\n"] {
        let body = format!(" \r\n\u{feff}\t{first_field}data: {{\"type\":\"response.failed\",\"response\":{{\"error\":{{\"code\":\"server_is_overloaded\"}}}}}}\r\n\r\n");
        let (tx, rx) = mpsc::sync_channel(body.len() + 1);
        for byte in body.bytes() { tx.send(GatewayByteStreamItem::Chunk(Bytes::from(vec![byte]))).unwrap(); }
        tx.send(GatewayByteStreamItem::Eof).unwrap();
        drop(tx);
        let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
            reqwest::StatusCode::OK, HeaderMap::new(), GatewayByteStream::from_receiver(rx),
        ));
        let outcome = preflight_stream_response_with_dynamic_failover(
            response, "/v1/responses", true, true, true, true,
        );
        let StreamPreflightOutcome::OverloadFailover { response, .. } = outcome else {
            panic!("fragmented SSE prefix must be recognized: {first_field:?}");
        };
        let (bytes, _) = response.into_buffered().unwrap();
        assert_eq!(bytes.as_ref(), body.as_bytes());
    }
}

#[test]
fn dynamic_sniff_never_replays_text_tool_or_unknown_events_before_overload() {
    for content in [
        r#"{"type":"response.output_text.delta","delta":"already produced"}"#,
        r#"{"type":"response.output_item.added","item":{"type":"function_call","name":"synthetic_tool","arguments":""}}"#,
        r#"{"type":"response.unknown_event"}"#,
    ] {
        let body = format!("data: {content}\n\n{OVERLOAD_EVENT}");
        for header in [None, Some("application/json")] {
            assert_ready_preserves_body(preflight_stream_response_with_dynamic_failover(
                sniff_response(header, body.as_bytes(), None),
                "/v1/responses", true, true, true, true,
            ), body.as_bytes());
        }
    }
}

#[test]
fn dynamic_sniff_respects_feature_replay_permission_stream_and_retry_after_gates() {
    for header in [None, Some("application/json")] {
        for (is_stream, allowed, enabled) in [(true, true, false), (true, false, true), (false, true, true)] {
            assert_ready_preserves_body(preflight_stream_response_with_dynamic_failover(
                sniff_response(header, OVERLOAD_EVENT.as_bytes(), None),
                "/v1/responses", is_stream, true, allowed, enabled,
            ), OVERLOAD_EVENT.as_bytes());
        }
        for retry_after in ["3", "120", "later", "-1"] {
            assert_ready_preserves_body(preflight_stream_response_with_dynamic_failover(
                sniff_response(header, OVERLOAD_EVENT.as_bytes(), Some(retry_after)),
                "/v1/responses", true, true, true, true,
            ), OVERLOAD_EVENT.as_bytes());
        }
        assert_ready_preserves_body(preflight_stream_response_with_dynamic_failover(
            sniff_response(header, OVERLOAD_EVENT.as_bytes(), None),
            "/v1/chat/completions", true, true, true, true,
        ), OVERLOAD_EVENT.as_bytes());
    }
}

#[test]
fn dynamic_sniff_keeps_the_single_prefix_budget() {
    for prefix in [
        " ".repeat(STREAM_PREFLIGHT_MAX_BYTES + 10),
        format!("data: {{\"type\":\"response.created\",\"response\":{{\"metadata\":{{\"padding\":\"{}\"}}}}}}\n\n", "x".repeat(STREAM_PREFLIGHT_MAX_BYTES)),
    ] {
        let body = format!("{prefix}{OVERLOAD_EVENT}");
        assert_ready_preserves_body(preflight_stream_response_with_dynamic_failover(
            sniff_response(None, body.as_bytes(), None),
            "/v1/responses", true, true, true, true,
        ), body.as_bytes());
    }
}

#[test]
fn dynamic_sniff_commits_json_immediately_without_waiting_for_eof() {
    let body = b"{\"error\":{\"code\":\"server_is_overloaded\"}}";
    let (tx, rx) = mpsc::sync_channel(2);
    tx.send(GatewayByteStreamItem::Chunk(Bytes::from_static(body))).unwrap();
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK, HeaderMap::new(), GatewayByteStream::from_receiver(rx),
    ));
    let started = std::time::Instant::now();
    let outcome = preflight_stream_response_with_dynamic_timeouts(
        response, "/v1/responses", true, true, Some(Duration::from_secs(1)),
        Some(Duration::from_secs(1)), true, true,
    );
    assert!(started.elapsed() < Duration::from_millis(250));
    tx.send(GatewayByteStreamItem::Eof).unwrap();
    assert_ready_preserves_body(outcome, body);
}

#[test]
fn dynamic_sniff_does_not_restart_the_wall_clock_while_waiting_for_overload() {
    let metadata = b"data: {\"type\":\"response.created\",\"response\":{\"output\":[]}}\n\n";
    let (tx, rx) = mpsc::sync_channel(3);
    tx.send(GatewayByteStreamItem::Chunk(Bytes::from_static(metadata))).unwrap();
    let producer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        tx.send(GatewayByteStreamItem::Chunk(Bytes::from_static(OVERLOAD_EVENT.as_bytes()))).unwrap();
        tx.send(GatewayByteStreamItem::Eof).unwrap();
    });
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK, HeaderMap::new(), GatewayByteStream::from_receiver(rx),
    ));
    let outcome = preflight_stream_response_with_dynamic_timeouts(
        response, "/v1/responses", true, true, Some(Duration::from_secs(1)),
        Some(Duration::from_millis(25)), true, true,
    );
    producer.join().unwrap();
    assert_ready_preserves_body(outcome, &[metadata.as_slice(), OVERLOAD_EVENT.as_bytes()].concat());
}
