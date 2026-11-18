use bytes::Bytes;
use codexmanager_core::storage::{Account, Token};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

use super::super::anthropic_context_management::{
    summary_prompt, CompactRequest, CompactionResult,
};
use super::super::IncomingHeaderSnapshot;
use super::attempt_flow::primary_flow::{run_primary_upstream_flow, PrimaryFlowDecision};
use super::attempt_flow::transport::UpstreamRequestContext;
use super::GatewayUpstreamResponse;

const SUMMARY_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) fn summarize_compact_request(
    request: &CompactRequest,
    model: Option<&str>,
    account: &Account,
    token: &mut Token,
    incoming_headers: &IncomingHeaderSnapshot,
    upstream_base: &str,
    upstream_fallback_base: Option<&str>,
    parent_deadline: Option<Instant>,
) -> Result<CompactionResult, String> {
    let model = super::super::current_claude_compact_summary_model()
        .or_else(|| {
            model
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .ok_or_else(|| "compact summary model is not configured".to_string())?;
    let max_tokens = super::super::current_claude_compact_summary_max_tokens();
    let body = summary_request_body(model.as_str(), summary_prompt(request).as_str(), max_tokens);
    let response_body = send_summary_request(
        &body,
        account,
        token,
        incoming_headers,
        upstream_base,
        upstream_fallback_base,
        parent_deadline,
    )?;
    compaction_summary_result_from_response(response_body.as_ref())
}

pub(super) fn summary_request_body(model: &str, prompt: &str, max_tokens: usize) -> Bytes {
    let body = json!({
        "model": model,
        "instructions": "Compact the supplied coding-agent transcript. Preserve the current state, decisions, failures, open questions and concrete next steps. Return only the requested <summary> block.",
        "input": [{
            "role": "user",
            "content": [{"type": "input_text", "text": prompt}]
        }],
        "reasoning": {"effort": "low", "summary": "auto"},
        "max_output_tokens": max_tokens,
        "parallel_tool_calls": false,
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content"]
    });
    Bytes::from(serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec()))
}

pub(super) fn compaction_summary_result_from_response(
    body: &[u8],
) -> Result<CompactionResult, String> {
    let (text, input_tokens, output_tokens) = parse_summary_response(body)?;
    let summary = extract_summary_text(text.as_str()).ok_or_else(|| {
        "summary_extraction_failed: response did not contain <summary>".to_string()
    })?;
    Ok(CompactionResult {
        summary,
        input_tokens,
        output_tokens,
    })
}

fn send_summary_request(
    body: &Bytes,
    account: &Account,
    token: &mut Token,
    incoming_headers: &IncomingHeaderSnapshot,
    upstream_base: &str,
    upstream_fallback_base: Option<&str>,
    parent_deadline: Option<Instant>,
) -> Result<Bytes, String> {
    let storage = crate::storage_helpers::open_storage()
        .ok_or_else(|| "storage unavailable for compact summary".to_string())?;
    let client = super::super::upstream_client_for_account(account.id.as_str())?;
    let request_ctx = UpstreamRequestContext {
        request_path: "/v1/responses",
        protocol_type: crate::apikey_profile::PROTOCOL_OPENAI_COMPAT,
        is_fedramp: false,
    };
    let deadline = Some(
        parent_deadline
            .map(|deadline| deadline.min(Instant::now() + SUMMARY_TIMEOUT))
            .unwrap_or_else(|| Instant::now() + SUMMARY_TIMEOUT),
    );
    let (url, _) =
        super::super::request_rewrite::compute_upstream_url(upstream_base, "/v1/responses");
    let _detached_budget = super::generation_budget::enter(false, "", "", "", None);
    let response = match run_primary_upstream_flow(
        &client,
        &storage,
        &reqwest::Method::POST,
        request_ctx,
        incoming_headers,
        body,
        false,
        upstream_base,
        "/v1/responses",
        url.as_str(),
        deadline,
        upstream_fallback_base,
        account,
        token,
        true,
        false,
        false,
        false,
        |_, _, _| {},
    ) {
        PrimaryFlowDecision::Continue { upstream, .. }
        | PrimaryFlowDecision::RespondUpstream(upstream) => upstream,
        PrimaryFlowDecision::Failover => {
            return Err("summary upstream failed over".to_string());
        }
        PrimaryFlowDecision::Terminal {
            status_code,
            message,
        } => {
            return Err(format!(
                "summary upstream returned {status_code}: {message}"
            ));
        }
    };
    let status = response.status();
    let body = match response {
        GatewayUpstreamResponse::Blocking(response) => response
            .bytes()
            .map_err(|err| format!("read summary response failed: {err}"))?,
        GatewayUpstreamResponse::Stream(response) => response
            .read_all_bytes()
            .map_err(|err| format!("read summary response failed: {err}"))?,
    };
    if status.as_u16() >= 400 {
        return Err(format!(
            "summary upstream status {}: {}",
            status.as_u16(),
            String::from_utf8_lossy(body.as_ref())
        ));
    }
    Ok(body)
}

fn parse_summary_response(body: &[u8]) -> Result<(String, i64, i64), String> {
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        if value.get("type").and_then(Value::as_str) != Some("error") {
            let text = extract_text_from_value(&value);
            let (input_tokens, output_tokens) = extract_usage_from_value(&value);
            if !text.trim().is_empty() {
                return Ok((text, input_tokens, output_tokens));
            }
        }
    }

    let mut text = String::new();
    let mut input_tokens = 0i64;
    let mut output_tokens = 0i64;
    for line in String::from_utf8_lossy(body).lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    text.push_str(delta);
                }
            }
            Some("response.completed" | "response.done") => {
                if text.trim().is_empty() {
                    if let Some(response) = value.get("response") {
                        text = extract_text_from_value(response);
                    }
                }
                let usage_source = value.get("response").unwrap_or(&value);
                let usage = extract_usage_from_value(usage_source);
                input_tokens = usage.0;
                output_tokens = usage.1;
            }
            Some("error" | "response.failed" | "response.incomplete") => {
                return Err("summary response did not complete successfully".to_string());
            }
            _ => {
                let usage = extract_usage_from_value(&value);
                if usage.0 > 0 || usage.1 > 0 {
                    input_tokens = usage.0;
                    output_tokens = usage.1;
                }
            }
        }
    }
    if text.trim().is_empty() {
        return Err("summary response did not contain text".to_string());
    }
    Ok((text, input_tokens, output_tokens))
}

