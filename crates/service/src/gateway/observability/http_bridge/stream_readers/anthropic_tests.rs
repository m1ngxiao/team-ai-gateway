use super::*;
use crate::gateway::anthropic_safeguards::{
    available_results, SafeguardEvaluator, SafeguardToolUse,
};
use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

struct StaticSafeguardEvaluator;

impl SafeguardEvaluator for StaticSafeguardEvaluator {
    fn evaluate(&self, tool_uses: &[SafeguardToolUse]) -> serde_json::Value {
        available_results(
            tool_uses
                .iter()
                .map(|tool_use| {
                    (
                        tool_use.id.clone(),
                        json!({"type": "evaluated", "outcome": "not_flagged"}),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        )
    }
}

#[test]
fn streamed_tool_use_gets_safeguard_results_in_message_delta() {
    let payload = concat!(
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"toolu_1\",\"name\":\"Bash\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-6-astra\",\"status\":\"completed\"}}\n\n"
    );
    let evaluator: Arc<dyn SafeguardEvaluator> = Arc::new(StaticSafeguardEvaluator);
    let mut reader = AnthropicSseReader::from_reader_with_safeguards(
        Cursor::new(payload.as_bytes().to_vec()),
        Arc::new(Mutex::new(UpstreamResponseUsage::default())),
        Some("gpt-6-astra"),
        None,
        Instant::now(),
        Some(evaluator),
    );
    let mut output = String::new();
    reader.read_to_string(&mut output).expect("read stream");
    let delta = output
        .split("\n\n")
        .filter_map(|frame| {
            frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .and_then(|data| serde_json::from_str::<Value>(data).ok())
        })
        .find(|value| value.get("type").and_then(Value::as_str) == Some("message_delta"))
        .expect("message delta");
    assert_eq!(
        delta["delta"]["safeguard_results"][0]["status"]["tool_uses"]["toolu_1"]["outcome"],
        "not_flagged"
    );
}

#[test]
fn streamed_compaction_injects_block_context_management_and_iteration_usage() {
    let payload = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"continued\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-6-astra\",\"status\":\"completed\",\"usage\":{\"input_tokens\":20,\"output_tokens\":5}}}\n\n"
    );
    let enrichment = std::sync::Arc::new(
        crate::gateway::anthropic_context_management::AnthropicContextEnrichment {
            applied_edits: vec![json!({
                "type": "compact_20260112",
                "summary_input_tokens": 120,
                "summary_output_tokens": 30
            })],
            compaction: Some(
                crate::gateway::anthropic_context_management::CompactionBlock {
                    summary: "fixed parser; tests pass".to_string(),
                    input_tokens: 120,
                    output_tokens: 30,
                },
            ),
        },
    );
    let mut reader = AnthropicSseReader::from_reader_with_context_enrichment(
        Cursor::new(payload.as_bytes().to_vec()),
        Arc::new(Mutex::new(UpstreamResponseUsage::default())),
        Some("gpt-6-astra"),
        None,
        Instant::now(),
        None,
        Some(enrichment),
    );
    let mut output = String::new();
    reader.read_to_string(&mut output).expect("read stream");
    assert!(output.contains("\"type\":\"compaction\""));
    assert!(output.contains("\"type\":\"compaction_delta\""));
    assert!(output.contains("fixed parser; tests pass"));
    let message_delta = output
        .split("\n\n")
        .filter_map(|frame| {
            frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .and_then(|data| serde_json::from_str::<Value>(data).ok())
        })
        .find(|value| value.get("type").and_then(Value::as_str) == Some("message_delta"))
        .expect("message delta");
    assert_eq!(
        message_delta["context_management"]["applied_edits"][0]["type"],
        "compact_20260112"
    );
    assert_eq!(
        message_delta["usage"]["iterations"][0]["type"],
        "compaction"
    );
    assert_eq!(message_delta["usage"]["iterations"][1]["type"], "message");
}

#[test]
fn failed_or_truncated_stream_never_fabricates_successful_summary() {
    for ending in ["", "data: {\"type\":\"response.failed\"}\n\n"] {
        let payload = format!(
            "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}}\n\n{ending}"
        );
        let usage = Arc::new(Mutex::new(UpstreamResponseUsage::default()));
        let mut reader = AnthropicSseReader::from_reader(
            Cursor::new(payload.into_bytes()),
            Arc::clone(&usage),
            None,
            None,
            Instant::now(),
        );
        let mut output = String::new();
        assert!(reader.read_to_string(&mut output).is_err());
        assert!(output.contains("partial"));
        assert!(output.contains("event: error"));
        assert!(!output.contains("event: message_stop"));
        assert!(!output.contains("event: message_delta"));
        assert!(usage.lock().unwrap().output_tokens.is_none());
    }
}

#[test]
fn output_limit_is_a_valid_stop_but_cancellation_is_an_error() {
    for (status, success) in [("incomplete", true), ("cancelled", false)] {
        let value = json!({"type":"response.incomplete", "response":{
            "status":status, "error":null,
            "incomplete_details":{"reason":"max_output_tokens"},
            "usage":{"input_tokens":10,"output_tokens":20}
        }});
        let payload = format!("data: {value}\n\n");
        let mut reader = AnthropicSseReader::from_reader(
            Cursor::new(payload.into_bytes()),
            Arc::new(Mutex::new(UpstreamResponseUsage::default())),
            None,
            None,
            Instant::now(),
        );
        let mut output = String::new();
        assert_eq!(reader.read_to_string(&mut output).is_ok(), success);
        assert_eq!(output.contains("event: message_stop"), success);
        assert_eq!(output.contains("event: error"), !success);
        if success {
            assert!(output.contains("\"stop_reason\":\"max_tokens\""));
        }
    }
}

#[test]
fn timed_out_or_incomplete_stream_emits_error_after_partial_output() {
    let _guard = crate::test_env_guard();
    struct TimeoutGuard(Option<std::ffi::OsString>);
    impl Drop for TimeoutGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("CODEXMANAGER_UPSTREAM_STREAM_TIMEOUT_MS", value),
                None => std::env::remove_var("CODEXMANAGER_UPSTREAM_STREAM_TIMEOUT_MS"),
            }
            crate::gateway::reload_runtime_config_from_env();
        }
    }
    let _timeout_guard = TimeoutGuard(std::env::var_os("CODEXMANAGER_UPSTREAM_STREAM_TIMEOUT_MS"));
    std::env::set_var("CODEXMANAGER_UPSTREAM_STREAM_TIMEOUT_MS", "10");
    let _keepalive_guard = super::super::SseKeepaliveRuntimeGuard::enabled_with_interval(1);
    let mut reader = AnthropicSseReader::from_reader(
        PausingReader::new(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        ),
        Arc::new(Mutex::new(UpstreamResponseUsage::default())),
        None,
        None,
        Instant::now(),
    );
    let mut output = String::new();
    assert!(reader.read_to_string(&mut output).is_err());
    assert!(output.contains("partial"));
    assert!(
        output.contains("upstream stream idle timeout")
            || output.contains("upstream stream ended before completion")
    );
    assert!(!output.contains("event: message_stop"));
}

