//! Observe semantic completion independently of delivery and replay eligibility.
//! Only structured error codes are used; response text is never retained in logs.
use serde_json::Value;
use std::sync::{Arc, Mutex};

const MAX_FRAME_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SemanticHealth {
    pub overloaded: bool,
    pub completed: bool,
    pub failed: bool,
    pub bytes_seen: u64,
    pub parsed_values: u64,
    pub parsed_frames: u64,
    pub format: &'static str,
}

impl Default for SemanticHealth {
    fn default() -> Self {
        Self { overloaded: false, completed: false, failed: false,
            bytes_seen: 0, parsed_values: 0, parsed_frames: 0, format: "unknown" }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireFormat {
    Unknown,
    Json,
    Sse,
}

#[derive(Debug)]
struct ObserverState {
    health: SemanticHealth,
    format: WireFormat,
    pending: Vec<u8>,
    oversized: bool,
    previous_newline: bool,
    format_prefix: Vec<u8>,
    bom_skipped: bool,
    finished: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct HealthObserver(Arc<Mutex<ObserverState>>);

impl HealthObserver {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(ObserverState {
            health: SemanticHealth::default(), format: WireFormat::Unknown, pending: Vec::new(),
            oversized: false, previous_newline: false,
            format_prefix: Vec::new(), bom_skipped: false, finished: false,
        })))
    }

    pub(crate) fn snapshot(&self) -> SemanticHealth {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).health
    }

    pub(crate) fn feed(&self, bytes: &[u8]) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.finished { return; }
        state.health.bytes_seen = state.health.bytes_seen.saturating_add(bytes.len() as u64);
        // Responses delivery already accepts SSE with missing or incorrect
        // content-type headers. Observe the actual framing too; never infer a
        // successful completion from HTTP headers or the delivery result.
        if state.format != WireFormat::Unknown {
            state.feed_detected(bytes);
            return;
        }
        for (index, &byte) in bytes.iter().enumerate() {
            if state.format_prefix.is_empty() && byte.is_ascii_whitespace() { continue; }
            state.format_prefix.push(byte);
            if !state.bom_skipped {
                // At most two bytes wait for a possible UTF-8 SSE BOM. Handle
                // a split BOM without buffering arbitrarily long whitespace.
                if matches!(state.format_prefix.as_slice(), [0xef] | [0xef, 0xbb]) { continue; }
                if state.format_prefix.as_slice() == [0xef, 0xbb, 0xbf] {
                    state.bom_skipped = true;
                    state.format_prefix.clear();
                    continue;
                }
            }
            state.format = if matches!(state.format_prefix[0], b'{' | b'[') { WireFormat::Json } else { WireFormat::Sse };
            state.health.format = match state.format { WireFormat::Json => "json", _ => "sse" };
            if state.bom_skipped && state.format == WireFormat::Json {
                // Keep JSON strict: unlike SSE, JSON does not accept a BOM.
                state.feed_detected(&[0xef, 0xbb, 0xbf]);
            }
            let prefix = std::mem::take(&mut state.format_prefix);
            state.feed_detected(&prefix);
            state.feed_detected(&bytes[index + 1..]);
            return;
        }
    }

    pub(crate) fn eof(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.finished { return; }
        if !state.oversized {
            if state.format == WireFormat::Sse { state.finish_frame(); }
            else if let Ok(value) = serde_json::from_slice::<Value>(&state.pending) {
                state.health.parsed_values = state.health.parsed_values.saturating_add(1);
                state.observe_value(&value, "");
            }
        }
        state.pending.clear();
        state.finished = true;
    }
}

impl ObserverState {
    fn feed_detected(&mut self, bytes: &[u8]) {
        if self.format == WireFormat::Json {
            if !self.oversized && self.pending.len().saturating_add(bytes.len()) <= MAX_FRAME_BYTES {
                self.pending.extend_from_slice(bytes);
            } else {
                self.oversized = true;
                self.pending.clear();
            }
            return;
        }
        for &byte in bytes {
            // SSE permits CRLF; normalizing also handles chunk-split CRLF.
            if byte == b'\r' { continue; }
            if byte == b'\n' && self.previous_newline {
                if !self.oversized { self.finish_frame(); }
                self.pending.clear();
                self.oversized = false;
                self.previous_newline = false;
                continue;
            }
            self.previous_newline = byte == b'\n';
            if !self.oversized {
                if self.pending.len() < MAX_FRAME_BYTES { self.pending.push(byte); }
                else { self.oversized = true; self.pending.clear(); }
            }
        }
    }

