use super::{
    append_output_text, collect_output_text_from_event_fields, collect_response_output_text, json,
    mark_first_response_ms_on_usage, sse_keepalive_interval, stream_idle_timed_out,
    stream_wait_timeout, Arc, Cursor, Map, Mutex, Read, SseKeepAliveFrame, UpstreamResponseUsage,
    UpstreamSseFramePump, UpstreamSseFramePumpItem, Value,
};
use crate::gateway::anthropic_context_management::AnthropicContextEnrichment;
use crate::gateway::anthropic_safeguards::{SafeguardEvaluator, SafeguardToolUse};
use std::time::{Duration, Instant};

pub(crate) struct AnthropicSseReader {
    upstream: UpstreamSseFramePump,
    out_cursor: Cursor<Vec<u8>>,
    state: AnthropicSseState,
    tool_name_restore_map: Option<crate::gateway::ToolNameRestoreMap>,
    usage_collector: Arc<Mutex<UpstreamResponseUsage>>,
    request_started_at: Instant,
    last_upstream_activity: Instant,
    last_client_write: Instant,
    saw_upstream_frame: bool,
    safeguards_evaluator: Option<Arc<dyn SafeguardEvaluator>>,
    context_enrichment: Option<Arc<AnthropicContextEnrichment>>,
    safeguard_tool_uses: Vec<SafeguardToolUse>,
}

#[derive(Default)]
struct AnthropicSseState {
    started: bool,
    finished: bool,
    failure: Option<String>,
    text_block_index: Option<usize>,
    next_block_index: usize,
    response_id: Option<String>,
    model: Option<String>,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    total_tokens: Option<i64>,
    reasoning_output_tokens: i64,
    output_text: String,
    emitted_text_to_client: bool,
    compaction_emitted: bool,
    stop_reason: Option<&'static str>,
}

impl AnthropicSseReader {
    pub(crate) fn from_reader<R>(
        upstream: R,
        usage_collector: Arc<Mutex<UpstreamResponseUsage>>,
        fallback_model: Option<&str>,
        tool_name_restore_map: Option<crate::gateway::ToolNameRestoreMap>,
        request_started_at: Instant,
    ) -> Self
    where
        R: Read + Send + 'static,
    {
        Self::from_reader_with_safeguards(
            upstream,
            usage_collector,
            fallback_model,
            tool_name_restore_map,
            request_started_at,
            None,
        )
    }

    pub(crate) fn from_reader_with_safeguards<R>(
        upstream: R,
        usage_collector: Arc<Mutex<UpstreamResponseUsage>>,
        fallback_model: Option<&str>,
        tool_name_restore_map: Option<crate::gateway::ToolNameRestoreMap>,
        request_started_at: Instant,
        safeguards_evaluator: Option<Arc<dyn SafeguardEvaluator>>,
    ) -> Self
    where
        R: Read + Send + 'static,
    {
        Self::from_reader_with_context_enrichment(
            upstream,
            usage_collector,
            fallback_model,
            tool_name_restore_map,
            request_started_at,
            safeguards_evaluator,
            None,
        )
    }

    pub(crate) fn from_reader_with_context_enrichment<R>(
        upstream: R,
        usage_collector: Arc<Mutex<UpstreamResponseUsage>>,
        fallback_model: Option<&str>,
        tool_name_restore_map: Option<crate::gateway::ToolNameRestoreMap>,
        request_started_at: Instant,
        safeguards_evaluator: Option<Arc<dyn SafeguardEvaluator>>,
        context_enrichment: Option<Arc<AnthropicContextEnrichment>>,
    ) -> Self
    where
        R: Read + Send + 'static,
    {
        let mut state = AnthropicSseState::default();
        state.model = fallback_model
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        Self {
            upstream: UpstreamSseFramePump::from_reader(upstream),
            out_cursor: Cursor::new(Vec::new()),
            state,
            tool_name_restore_map,
            usage_collector,
            request_started_at,
            last_upstream_activity: Instant::now(),
            last_client_write: Instant::now(),
            saw_upstream_frame: false,
            safeguards_evaluator,
            context_enrichment,
            safeguard_tool_uses: Vec::new(),
        }
    }

