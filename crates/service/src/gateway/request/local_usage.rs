use chrono::{FixedOffset, TimeZone};
use codexmanager_core::storage::{
    now_ts, KeyRollingUsageSummary, Storage, DEFAULT_KEY_WEEKLY_LIMIT_CENTS, SECONDS_PER_WEEK,
};
use serde_json::{json, Value};
use tiny_http::{Header, Request};

const USAGE_RULE_WIDTH: usize = 60;
const USAGE_BAR_WIDTH: usize = 32;
const MIN_CHAT_QUOTA_MAX_TOKENS: i64 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChatUsageTarget {
    OpenAiResponses,
    AnthropicMessages,
    OpenAiChatCompletions,
}

pub(super) fn path_without_query(path: &str) -> &str {
    path.split_once('?')
        .map(|(prefix, _)| prefix)
        .unwrap_or(path)
}

pub(super) fn is_usage_endpoint_path(path: &str) -> bool {
    matches!(
        path_without_query(path),
        "/v1/usage" | "/usage" | "/v1/v1/usage"
    )
}

pub(super) fn is_local_usage_shortcut_request(
    request_url: &str,
    method: &str,
    body: &[u8],
) -> bool {
    if is_usage_endpoint_path(request_url) {
        return true;
    }
    let Some(value) = super::parse_request_json_value(body) else {
        return false;
    };
    detect_chat_usage_command(request_url, method, Some(&value)).is_some()
}

pub(super) fn detect_chat_usage_command(
    path: &str,
    method: &str,
    parsed: Option<&Value>,
) -> Option<ChatUsageTarget> {
    if !method.eq_ignore_ascii_case("POST") {
        return None;
    }
    let target = match path_without_query(path) {
        "/v1/responses" | "/responses" | "/v1/v1/responses" => ChatUsageTarget::OpenAiResponses,
        "/v1/messages" | "/messages" | "/v1/v1/messages" => ChatUsageTarget::AnthropicMessages,
        "/v1/chat/completions" | "/chat/completions" | "/v1/v1/chat/completions" => {
            ChatUsageTarget::OpenAiChatCompletions
        }
        _ => return None,
    };
    let obj = parsed.and_then(Value::as_object)?;
    if has_non_text_structured_output_or_forced_tool(obj) {
        return None;
    }
    for field in ["max_tokens", "max_output_tokens", "max_completion_tokens"] {
        if let Some(limit) = obj.get(field).and_then(Value::as_i64) {
            if limit < MIN_CHAT_QUOTA_MAX_TOKENS {
                return None;
            }
        }
    }
    let turn_text = match target {
        ChatUsageTarget::OpenAiResponses => extract_last_responses_user_text(obj)?,
        ChatUsageTarget::AnthropicMessages | ChatUsageTarget::OpenAiChatCompletions => {
            extract_last_messages_user_text(obj)?
        }
    };
    let normalized = strip_client_context_wrappers(turn_text.as_str());
    is_exact_quota_command(normalized).then_some(target)
}

fn has_non_text_structured_output_or_forced_tool(
    obj: &serde_json::Map<String, Value>,
) -> bool {
    let format_type_non_text = [
        obj.get("response_format"),
        obj.get("text").and_then(|v| v.get("format")),
        obj.get("output_config").and_then(|v| v.get("format")),
    ]
    .into_iter()
    .flatten()
    .any(|format_val| {
        format_val
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| !kind.trim().eq_ignore_ascii_case("text"))
    });
    if format_type_non_text {
        return true;
    }
    match obj.get("tool_choice") {
        Some(Value::String(choice)) => {
            let trimmed = choice.trim();
            trimmed.eq_ignore_ascii_case("required") || trimmed.eq_ignore_ascii_case("any")
        }
        Some(Value::Object(tc)) => tc
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| {
                let kind = kind.trim();
                kind.eq_ignore_ascii_case("any")
                    || kind.eq_ignore_ascii_case("tool")
                    || kind.eq_ignore_ascii_case("function")
            }),
        _ => false,
    }
}

fn is_system_or_developer_role(role: Option<&str>) -> bool {
    role.is_some_and(|r| r == "system" || r == "developer")
}

fn extract_last_responses_user_text(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let input = obj.get("input")?;
    match input {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let last = items.iter().rev().find(|item| {
                let Some(obj) = item.as_object() else {
                    return true;
                };
                let is_message = obj
                    .get("type")
                    .and_then(Value::as_str)
                    .is_none_or(|kind| kind == "message");
                !(is_message && is_system_or_developer_role(obj.get("role").and_then(Value::as_str)))
            })?
            .as_object()?;
            if let Some(kind) = last.get("type").and_then(Value::as_str) {
                if kind != "message" {
                    return None;
                }
            }
            if last.get("role").and_then(Value::as_str) != Some("user") {
                return None;
            }
            extract_safe_turn_text(last.get("content"))
        }
        _ => None,
    }
}

fn extract_last_messages_user_text(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let items = obj.get("messages").and_then(Value::as_array)?;
    let last = items.iter().rev().find(|item| {
        let Some(obj) = item.as_object() else {
            return true;
        };
        !(obj.get("tool_call_id").is_none()
            && is_system_or_developer_role(obj.get("role").and_then(Value::as_str)))
    })?
    .as_object()?;
    if last.get("tool_call_id").is_some() {
        return None;
    }
    if last.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    extract_safe_turn_text(last.get("content"))
}

