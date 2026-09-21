use std::sync::mpsc;
use std::time::Duration;

use bytes::Bytes;

use super::*;
use crate::gateway::http_bridge::{
    OpenAIResponsesPassthroughSseReader, PassthroughSseCollector, SseKeepAliveFrame,
};

#[test]
fn prefetch_caps_the_classification_copy_and_replays_the_full_chunk() {
    let body = Bytes::from(vec![b'x'; 128 * 1024]);
    let stream = GatewayByteStream::from_bytes(body.clone());

    let (prefix, replayed, terminal) =
        stream.prefetch_until(64 * 1024, Some(Duration::from_secs(1)), None, |_| false);

    assert_eq!(prefix.len(), 64 * 1024);
    assert_eq!(terminal, GatewayStreamPrefetchTerminal::PrefixLimit);
    assert_eq!(replayed.read_all_bytes().expect("replayed body"), body);
}

#[test]
fn prefetch_reports_eof_without_losing_the_buffered_body() {
    let body = Bytes::from_static(b"metadata only");
    let stream = GatewayByteStream::from_bytes(body.clone());

    let (prefix, replayed, terminal) =
        stream.prefetch_until(1024, Some(Duration::from_secs(1)), None, |_| false);

    assert_eq!(prefix, body);
    assert_eq!(terminal, GatewayStreamPrefetchTerminal::Eof);
    assert_eq!(replayed.read_all_bytes().expect("replayed body"), body);
}

#[test]
fn prefetch_reports_and_replays_stream_errors() {
    let (tx, rx) = mpsc::sync_channel(2);
    tx.send(GatewayByteStreamItem::Chunk(Bytes::from_static(
        b"metadata",
    )))
    .expect("send metadata");
    tx.send(GatewayByteStreamItem::Error("upstream reset".to_string()))
        .expect("send stream error");
    let stream = GatewayByteStream::from_receiver(rx);

    let (prefix, replayed, terminal) =
        stream.prefetch_until(1024, Some(Duration::from_secs(1)), None, |_| false);

    assert_eq!(prefix.as_ref(), b"metadata");
    assert_eq!(
        terminal,
        GatewayStreamPrefetchTerminal::Error("upstream reset".to_string())
    );
    assert_eq!(replayed.read_all_bytes(), Err("upstream reset".to_string()));
}

#[test]
fn prefetch_distinguishes_a_disconnected_producer_from_clean_eof() {
    let (tx, rx) = mpsc::sync_channel(1);
    drop(tx);
    let stream = GatewayByteStream::from_receiver(rx);

    let (prefix, _replayed, terminal) =
        stream.prefetch_until(1024, Some(Duration::from_secs(1)), None, |_| false);

    assert!(prefix.is_empty());
    assert_eq!(terminal, GatewayStreamPrefetchTerminal::Disconnected);
}

#[test]
fn prefetch_wall_clock_timeout_is_not_reset_by_activity_and_replays_all_bytes() {
    let (tx, rx) = mpsc::sync_channel(32);
    let producer = std::thread::spawn(move || {
        let mut expected = Vec::new();
        for index in 0..20 {
            let chunk = format!("chunk-{index};").into_bytes();
            expected.extend_from_slice(chunk.as_slice());
            tx.send(GatewayByteStreamItem::Chunk(Bytes::from(chunk)))
                .expect("send active stream chunk");
            std::thread::sleep(Duration::from_millis(5));
        }
        tx.send(GatewayByteStreamItem::Eof)
            .expect("send active stream EOF");
        expected
    });
    let stream = GatewayByteStream::from_receiver(rx);

    let started_at = std::time::Instant::now();
    let (_prefix, replayed, terminal) = stream.prefetch_until(
        1024,
        Some(Duration::from_secs(1)),
        Some(Duration::from_millis(30)),
        |_| false,
    );

    assert_eq!(terminal, GatewayStreamPrefetchTerminal::WallClockTimeout);
    assert!(started_at.elapsed() < Duration::from_millis(750));
    let expected = producer.join().expect("join active stream producer");
    assert_eq!(replayed.read_all_bytes().expect("replayed body"), expected);
}