    /// 函数 `new`
    ///
    /// 作者: gaohongshun
    ///
    /// 时间: 2026-04-02
    ///
    /// # 参数
    /// - crate: 参数 crate
    ///
    /// # 返回
    /// 返回函数执行结果
    pub(crate) fn new(
        upstream: reqwest::blocking::Response,
        usage_collector: Arc<Mutex<UpstreamResponseUsage>>,
        fallback_model: Option<&str>,
        tool_name_restore_map: Option<crate::gateway::ToolNameRestoreMap>,
        request_started_at: Instant,
    ) -> Self {
        Self::from_reader(
            upstream,
            usage_collector,
            fallback_model,
            tool_name_restore_map,
            request_started_at,
        )
    }

    pub(crate) fn new_with_context_enrichment(
        upstream: reqwest::blocking::Response,
        usage_collector: Arc<Mutex<UpstreamResponseUsage>>,
        fallback_model: Option<&str>,
        tool_name_restore_map: Option<crate::gateway::ToolNameRestoreMap>,
        request_started_at: Instant,
        safeguards_evaluator: Option<Arc<dyn SafeguardEvaluator>>,
        context_enrichment: Option<Arc<AnthropicContextEnrichment>>,
    ) -> Self {
        Self::from_reader_with_context_enrichment(
            upstream,
            usage_collector,
            fallback_model,
            tool_name_restore_map,
            request_started_at,
            safeguards_evaluator,
            context_enrichment,
        )
    }