fn extract_safe_turn_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            if parts.is_empty() {
                return None;
            }
            let mut joined = Vec::with_capacity(parts.len());
            for part in parts {
                let obj = part.as_object()?;
                if let Some(kind) = obj.get("type").and_then(Value::as_str) {
                    if !matches!(kind, "text" | "input_text" | "output_text") {
                        return None;
                    }
                }
                let text = obj.get("text").and_then(Value::as_str)?;
                joined.push(text);
            }
            Some(joined.join("\n"))
        }
        _ => None,
    }
}

fn is_strippable_context_tag(tag_lower: &str) -> bool {
    matches!(
        tag_lower,
        "system-reminder"
            | "environment_context"
            | "permissions instructions"
            | "collaboration_mode"
            | "user-prompt-submit-hook"
            | "fast_mode_info"
            | "agents_instructions"
            | "user_instructions"
    ) || tag_lower.starts_with("ide_")
        || tag_lower.starts_with("context")
}

fn strip_leading_xml_block(input: &str) -> Option<&str> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('<') {
        return None;
    }
    let gt = trimmed.find('>')?;
    let header = trimmed[1..gt].trim();
    if header.is_empty() || header.starts_with('/') || header.ends_with('/') {
        return None;
    }
    let tag_name = if header.eq_ignore_ascii_case("permissions instructions") {
        "permissions instructions"
    } else {
        header.split_ascii_whitespace().next().unwrap_or("")
    };
    let tag_lower = tag_name.to_ascii_lowercase();
    if !is_strippable_context_tag(tag_lower.as_str()) {
        return None;
    }
    let after_open = &trimmed[gt + 1..];
    let close_tag = format!("</{tag_lower}>");
    let lower_rest = after_open.to_ascii_lowercase();
    let close_start = lower_rest.find(close_tag.as_str())?;
    Some(&after_open[close_start + close_tag.len()..])
}

fn strip_trailing_xml_block(input: &str) -> Option<&str> {
    let trimmed = input.trim_end();
    if !trimmed.ends_with('>') {
        return None;
    }
    let close_open = trimmed.rfind("</")?;
    let tag_name = trimmed[close_open + 2..trimmed.len() - 1].trim();
    if tag_name.is_empty() {
        return None;
    }
    let tag_lower = tag_name.to_ascii_lowercase();
    if !is_strippable_context_tag(tag_lower.as_str()) {
        return None;
    }
    let prefix = &trimmed[..close_open];
    let lower_prefix = prefix.to_ascii_lowercase();
    let needle = format!("<{tag_lower}");
    let mut search_end = lower_prefix.len();
    while let Some(open_start) = lower_prefix[..search_end].rfind(needle.as_str()) {
        let after_tag = &lower_prefix[open_start + needle.len()..];
        let valid_boundary = after_tag
            .chars()
            .next()
            .is_some_and(|ch| ch == '>' || ch.is_ascii_whitespace());
        if valid_boundary && after_tag.contains('>') {
            return Some(&trimmed[..open_start]);
        }
        if open_start == 0 {
            break;
        }
        search_end = open_start;
    }
    None
}

fn strip_client_context_wrappers(raw: &str) -> &str {
    let mut current = raw.trim();
    for _ in 0..32 {
        let before = current;
        while let Some(next) = strip_leading_xml_block(current) {
            current = next.trim();
        }
        while let Some(next) = strip_trailing_xml_block(current) {
            current = next.trim();
        }
        let lower = current.to_ascii_lowercase();
        if lower.starts_with("<user_query") && lower.ends_with("</user_query>") {
            if let Some(gt) = current.find('>') {
                let inner_end = current.len() - "</user_query>".len();
                if gt + 1 <= inner_end {
                    current = current[gt + 1..inner_end].trim();
                    continue;
                }
            }
        }
        if current == before {
            break;
        }
    }
    current
}

fn is_exact_quota_command(raw: &str) -> bool {
    let trimmed = raw.trim();
    let unwrapped = if trimmed.starts_with('`') && trimmed.ends_with('`') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    };
    matches!(
        unwrapped.to_ascii_lowercase().as_str(),
        "?usage"
            | "?quota"
            | "#usage"
            | "#quota"
            | "/usage"
            | "/quota"
            | "/key"
            | "查额度"
            | "查询额度"
            | "查看额度"
            | "key余量"
    )
}

/// Read key rolling quota and build native Codex + Team-Key response headers.
pub(crate) fn build_key_quota_response_headers(storage: &Storage, key_id: &str) -> Vec<Header> {
    let now = now_ts();
    let Ok(summary) = storage.key_rolling_7d_usage_summary(key_id, now) else {
        return Vec::new();
    };
    build_quota_headers_from_summary(&summary, DEFAULT_KEY_WEEKLY_LIMIT_CENTS, now)
}