#[test]
fn upstream_comment_heartbeat_precedes_slow_model_content() {
    // The producer stays open until the downstream has received the heartbeat.
    // This detects aggregation without waiting for a real proxy timeout.
    struct GatedReader(std::sync::mpsc::Receiver<Vec<u8>>, Cursor<Vec<u8>>);
    impl Read for GatedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            loop {
                let n = self.1.read(buf)?;
                if n > 0 {
                    return Ok(n);
                }
                match self.0.recv() {
                    Ok(bytes) => self.1 = Cursor::new(bytes),
                    Err(_) => return Ok(0),
                }
            }
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (seen_tx, seen_rx) = std::sync::mpsc::channel();
    let join = thread::spawn(move || {
        let usage = Arc::new(Mutex::new(UpstreamResponseUsage::default()));
        let mut reader = AnthropicSseReader::from_reader(
            GatedReader(rx, Cursor::new(Vec::new())),
            Arc::clone(&usage),
            Some("test-model"),
            None,
            Instant::now(),
        );
        let mut buf = [0; 1024];
        let n = reader.read(&mut buf).unwrap();
        seen_tx.send(buf[..n].to_vec()).unwrap();
        assert!(usage.lock().unwrap().output_tokens.is_none());
        let mut rest = String::new();
        reader.read_to_string(&mut rest).unwrap();
        rest
    });
    tx.send(b": private upstream heartbeat\r\n\r\n".to_vec())
        .unwrap();
    let first = seen_rx.recv_timeout(Duration::from_secs(2));
    // Release the producer even on failure, so a failing regression cannot hang.
    tx.send(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"summary\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n".to_vec()).unwrap();
    drop(tx);
    let rest = join.join().unwrap();
    assert_eq!(first.unwrap(), SseKeepAliveFrame::Anthropic.bytes());
    assert!(rest.contains("summary"));
    assert!(rest.contains("message_stop"));
    assert!(!rest.contains("private upstream heartbeat"));
}