    /// 函数 `next_chunk`
    ///
    /// 作者: gaohongshun
    ///
    /// 时间: 2026-04-02
    ///
    /// # 参数
    /// - self: 参数 self
    ///
    /// # 返回
    /// 返回函数执行结果
    fn next_chunk(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            match self.upstream.recv_timeout(self.stream_read_timeout()) {
                Ok(UpstreamSseFramePumpItem::Frame(frame)) => {
                    self.last_upstream_activity = Instant::now();
                    self.saw_upstream_frame = true;
                    mark_first_response_ms_on_usage(&self.usage_collector, self.request_started_at);
                    let mapped = self.process_sse_frame(&frame);
                    if !mapped.is_empty() {
                        mark_first_response_ms_on_usage(
                            &self.usage_collector,
                            self.request_started_at,
                        );
                        self.last_client_write = Instant::now();
                        return Ok(mapped);
                    }
                    if !self.state.started {
                        let mut started = String::new();
                        self.ensure_message_start(&mut started);
                        if !started.is_empty() {
                            self.last_client_write = Instant::now();
                            return Ok(started.into_bytes());
                        }
                    }
                    if self.keepalive_due() {
                        self.last_client_write = Instant::now();
                        return Ok(SseKeepAliveFrame::Anthropic.bytes().to_vec());
                    }
                    continue;
                }
                Ok(UpstreamSseFramePumpItem::Eof) => {
                    if stream_idle_timed_out(self.last_upstream_activity) {
                        return Ok(self.fail_stream("upstream stream idle timeout"));
                    }
                    return Ok(self.fail_stream("upstream stream ended before completion"));
                }
                Ok(UpstreamSseFramePumpItem::Error(_err)) => {
                    return Ok(self.fail_stream("upstream stream read failed"));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if stream_idle_timed_out(self.last_upstream_activity) {
                        return Ok(self.fail_stream("upstream stream idle timeout"));
                    }
                    if self.keepalive_due() {
                        self.last_client_write = Instant::now();
                        return Ok(SseKeepAliveFrame::Anthropic.bytes().to_vec());
                    }
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Ok(self.fail_stream("upstream stream reader disconnected"));
                }
            }
        }
    }

    fn stream_read_timeout(&self) -> Duration {
        let upstream_timeout = stream_wait_timeout(self.last_upstream_activity);
        if !crate::gateway::current_sse_keepalive_enabled() {
            return upstream_timeout;
        }
        let keepalive_remaining = sse_keepalive_interval()
            .saturating_sub(self.last_client_write.elapsed())
            .max(Duration::from_millis(1));
        upstream_timeout.min(keepalive_remaining)
    }

    fn keepalive_due(&self) -> bool {
        crate::gateway::current_sse_keepalive_enabled()
            && self.last_client_write.elapsed() >= sse_keepalive_interval()
    }

    /// 函数 `process_sse_frame`
    ///
    /// 作者: gaohongshun
    ///
    /// 时间: 2026-04-02
    ///
    /// # 参数
    /// - self: 参数 self
    /// - lines: 参数 lines
    ///
    /// # 返回
    /// 返回函数执行结果
    fn process_sse_frame(&mut self, lines: &[String]) -> Vec<u8> {
        let mut data_lines = Vec::new();
        for line in lines {
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if let Some(rest) = trimmed.strip_prefix("data:") {
                data_lines.push(rest.trim_start().to_string());
            }
        }
        if data_lines.is_empty() {
            // SSE comments are upstream liveness, even while the model is reasoning.
            // Translate comment-only frames to the Messages protocol's ping without
            // exposing comment text or synthesizing model output/usage. Forwarding
            // upstream liveness must not depend on locally generated keepalives.
            if lines.iter().any(|line| line.starts_with(':')) {
                return SseKeepAliveFrame::Anthropic.bytes().to_vec();
            }
            return Vec::new();
        }
        let data = data_lines.join("\n");
        if data.trim() == "[DONE]" {
            return self.finish_stream();
        }

        let value = match serde_json::from_str::<Value>(&data) {
            Ok(value) => value,
            Err(_) => return Vec::new(),
        };
        self.consume_openai_event(&value)
    }

    /// 函数 `consume_openai_event`
    ///
    /// 作者: gaohongshun
    ///
    /// 时间: 2026-04-02
    ///
    /// # 参数
    /// - self: 参数 self
    /// - value: 参数 value
    ///
    /// # 返回
    /// 返回函数执行结果
    fn consume_openai_event(&mut self, value: &Value) -> Vec<u8> {
        self.capture_response_meta(value);
        let mut out = String::new();
        let Some(event_type) = value.get("type").and_then(Value::as_str) else {
            return Vec::new();
        };
        match event_type {
            "response.output_text.delta" => {
                let fragment = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if fragment.is_empty() {
                    return Vec::new();
                }
                append_output_text(&mut self.state.output_text, fragment);
                self.ensure_message_start(&mut out);
                self.ensure_text_block_start(&mut out);
                let text_index = self.state.text_block_index.unwrap_or(0);
                append_sse_event(
                    &mut out,
                    "content_block_delta",
                    &json!({
                        "type": "content_block_delta",
                        "index": text_index,
                        "delta": {
                            "type": "text_delta",
                            "text": fragment
                        }
                    }),
                );
                self.state.emitted_text_to_client = true;
                self.state.stop_reason.get_or_insert("end_turn");
            }
            "response.output_item.done" => {
                collect_output_text_from_event_fields(value, &mut self.state.output_text);
                let Some(item_obj) = value
                    .get("item")
                    .or_else(|| value.get("output_item"))
                    .and_then(Value::as_object)
                else {
                    return Vec::new();
                };
                if item_obj
                    .get("type")
                    .and_then(Value::as_str)
                    .is_none_or(|kind| kind != "function_call")
                {
                    return Vec::new();
                }
                self.ensure_message_start(&mut out);
                self.close_text_block(&mut out);
                let block_index = self.state.next_block_index;
                self.state.next_block_index = self.state.next_block_index.saturating_add(1);
                let tool_use_id = item_obj
                    .get("call_id")
                    .or_else(|| item_obj.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("toolu_unknown");
                let tool_name = item_obj
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| restore_tool_name(name, self.tool_name_restore_map.as_ref()))
                    .unwrap_or_else(|| "tool".to_string());
                self.remember_safeguard_tool_use(
                    tool_use_id,
                    tool_name.as_str(),
                    extract_function_call_input(item_obj).unwrap_or_else(|| json!({})),
                    false,
                );
                append_sse_event(
                    &mut out,
                    "content_block_start",
                    &json!({
                        "type": "content_block_start",
                        "index": block_index,
                        "content_block": {
                            "type": "tool_use",
                            "id": tool_use_id,
                            "name": tool_name,
                            "input": {}
                        }
                    }),
                );
                if let Some(partial_json) =
                    extract_function_call_input(item_obj).and_then(tool_input_partial_json)
                {
                    append_sse_event(
                        &mut out,
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": block_index,
                            "delta": {
                                "type": "input_json_delta",
                                "partial_json": partial_json,
                            }
                        }),
                    );
                }
                append_sse_event(
                    &mut out,
                    "content_block_stop",
                    &json!({
                        "type": "content_block_stop",
                        "index": block_index
                    }),
                );
                self.state.stop_reason = Some("tool_use");
            }
            _ if event_type.starts_with("response.output_item.")
                || event_type.starts_with("response.content_part.") =>
            {
                collect_output_text_from_event_fields(value, &mut self.state.output_text);
            }
            "response.incomplete"
                if value
                    .pointer("/response/incomplete_details/reason")
                    .and_then(Value::as_str)
                    == Some("max_output_tokens")
                    && value.pointer("/response/status").and_then(Value::as_str)
                        == Some("incomplete")
                    && value.pointer("/response/error").is_none_or(Value::is_null) =>
            {
                self.state.stop_reason = Some("max_tokens");
                return self.finish_stream();
            }
            "error" | "response.failed" | "response.incomplete" => {
                return self.fail_stream("upstream response did not complete successfully");
            }
            "response.completed" | "response.done" => {
                if let Some(response) = value.get("response") {
                    let mut extracted_output_text = String::new();
                    collect_response_output_text(response, &mut extracted_output_text);
                    if !extracted_output_text.trim().is_empty() {
                        // Completed carries a full text snapshot. If any text delta was already
                        // delivered, do not replay that snapshot after tool blocks close text.
                        if !self.state.emitted_text_to_client {
                            append_output_text(
                                &mut self.state.output_text,
                                extracted_output_text.as_str(),
                            );
                            self.ensure_message_start(&mut out);
                            self.ensure_text_block_start(&mut out);
                            let text_index = self.state.text_block_index.unwrap_or(0);
                            append_sse_event(
                                &mut out,
                                "content_block_delta",
                                &json!({
                                    "type": "content_block_delta",
                                    "index": text_index,
                                    "delta": {
                                        "type": "text_delta",
                                        "text": extracted_output_text
                                    }
                                }),
                            );
                            self.state.emitted_text_to_client = true;
                        }
                        self.state.stop_reason.get_or_insert("end_turn");
                    }
                    if let Some(output) = response.get("output").and_then(Value::as_array) {
                        for item in output {
                            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                                continue;
                            }
                            let tool_use_id = item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or("toolu_unknown");
                            let tool_name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .map(|name| {
                                    restore_tool_name(name, self.tool_name_restore_map.as_ref())
                                })
                                .unwrap_or_else(|| "tool".to_string());
                            self.remember_safeguard_tool_use(
                                tool_use_id,
                                tool_name.as_str(),
                                item.as_object()
                                    .and_then(extract_function_call_input)
                                    .unwrap_or_else(|| json!({})),
                                false,
                            );
                        }
                    }
                }
                out.push_str(&String::from_utf8(self.finish_stream()).expect("SSE JSON is UTF-8"));
            }
            _ => {}
        }
        out.into_bytes()
    }

    /// 函数 `capture_response_meta`
    ///
    /// 作者: gaohongshun
    ///
    /// 时间: 2026-04-02
    ///
    /// # 参数
    /// - self: 参数 self
    /// - value: 参数 value
    ///
    /// # 返回
    /// 无
    fn capture_response_meta(&mut self, value: &Value) {
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            self.state.response_id = Some(id.to_string());
        }
        if let Some(model) = value.get("model").and_then(Value::as_str) {
            self.state.model = Some(model.to_string());
        }
        if let Some(response) = value.get("response").and_then(Value::as_object) {
            if let Some(id) = response.get("id").and_then(Value::as_str) {
                self.state.response_id = Some(id.to_string());
            }
            if let Some(model) = response.get("model").and_then(Value::as_str) {
                self.state.model = Some(model.to_string());
            }
            if let Some(usage) = response.get("usage").and_then(Value::as_object) {
                self.state.input_tokens = usage
                    .get("input_tokens")
                    .and_then(Value::as_i64)
                    .or_else(|| usage.get("prompt_tokens").and_then(Value::as_i64))
                    .unwrap_or(self.state.input_tokens);
                self.state.cached_input_tokens = usage
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_i64)
                    .or_else(|| {
                        usage
                            .get("input_tokens_details")
                            .and_then(Value::as_object)
                            .and_then(|details| details.get("cached_tokens"))
                            .and_then(Value::as_i64)
                    })
                    .or_else(|| {
                        usage
                            .get("prompt_tokens_details")
                            .and_then(Value::as_object)
                            .and_then(|details| details.get("cached_tokens"))
                            .and_then(Value::as_i64)
                    })
                    .unwrap_or(self.state.cached_input_tokens);
                self.state.cache_write_tokens = usage
                    .get("input_tokens_details")
                    .and_then(Value::as_object)
                    .and_then(|details| details.get("cache_write_tokens"))
                    .and_then(Value::as_i64)
                    .or_else(|| {
                        usage
                            .get("prompt_tokens_details")
                            .and_then(Value::as_object)
                            .and_then(|details| details.get("cache_write_tokens"))
                            .and_then(Value::as_i64)
                    })
                    .or_else(|| {
                        usage
                            .get("cache_write_input_tokens")
                            .and_then(Value::as_i64)
                    })
                    .unwrap_or(self.state.cache_write_tokens);
                self.state.output_tokens = usage
                    .get("output_tokens")
                    .and_then(Value::as_i64)
                    .or_else(|| usage.get("completion_tokens").and_then(Value::as_i64))
                    .unwrap_or(self.state.output_tokens);
                self.state.total_tokens = usage
                    .get("total_tokens")
                    .and_then(Value::as_i64)
                    .or(self.state.total_tokens);
                self.state.reasoning_output_tokens = usage
                    .get("reasoning_output_tokens")
                    .and_then(Value::as_i64)
                    .or_else(|| {
                        usage
                            .get("output_tokens_details")
                            .and_then(Value::as_object)
                            .and_then(|details| details.get("reasoning_tokens"))
                            .and_then(Value::as_i64)
                    })
                    .or_else(|| {
                        usage
                            .get("completion_tokens_details")
                            .and_then(Value::as_object)
                            .and_then(|details| details.get("reasoning_tokens"))
                            .and_then(Value::as_i64)
                    })
                    .unwrap_or(self.state.reasoning_output_tokens);
            }
        }
    }

    /// 函数 `ensure_message_start`
    ///
    /// 作者: gaohongshun
    ///
    /// 时间: 2026-04-02
    ///
    /// # 参数
    /// - self: 参数 self
    /// - out: 参数 out
    ///
    /// # 返回
    /// 无
    fn ensure_message_start(&mut self, out: &mut String) {
        if self.state.started {
            return;
        }
        self.state.started = true;
        append_sse_event(
            out,
            "message_start",
            &json!({
                "type": "message_start",
                "message": {
                    "id": self.state.response_id.clone().unwrap_or_else(|| "msg_proxy".to_string()),
                    "type": "message",
                    "role": "assistant",
                    "model": self.state.model.clone().unwrap_or_else(|| "gpt-5.3-codex".to_string()),
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": build_anthropic_usage(
                        self.state.input_tokens.max(0),
                        0,
                        self.state.cached_input_tokens.max(0),
                        self.state.cache_write_tokens.max(0),
                        None,
                        None,
                    )
                }
            }),
        );
        self.emit_compaction_block_if_needed(out);
    }

    fn emit_compaction_block_if_needed(&mut self, out: &mut String) {
        if self.state.compaction_emitted {
            return;
        }
        let Some(compaction) = self
            .context_enrichment
            .as_ref()
            .and_then(|enrichment| enrichment.compaction.as_ref())
        else {
            return;
        };
        self.state.compaction_emitted = true;
        let index = self.state.next_block_index;
        self.state.next_block_index = self.state.next_block_index.saturating_add(1);
        append_sse_event(
            out,
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "compaction",
                    "content": ""
                }
            }),
        );
        append_sse_event(
            out,
            "content_block_delta",
            &json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {
                    "type": "compaction_delta",
                    "content": compaction.summary
                }
            }),
        );
        append_sse_event(
            out,
            "content_block_stop",
            &json!({
                "type": "content_block_stop",
                "index": index
            }),
        );
    }

    /// 函数 `ensure_text_block_start`
    ///
    /// 作者: gaohongshun
    ///
    /// 时间: 2026-04-02
    ///
    /// # 参数
    /// - self: 参数 self
    /// - out: 参数 out
    ///
    /// # 返回
    /// 无
    fn ensure_text_block_start(&mut self, out: &mut String) {
        if self.state.text_block_index.is_some() {
            return;
        }
        let index = self.state.next_block_index;
        self.state.next_block_index = self.state.next_block_index.saturating_add(1);
        self.state.text_block_index = Some(index);
        append_sse_event(
            out,
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "text",
                    "text": ""
                }
            }),
        );
    }

    /// 函数 `close_text_block`
    ///
    /// 作者: gaohongshun
    ///
    /// 时间: 2026-04-02
    ///
    /// # 参数
    /// - self: 参数 self
    /// - out: 参数 out
    ///
    /// # 返回
    /// 无
    fn close_text_block(&mut self, out: &mut String) {
        let Some(index) = self.state.text_block_index.take() else {
            return;
        };
        append_sse_event(
            out,
            "content_block_stop",
            &json!({
                "type": "content_block_stop",
                "index": index
            }),
        );
    }

    /// 函数 `finish_stream`
    ///
    /// 作者: gaohongshun
    ///
    /// 时间: 2026-04-02
    ///
    /// # 参数
    /// - self: 参数 self
    ///
    /// # 返回
    /// 返回函数执行结果
    fn fail_stream(&mut self, message: &str) -> Vec<u8> {
        self.state.finished = true;
        self.state.failure = Some(message.to_string());
        let mut out = String::new();
        append_sse_event(
            &mut out,
            "error",
            &json!({
                "type": "error", "error": {"type": "api_error", "message": message}
            }),
        );
        out.into_bytes()
    }

    fn finish_stream(&mut self) -> Vec<u8> {
        if self.state.finished {
            return Vec::new();
        }
        self.state.finished = true;
        if let Ok(mut usage) = self.usage_collector.lock() {
            usage.input_tokens = Some(self.state.input_tokens.max(0));
            usage.cached_input_tokens = Some(self.state.cached_input_tokens.max(0));
            usage.cache_write_tokens = Some(self.state.cache_write_tokens.max(0));
            usage.output_tokens = Some(self.state.output_tokens.max(0));
            usage.total_tokens = self.state.total_tokens.map(|value| value.max(0));
            usage.reasoning_output_tokens = Some(self.state.reasoning_output_tokens.max(0));
            if !self.state.output_text.trim().is_empty() {
                usage.output_text = Some(self.state.output_text.clone());
            }
        }
        let mut out = String::new();
        self.ensure_message_start(&mut out);
        self.close_text_block(&mut out);
        let safeguards = self
            .safeguards_evaluator
            .as_ref()
            .map(|evaluator| evaluator.evaluate(self.safeguard_tool_uses.as_slice()));
        let implicit_compaction_stop = self.state.compaction_emitted
            && !self.state.emitted_text_to_client
            && self.safeguard_tool_uses.is_empty();
        let mut delta = json!({
            "stop_reason": self.state.stop_reason.unwrap_or(if implicit_compaction_stop {
                "compaction"
            } else {
                "end_turn"
            }),
            "stop_sequence": Value::Null
        });
        if let Some(safeguards) = safeguards {
            delta["safeguard_results"] = safeguards;
        }
        let context_management = self
            .context_enrichment
            .as_ref()
            .and_then(|enrichment| enrichment.context_management_value());
        let iterations = self.context_enrichment.as_ref().and_then(|enrichment| {
            let compaction = enrichment.compaction_usage_value()?;
            Some(vec![
                compaction,
                json!({
                    "type": "message",
                    "input_tokens": self.state.input_tokens.max(0),
                    "output_tokens": self.state.output_tokens.max(0)
                }),
            ])
        });
        let mut usage = build_anthropic_usage(
            self.state.input_tokens.max(0),
            self.state.output_tokens.max(0),
            self.state.cached_input_tokens.max(0),
            self.state.cache_write_tokens.max(0),
            self.state.total_tokens.map(|value| value.max(0)),
            Some(self.state.reasoning_output_tokens.max(0)),
        );
        if let Some(iterations) = iterations {
            usage["iterations"] = json!(iterations);
        }
        let mut message_delta = json!({
            "type": "message_delta",
            "delta": delta,
            "usage": usage
        });
        if let Some(context_management) = context_management {
            message_delta["context_management"] = context_management;
        }
        append_sse_event(&mut out, "message_delta", &message_delta);
        append_sse_event(&mut out, "message_stop", &json!({ "type": "message_stop" }));
        out.into_bytes()
    }

    fn remember_safeguard_tool_use(&mut self, id: &str, name: &str, input: Value, truncated: bool) {
        if self.safeguards_evaluator.is_none()
            || self
                .safeguard_tool_uses
                .iter()
                .any(|tool_use| tool_use.id == id)
        {
            return;
        }
        self.safeguard_tool_uses.push(SafeguardToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
            truncated,
        });
    }
}