    fn finish_frame(&mut self) {
        let text = String::from_utf8_lossy(&self.pending);
        let mut data = String::new();
        let mut event = "";
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("event:") { event = value.trim(); }
            if let Some(value) = line.strip_prefix("data:") {
                if !data.is_empty() { data.push('\n'); }
                data.push_str(value.trim_start());
            }
        }
        let event = event.to_string();
        if let Ok(value) = serde_json::from_str::<Value>(&data) {
            self.health.parsed_values = self.health.parsed_values.saturating_add(1);
            self.health.parsed_frames = self.health.parsed_frames.saturating_add(1);
            self.observe_value(&value, &event);
        }
    }

    fn observe_value(&mut self, value: &Value, event: &str) {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or(event);
        let errors = [value.get("error"), value.pointer("/response/error"),
            value.pointer("/response/status_details/error"), (kind == "error").then_some(value)];
        for error in errors.into_iter().flatten().filter(|value| !value.is_null()) {
            self.health.failed = true;
            self.health.overloaded |= error.get("code").and_then(Value::as_str) == Some("server_is_overloaded");
        }
        self.health.failed |= matches!(kind, "response.failed" | "response.incomplete" | "error")
            || value.get("status").and_then(Value::as_str).is_some_and(|v| matches!(v, "failed" | "incomplete"));
        self.health.completed |= kind == "response.completed"
            || value.get("status").and_then(Value::as_str) == Some("completed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_health_observes_split_error_after_content() {
        let observer = HealthObserver::new();
        let body = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\r\n\r\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_is_overloaded\"}}}\r\n\r\n";
        for chunk in body.chunks(3) { observer.feed(chunk); }
        assert!(observer.snapshot().overloaded);
        assert!(observer.snapshot().failed);
        assert!(!observer.snapshot().completed);
    }

    #[test]
    fn semantic_health_text_mentions_are_not_errors() {
        let observer = HealthObserver::new();
        observer.feed(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"server_is_overloaded\"}\n\ndata: {\"type\":\"response.completed\"}\n\n");
        assert!(!observer.snapshot().overloaded);
        assert!(observer.snapshot().completed);
    }

    #[test]
    fn semantic_health_large_frame_keeps_bounded_and_reads_next_terminal() {
        let observer = HealthObserver::new();
        observer.feed(&vec![b'x'; MAX_FRAME_BYTES + 8]);
        observer.feed(b"\n\ndata: {\"type\":\"response.completed\"}\n\n");
        assert!(observer.snapshot().completed);
        assert!(observer.0.lock().unwrap().pending.len() <= MAX_FRAME_BYTES);
    }

    #[test]
    fn semantic_health_checks_all_structured_error_locations() {
        let observer = HealthObserver::new();
        observer.feed(b"data: {\"type\":\"response.failed\",\"error\":{\"message\":\"summary\"},\"response\":{\"error\":null,\"status_details\":{\"error\":{\"code\":\"server_is_overloaded\"}}}}\n\n");
        assert!(observer.snapshot().overloaded);
    }

    #[test]
    fn semantic_health_json_observes_structured_failure() {
        let observer = HealthObserver::new();
        observer.feed(b"{\"error\":{\"code\":\"server_is_overloaded\"}}");
        observer.eof();
        assert!(observer.snapshot().overloaded);
    }

    #[test]
    fn semantic_health_detects_json_after_split_whitespace_and_counts_values() {
        let observer = HealthObserver::new();
        for bytes in [b" \r".as_slice(), b"\n\t", b"{\"error\":", b"{\"code\":\"server_is_overloaded\"}}"] {
            observer.feed(bytes);
        }
        assert!(!observer.snapshot().overloaded, "JSON is classified only after complete EOF");
        observer.eof();
        let health = observer.snapshot();
        assert_eq!(health.format, "json");
        assert_eq!(health.parsed_values, 1);
        assert_eq!(health.parsed_frames, 0);
        assert!(health.overloaded && health.failed && !health.completed);
        observer.eof();
        assert_eq!(observer.snapshot().parsed_values, 1);
    }

    #[test]
    fn semantic_health_bytewise_crlf_sse_records_complete_semantics() {
        let observer = HealthObserver::new();
        let bytes = b" \t\r\nevent: response.completed\r\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\r\n\r\n";
        for byte in bytes { observer.feed(std::slice::from_ref(byte)); }
        observer.eof();
        let health = observer.snapshot();
        assert!(health.completed && !health.failed && !health.overloaded);
        assert_eq!(health.bytes_seen, bytes.len() as u64);
        assert_eq!(health.format, "sse");
        assert_eq!(health.parsed_values, 1);
        assert_eq!(health.parsed_frames, 1);
    }

    #[test]
    fn semantic_health_unknown_whitespace_and_large_json_keep_memory_bounded() {
        let observer = HealthObserver::new();
        observer.feed(&vec![b' '; MAX_FRAME_BYTES * 2]);
        assert_eq!(observer.snapshot().format, "unknown");
        assert!(observer.0.lock().unwrap().pending.is_empty());
        observer.feed(b"{\"status\":\"completed\",\"padding\":\"");
        observer.feed(&vec![b'x'; MAX_FRAME_BYTES]);
        observer.feed(b"\"}");
        observer.eof();
        let health = observer.snapshot();
        assert_eq!(health.format, "json");
        assert_eq!(health.parsed_values, 0);
        assert!(!health.completed, "oversized JSON must not certify recovery");
        assert!(observer.0.lock().unwrap().pending.len() <= MAX_FRAME_BYTES);
    }

    #[test]
    fn semantic_health_array_is_json_but_cannot_certify_response_completion() {
        let observer = HealthObserver::new();
        observer.feed(b" [ {\"type\":\"response.completed\"} ]");
        observer.eof();
        assert_eq!(observer.snapshot().format, "json");
        assert_eq!(observer.snapshot().parsed_values, 1);
        assert!(!observer.snapshot().completed);
    }

    #[test]
    fn semantic_health_invalid_data_does_not_trust_completed_event_name() {
        let observer = HealthObserver::new();
        observer.feed(b"event: response.completed\ndata: invalid-json\n\n");
        observer.eof();
        assert_eq!(observer.snapshot().format, "sse");
        assert_eq!(observer.snapshot().parsed_values, 0);
        assert!(!observer.snapshot().completed);
    }

    #[test]
    fn semantic_health_split_bom_and_sse_field_prefixes_preserve_single_frame_errors() {
        for prefix in ["", ": heartbeat\r\n", "id: synthetic\r\n", "retry: 1000\r\n", "event: response.failed\r\n"] {
            let observer = HealthObserver::new();
            let bytes = format!("\u{feff}{prefix}data: {{\"type\":\"response.failed\",\"response\":{{\"error\":{{\"code\":\"server_is_overloaded\"}}}}}}\r\n\r\n").into_bytes();
            for byte in &bytes { observer.feed(std::slice::from_ref(byte)); }
            observer.eof();
            let health = observer.snapshot();
            assert!(health.overloaded && health.failed && !health.completed, "prefix={prefix:?}");
            assert_eq!(health.format, "sse");
            assert_eq!(health.parsed_frames, 1);
            assert_eq!(health.bytes_seen, bytes.len() as u64);
        }
    }

    #[test]
    fn semantic_health_json_bom_remains_conservative() {
        let observer = HealthObserver::new();
        for byte in b"\xef\xbb\xbf{\"status\":\"completed\"}" { observer.feed(std::slice::from_ref(byte)); }
        observer.eof();
        assert_eq!(observer.snapshot().format, "json");
        assert_eq!(observer.snapshot().parsed_values, 0);
        assert!(!observer.snapshot().completed);
    }
}