pub(super) fn build_quota_headers_from_summary(
    summary: &KeyRollingUsageSummary,
    limit_cents: i64,
    now: i64,
) -> Vec<Header> {
    let used_cents = summary.used_cents.max(0);
    let remaining_cents = (limit_cents - used_cents).max(0);
    let used_pct = if limit_cents > 0 {
        ((used_cents as f64 / limit_cents as f64) * 100.0)
            .round()
            .clamp(0.0, 100.0) as i64
    } else {
        0
    };
    let reset_at = summary
        .next_release_at
        .unwrap_or_else(|| now + SECONDS_PER_WEEK);
    let mut pairs: Vec<(&str, String)> = vec![
        ("x-codex-primary-used-percent", used_pct.to_string()),
        ("x-codex-primary-window-minutes", "10080".to_string()),
        ("x-codex-primary-reset-at", reset_at.to_string()),
        (
            "x-codex-credits-has-credits",
            if remaining_cents > 0 { "true" } else { "false" }.to_string(),
        ),
        ("x-codex-credits-unlimited", "false".to_string()),
        ("x-codex-credits-balance", usd(remaining_cents)),
        ("x-team-key-used-usd", usd(used_cents)),
        ("x-team-key-limit-usd", usd(limit_cents)),
        ("x-team-key-remaining-usd", usd(remaining_cents)),
    ];
    if let Some(release_at) = summary.next_release_at {
        pairs.push(("x-team-key-next-release-at", release_at.to_string()));
    }
    if let Some(release_cents) = summary.next_release_cents {
        pairs.push(("x-team-key-next-release-usd", usd(release_cents)));
    }
    pairs
        .into_iter()
        .filter_map(|(name, value)| Header::from_bytes(name.as_bytes(), value.as_bytes()).ok())
        .collect()
}

/// Respond to `GET /usage`, `GET /v1/usage`, or explicit chat quota commands locally.
///
/// This is a local read-only endpoint: it never selects an upstream account and
/// never consumes quota.
#[allow(clippy::too_many_arguments)]
pub(super) fn maybe_respond_local_usage(
    request: Request,
    trace_id: &str,
    key_id: &str,
    protocol_type: &str,
    original_path: &str,
    path: &str,
    response_adapter: super::ResponseAdapter,
    request_method: &str,
    passthrough_body: &[u8],
    is_stream: bool,
    model_for_log: Option<&str>,
    reasoning_for_log: Option<&str>,
    storage: &Storage,
) -> Result<Option<Request>, String> {
    let is_usage_endpoint =
        is_usage_endpoint_path(original_path) || is_usage_endpoint_path(path);
    let parsed_body = (!is_usage_endpoint && request_method.eq_ignore_ascii_case("POST"))
        .then(|| super::parse_request_json_value(passthrough_body))
        .flatten();
    let chat_target = if is_usage_endpoint {
        None
    } else {
        detect_chat_usage_command(original_path, request_method, parsed_body.as_ref())
            .or_else(|| detect_chat_usage_command(path, request_method, parsed_body.as_ref()))
    };

    if !is_usage_endpoint && chat_target.is_none() {
        return Ok(Some(request));
    }

    let context = super::local_response::LocalResponseContext {
        trace_id,
        key_id,
        protocol_type,
        original_path,
        path,
        response_adapter,
        request_method,
        model_for_log,
        reasoning_for_log,
        storage,
    };

    if is_usage_endpoint && !request_method.eq_ignore_ascii_case("GET") {
        super::local_response::respond_local_terminal_error(
            request,
            &context,
            405,
            "usage endpoint only supports GET".to_string(),
        )?;
        return Ok(None);
    }

    let key = match storage.find_api_key_by_id(key_id) {
        Ok(Some(key)) => key,
        Ok(None) => {
            super::local_response::respond_local_terminal_error(
                request,
                &context,
                404,
                "api key not found".to_string(),
            )?;
            return Ok(None);
        }
        Err(err) => {
            super::local_response::respond_local_terminal_error(
                request,
                &context,
                503,
                format!("read api key failed: {err}"),
            )?;
            return Ok(None);
        }
    };

    let now = now_ts();
    let summary = match storage.key_rolling_7d_usage_summary(key_id, now) {
        Ok(value) => value,
        Err(err) => {
            super::local_response::respond_local_terminal_error(
                request,
                &context,
                503,
                format!("read weekly usage failed: {err}"),
            )?;
            return Ok(None);
        }
    };

    let limit_cents = DEFAULT_KEY_WEEKLY_LIMIT_CENTS;
    let used_cents = summary.used_cents.max(0);
    let remaining_cents = (limit_cents - used_cents).max(0);
    let label = key.name.as_deref().unwrap_or("-");
    let quota_headers = build_quota_headers_from_summary(&summary, limit_cents, now);
    let zero_usage = super::request_log::RequestLogUsage::default();

    if let Some(target) = chat_target {
        let markdown = format_chat_usage_markdown(label, &key.status, &summary, limit_cents, now);
        let model_name = model_for_log
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or("gpt-5");
        let payload = render_chat_quota_payload(
            target,
            is_stream,
            trace_id,
            model_name,
            markdown.as_str(),
            now,
        );
        if is_stream {
            super::local_response::respond_local_sse_with_headers(
                request,
                &context,
                payload,
                zero_usage,
                quota_headers,
            )?;
        } else {
            super::local_response::respond_local_json_with_headers(
                request,
                &context,
                payload,
                zero_usage,
                quota_headers,
            )?;
        }
        return Ok(None);
    }

    if wants_statusline(path) || wants_statusline(original_path) {
        let line = render_usage_statusline(
            used_cents,
            limit_cents,
            remaining_cents,
            summary.next_release_at,
            summary.next_release_cents,
            now,
        );
        super::local_response::respond_local_text_with_headers(
            request,
            &context,
            format!("{line}\n"),
            zero_usage,
            quota_headers,
        )?;
        return Ok(None);
    }

    if wants_text(path) || wants_text(original_path) {
        let body = render_usage_text(
            label,
            &key.status,
            used_cents,
            limit_cents,
            remaining_cents,
            summary.next_release_at,
            summary.next_release_cents,
            now,
        );
        super::local_response::respond_local_text_with_headers(
            request,
            &context,
            body,
            zero_usage,
            quota_headers,
        )?;
        return Ok(None);
    }

    let used_percent = if limit_cents > 0 {
        ((used_cents as f64 * 100.0) / limit_cents as f64).min(100.0)
    } else {
        0.0
    };
    let remaining_percent = (100.0 - used_percent).max(0.0);
    let next_release_in_seconds = summary.next_release_at.map(|ts| (ts - now).max(0));
    let next_release_usd = summary.next_release_cents.map(usd);
    let statusline = render_usage_statusline(
        used_cents,
        limit_cents,
        remaining_cents,
        summary.next_release_at,
        summary.next_release_cents,
        now,
    );

    let body = json!({
        "key": {
            "id": key.id,
            "label": key.name,
            "status": key.status,
        },
        "window": "rolling_7d",
        "used_cents": used_cents,
        "limit_cents": limit_cents,
        "remaining_cents": remaining_cents,
        "used_usd": format!("{:.4}", used_cents as f64 / 100.0),
        "limit_usd": format!("{:.2}", limit_cents as f64 / 100.0),
        "remaining_usd": format!("{:.4}", remaining_cents as f64 / 100.0),
        "used_percent": (used_percent * 10.0).round() / 10.0,
        "remaining_percent": (remaining_percent * 10.0).round() / 10.0,
        "next_release_at": summary.next_release_at,
        "next_release_cents": summary.next_release_cents,
        "next_release_usd": next_release_usd,
        "next_release_in_seconds": next_release_in_seconds,
        "statusline": statusline,
    })
    .to_string();

    super::local_response::respond_local_json_with_headers(
        request,
        &context,
        body,
        zero_usage,
        quota_headers,
    )?;
    Ok(None)
}