impl Read for AnthropicSseReader {
    /// 函数 `read`
    ///
    /// 作者: gaohongshun
    ///
    /// 时间: 2026-04-02
    ///
    /// # 参数
    /// - self: 参数 self
    /// - buf: 参数 buf
    ///
    /// # 返回
    /// 返回函数执行结果
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let read = self.out_cursor.read(buf)?;
            if read > 0 {
                return Ok(read);
            }
            if let Some(message) = self.state.failure.take() {
                // The protocol error was delivered first; also report failure to the
                // bridge so it cannot record successful completion or rebind a session.
                return Err(std::io::Error::new(std::io::ErrorKind::Other, message));
            }
            if self.state.finished {
                return Ok(0);
            }
            let next = self.next_chunk()?;
            self.out_cursor = Cursor::new(next);
        }
    }
}

/// 函数 `append_sse_event`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-02
///
/// # 参数
/// - buffer: 参数 buffer
/// - event_name: 参数 event_name
/// - payload: 参数 payload
///
/// # 返回
/// 无
fn append_sse_event(buffer: &mut String, event_name: &str, payload: &Value) {
    let data = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    buffer.push_str("event: ");
    buffer.push_str(event_name);
    buffer.push('\n');
    buffer.push_str("data: ");
    buffer.push_str(&data);
    buffer.push_str("\n\n");
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;

/// 函数 `build_anthropic_usage`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-02
///
/// # 参数
/// - input_tokens: 参数 input_tokens
/// - output_tokens: 参数 output_tokens
/// - cache_read_input_tokens: 参数 cache_read_input_tokens
/// - total_tokens: 参数 total_tokens
/// - reasoning_output_tokens: 参数 reasoning_output_tokens
///
/// # 返回
/// 返回函数执行结果
fn build_anthropic_usage(
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    total_tokens: Option<i64>,
    reasoning_output_tokens: Option<i64>,
) -> Value {
    let mut usage = Map::new();
    usage.insert("input_tokens".to_string(), Value::from(input_tokens));
    usage.insert("output_tokens".to_string(), Value::from(output_tokens));
    usage.insert(
        "cache_read_input_tokens".to_string(),
        Value::from(cache_read_input_tokens),
    );
    usage.insert(
        "cache_creation_input_tokens".to_string(),
        Value::from(cache_creation_input_tokens),
    );
    if let Some(value) = total_tokens {
        usage.insert("total_tokens".to_string(), Value::from(value));
    }
    if let Some(value) = reasoning_output_tokens {
        usage.insert("reasoning_output_tokens".to_string(), Value::from(value));
    }
    Value::Object(usage)
}

/// 函数 `extract_function_call_input`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-02
///
/// # 参数
/// - item_obj: 参数 item_obj
///
/// # 返回
/// 返回函数执行结果
fn extract_function_call_input(item_obj: &Map<String, Value>) -> Option<Value> {
    const ARGUMENT_KEYS: [&str; 5] = [
        "arguments",
        "input",
        "arguments_json",
        "parsed_arguments",
        "args",
    ];
    for key in ARGUMENT_KEYS {
        let Some(value) = item_obj.get(key) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        if let Some(text) = value.as_str() {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                return Some(parsed);
            }
            return Some(Value::String(trimmed.to_string()));
        }
        return Some(value.clone());
    }
    None
}

/// 函数 `tool_input_partial_json`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-02
///
/// # 参数
/// - value: 参数 value
///
/// # 返回
/// 返回函数执行结果
fn tool_input_partial_json(value: Value) -> Option<String> {
    let serialized = serde_json::to_string(&value).ok()?;
    let trimmed = serialized.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return None;
    }
    Some(trimmed.to_string())
}

fn restore_tool_name(
    name: &str,
    tool_name_restore_map: Option<&crate::gateway::ToolNameRestoreMap>,
) -> String {
    tool_name_restore_map
        .and_then(|map| map.get(name))
        .cloned()
        .unwrap_or_else(|| name.to_string())
}