fn extract_text_from_value(value: &Value) -> String {
    let mut text = String::new();
    if let Some(value) = value.get("output_text").and_then(Value::as_str) {
        text.push_str(value);
    }
    if let Some(output) = value.get("output") {
        collect_text(output, &mut text);
    }
    if text.is_empty() {
        if let Some(choices) = value.get("choices").and_then(Value::as_array) {
            for choice in choices {
                if let Some(content) = choice.pointer("/message/content").and_then(Value::as_str) {
                    text.push_str(content);
                }
            }
        }
    }
    text
}

fn collect_text(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_text(item, out);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("output_text") {
                if let Some(text) = object.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                }
            }
            for key in ["content", "message", "output"] {
                if let Some(value) = object.get(key) {
                    collect_text(value, out);
                }
            }
        }
        _ => {}
    }
}

fn extract_usage_from_value(value: &Value) -> (i64, i64) {
    let usage = value
        .get("usage")
        .or_else(|| value.pointer("/response/usage"));
    let Some(usage) = usage else {
        return (0, 0);
    };
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    (input_tokens.max(0), output_tokens.max(0))
}

fn extract_summary_text(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let start = lower.find("<summary>")? + "<summary>".len();
    let end = lower[start..].find("</summary>")? + start;
    let summary = raw[start..end].trim();
    (!summary.is_empty()).then(|| summary.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_summary_and_usage_from_stream() {
        let body = br#"data: {"type":"response.output_text.delta","delta":"<summary>fixed parser"}

data: {"type":"response.output_text.delta","delta":" and tests pass</summary>"}

data: {"type":"response.completed","response":{"usage":{"input_tokens":42,"output_tokens":12}}}

"#;
        let (text, input, output) = parse_summary_response(body).expect("summary");
        assert_eq!(
            extract_summary_text(text.as_str()).as_deref(),
            Some("fixed parser and tests pass")
        );
        assert_eq!(input, 42);
        assert_eq!(output, 12);
    }

    #[test]
    fn rejects_missing_summary_tag() {
        assert_eq!(extract_summary_text("a bare answer"), None);
    }
}