fn sanitize_id_suffix(trace_id: &str) -> String {
    let cleaned: String = trace_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .take(24)
        .collect();
    if cleaned.is_empty() {
        "local".to_string()
    } else {
        cleaned
    }
}

fn append_sse_frame(out: &mut String, event: Option<&str>, data: &Value) {
    if let Some(name) = event {
        out.push_str("event: ");
        out.push_str(name);
        out.push('\n');
    }
    out.push_str("data: ");
    out.push_str(data.to_string().as_str());
    out.push_str("\n\n");
}

pub(crate) struct WsLocalUsageReply {
    pub(crate) events: Vec<String>,
    pub(crate) client_model: Option<String>,
    pub(crate) previous_response_id: Option<String>,
    pub(crate) input: Value,
}

fn format_chat_usage_markdown(
    label: &str,
    status: &str,
    summary: &KeyRollingUsageSummary,
    limit_cents: i64,
    now: i64,
) -> String {
    let used_cents = summary.used_cents.max(0);
    let remaining_cents = (limit_cents - used_cents).max(0);
    let card = render_usage_text(
        label,
        status,
        used_cents,
        limit_cents,
        remaining_cents,
        summary.next_release_at,
        summary.next_release_cents,
        now,
    );
    format!(
        "```text\n{}```\n以下为本 Key 实时滚动额度，未调用上游模型（0 token / $0.00 / <1ms）。",
        card.trim_start_matches('\n')
    )
}

fn parse_ws_usage_response_create(text: &str) -> Option<Value> {
    let val = serde_json::from_str::<Value>(text).ok()?;
    let obj = val.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("response.create") {
        return None;
    }
    detect_chat_usage_command("/v1/responses", "POST", Some(&val)).map(|_| val)
}

pub(crate) fn is_ws_local_usage_frame(text: &str) -> bool {
    parse_ws_usage_response_create(text).is_some()
}

