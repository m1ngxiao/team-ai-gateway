use super::*;
use crate::gateway::upstream::{GatewayByteStream, GatewayByteStreamItem, GatewayStreamResponse};
use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde_json::json;
use std::sync::mpsc;
use std::time::Duration;

fn response(body: GatewayByteStream) -> GatewayStreamResponse {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    GatewayStreamResponse::new(reqwest::StatusCode::OK, headers, body)
}

fn chunk(value: Value) -> GatewayByteStreamItem {
    GatewayByteStreamItem::Chunk(Bytes::from(format!("data: {value}\n\n")))
}

fn parse_chat_chunks(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).expect("parse chat chunk"))
        .collect()
}

#[test]
fn chat_reader_emits_role_on_created_and_uses_dense_tool_indices() {
    let body = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_dense\",\"model\":\"gpt-test\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":5,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_a\",\"name\":\"read_file\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":5,\"delta\":\"{\\\"path\\\":\"}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":9,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_b\",\"name\":\"write_file\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":9,\"delta\":\"{\\\"path\\\":\\\"out.txt\\\"}\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_dense\",\"model\":\"gpt-test\",\"output\":[],\"usage\":{\"input_tokens\":7,\"output_tokens\":3,\"total_tokens\":10}}}\n\n"
    );
    let collector = Arc::new(Mutex::new(PassthroughSseCollector::default()));
    let mut reader = ChatCompletionsFromResponsesSseReader::from_stream_response(
        response(GatewayByteStream::from_bytes(Bytes::from_static(
            body.as_bytes(),
        ))),
        Arc::clone(&collector),
        Instant::now(),
    );
    let mut output = String::new();
    reader.read_to_string(&mut output).unwrap();
    let chunks = parse_chat_chunks(output.as_str());

    assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
    assert_eq!(chunks[0]["choices"][0]["delta"]["content"], "");

    let tool_chunks = chunks
        .iter()
        .filter_map(|chunk| chunk["choices"][0]["delta"]["tool_calls"].as_array())
        .flatten()
        .collect::<Vec<_>>();
    let call_a = tool_chunks
        .iter()
        .find(|call| call["id"] == "call_a")
        .expect("first tool addition");
    let call_b = tool_chunks
        .iter()
        .find(|call| call["id"] == "call_b")
        .expect("second tool addition");
    assert_eq!(call_a["index"], 0);
    assert_eq!(call_a["function"]["arguments"], "");
    assert_eq!(call_b["index"], 1);
    assert_eq!(call_b["function"]["arguments"], "");

    let argument_chunks = tool_chunks
        .iter()
        .filter(|call| call.get("id").is_none())
        .collect::<Vec<_>>();
    assert!(argument_chunks
        .iter()
        .any(|call| { call["index"] == 0 && call["function"]["arguments"] == "{\"path\":" }));
    assert!(argument_chunks.iter().any(|call| {
        call["index"] == 1 && call["function"]["arguments"] == "{\"path\":\"out.txt\"}"
    }));
    assert!(chunks.iter().any(|chunk| {
        chunk["choices"][0]["finish_reason"] == "tool_calls"
            && chunk["usage"]["prompt_tokens"] == 7
            && chunk["usage"]["completion_tokens"] == 3
    }));
    assert!(output.ends_with("data: [DONE]\n\n"));
}