#[test]
fn prefetch_idle_timeout_wins_before_later_wall_clock_timeout() {
    let (_tx, rx) = mpsc::sync_channel(1);
    let stream = GatewayByteStream::from_receiver(rx);

    let (_prefix, _replayed, terminal) = stream.prefetch_until(
        1024,
        Some(Duration::from_millis(20)),
        Some(Duration::from_millis(200)),
        |_| false,
    );

    assert_eq!(terminal, GatewayStreamPrefetchTerminal::IdleTimeout);
}

#[test]
fn dropping_a_stream_signals_its_upstream_producer_to_cancel() {
    let (_tx, rx) = mpsc::sync_channel(1);
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    let stream = GatewayByteStream::from_receiver_with_cancel(rx, Some(cancel_tx));

    drop(stream);

    assert_eq!(cancel_rx.try_recv(), Ok(()));
}

#[test]
fn dropping_both_tee_consumers_cancels_a_silent_upstream() {
    let (_source_tx, source_rx) = mpsc::sync_channel(1);
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    let source = GatewayByteStream::from_receiver_with_cancel(source_rx, Some(cancel_tx));
    let (left, right) = source.tee();

    drop(left);
    drop(right);

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match cancel_rx.try_recv() {
            Ok(()) => break,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                if std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            other => panic!("tee relay did not cancel silent upstream: {other:?}"),
        }
    }
}

#[test]
fn dropping_openai_responses_reader_cancels_sidecar_and_silent_upstream() {
    let (_source_tx, source_rx) = mpsc::sync_channel(1);
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    let source = GatewayByteStream::from_receiver_with_cancel(source_rx, Some(cancel_tx));
    let response = GatewayStreamResponse::new(
        reqwest::StatusCode::OK,
        reqwest::header::HeaderMap::new(),
        source,
    );
    let reader = OpenAIResponsesPassthroughSseReader::from_stream_response(
        response,
        std::sync::Arc::new(std::sync::Mutex::new(PassthroughSseCollector::default())),
        SseKeepAliveFrame::Comment,
        std::time::Instant::now(),
    );

    drop(reader);

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match cancel_rx.try_recv() {
            Ok(()) => break,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                if std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            other => panic!("responses reader did not cancel silent upstream: {other:?}"),
        }
    }
}

fn response_with_health_fixture(content_type: Option<&str>, bytes: Bytes) -> GatewayUpstreamResponse {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(value) = content_type {
        headers.insert(reqwest::header::CONTENT_TYPE, reqwest::header::HeaderValue::from_str(value).unwrap());
    }
    GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK, headers, GatewayByteStream::from_bytes(bytes),
    ))
}

#[test]
fn attached_health_detects_sse_independently_of_missing_or_incorrect_content_type() {
    let bytes = Bytes::from_static(b"data: {\"type\":\"response.created\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n");
    for header in [None, Some("text/event-stream"), Some("Text/Event-Stream; charset=UTF-8"),
                   Some("application/json"), Some("application/octet-stream")] {
        let (response, observer) = response_with_health_fixture(header, bytes.clone()).observe_health();
        let (body, _) = response.into_buffered().unwrap();
        assert_eq!(body, bytes);
        let health = observer.snapshot();
        assert!(health.completed && !health.failed && !health.overloaded, "header={header:?}");
        assert_eq!(health.format, "sse");
        assert_eq!(health.bytes_seen, bytes.len() as u64);
        assert_eq!(health.parsed_values, 2);
        assert_eq!(health.parsed_frames, 2);
    }
}