#[test]
fn comment_with_data_does_not_replace_model_delta_with_ping() {
    let mut reader = AnthropicSseReader::from_reader(
        Cursor::new(Vec::<u8>::new()),
        Arc::new(Mutex::new(UpstreamResponseUsage::default())),
        None,
        None,
        Instant::now(),
    );
    let chunk = reader.process_sse_frame(&[
        ": comment".into(),
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}".into(),
    ]);
    let text = String::from_utf8(chunk).unwrap();
    assert!(text.contains("hello"));
    assert!(!text.contains("event: ping"));
}

struct PausingReader {
    payload: Cursor<Vec<u8>>,
    paused: bool,
}

impl PausingReader {
    fn new(payload: &str) -> Self {
        Self {
            payload: Cursor::new(payload.as_bytes().to_vec()),
            paused: false,
        }
    }
}

impl Read for PausingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.payload.read(buf)?;
        if read > 0 {
            return Ok(read);
        }
        if !self.paused {
            self.paused = true;
            thread::sleep(Duration::from_millis(200));
        }
        Ok(0)
    }
}

struct ChannelReader {
    receiver: std::sync::mpsc::Receiver<Vec<u8>>,
    pending: Cursor<Vec<u8>>,
}

impl ChannelReader {
    fn new(receiver: std::sync::mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            receiver,
            pending: Cursor::new(Vec::new()),
        }
    }
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let read = self.pending.read(buf)?;
            if read > 0 {
                return Ok(read);
            }
            match self.receiver.recv() {
                Ok(bytes) => self.pending = Cursor::new(bytes),
                Err(_) => return Ok(0),
            }
        }
    }
}

#[test]
fn anthropic_reader_keepalive_precedes_delayed_first_upstream_frame() {
    let _guard = crate::test_env_guard();
    let _runtime_guard = super::super::SseKeepaliveRuntimeGuard::enabled_with_interval(10);
    let (tx, rx) = std::sync::mpsc::channel();
    let usage = Arc::new(Mutex::new(UpstreamResponseUsage::default()));
    let mut reader = AnthropicSseReader::from_reader(
        ChannelReader::new(rx),
        Arc::clone(&usage),
        Some("fallback-model"),
        None,
        Instant::now(),
    );
    let mut buf = [0_u8; 4096];

    let read = reader.read(&mut buf).expect("read pre-frame keepalive");
    assert_eq!(&buf[..read], SseKeepAliveFrame::Anthropic.bytes());
    assert!(usage.lock().unwrap().first_response_ms.is_none());

    tx.send(
        b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_delayed\",\"model\":\"gpt-6-astra\"}}\n\n"
            .to_vec(),
    )
    .expect("send delayed first frame");
    let read = reader.read(&mut buf).expect("read message start");
    let first = std::str::from_utf8(&buf[..read]).expect("utf8");
    assert!(first.contains("event: message_start"));
    assert!(first.contains("\"id\":\"resp_delayed\""));
    assert!(usage.lock().unwrap().first_response_ms.is_some());

    tx.send(
        concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"delayed result\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_delayed\",\"model\":\"gpt-6-astra\",\"status\":\"completed\"}}\n\n"
        )
        .as_bytes()
        .to_vec(),
    )
    .expect("send delayed result");
    drop(tx);
    let mut rest = String::new();
    reader
        .read_to_string(&mut rest)
        .expect("read delayed stream");
    assert!(rest.contains("delayed result"));
    assert!(rest.contains("event: message_stop"));
}