#[test]
fn async_chat_reader_delivers_tool_delta_before_upstream_completion() {
    let _env = crate::test_env_guard();
    let (tx, rx) = mpsc::sync_channel(8);
    let collector = Arc::new(Mutex::new(PassthroughSseCollector::default()));
    let mut reader = ChatCompletionsFromResponsesSseReader::from_stream_response(
        response(GatewayByteStream::from_receiver(rx)),
        Arc::clone(&collector),
        Instant::now(),
    );
    let (seen_tx, seen_rx) = mpsc::channel();
    let client = std::thread::spawn(move || {
        let mut output = String::new();
        loop {
            let mut buf = [0; 4096];
            let read = reader.read(&mut buf).expect("read incremental chat chunk");
            assert!(
                read > 0,
                "stream ended before the held completion was released"
            );
            output.push_str(std::str::from_utf8(&buf[..read]).unwrap());
            if output.contains("synthetic-argument-marker") {
                seen_tx.send(output.clone()).unwrap();
                break;
            }
        }
        reader
            .read_to_string(&mut output)
            .expect("read completion after client ACK");
        output
    });
    // Keep the upstream open and silent: polling must not synthesize EOF.
    assert!(matches!(
        seen_rx.recv_timeout(Duration::from_millis(150)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    tx.send(chunk(json!({"type":"response.created","response":{"id":"resp_incremental","model":"synthetic-model"}}))).unwrap();
    tx.send(chunk(json!({"type":"response.output_item.added","output_index":0,
        "item":{"type":"function_call","id":"fc_item","call_id":"call_incremental","name":"lookup","arguments":""}}))).unwrap();
    tx.send(chunk(
        json!({"type":"response.function_call_arguments.delta","output_index":0,
        "delta":"{\"q\":\"synthetic-argument-marker\"}"}),
    ))
    .unwrap();
    let first = seen_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("tool delta must precede completion");
    assert!(first.contains("lookup") && first.contains("call_incremental"));
    assert!(first.contains("tool_calls"));
    assert!(!first.contains("[DONE]"));
    // This event does not exist until the client has acknowledged the tool delta.
    tx.send(chunk(json!({"type":"response.completed","response":{"id":"resp_incremental",
        "model":"synthetic-model","output":[],"usage":{"input_tokens":3,"output_tokens":4,"total_tokens":7}}}))).unwrap();
    tx.send(GatewayByteStreamItem::Eof).unwrap();
    let output = client.join().expect("join chat client");
    assert!(output.contains("\"finish_reason\":\"tool_calls\""));
    assert!(output.contains("[DONE]"));
    assert_eq!(output.matches("synthetic-argument-marker").count(), 1);
    let usage = collector.lock().unwrap();
    assert!(usage.saw_terminal);
    assert_eq!(usage.usage.total_tokens, Some(7));
    assert!(usage.terminal_error.is_none());
}

#[test]
fn dropping_async_chat_reader_cancels_silent_upstream() {
    let (tx, rx) = mpsc::sync_channel(1);
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    let reader = ChatCompletionsFromResponsesSseReader::from_stream_response(
        response(GatewayByteStream::from_receiver_with_cancel(
            rx,
            Some(cancel_tx),
        )),
        Arc::new(Mutex::new(PassthroughSseCollector::default())),
        Instant::now(),
    );
    assert!(matches!(
        cancel_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    drop(reader);
    let started = Instant::now();
    loop {
        match cancel_rx.try_recv() {
            Ok(()) => break,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                assert!(
                    started.elapsed() < Duration::from_secs(2),
                    "idle upstream was not cancelled"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("cancellation sender lost without signal: {error}"),
        }
    }
    // Producer remains alive throughout: cancellation is caused by reader drop,
    // not by an artificial upstream EOF or a test timeout.
    drop(tx);
}

#[test]
fn async_chat_reader_propagates_transport_error_into_collector() {
    let (tx, rx) = mpsc::sync_channel(1);
    tx.send(GatewayByteStreamItem::Error(
        "synthetic read timeout".to_string(),
    ))
    .unwrap();
    let collector = Arc::new(Mutex::new(PassthroughSseCollector::default()));
    let mut reader = ChatCompletionsFromResponsesSseReader::from_stream_response(
        response(GatewayByteStream::from_receiver(rx)),
        Arc::clone(&collector),
        Instant::now(),
    );
    let mut output = String::new();
    reader.read_to_string(&mut output).unwrap();
    assert!(output.is_empty());
    assert!(collector.lock().unwrap().terminal_error.is_some());
}

#[test]
fn async_chat_reader_delivers_terminal_error_and_marks_failure() {
    let body = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"synthetic capacity failure\"}}}\n\n";
    let collector = Arc::new(Mutex::new(PassthroughSseCollector::default()));
    let mut reader = ChatCompletionsFromResponsesSseReader::from_stream_response(
        response(GatewayByteStream::from_bytes(Bytes::from_static(
            body.as_bytes(),
        ))),
        Arc::clone(&collector),
        Instant::now(),
    );
    let mut output = String::new();
    reader.read_to_string(&mut output).unwrap();
    assert!(output.contains("server_is_overloaded"));
    assert!(output.contains("synthetic capacity failure"));
    assert!(!output.contains("finish_reason"));
    let state = collector.lock().unwrap();
    assert!(state.saw_terminal);
    assert!(state
        .terminal_error
        .as_deref()
        .unwrap()
        .contains("server_is_overloaded"));
    assert_eq!(state.last_event_type.as_deref(), Some("response.failed"));
}

#[test]
fn async_chat_output_budget_keeps_partial_text_or_tool_and_finishes_with_length() {
    for tool in [false, true] {
        let prefix = if tool {
            format!(
                "data: {}\n\ndata: {}\n\n",
                json!({"type":"response.output_item.added","output_index":0,
                    "item":{"type":"function_call","call_id":"call_limit","name":"lookup","arguments":""}}),
                json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"partial-marker\":"})
            )
        } else {
            format!(
                "data: {}\n\n",
                json!({"type":"response.output_text.delta","delta":"partial-marker"})
            )
        };
        let terminal = json!({"type":"response.incomplete","response":{"id":"resp_limit",
            "status":"incomplete","error":null,"output_text":if tool {""} else {"partial-marker"},
            "incomplete_details":{"reason":"max_output_tokens"},
            "usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}});
        let collector = Arc::new(Mutex::new(PassthroughSseCollector::default()));
        let body = Bytes::from(format!("{prefix}data: {terminal}\n\n"));
        let mut reader = ChatCompletionsFromResponsesSseReader::from_stream_response(
            response(GatewayByteStream::from_bytes(body)),
            Arc::clone(&collector),
            Instant::now(),
        );
        let mut output = String::new();
        reader.read_to_string(&mut output).unwrap();
        assert_eq!(output.matches("partial-marker").count(), 1);
        assert!(output.contains("\"finish_reason\":\"length\""));
        assert!(!output.contains("\"finish_reason\":\"tool_calls\""));
        assert!(!output.contains("\"error\":"));
        assert!(output.contains("[DONE]"));
        let state = collector.lock().unwrap();
        assert!(state.saw_terminal);
        assert!(state.terminal_error.is_none());
        assert_eq!(
            state.last_event_type.as_deref(),
            Some("response.incomplete")
        );
        assert_eq!(state.usage.total_tokens, Some(5));
    }
}