pub(crate) fn build_ws_local_usage_reply(
    storage: &Storage,
    key_id: &str,
    trace_id: &str,
    text: &str,
) -> Result<Option<WsLocalUsageReply>, (u16, String)> {
    let Some(val) = parse_ws_usage_response_create(text) else {
        return Ok(None);
    };
    let obj = val.as_object();
    let client_model = obj
        .and_then(|map| map.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let previous_response_id = obj
        .and_then(|map| map.get("previous_response_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let input = obj
        .and_then(|map| map.get("input"))
        .cloned()
        .unwrap_or(Value::Null);
    let key = storage
        .find_api_key_by_id(key_id)
        .map_err(|err| (503, format!("read api key failed: {err}")))?
        .ok_or_else(|| (404, "api key not found".to_string()))?;
    let now = now_ts();
    let summary = storage
        .key_rolling_7d_usage_summary(key_id, now)
        .map_err(|err| (503, format!("read weekly usage failed: {err}")))?;
    let label = key.name.as_deref().unwrap_or("-");
    let markdown =
        format_chat_usage_markdown(label, &key.status, &summary, DEFAULT_KEY_WEEKLY_LIMIT_CENTS, now);
    let model_name = client_model.as_deref().unwrap_or("gpt-5");
    let events = render_openai_responses_stream_events(trace_id, model_name, markdown.as_str(), now)
        .into_iter()
        .map(|ev| ev.to_string())
        .collect();
    Ok(Some(WsLocalUsageReply {
        events,
        client_model,
        previous_response_id,
        input,
    }))
}

fn render_openai_responses_stream_events(
    trace_id: &str,
    model: &str,
    text: &str,
    now: i64,
) -> Vec<Value> {
    let suffix = sanitize_id_suffix(trace_id);
    let resp_id = format!("resp_quota_{suffix}");
    let msg_id = format!("msg_quota_{suffix}");
    let usage = json!({
        "input_tokens": 0,
        "output_tokens": 0,
        "total_tokens": 0,
        "input_tokens_details": { "cached_tokens": 0 },
        "output_tokens_details": { "reasoning_tokens": 0 },
    });
    let completed_item = json!({
        "id": msg_id,
        "type": "message",
        "status": "completed",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": text, "annotations": [] }],
    });
    let completed_response = json!({
        "id": resp_id,
        "object": "response",
        "created_at": now,
        "status": "completed",
        "model": model,
        "output": [completed_item],
        "usage": usage,
    });
    let in_progress = json!({
        "id": resp_id,
        "object": "response",
        "created_at": now,
        "status": "in_progress",
        "model": model,
        "output": [],
        "usage": Value::Null,
    });
    vec![
        json!({ "type": "response.created", "sequence_number": 0, "response": in_progress }),
        json!({ "type": "response.in_progress", "sequence_number": 1, "response": in_progress }),
        json!({
            "type": "response.output_item.added",
            "sequence_number": 2,
            "output_index": 0,
            "item": {
                "id": msg_id,
                "type": "message",
                "status": "in_progress",
                "role": "assistant",
                "content": [],
            }
        }),
        json!({
            "type": "response.content_part.added",
            "sequence_number": 3,
            "item_id": msg_id,
            "output_index": 0,
            "content_index": 0,
            "part": { "type": "output_text", "text": "", "annotations": [] }
        }),
        json!({
            "type": "response.output_text.delta",
            "sequence_number": 4,
            "item_id": msg_id,
            "output_index": 0,
            "content_index": 0,
            "delta": text
        }),
        json!({
            "type": "response.output_text.done",
            "sequence_number": 5,
            "item_id": msg_id,
            "output_index": 0,
            "content_index": 0,
            "text": text
        }),
        json!({
            "type": "response.content_part.done",
            "sequence_number": 6,
            "item_id": msg_id,
            "output_index": 0,
            "content_index": 0,
            "part": { "type": "output_text", "text": text, "annotations": [] }
        }),
        json!({
            "type": "response.output_item.done",
            "sequence_number": 7,
            "output_index": 0,
            "item": {
                "id": msg_id,
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": text, "annotations": [] }]
            }
        }),
        json!({
            "type": "response.completed",
            "sequence_number": 8,
            "response": completed_response
        }),
    ]
}

pub(super) fn render_chat_quota_payload(
    target: ChatUsageTarget,
    is_stream: bool,
    trace_id: &str,
    model: &str,
    text: &str,
    now: i64,
) -> String {
    let suffix = sanitize_id_suffix(trace_id);
    match target {
        ChatUsageTarget::OpenAiResponses => {
            let events = render_openai_responses_stream_events(trace_id, model, text, now);
            if !is_stream {
                let mut completed = events
                    .last()
                    .and_then(|ev| ev.get("response"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if let Some(map) = completed.as_object_mut() {
                    map.insert("output_text".to_string(), json!(text));
                }
                return completed.to_string();
            }
            let mut out = String::new();
            for event in &events {
                let event_name = event.get("type").and_then(Value::as_str);
                append_sse_frame(&mut out, event_name, event);
            }
            out
        }
        ChatUsageTarget::AnthropicMessages => {
            let msg_id = format!("msg_quota_{suffix}");
            if !is_stream {
                return json!({
                    "id": msg_id,
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [{ "type": "text", "text": text }],
                    "stop_reason": "end_turn",
                    "stop_sequence": Value::Null,
                    "usage": { "input_tokens": 0, "output_tokens": 0 }
                })
                .to_string();
            }
            let mut out = String::new();
            append_sse_frame(
                &mut out,
                Some("message_start"),
                &json!({
                    "type": "message_start",
                    "message": {
                        "id": msg_id,
                        "type": "message",
                        "role": "assistant",
                        "model": model,
                        "content": [],
                        "stop_reason": Value::Null,
                        "stop_sequence": Value::Null,
                        "usage": {
                            "input_tokens": 0,
                            "cache_creation_input_tokens": 0,
                            "cache_read_input_tokens": 0,
                            "output_tokens": 0
                        }
                    }
                }),
            );
            append_sse_frame(
                &mut out,
                Some("content_block_start"),
                &json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": { "type": "text", "text": "" }
                }),
            );
            append_sse_frame(&mut out, Some("ping"), &json!({ "type": "ping" }));
            append_sse_frame(
                &mut out,
                Some("content_block_delta"),
                &json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": { "type": "text_delta", "text": text }
                }),
            );
            append_sse_frame(
                &mut out,
                Some("content_block_stop"),
                &json!({ "type": "content_block_stop", "index": 0 }),
            );
            append_sse_frame(
                &mut out,
                Some("message_delta"),
                &json!({
                    "type": "message_delta",
                    "delta": { "stop_reason": "end_turn", "stop_sequence": Value::Null },
                    "usage": { "output_tokens": 0 }
                }),
            );
            append_sse_frame(&mut out, Some("message_stop"), &json!({ "type": "message_stop" }));
            out
        }
        ChatUsageTarget::OpenAiChatCompletions => {
            let chat_id = format!("chatcmpl_quota_{suffix}");
            if !is_stream {
                return json!({
                    "id": chat_id,
                    "object": "chat.completion",
                    "created": now,
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": text },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0 }
                })
                .to_string();
            }
            let mut out = String::new();
            append_sse_frame(
                &mut out,
                None,
                &json!({
                    "id": chat_id,
                    "object": "chat.completion.chunk",
                    "created": now,
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "delta": { "role": "assistant", "content": "" },
                        "finish_reason": Value::Null
                    }]
                }),
            );
            append_sse_frame(
                &mut out,
                None,
                &json!({
                    "id": chat_id,
                    "object": "chat.completion.chunk",
                    "created": now,
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "delta": { "content": text },
                        "finish_reason": Value::Null
                    }]
                }),
            );
            append_sse_frame(
                &mut out,
                None,
                &json!({
                    "id": chat_id,
                    "object": "chat.completion.chunk",
                    "created": now,
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0 }
                }),
            );
            out.push_str("data: [DONE]\n\n");
            out
        }
    }
}