#[test]
fn metadata_only_upstream_frame_starts_message_without_keepalive_race() {
    let _guard = crate::test_env_guard();
    let _runtime_guard = super::super::SseKeepaliveRuntimeGuard::disabled();
    let upstream = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_first\",\"model\":\"gpt-5.4\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_first\",\"model\":\"gpt-5.4\",\"status\":\"completed\"}}\n\n",
    );
    let usage_collector = Arc::new(Mutex::new(UpstreamResponseUsage::default()));
    let mut reader = AnthropicSseReader::from_reader(
        PausingReader::new(upstream),
        Arc::clone(&usage_collector),
        Some("fallback-model"),
        None,
        Instant::now(),
    );
    let mut output = String::new();
    reader.read_to_string(&mut output).expect("read stream");
    assert_eq!(output.matches("event: message_start").count(), 1);
    assert!(output.contains("\"id\":\"resp_first\""));
    assert!(output.contains("\"model\":\"gpt-5.4\""));
    let usage = usage_collector.lock().expect("usage lock").clone();
    assert!(usage.first_response_ms.is_some());
}

#[test]
fn reasoning_only_frames_do_not_suppress_keepalive_or_delay_final_output() {
    let _guard = crate::test_env_guard();
    let _runtime_guard = super::super::SseKeepaliveRuntimeGuard::enabled_with_interval(5);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut reader = AnthropicSseReader::from_reader(
        ChannelReader::new(rx),
        Arc::new(Mutex::new(UpstreamResponseUsage::default())),
        Some("fallback-model"),
        None,
        Instant::now(),
    );
    let mut buf = [0_u8; 4096];

    tx.send(
        b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_reasoning\",\"model\":\"gpt-5.6-sol\"}}\n\n"
            .to_vec(),
    )
    .expect("send response metadata");
    let read = reader.read(&mut buf).expect("read initial message start");
    let first = std::str::from_utf8(&buf[..read]).expect("utf8").to_string();
    assert!(first.contains("event: message_start"));
    assert!(first.contains("resp_reasoning"));

    reader.last_client_write = Instant::now()
        .checked_sub(sse_keepalive_interval())
        .expect("keepalive interval fits in Instant");
    tx.send(
        b"data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"still reasoning\"}\n\n"
            .to_vec(),
    )
    .expect("send reasoning delta");
    let read = reader
        .read(&mut buf)
        .expect("read keepalive after reasoning");
    assert_eq!(&buf[..read], SseKeepAliveFrame::Anthropic.bytes());

    tx.send(
        concat!(
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"more reasoning\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"compaction summary\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_reasoning\",\"model\":\"gpt-5.6-sol\",\"status\":\"completed\",\"usage\":{\"input_tokens\":100,\"output_tokens\":10}}}\n\n"
        )
        .as_bytes()
        .to_vec(),
    )
    .expect("send final output");
    drop(tx);
    let mut rest = String::new();
    reader.read_to_string(&mut rest).expect("read final stream");

    assert_eq!(rest.matches("event: message_start").count(), 0);
    assert!(rest.contains("compaction summary"));
    assert!(rest.contains("event: message_stop"));
    assert_eq!(first.matches("event: message_start").count(), 1);
}