#[test]
fn attached_health_preserves_json_errors_even_with_an_sse_header() {
    let bytes = Bytes::from_static(b" \n{\"error\":{\"code\":\"server_is_overloaded\"}}");
    for header in [None, Some("application/json"), Some("text/event-stream")] {
        let (response, observer) = response_with_health_fixture(header, bytes.clone()).observe_health();
        response.into_buffered().unwrap();
        let health = observer.snapshot();
        assert!(health.overloaded && health.failed && !health.completed);
        assert_eq!(health.format, "json");
        assert_eq!(health.bytes_seen, bytes.len() as u64);
        assert_eq!(health.parsed_values, 1);
        assert_eq!(health.parsed_frames, 0);
    }
}

#[test]
fn attached_health_observes_late_sse_overload_despite_an_incorrect_header() {
    let bytes = Bytes::from_static(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_is_overloaded\"}}}\n\n");
    let (response, observer) = response_with_health_fixture(Some("application/json"), bytes).observe_health();
    response.into_buffered().unwrap();
    let health = observer.snapshot();
    assert!(health.overloaded && health.failed && !health.completed);
    assert_eq!(health.parsed_frames, 2);
}

#[test]
fn attached_health_follows_prefetch_and_tee_without_double_counting_replay() {
    let first = Bytes::from_static(b"data: {\"type\":\"response.created\"}\n\ndata: {\"type\":\"response.comp");
    let second = Bytes::from_static(b"leted\"}\r\n\r\n");
    let (tx, rx) = mpsc::sync_channel(3);
    tx.send(GatewayByteStreamItem::Chunk(first.clone())).unwrap();
    tx.send(GatewayByteStreamItem::Chunk(second.clone())).unwrap();
    tx.send(GatewayByteStreamItem::Eof).unwrap();
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK, reqwest::header::HeaderMap::new(), GatewayByteStream::from_receiver(rx),
    ));
    let (response, observer) = response.observe_health();
    let (_, response, _) = response.prefetch_stream_prefix(64 * 1024,
        Some(Duration::from_secs(1)), None, |bytes| !bytes.is_empty());
    assert_eq!(observer.snapshot().parsed_frames, 1);
    let GatewayUpstreamResponse::Stream(response) = response else { panic!("expected stream"); };
    let (left, right) = response.into_body().tee();
    let body = left.read_all_bytes().unwrap();
    assert_eq!(body, right.read_all_bytes().unwrap());
    assert_eq!(body.len(), first.len() + second.len());
    let health = observer.snapshot();
    assert!(health.completed && !health.failed);
    assert_eq!(health.bytes_seen, body.len() as u64);
    assert_eq!(health.parsed_values, 2);
    assert_eq!(health.parsed_frames, 2);
}

#[test]
fn attached_health_producer_close_flushes_complete_json_but_not_partial_or_error() {
    for (body, complete) in [
        (b"{\"status\":\"completed\"}".as_slice(), true),
        (b"{\"status\":\"completed\"", false),
        (b"{\"status\":\"in_progress\"}", false),
    ] {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(GatewayByteStreamItem::Chunk(Bytes::copy_from_slice(body))).unwrap();
        drop(tx);
        let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
            reqwest::StatusCode::OK, reqwest::header::HeaderMap::new(), GatewayByteStream::from_receiver(rx),
        ));
        let (response, observer) = response.observe_health();
        response.into_buffered().unwrap();
        assert_eq!(observer.snapshot().completed, complete);
    }
    let (tx, rx) = mpsc::sync_channel(2);
    tx.send(GatewayByteStreamItem::Chunk(Bytes::from_static(b"{\"status\":\"completed\"}"))).unwrap();
    tx.send(GatewayByteStreamItem::Error("synthetic read failure".into())).unwrap();
    drop(tx);
    let response = GatewayUpstreamResponse::Stream(GatewayStreamResponse::new(
        reqwest::StatusCode::OK, reqwest::header::HeaderMap::new(), GatewayByteStream::from_receiver(rx),
    ));
    let (response, observer) = response.observe_health();
    assert!(response.into_buffered().is_err());
    assert!(!observer.snapshot().completed);
    assert_eq!(observer.snapshot().parsed_values, 0);
}