fn query_param<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        if key == name {
            Some(value)
        } else {
            None
        }
    })
}

fn wants_statusline(path: &str) -> bool {
    matches!(query_param(path, "format"), Some("statusline"))
        || query_param(path, "statusline") == Some("1")
}

fn wants_text(path: &str) -> bool {
    matches!(
        query_param(path, "format"),
        Some("text") | Some("pretty")
    ) || query_param(path, "pretty") == Some("1")
}

fn usd(cents: i64) -> String {
    format!("{:.2}", cents.max(0) as f64 / 100.0)
}

fn format_countdown(seconds: i64) -> String {
    let secs = seconds.max(0);
    if secs < 60 {
        return "<1m".to_string();
    }
    let total_mins = secs / 60;
    let days = total_mins / (24 * 60);
    let hours = (total_mins % (24 * 60)) / 60;
    let mins = total_mins % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

fn format_next_release(
    next_release_at: Option<i64>,
    next_release_cents: Option<i64>,
    now: i64,
) -> String {
    let Some(timestamp) = next_release_at else {
        return "none (no billable usage in window)".to_string();
    };
    let time_part = FixedOffset::east_opt(8 * 3600)
        .and_then(|tz| tz.timestamp_opt(timestamp, 0).single())
        .map(|dt| format!("{} (UTC+8)", dt.format("%Y-%m-%d %H:%M")))
        .unwrap_or_else(|| format!("{timestamp} (unix)"));
    let countdown = format_countdown((timestamp - now).max(0));
    match next_release_cents.filter(|c| *c > 0) {
        Some(cents) => format!("+${} in {} · {}", usd(cents), countdown, time_part),
        None => format!("in {} · {}", countdown, time_part),
    }
}

fn render_usage_statusline(
    used_cents: i64,
    limit_cents: i64,
    remaining_cents: i64,
    next_release_at: Option<i64>,
    next_release_cents: Option<i64>,
    now: i64,
) -> String {
    let remaining_pct = if limit_cents > 0 {
        (remaining_cents.max(0) as f64 * 100.0 / limit_cents as f64).min(100.0)
    } else {
        100.0
    };
    let base = format!(
        "${}/${} left ({:.1}%)",
        usd(remaining_cents),
        (limit_cents / 100).max(0),
        remaining_pct
    );
    match (next_release_at, next_release_cents.filter(|c| *c > 0)) {
        (Some(ts), Some(cents)) if used_cents > 0 => format!(
            "{base} · next +${} in {}",
            usd(cents),
            format_countdown((ts - now).max(0))
        ),
        (Some(ts), None) if used_cents > 0 => {
            format!("{base} · next in {}", format_countdown((ts - now).max(0)))
        }
        _ => base,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_usage_text(
    label: &str,
    status: &str,
    used_cents: i64,
    limit_cents: i64,
    remaining_cents: i64,
    next_release_at: Option<i64>,
    next_release_cents: Option<i64>,
    now: i64,
) -> String {
    let percent = if limit_cents > 0 {
        (used_cents.max(0) as f64 * 100.0 / limit_cents as f64).min(100.0)
    } else {
        0.0
    };
    let filled =
        ((USAGE_BAR_WIDTH as f64 * percent / 100.0).round() as usize).min(USAGE_BAR_WIDTH);
    let bar = format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(USAGE_BAR_WIDTH - filled)
    );

    format!(
        concat!(
            "\n",
            "  GPT USAGE{}rolling 7d\n",
            "  {}\n",
            "  {:<12}{}\n",
            "  {:<12}{}\n",
            "\n",
            "  ${}  remaining\n",
            "  ${} used of ${}  |  {:.1}% of window\n",
            "  {}\n",
            "\n",
            "  {:<12}{}\n",
            "  {:<12}{}\n",
            "\n",
            "  {}\n",
            "\n"
        ),
        " ".repeat(USAGE_RULE_WIDTH - 11 - 10),
        "─".repeat(USAGE_RULE_WIDTH),
        "MEMBER",
        label,
        "STATUS",
        status.to_uppercase(),
        usd(remaining_cents),
        usd(used_cents),
        usd(limit_cents),
        percent,
        bar,
        "NEXT FREE",
        format_next_release(next_release_at, next_release_cents, now),
        "WINDOW",
        "rolling 7 days",
        "─".repeat(USAGE_RULE_WIDTH),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        build_quota_headers_from_summary, build_ws_local_usage_reply, detect_chat_usage_command,
        is_usage_endpoint_path, is_ws_local_usage_frame, render_chat_quota_payload,
        render_usage_statusline, render_usage_text, ChatUsageTarget,
    };
    use codexmanager_core::storage::{now_ts, KeyRollingUsageSummary, Storage, SECONDS_PER_WEEK};
    use serde_json::{json, Value};

    #[test]
    fn text_usage_contains_visual_card() {
        let body = render_usage_text("Alice", "active", 0, 24_000, 24_000, None, None, 1_800_000_000);
        assert!(body.contains("GPT USAGE"));
        assert!(body.contains("MEMBER"));
        assert!(body.contains("Alice"));
        assert!(body.contains("$240.00"));
        assert!(body.contains("0.0% of window"));
        assert!(body.contains("rolling 7 days"));
    }

    #[test]
    fn text_usage_shows_release_amount_and_countdown() {
        let now = 1_800_000_000;
        let body = render_usage_text(
            "Alice",
            "active",
            5_360,
            24_000,
            18_640,
            Some(now + 9_000),
            Some(1_250),
            now,
        );
        assert!(body.contains("$186.40"));
        assert!(body.contains("+$12.50 in 2h 30m"));
        assert!(body.contains("UTC+8"));
    }

    #[test]
    fn statusline_renders_compact_summary() {
        let now = 1_800_000_000;
        let line = render_usage_statusline(5_360, 24_000, 18_640, Some(now + 9_000), Some(1_250), now);
        assert_eq!(line, "$186.40/$240 left (77.7%) · next +$12.50 in 2h 30m");
        assert_eq!(
            render_usage_statusline(0, 24_000, 24_000, None, None, now),
            "$240.00/$240 left (100.0%)"
        );
    }

    #[test]
    fn endpoint_paths_cover_aliases() {
        assert!(is_usage_endpoint_path("/v1/usage"));
        assert!(is_usage_endpoint_path("/usage?format=statusline"));
        assert!(is_usage_endpoint_path("/v1/v1/usage?pretty=1"));
        assert!(!is_usage_endpoint_path("/v1/responses"));
    }

    #[test]
    fn chat_command_intercept_handles_xml_wrappers_and_guards() {
        let wrapped_claude = json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 1024,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "<system-reminder>hook note</system-reminder>" },
                        { "type": "text", "text": "<user_query>/usage</user_query>" }
                    ]
                }
            ]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/messages?beta=true", "POST", Some(&wrapped_claude)),
            Some(ChatUsageTarget::AnthropicMessages)
        );

        let wrapped_codex = json!({
            "model": "gpt-5",
            "input": [
                { "role": "developer", "content": [{ "type": "input_text", "text": "rules" }] },
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "<permissions instructions>ok</permissions instructions>\n<environment_context><cwd>/tmp</cwd></environment_context>\n<collaboration_mode>default</collaboration_mode>\n?usage" }
                    ]
                }
            ]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/responses", "POST", Some(&wrapped_codex)),
            Some(ChatUsageTarget::OpenAiResponses)
        );

        let low_max_tokens = json!({
            "model": "claude-haiku",
            "max_tokens": 16,
            "messages": [{ "role": "user", "content": "/usage" }]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/messages", "POST", Some(&low_max_tokens)),
            None
        );

        let with_tool_result = json!({
            "model": "claude-sonnet",
            "max_tokens": 1024,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "ok" },
                    { "type": "text", "text": "/usage" }
                ]
            }]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/messages", "POST", Some(&with_tool_result)),
            None
        );

        let tool_output_last = json!({
            "model": "gpt-5",
            "input": [
                { "role": "user", "content": "?usage" },
                { "type": "function_call_output", "call_id": "c1", "output": "?usage" }
            ]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/responses", "POST", Some(&tool_output_last)),
            None
        );

        let normal_question = json!({
            "model": "gpt-5",
            "messages": [{ "role": "user", "content": "how do I check /usage in bash?" }]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/chat/completions", "POST", Some(&normal_question)),
            None
        );
    }

    #[test]
    fn quota_headers_and_sse_payloads_match_client_schemas() {
        let now = 1_800_000_000;
        let summary = KeyRollingUsageSummary {
            used_cents: 5_360,
            next_release_at: Some(now + SECONDS_PER_WEEK - 600),
            next_release_cents: Some(1_250),
        };
        let hdrs = build_quota_headers_from_summary(&summary, 24_000, now);
        let pairs: Vec<(String, String)> = hdrs
            .iter()
            .map(|h| {
                (
                    h.field.as_str().as_str().to_ascii_lowercase(),
                    h.value.as_str().to_string(),
                )
            })
            .collect();
        assert!(pairs.contains(&("x-codex-primary-used-percent".to_string(), "22".to_string())));
        assert!(pairs.contains(&("x-codex-primary-window-minutes".to_string(), "10080".to_string())));
        assert!(pairs.contains(&("x-codex-credits-balance".to_string(), "186.40".to_string())));
        assert!(pairs.contains(&("x-team-key-next-release-usd".to_string(), "12.50".to_string())));

        let responses_sse = render_chat_quota_payload(
            ChatUsageTarget::OpenAiResponses,
            true,
            "tr-1",
            "gpt-5",
            "card body",
            now,
        );
        assert!(responses_sse.contains("event: response.completed"));
        assert!(responses_sse.contains("\"annotations\":[]"));
        assert!(responses_sse.contains("\"sequence_number\":8"));

        let claude_sse = render_chat_quota_payload(
            ChatUsageTarget::AnthropicMessages,
            true,
            "tr-2",
            "claude-sonnet",
            "card body",
            now,
        );
        assert!(claude_sse.contains("event: message_start"));
        assert!(claude_sse.contains("event: message_stop"));

        let chat_json: Value = serde_json::from_str(
            &render_chat_quota_payload(
                ChatUsageTarget::OpenAiChatCompletions,
                false,
                "tr-3",
                "gpt-5",
                "card body",
                now,
            ),
        )
        .expect("valid chat json");
        assert_eq!(chat_json["choices"][0]["message"]["content"], "card body");
        assert_eq!(chat_json["usage"]["total_tokens"], 0);
    }

    #[test]
    fn ws_local_usage_detects_codex_response_create_and_builds_events() {
        let storage = Storage::open_in_memory().expect("open memory storage");
        storage.init().expect("init storage");
        let now = now_ts();
        storage
            .insert_api_key(&codexmanager_core::storage::ApiKey {
                id: "gk_ws_test".to_string(),
                name: Some("WebSocket Tester".to_string()),
                model_slug: None,
                reasoning_effort: None,
                service_tier: None,
                rotation_strategy: "account_rotation".to_string(),
                upstream_provider: codexmanager_core::storage::UpstreamProvider::Openai,
                aggregate_api_id: None,
                account_plan_filter: None,
                aggregate_api_url: None,
                client_type: "codex".to_string(),
                protocol_type: "openai_compat".to_string(),
                auth_scheme: "authorization_bearer".to_string(),
                upstream_base_url: None,
                static_headers_json: None,
                key_hash: "hash_ws".to_string(),
                status: "active".to_string(),
                created_at: now - 3600,
                last_used_at: None,
            })
            .expect("insert api key");

        let ws_usage_frame = r#"{"type":"response.create","model":"gpt-5.4","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>"}]},{"type":"message","role":"user","content":[{"type":"input_text","text":"?usage"}]}]}"#;
        assert!(is_ws_local_usage_frame(ws_usage_frame));
        let reply = build_ws_local_usage_reply(&storage, "gk_ws_test", "tr_ws_1", ws_usage_frame)
            .expect("build reply")
            .expect("should match");
        assert_eq!(reply.events.len(), 9);
        assert_eq!(reply.client_model.as_deref(), Some("gpt-5.4"));
        assert!(reply.events[8].contains(r#""type":"response.completed""#));
        assert!(reply.events[4].contains("$240.00"));

        let normal_ws_frame = r#"{"type":"response.create","model":"gpt-5.4","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}]}"#;
        assert!(!is_ws_local_usage_frame(normal_ws_frame));
        let non_create_frame = r#"{"type":"response.cancel","response_id":"resp_1"}"#;
        assert!(!is_ws_local_usage_frame(non_create_frame));
    }

    #[test]
    fn claude_code_trailing_system_and_wrappers_match_local_usage_safely() {
        let claude_2_1_312_wire = json!({
            "model": "gpt-6-astra",
            "max_tokens": 1024,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "<system-reminder>git attribution</system-reminder>\n" },
                        { "type": "text", "text": "?usage" }
                    ]
                },
                {
                    "role": "system",
                    "content": [
                        { "type": "text", "text": "# Environment\nworking dir: /tmp" }
                    ]
                }
            ]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/messages?beta=true", "POST", Some(&claude_2_1_312_wire)),
            Some(ChatUsageTarget::AnthropicMessages)
        );
        assert_eq!(
            detect_chat_usage_command("/v1/v1/messages?beta=true", "POST", Some(&claude_2_1_312_wire)),
            Some(ChatUsageTarget::AnthropicMessages)
        );

        let trailing_xml = json!({
            "model": "gpt-6-astra",
            "max_tokens": 1024,
            "messages": [
                {
                    "role": "user",
                    "content": "查额度\n<system-reminder>after hook</system-reminder>"
                },
                {
                    "role": "developer",
                    "content": "runtime instructions"
                }
            ]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/messages", "POST", Some(&trailing_xml)),
            Some(ChatUsageTarget::AnthropicMessages)
        );

        let assistant_followed = json!({
            "model": "gpt-6-astra",
            "max_tokens": 1024,
            "messages": [
                { "role": "user", "content": "?usage" },
                { "role": "assistant", "content": "already answered" },
                { "role": "system", "content": "# Environment" }
            ]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/messages", "POST", Some(&assistant_followed)),
            None
        );

        let tool_result_followed = json!({
            "model": "gpt-6-astra",
            "max_tokens": 1024,
            "messages": [
                { "role": "user", "content": "?usage" },
                { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }] },
                { "role": "system", "content": "# Environment" }
            ]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/messages", "POST", Some(&tool_result_followed)),
            None
        );

        let json_schema_output = json!({
            "model": "gpt-6-astra",
            "max_tokens": 1024,
            "output_config": {
                "format": {
                    "type": "json_schema",
                    "schema": { "type": "object" }
                }
            },
            "messages": [{ "role": "user", "content": "?usage" }]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/messages", "POST", Some(&json_schema_output)),
            None
        );

        let forced_tool = json!({
            "model": "gpt-6-astra",
            "max_tokens": 1024,
            "tool_choice": { "type": "tool", "name": "StructuredOutput" },
            "messages": [{ "role": "user", "content": "?usage" }]
        });
        assert_eq!(
            detect_chat_usage_command("/v1/messages", "POST", Some(&forced_tool)),
            None
        );
    }
}
