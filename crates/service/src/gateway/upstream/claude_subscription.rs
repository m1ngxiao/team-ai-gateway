use bytes::Bytes;
use codexmanager_core::storage::{
    ClaudeSubscriptionAccount, ManagedModelV2, ModelRouteV2, Storage,
};
use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::{Method, Url};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tiny_http::Request;

use super::super::{PassthroughSseProtocol, ResponseAdapter};
use super::GatewayUpstreamResponse;

const CLAUDE_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const CLAUDE_CODE_SYSTEM_PREFIX: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const MAX_CLAUDE_ACCOUNT_ATTEMPTS: usize = 2;
static NEXT_CLAUDE_ACCOUNT: AtomicUsize = AtomicUsize::new(0);

pub(super) struct ClaudeSubscriptionProxyRequest<'a> {
    pub request: Option<Request>,
    pub storage: &'a Storage,
    pub trace_id: &'a str,
    pub key_id: &'a str,
    pub original_path: &'a str,
    pub path: &'a str,
    pub request_method: &'a str,
    pub method: &'a Method,
    pub body: &'a Bytes,
    pub client_is_stream: bool,
    pub configured_model: Option<&'a ManagedModelV2>,
    pub client_model_for_log: Option<&'a str>,
    pub model_for_log: Option<&'a str>,
    pub model_source_for_log: Option<&'a str>,
    pub client_reasoning_for_log: Option<&'a str>,
    pub reasoning_for_log: Option<&'a str>,
    pub reasoning_source_for_log: Option<&'a str>,
    pub service_tier_for_log: Option<&'a str>,
    pub effective_service_tier_for_log: Option<&'a str>,
    pub service_tier_source_for_log: Option<&'a str>,
    pub gateway_mode_for_log: Option<&'a str>,
    pub started_at: Instant,
    pub request_deadline: Option<Instant>,
}

fn route_model_override(model: Option<&ManagedModelV2>) -> Option<&str> {
    claude_route_model(&model?.routes)
}

fn claude_route_model(routes: &[ModelRouteV2]) -> Option<&str> {
    routes
        .iter()
        .filter(|route| {
            route.enabled
                && route.source_kind == "account_pool"
                && route.source_id == "claude"
                && !route.upstream_model.trim().is_empty()
        })
        .max_by_key(|route| route.priority)
        .map(|route| route.upstream_model.as_str())
}

fn build_messages_body(
    path: &str,
    body: &[u8],
    model_override: Option<&str>,
    client_is_stream: bool,
) -> Result<(Vec<u8>, ResponseAdapter), String> {
    let path_only = path.split('?').next().unwrap_or(path);
    match path_only {
        "/v1/responses" => {
            let original: Value = serde_json::from_slice(body)
                .map_err(|err| format!("invalid responses request JSON: {err}"))?;
            if has_server_scoped_reference(&original) {
                return Err("Claude subscription routing cannot migrate server-side conversation or file references".to_string());
            }
            let converted =
                super::super::protocol_adapter::adapt_openai_responses_to_anthropic_messages(
                    body,
                    model_override,
                )?;
            let mut converted: Value = serde_json::from_slice(&converted)
                .map_err(|err| format!("invalid converted Claude messages JSON: {err}"))?;
            converted["stream"] = Value::Bool(client_is_stream);
            let converted = serde_json::to_vec(&converted)
                .map_err(|err| format!("serialize Claude messages request failed: {err}"))?;
            Ok((converted, ResponseAdapter::ResponsesFromAnthropicMessages))
        }
        "/v1/messages" => {
            let original: Value = serde_json::from_slice(body)
                .map_err(|err| format!("invalid Claude messages request JSON: {err}"))?;
            if has_server_scoped_reference(&original) {
                return Err("Claude subscription routing cannot migrate server-side conversation or file references".to_string());
            }
            if !original.is_object() {
                return Err("Claude messages request must be a JSON object".to_string());
            }
            let Some(model_override) = model_override else {
                return Ok((body.to_vec(), ResponseAdapter::Passthrough));
            };
            let mut value = original;
            let object = value
                .as_object_mut()
                .ok_or_else(|| "Claude messages request must be a JSON object".to_string())?;
            object.insert(
                "model".to_string(),
                Value::String(model_override.to_string()),
            );
            let body = serde_json::to_vec(&value)
                .map_err(|err| format!("serialize Claude messages request failed: {err}"))?;
            Ok((body, ResponseAdapter::Passthrough))
        }
        _ => Err(format!(
            "Claude subscription pool does not support path: {path_only}"
        )),
    }
}

fn add_claude_code_system_prefix(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut value: Value = serde_json::from_slice(body)
        .map_err(|err| format!("invalid Claude messages request JSON: {err}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "Claude messages request must be a JSON object".to_string())?;
    let existing = object.remove("system");
    let mut blocks = match existing {
        None => Vec::new(),
        Some(Value::String(text)) => vec![serde_json::json!({"type": "text", "text": text})],
        Some(Value::Array(blocks)) => blocks,
        Some(_) => {
            return Err(
                "Claude messages system must be text or an array of content blocks".to_string(),
            )
        }
    };
    let already_present = blocks.first().is_some_and(|block| {
        block.get("type").and_then(Value::as_str) == Some("text")
            && block.get("text").and_then(Value::as_str) == Some(CLAUDE_CODE_SYSTEM_PREFIX)
    });
    if !already_present {
        blocks.insert(
            0,
            serde_json::json!({"type": "text", "text": CLAUDE_CODE_SYSTEM_PREFIX}),
        );
    }
    object.insert("system".to_string(), Value::Array(blocks));
    serde_json::to_vec(&value)
        .map_err(|err| format!("serialize Claude messages request failed: {err}"))
}

fn messages_url(path: &str) -> Result<Url, String> {
    let mut url = Url::parse(CLAUDE_MESSAGES_URL)
        .map_err(|err| format!("invalid Claude upstream URL: {err}"))?;
    if let Some((_, query)) = path.split_once('?') {
        url.set_query(Some(query));
    }
    let query = url
        .query_pairs()
        .filter(|(key, _)| key != "beta")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(&key, &value);
        }
        pairs.append_pair("beta", "true");
    }
    Ok(url)
}

const DEFAULT_ANTHROPIC_BETA: &str = "claude-code-20250219,oauth-2025-04-20";

fn should_forward_claude_request_header(name: &str) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "x-api-key"
            | "api-key"
            | "x-goog-api-key"
            | "content-length"
            | "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
}

fn request_has_header(request: &Request, name: &str) -> bool {
    request
        .headers()
        .iter()
        .any(|header| header.field.to_string().eq_ignore_ascii_case(name))
}

fn build_upstream_request(
    client: &Client,
    request: &Request,
    method: &Method,
    url: Url,
    body: &[u8],
    access_token: &str,
    deadline: Option<Instant>,
) -> Result<reqwest::blocking::Request, String> {
    if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
        return Err("Claude upstream request deadline expired".to_string());
    }
    let mut builder = client
        .request(method.clone(), url)
        .bearer_auth(access_token)
        .body(body.to_vec());
    if let Some(remaining) =
        deadline.and_then(|deadline| deadline.checked_duration_since(Instant::now()))
    {
        builder = builder.timeout(remaining);
    }
    let mut upstream_request = builder
        .build()
        .map_err(|err| format!("build Claude upstream request failed: {err}"))?;

    // Forward the open Anthropic header surface verbatim. In particular, do not
    // merge or normalize duplicate anthropic-beta values: Claude Code uses the
    // safeguards beta value as a protocol signal.
    let mut has_anthropic_version = false;
    let mut has_anthropic_beta = false;
    for header in request.headers() {
        let raw_name = header.field.to_string();
        if !should_forward_claude_request_header(raw_name.as_str()) {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(raw_name.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(header.value.as_str()) else {
            continue;
        };
        has_anthropic_version |= name.as_str().eq_ignore_ascii_case("anthropic-version");
        has_anthropic_beta |= name.as_str().eq_ignore_ascii_case("anthropic-beta");
        upstream_request.headers_mut().append(name, value);
    }
    if !has_anthropic_version {
        upstream_request.headers_mut().insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static("2023-06-01"),
        );
    }
    // Keep the OAuth/Claude Code negotiation for older clients that omit all
    // beta headers. If Claude Code supplied any value, its complete set is
    // forwarded unchanged.
    if !has_anthropic_beta {
        upstream_request.headers_mut().append(
            HeaderName::from_static("anthropic-beta"),
            HeaderValue::from_static(DEFAULT_ANTHROPIC_BETA),
        );
    }
    if !request_has_header(request, "x-app") {
        upstream_request.headers_mut().insert(
            HeaderName::from_static("x-app"),
            HeaderValue::from_static("cli"),
        );
    }
    if !request_has_header(request, "user-agent") {
        upstream_request.headers_mut().insert(
            HeaderName::from_static("user-agent"),
            HeaderValue::from_static("claude-code/2.1.280"),
        );
    }
    if !request_has_header(request, CONTENT_TYPE.as_str()) {
        upstream_request.headers_mut().insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );
    }
    if !request_has_header(request, ACCEPT.as_str()) {
        upstream_request.headers_mut().insert(
            HeaderName::from_static("accept"),
            HeaderValue::from_static("application/json, text/event-stream"),
        );
    }
    Ok(upstream_request)
}

fn choose_candidate_id(
    accounts: &[ClaudeSubscriptionAccount],
    attempted: &HashSet<String>,
    inflight: &HashMap<String, usize>,
    start: usize,
    limit: usize,
) -> Option<String> {
    if accounts.is_empty() {
        return None;
    }
    (0..accounts.len())
        .map(|offset| &accounts[(start + offset) % accounts.len()])
        .filter(|account| !attempted.contains(&account.id))
        .filter(|account| limit == 0 || inflight.get(&account.id).copied().unwrap_or(0) < limit)
        .min_by_key(|account| inflight.get(&account.id).copied().unwrap_or(0))
        .map(|account| account.id.clone())
}

fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 503 | 529)
}

fn has_server_scoped_reference(value: &Value) -> bool {
    let Some(request) = value.as_object() else {
        return false;
    };
    if ["previous_response_id", "conversation", "container"]
        .into_iter()
        .any(|field| request.get(field).is_some_and(|value| !value.is_null()))
    {
        return true;
    }
    request
        .get("messages")
        .or_else(|| request.get("input"))
        .is_some_and(content_has_server_scoped_file)
}

fn content_has_server_scoped_file(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(content_has_server_scoped_file),
        Value::Object(object) => {
            let kind = object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if matches!(kind, "input_file" | "file")
                && object.get("file_id").and_then(Value::as_str).is_some()
            {
                return true;
            }
            if matches!(kind, "document" | "image")
                && object
                    .get("source")
                    .and_then(Value::as_object)
                    .is_some_and(|source| {
                        source.get("type").and_then(Value::as_str) == Some("file")
                            && source.get("file_id").and_then(Value::as_str).is_some()
                    })
            {
                return true;
            }
            // Only inspect message/content structure. Tool schemas and tool arguments may
            // legitimately contain ordinary properties named file_id or container.
            object
                .get("content")
                .is_some_and(content_has_server_scoped_file)
        }
        _ => false,
    }
}

fn safe_to_retry_before_delivery(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .is_some_and(|value| !has_server_scoped_reference(&value))
}

fn active_account_now(
    storage: &Storage,
    account_id: &str,
) -> Result<Option<ClaudeSubscriptionAccount>, String> {
    storage
        .find_claude_subscription_account(account_id)
        .map_err(|err| format!("recheck Claude subscription account failed: {err}"))
        .map(|account| {
            account.filter(|account| account.status == "active" && !account.access_token.is_empty())
        })
}

fn finish_request(
    params: &ClaudeSubscriptionProxyRequest<'_>,
    status: u16,
    account_id: Option<&str>,
    upstream_url: Option<&str>,
    adapter: ResponseAdapter,
    upstream_model: Option<&str>,
    usage: super::super::request_log::RequestLogUsage,
    error: Option<&str>,
    attempted_ids: &[String],
) {
    super::super::record_gateway_request_outcome(params.path, status, Some("claude_subscription"));
    super::super::trace_log::log_request_final(
        params.trace_id,
        status,
        account_id,
        upstream_url,
        error,
        params.started_at.elapsed().as_millis(),
    );
    super::super::request_log::write_request_log_with_attempts(
        params.storage,
        super::super::request_log::RequestLogTraceContext {
            trace_id: Some(params.trace_id),
            original_path: Some(params.original_path),
            adapted_path: Some(params.path),
            request_type: Some("http"),
            gateway_mode: params.gateway_mode_for_log,
            route_strategy: Some("account_rotation"),
            route_source: Some("claude_subscription"),
            client_model: params.client_model_for_log,
            model_source: params.model_source_for_log,
            client_reasoning_effort: params.client_reasoning_for_log,
            reasoning_source: params.reasoning_source_for_log,
            service_tier: params.service_tier_for_log,
            effective_service_tier: params.effective_service_tier_for_log,
            service_tier_source: params.service_tier_source_for_log,
            response_adapter: Some(adapter),
            upstream_model,
            actual_source_kind: account_id.map(|_| "claude_subscription_account"),
            actual_source_id: account_id,
            ..Default::default()
        },
        Some(params.key_id),
        account_id,
        params.path,
        params.request_method,
        params.model_for_log,
        params.reasoning_for_log,
        upstream_url,
        Some(status),
        usage,
        error,
        Some(params.started_at.elapsed().as_millis()),
        Some(attempted_ids),
    );
}

fn deliver_upstream(
    mut params: ClaudeSubscriptionProxyRequest<'_>,
    upstream: Response,
    inflight_guard: super::super::AccountInFlightGuard,
    account_id: &str,
    url: &str,
    adapter: ResponseAdapter,
    upstream_model: Option<&str>,
    attempted_ids: &[String],
) -> Result<(), String> {
    let is_stream = params.client_is_stream;
    let passthrough_sse_protocol = (adapter == ResponseAdapter::Passthrough)
        .then_some(PassthroughSseProtocol::AnthropicNative);
    let quota_headers = super::super::local_usage::build_key_quota_response_headers(
        params.storage,
        params.key_id,
    );
    let request = params
        .request
        .take()
        .ok_or_else(|| "Claude request already consumed".to_string())?;
    let bridge = super::super::respond_with_upstream(
        request,
        GatewayUpstreamResponse::Blocking(upstream),
        inflight_guard,
        adapter,
        passthrough_sse_protocol,
        None,
        params.path,
        None,
        is_stream,
        false,
        Some(params.trace_id),
        upstream_model.or(params.model_for_log),
        params.started_at,
        None,
        None,
        &quota_headers,
    )?;
    let mut status = bridge
        .delivered_status_code
        .unwrap_or(if bridge.is_ok(is_stream) { 200 } else { 502 });
    let error = bridge
        .upstream_error_hint
        .clone()
        .or_else(|| bridge.error_message(is_stream));
    if error.is_some() && status < 400 {
        status = 502;
    }
    finish_request(
        &params,
        status,
        Some(account_id),
        Some(url),
        adapter,
        upstream_model,
        super::super::request_log::RequestLogUsage {
            input_tokens: bridge.usage.input_tokens,
            cached_input_tokens: bridge.usage.cached_input_tokens,
            cache_write_tokens: bridge.usage.cache_write_tokens,
            output_tokens: bridge.usage.output_tokens,
            total_tokens: bridge.usage.total_tokens,
            reasoning_output_tokens: bridge.usage.reasoning_output_tokens,
            first_response_ms: bridge.usage.first_response_ms,
            estimated_input_tokens: Some(
                super::super::request_log::estimate_input_tokens_from_body(params.body),
            ),
        },
        error.as_deref(),
        attempted_ids,
    );
    Ok(())
}

fn respond_error(
    mut params: ClaudeSubscriptionProxyRequest<'_>,
    status: u16,
    message: String,
    adapter: ResponseAdapter,
    attempted_ids: &[String],
) -> Result<(), String> {
    finish_request(
        &params,
        status,
        attempted_ids.last().map(String::as_str),
        None,
        adapter,
        None,
        super::super::request_log::RequestLogUsage::default(),
        Some(&message),
        attempted_ids,
    );
    let request = params
        .request
        .take()
        .ok_or_else(|| "Claude request already consumed".to_string())?;
    let response = super::super::error_response::terminal_text_response(
        status,
        super::super::error_message_for_client(
            super::super::prefers_raw_errors_for_tiny_http_request(&request),
            message,
        ),
        Some(params.trace_id),
    );
    let _ = request.respond(response);
    Ok(())
}

pub(super) fn proxy_claude_subscription_request(
    params: ClaudeSubscriptionProxyRequest<'_>,
) -> Result<(), String> {
    let url = match messages_url(params.path) {
        Ok(url) => url,
        Err(err) => return respond_error(params, 500, err, ResponseAdapter::Passthrough, &[]),
    };
    proxy_claude_subscription_request_with_transport(params, url, |account_id| {
        super::super::runtime_config::upstream_client_for_account(account_id)
    })
}

fn proxy_claude_subscription_request_with_transport(
    params: ClaudeSubscriptionProxyRequest<'_>,
    url: Url,
    client_for_account: impl Fn(&str) -> Result<Client, String>,
) -> Result<(), String> {
    let mut attempted_ids = Vec::new();
    if *params.method != Method::POST {
        return respond_error(
            params,
            405,
            "Claude subscription requests require POST".to_string(),
            ResponseAdapter::Passthrough,
            &attempted_ids,
        );
    }
    let upstream_model = route_model_override(params.configured_model);
    let (upstream_body, adapter) = match build_messages_body(
        params.path,
        params.body,
        upstream_model,
        params.client_is_stream,
    ) {
        Ok(result) => result,
        Err(err) => {
            return respond_error(
                params,
                400,
                err,
                ResponseAdapter::Passthrough,
                &attempted_ids,
            )
        }
    };
    let upstream_body = match add_claude_code_system_prefix(&upstream_body) {
        Ok(body) => body,
        Err(err) => return respond_error(params, 400, err, adapter, &attempted_ids),
    };
    let url_label = url.as_str().to_string();
    let accounts = match params.storage.list_active_claude_subscription_accounts() {
        Ok(accounts) => accounts,
        Err(err) => {
            return respond_error(
                params,
                500,
                format!("read Claude subscription accounts failed: {err}"),
                adapter,
                &attempted_ids,
            )
        }
    };
    if accounts.is_empty() {
        return respond_error(
            params,
            503,
            "no available Claude subscription account".to_string(),
            adapter,
            &attempted_ids,
        );
    }

    let account_start = NEXT_CLAUDE_ACCOUNT.fetch_add(1, Ordering::Relaxed) % accounts.len();
    let limit = super::super::runtime_config::account_max_inflight_limit();
    let mut attempted = HashSet::new();
    let mut last_error = None;
    let mut last_retryable_status = None;
    for _ in 0..MAX_CLAUDE_ACCOUNT_ATTEMPTS.min(accounts.len()) {
        let mut unavailable = attempted.clone();
        unavailable.extend(
            accounts
                .iter()
                .filter(|account| super::super::is_account_in_cooldown(&account.id))
                .map(|account| account.id.clone()),
        );
        let Some((guard, _)) =
            super::super::metrics::select_and_acquire_account_inflight(|inflight| {
                choose_candidate_id(&accounts, &unavailable, inflight, account_start, limit)
            })
        else {
            break;
        };
        let account_id = guard.account_id().to_string();
        attempted.insert(account_id.clone());
        attempted_ids.push(account_id.clone());
        let current = match active_account_now(params.storage, &account_id) {
            Ok(Some(account)) => account,
            Ok(None) => {
                drop(guard);
                continue;
            }
            Err(err) => {
                last_error = Some(err);
                drop(guard);
                continue;
            }
        };
        if let Err(err) =
            crate::claude_subscription_auth::refresh_account_token(params.storage, &current)
        {
            last_error = Some(err);
            super::super::mark_account_cooldown(&account_id, super::super::CooldownReason::Network);
            drop(guard);
            continue;
        }
        let client = match client_for_account(&account_id) {
            Ok(client) => client,
            Err(err) => {
                last_error = Some(err);
                drop(guard);
                continue;
            }
        };
        // The list and token refresh can race with an administrator disabling the account.
        // Use the current DB row for both status and token immediately before sending.
        let refreshed = match active_account_now(params.storage, &account_id) {
            Ok(Some(account)) => account,
            Ok(None) => {
                drop(guard);
                continue;
            }
            Err(err) => {
                last_error = Some(err);
                drop(guard);
                continue;
            }
        };
        let upstream_request = match build_upstream_request(
            &client,
            params
                .request
                .as_ref()
                .ok_or_else(|| "Claude request already consumed".to_string())?,
            params.method,
            url.clone(),
            &upstream_body,
            &refreshed.access_token,
            params.request_deadline,
        ) {
            Ok(request) => request,
            Err(err) => {
                last_error = Some(err);
                drop(guard);
                continue;
            }
        };
        match client.execute(upstream_request) {
            Ok(response) => {
                let status = response.status().as_u16();
                if status == 401 {
                    let _ = crate::claude_subscription_auth::mark_account_needs_login_after_401(
                        params.storage,
                        &account_id,
                        &refreshed.access_token,
                        &refreshed.refresh_token,
                    );
                    last_error = Some("Claude subscription login expired".to_string());
                    drop(guard);
                    continue;
                }
                if is_retryable_status(status) {
                    super::super::mark_account_cooldown_for_status(&account_id, status);
                } else if status < 400 {
                    super::super::clear_account_cooldown(&account_id);
                }
                if is_retryable_status(status)
                    && safe_to_retry_before_delivery(&upstream_body)
                    && attempted_ids.len() < MAX_CLAUDE_ACCOUNT_ATTEMPTS
                    && attempted.len() < accounts.len()
                {
                    last_error = Some(format!("Claude upstream returned HTTP {status}"));
                    last_retryable_status = Some(status);
                    drop(response);
                    drop(guard);
                    super::super::record_gateway_failover_attempt();
                    continue;
                }
                return deliver_upstream(
                    params,
                    response,
                    guard,
                    &account_id,
                    &url_label,
                    adapter,
                    upstream_model,
                    &attempted_ids,
                );
            }
            Err(err) => {
                let message = format!("Claude upstream request failed: {err}");
                super::super::mark_account_cooldown(
                    &account_id,
                    super::super::CooldownReason::Network,
                );
                drop(guard);
                return respond_error(params, 502, message, adapter, &attempted_ids);
            }
        }
    }
    let status =
        last_retryable_status.unwrap_or(if attempted_ids.is_empty() || last_error.is_none() {
            503
        } else {
            502
        });
    respond_error(
        params,
        status,
        last_error.unwrap_or_else(|| "no available Claude subscription account".to_string()),
        adapter,
        &attempted_ids,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

    fn account(id: &str) -> ClaudeSubscriptionAccount {
        ClaudeSubscriptionAccount {
            id: id.to_string(),
            label: id.to_string(),
            email: None,
            account_uuid: None,
            organization_uuid: None,
            subscription_type: None,
            status: "active".to_string(),
            sort: 0,
            access_token: format!("token-{id}"),
            refresh_token: "test-refresh".to_string(),
            scopes: "user:inference".to_string(),
            expires_at: i64::MAX,
            last_error: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn storage_with_accounts(ids: &[&str]) -> Storage {
        let storage = Storage::open_in_memory().expect("open storage");
        storage.init().expect("initialize storage");
        for id in ids {
            storage
                .upsert_claude_subscription_account(&account(id))
                .expect("insert Claude account");
        }
        storage
    }

    fn run_mocked_proxy(
        storage: &Storage,
        upstream_url: Url,
        path: &str,
        payload: Value,
        client_is_stream: bool,
    ) -> (u16, String) {
        run_mocked_proxy_with_client(
            storage,
            upstream_url,
            path,
            payload,
            client_is_stream,
            |_| Ok(Client::new()),
        )
    }

    fn run_mocked_proxy_with_client(
        storage: &Storage,
        upstream_url: Url,
        path: &str,
        payload: Value,
        client_is_stream: bool,
        client_for_account: impl Fn(&str) -> Result<Client, String>,
    ) -> (u16, String) {
        let server = Server::http("127.0.0.1:0").expect("start local gateway");
        let gateway_url = format!("http://{}{}", server.server_addr(), path);
        let payload_bytes = serde_json::to_vec(&payload).expect("serialize request");
        let client = thread::spawn(move || {
            let response = Client::new()
                .post(gateway_url)
                .bearer_auth("platform-key-must-not-escape")
                .header("anthropic-beta", "interleaved-thinking-2025-05-14")
                .body(payload_bytes)
                .send()
                .expect("receive gateway response");
            let status = response.status().as_u16();
            (status, response.text().expect("read gateway response"))
        });
        let request = server
            .recv_timeout(Duration::from_secs(3))
            .expect("receive gateway request")
            .expect("gateway request present");
        let body = Bytes::from(serde_json::to_vec(&payload).expect("serialize request"));
        let method = Method::POST;
        proxy_claude_subscription_request_with_transport(
            ClaudeSubscriptionProxyRequest {
                request: Some(request),
                storage,
                trace_id: "claude-subscription-mock",
                key_id: "platform-test-key",
                original_path: path,
                path,
                request_method: "POST",
                method: &method,
                body: &body,
                client_is_stream,
                configured_model: None,
                client_model_for_log: Some("claude-test"),
                model_for_log: Some("claude-test"),
                model_source_for_log: Some("client_request"),
                client_reasoning_for_log: None,
                reasoning_for_log: None,
                reasoning_source_for_log: None,
                service_tier_for_log: None,
                effective_service_tier_for_log: None,
                service_tier_source_for_log: None,
                gateway_mode_for_log: None,
                started_at: Instant::now(),
                request_deadline: None,
            },
            upstream_url,
            client_for_account,
        )
        .expect("proxy Claude request");
        client.join().expect("join gateway client")
    }

    fn mock_upstream_url(server: &Server) -> Url {
        Url::parse(&format!(
            "http://{}/v1/messages?beta=true",
            server.server_addr()
        ))
        .expect("parse mock upstream URL")
    }

    fn response_with_type(
        body: &str,
        status: u16,
        content_type: &str,
    ) -> TinyResponse<std::io::Cursor<Vec<u8>>> {
        TinyResponse::from_string(body)
            .with_status_code(StatusCode(status))
            .with_header(Header::from_bytes("content-type", content_type).expect("content type"))
    }

    #[test]
    fn responses_use_anthropic_messages_and_preserve_model_override() {
        let body = serde_json::json!({"model":"claude-client", "input":"hi", "stream":true});
        let (rewritten, adapter) = build_messages_body(
            "/v1/responses",
            &serde_json::to_vec(&body).unwrap(),
            Some("claude-upstream"),
            true,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&rewritten).unwrap();
        assert_eq!(adapter, ResponseAdapter::ResponsesFromAnthropicMessages);
        assert_eq!(value["model"], "claude-upstream");
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["stream"], true);
    }

    #[test]
    fn oauth_messages_prepend_cli_system_block_without_losing_client_instructions() {
        let native = serde_json::json!({
            "model": "claude-sonnet-5",
            "system": "Keep the answer brief.",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 32,
        });
        let rewritten =
            add_claude_code_system_prefix(&serde_json::to_vec(&native).unwrap()).unwrap();
        let value: Value = serde_json::from_slice(&rewritten).unwrap();
        assert_eq!(value["system"][0]["text"], CLAUDE_CODE_SYSTEM_PREFIX);
        assert_eq!(value["system"][1]["text"], "Keep the answer brief.");
        assert_eq!(value["messages"], native["messages"]);

        let rewritten_again = add_claude_code_system_prefix(&rewritten).unwrap();
        let unchanged: Value = serde_json::from_slice(&rewritten_again).unwrap();
        assert_eq!(unchanged["system"].as_array().unwrap().len(), 2);

        let responses = serde_json::json!({"model": "claude-sonnet-5", "input": "hi"});
        let (converted, _) = build_messages_body(
            "/v1/responses",
            &serde_json::to_vec(&responses).unwrap(),
            None,
            false,
        )
        .unwrap();
        let converted = add_claude_code_system_prefix(&converted).unwrap();
        let converted: Value = serde_json::from_slice(&converted).unwrap();
        assert_eq!(converted["system"][0]["text"], CLAUDE_CODE_SYSTEM_PREFIX);
    }

    #[test]
    fn non_streaming_messages_and_responses_keep_response_shape() {
        let native = serde_json::json!({"model":"claude-client", "messages":[{"role":"user","content":"hi"}], "max_tokens":32, "stream":false});
        let (native_body, native_adapter) = build_messages_body(
            "/v1/messages",
            &serde_json::to_vec(&native).unwrap(),
            Some("claude-upstream"),
            false,
        )
        .unwrap();
        let native_value: Value = serde_json::from_slice(&native_body).unwrap();
        assert_eq!(native_adapter, ResponseAdapter::Passthrough);
        assert_eq!(native_value["stream"], false);
        assert_eq!(native_value["model"], "claude-upstream");

        let responses = serde_json::json!({"model":"claude-client", "input":"hi", "stream":false});
        let (converted, adapter) = build_messages_body(
            "/v1/responses",
            &serde_json::to_vec(&responses).unwrap(),
            None,
            false,
        )
        .unwrap();
        let converted: Value = serde_json::from_slice(&converted).unwrap();
        assert_eq!(adapter, ResponseAdapter::ResponsesFromAnthropicMessages);
        assert_eq!(converted["stream"], false);

        let omitted = serde_json::json!({"model":"claude-client", "input":"hi"});
        let (converted, _) = build_messages_body(
            "/v1/responses",
            &serde_json::to_vec(&omitted).unwrap(),
            None,
            false,
        )
        .unwrap();
        let converted: Value = serde_json::from_slice(&converted).unwrap();
        assert_eq!(converted["stream"], false);

        // The shared Codex request rewrite can set the transport body to stream=true
        // even when the client omitted stream. Claude must honor the original flag.
        let rewritten_omitted =
            serde_json::json!({"model":"claude-client", "input":"hi", "stream":true});
        let (converted, _) = build_messages_body(
            "/v1/responses",
            &serde_json::to_vec(&rewritten_omitted).unwrap(),
            None,
            false,
        )
        .unwrap();
        let converted: Value = serde_json::from_slice(&converted).unwrap();
        assert_eq!(converted["stream"], false);
    }

    #[test]
    fn claude_paths_keep_oauth_beta_query() {
        assert_eq!(
            messages_url("/v1/messages").unwrap().as_str(),
            "https://api.anthropic.com/v1/messages?beta=true"
        );
        assert_eq!(
            messages_url("/v1/messages?beta=true").unwrap().as_str(),
            "https://api.anthropic.com/v1/messages?beta=true"
        );
        assert_eq!(
            messages_url("/v1/messages?beta=false").unwrap().as_str(),
            "https://api.anthropic.com/v1/messages?beta=true"
        );
    }

    #[test]
    fn upstream_builder_preserves_anthropic_beta_duplicates_and_version() {
        let request: Request = tiny_http::TestRequest::new()
            .with_header(Header::from_bytes("anthropic-beta", "safeguards-review-2025").unwrap())
            .with_header(
                Header::from_bytes("anthropic-beta", "interleaved-thinking-2025-05-14").unwrap(),
            )
            .with_header(Header::from_bytes("anthropic-version", "2023-06-01").unwrap())
            .with_header(Header::from_bytes("x-claude-code-test", "preserve-me").unwrap())
            .into();
        let method = Method::POST;
        let upstream = build_upstream_request(
            &Client::new(),
            &request,
            &method,
            Url::parse("http://127.0.0.1:1/v1/messages").unwrap(),
            br#"{"messages":[]}"#,
            "token",
            None,
        )
        .expect("build upstream request");
        let beta_values = upstream
            .headers()
            .get_all("anthropic-beta")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(beta_values[0], "safeguards-review-2025");
        assert_eq!(beta_values[1], "interleaved-thinking-2025-05-14");
        assert_eq!(beta_values.len(), 2);
        assert_eq!(
            upstream
                .headers()
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some("2023-06-01")
        );
        assert_eq!(
            upstream
                .headers()
                .get("x-claude-code-test")
                .and_then(|value| value.to_str().ok()),
            Some("preserve-me")
        );
    }

    #[test]
    fn route_override_uses_highest_priority_claude_pool_only() {
        let routes = vec![
            ModelRouteV2 {
                source_kind: "account_pool".into(),
                source_id: "default".into(),
                upstream_model: "openai-model".into(),
                priority: 99,
                enabled: true,
                ..Default::default()
            },
            ModelRouteV2 {
                source_kind: "account_pool".into(),
                source_id: "claude".into(),
                upstream_model: "claude-low".into(),
                priority: 1,
                enabled: true,
                ..Default::default()
            },
            ModelRouteV2 {
                source_kind: "account_pool".into(),
                source_id: "claude".into(),
                upstream_model: "claude-high".into(),
                priority: 2,
                enabled: true,
                ..Default::default()
            },
        ];
        assert_eq!(claude_route_model(&routes), Some("claude-high"));
    }

    #[test]
    fn account_selection_balances_load_and_respects_capacity() {
        let accounts = vec![account("claude:one"), account("claude:two")];
        let attempted = HashSet::new();
        let loads = HashMap::from([("claude:one".to_string(), 1), ("claude:two".to_string(), 0)]);
        assert_eq!(
            choose_candidate_id(&accounts, &attempted, &loads, 0, 1).as_deref(),
            Some("claude:two")
        );
        assert_eq!(
            choose_candidate_id(&accounts, &attempted, &HashMap::new(), 1, 1).as_deref(),
            Some("claude:two")
        );
        let attempted = HashSet::from(["claude:two".to_string()]);
        assert_eq!(
            choose_candidate_id(&accounts, &attempted, &HashMap::new(), 1, 1).as_deref(),
            Some("claude:one")
        );
    }

    #[test]
    fn account_selection_skips_claude_cooldown() {
        let first = "claude:cooldown-first";
        let second = "claude:cooldown-second";
        let accounts = vec![account(first), account(second)];
        super::super::super::mark_account_cooldown(
            first,
            super::super::super::CooldownReason::RateLimited,
        );
        let unavailable = accounts
            .iter()
            .filter(|account| super::super::super::is_account_in_cooldown(&account.id))
            .map(|account| account.id.clone())
            .collect::<HashSet<_>>();
        assert_eq!(
            choose_candidate_id(&accounts, &unavailable, &HashMap::new(), 0, 0).as_deref(),
            Some(second)
        );
        super::super::super::clear_account_cooldown(first);
    }

    #[test]
    fn scoped_file_reference_disables_automatic_retry() {
        assert!(safe_to_retry_before_delivery(
            br#"{"messages":[{"content":"hi"}]}"#
        ));
        assert!(!safe_to_retry_before_delivery(br#"{"messages":[{"content":[{"type":"document","source":{"type":"file","file_id":"file-1"}}]}]}"#));
        assert!(!safe_to_retry_before_delivery(
            br#"{"previous_response_id":"resp-1"}"#
        ));
    }

    #[test]
    fn native_messages_stream_through_subscription_token() {
        let storage = storage_with_accounts(&["claude:stream-only"]);
        let upstream = Server::http("127.0.0.1:0").expect("start mock upstream");
        let url = mock_upstream_url(&upstream);
        let (headers_tx, headers_rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let request = upstream
                .recv_timeout(Duration::from_secs(3))
                .expect("receive upstream request")
                .expect("request present");
            let headers = request
                .headers()
                .iter()
                .map(|header| {
                    (
                        header.field.as_str().to_string().to_ascii_lowercase(),
                        header.value.as_str().to_string(),
                    )
                })
                .collect::<Vec<_>>();
            headers_tx.send(headers).expect("send upstream headers");
            let stream_body = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-test\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\nevent: safeguard_results\ndata: {\"type\":\"safeguard_results\",\"classifier\":{\"decision\":\"allow\"},\"opaque\":{\"keep\":true}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
            request
                .respond(response_with_type(stream_body, 200, "text/event-stream"))
                .expect("respond upstream stream");
        });
        let (status, response_body) = run_mocked_proxy(
            &storage,
            url,
            "/v1/messages",
            serde_json::json!({
                "model":"claude-test",
                "max_tokens":32,
                "stream":true,
                "safeguards":{"classifier":"server","opaque":{"keep":true}},
                "unknown_stream_field":{"keep":"exact"},
                "messages":[{"role":"user","content":"hi"}]
            }),
            true,
        );
        join.join().expect("join upstream server");
        let headers = headers_rx.recv().expect("capture headers");
        assert_eq!(status, 200);
        assert!(response_body.contains("message_stop"));
        assert_eq!(response_body, "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-test\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\nevent: safeguard_results\ndata: {\"type\":\"safeguard_results\",\"classifier\":{\"decision\":\"allow\"},\"opaque\":{\"keep\":true}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
        let safeguard_offset = response_body
            .find("safeguard_results")
            .expect("safeguard event");
        let stop_offset = response_body
            .find("message_stop")
            .expect("message stop event");
        assert!(safeguard_offset < stop_offset);
        assert!(headers.iter().any(|(name, value)| {
            name == "authorization" && value == "Bearer token-claude:stream-only"
        }));
        let beta_values = headers
            .iter()
            .filter(|(name, _)| name == "anthropic-beta")
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(beta_values, vec!["interleaved-thinking-2025-05-14"]);
    }

    #[test]
    fn native_safeguards_and_unknown_fields_reach_upstream_and_return_verbatim() {
        let storage = storage_with_accounts(&["claude:safeguards-json"]);
        let upstream = Server::http("127.0.0.1:0").expect("start mock upstream");
        let url = mock_upstream_url(&upstream);
        let payload = serde_json::json!({
            "model": "claude-test",
            "max_tokens": 32,
            "stream": false,
            "safeguards": {
                "classifier": "server",
                "review": {"required": true, "opaque": [1, 2, 3]}
            },
            "unknown_anthropic_field": {"preserve": "this"},
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_exact_123", "name": "lookup", "input": {"q": "hi"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_exact_123", "content": "done"}]}
            ]
        });
        let (body_tx, body_rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut request = upstream
                .recv_timeout(Duration::from_secs(3))
                .expect("receive upstream request")
                .expect("request present");
            let mut request_body = Vec::new();
            std::io::Read::read_to_end(request.as_reader(), &mut request_body)
                .expect("read upstream body");
            body_tx.send(request_body).expect("send upstream body");
            request.respond(response_with_type(
                r#"{"id":"msg_safeguard","type":"message","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"toolu_exact_123","name":"lookup","input":{"q":"hi"}}],"safeguard_results":{"decision":"allow","opaque":{"retain":["a",{"b":true}]}},"unknown_response_field":{"keep":true},"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
                200,
                "application/json",
            )).expect("respond upstream JSON");
        });
        let (status, response_body) =
            run_mocked_proxy(&storage, url, "/v1/messages", payload, false);
        join.join().expect("join upstream server");
        let request_value: Value = serde_json::from_slice(&body_rx.recv().expect("request body"))
            .expect("parse upstream body");
        assert_eq!(
            request_value["safeguards"]["review"]["opaque"],
            serde_json::json!([1, 2, 3])
        );
        assert_eq!(request_value["unknown_anthropic_field"]["preserve"], "this");
        assert_eq!(
            request_value["messages"][0]["content"][0]["id"],
            "toolu_exact_123"
        );
        assert_eq!(
            request_value["messages"][1]["content"][0]["tool_use_id"],
            "toolu_exact_123"
        );
        assert_eq!(status, 200);
        let expected_response = r#"{"id":"msg_safeguard","type":"message","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"toolu_exact_123","name":"lookup","input":{"q":"hi"}}],"safeguard_results":{"decision":"allow","opaque":{"retain":["a",{"b":true}]}},"unknown_response_field":{"keep":true},"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#;
        assert_eq!(response_body, expected_response);
        let response_value: Value = serde_json::from_str(&response_body).expect("parse response");
        assert_eq!(
            response_value["safeguard_results"],
            serde_json::json!({
                "decision": "allow",
                "opaque": {"retain": ["a", {"b": true}]}
            })
        );
        assert_eq!(response_value["unknown_response_field"]["keep"], true);
    }

    #[test]
    fn non_streaming_responses_convert_from_anthropic_message() {
        let storage = storage_with_accounts(&["claude:responses-only"]);
        let upstream = Server::http("127.0.0.1:0").expect("start mock upstream");
        let url = mock_upstream_url(&upstream);
        let join = thread::spawn(move || {
            let mut request = upstream
                .recv_timeout(Duration::from_secs(3))
                .expect("receive upstream request")
                .expect("request present");
            let mut request_body = Vec::new();
            std::io::Read::read_to_end(request.as_reader(), &mut request_body)
                .expect("read upstream body");
            let value: Value = serde_json::from_slice(&request_body).expect("parse upstream body");
            assert_eq!(value["messages"][0]["role"], "user");
            assert_eq!(value["stream"], false);
            request.respond(response_with_type(
                r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-test","content":[{"type":"text","text":"hello from claude"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":3}}"#,
                200,
                "application/json",
            )).expect("respond upstream JSON");
        });
        let (status, response_body) = run_mocked_proxy(
            &storage,
            url,
            "/v1/responses",
            serde_json::json!({"model":"claude-test","input":"hi","stream":false}),
            false,
        );
        join.join().expect("join upstream server");
        assert_eq!(status, 200);
        let value: Value = serde_json::from_str(&response_body).expect("parse converted response");
        assert_eq!(value["object"], "response");
        assert!(response_body.contains("hello from claude"));
    }

    #[test]
    fn responses_return_json_when_generic_router_rewrites_omitted_stream_to_true() {
        let storage = storage_with_accounts(&["claude:responses-default-json"]);
        let upstream = Server::http("127.0.0.1:0").expect("start mock upstream");
        let url = mock_upstream_url(&upstream);
        let join = thread::spawn(move || {
            let mut request = upstream
                .recv_timeout(Duration::from_secs(3))
                .expect("receive upstream request")
                .expect("request present");
            let mut request_body = Vec::new();
            std::io::Read::read_to_end(request.as_reader(), &mut request_body)
                .expect("read upstream body");
            let value: Value = serde_json::from_slice(&request_body).expect("parse upstream body");
            assert_eq!(value["stream"], false);
            request.respond(response_with_type(
                r#"{"id":"msg_default","type":"message","role":"assistant","model":"claude-test","content":[{"type":"text","text":"json worked"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":2}}"#,
                200,
                "application/json",
            )).expect("respond upstream JSON");
        });
        let (status, response_body) = run_mocked_proxy(
            &storage,
            url,
            "/v1/responses",
            serde_json::json!({"model":"claude-test","input":"hi","stream":true}),
            false,
        );
        join.join().expect("join upstream server");
        assert_eq!(status, 200);
        let value: Value =
            serde_json::from_str(&response_body).expect("non-stream response must be JSON");
        assert_eq!(value["object"], "response");
        assert_eq!(value["status"], "completed");
    }

    #[test]
    fn upstream_524_is_preserved_without_replaying_generation() {
        for stream in [false, true] {
            let storage = storage_with_accounts(&["claude:timeout-one", "claude:timeout-two"]);
            let upstream = Server::http("127.0.0.1:0").unwrap();
            let url = mock_upstream_url(&upstream);
            let join = thread::spawn(move || {
                let request = upstream
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .unwrap();
                request.respond(response_with_type(
                    r#"{"title":"Error 524: A timeout occurred","status":524,"detail":"The origin web server did not return a complete response within the 120-second Proxy Read Timeout"}"#,
                    524, "application/problem+json",
                ).with_header(Header::from_bytes("cf-ray", "synthetic-ray").unwrap())).unwrap();
                upstream
                    .recv_timeout(Duration::from_millis(100))
                    .unwrap()
                    .is_some()
            });
            let (status, body) = run_mocked_proxy(
                &storage,
                url,
                "/v1/messages",
                serde_json::json!({"model":"claude-test","max_tokens":32,"stream":stream,
                    "messages":[{"role":"user","content":"synthetic compaction prompt"}]}),
                stream,
            );
            assert_eq!(status, 524);
            assert!(body.contains("524"));
            assert!(!body.contains("event: ping"));
            assert!(!join.join().unwrap());
        }
    }

    #[test]
    fn overload_retries_one_other_claude_account_before_delivery() {
        let storage = storage_with_accounts(&["claude:retry-one", "claude:retry-two"]);
        let upstream = Server::http("127.0.0.1:0").expect("start mock upstream");
        let url = mock_upstream_url(&upstream);
        let join = thread::spawn(move || {
            let mut authorizations = Vec::new();
            let mut bodies = Vec::new();
            for (index, status) in [529_u16, 200_u16].into_iter().enumerate() {
                let mut request = upstream
                    .recv_timeout(Duration::from_secs(3))
                    .expect("receive upstream request")
                    .expect("request present");
                authorizations.push(
                    request
                        .headers()
                        .iter()
                        .find(|header| header.field.equiv("authorization"))
                        .map(|header| header.value.as_str().to_string())
                        .expect("subscription authorization"),
                );
                let mut request_body = Vec::new();
                std::io::Read::read_to_end(request.as_reader(), &mut request_body)
                    .expect("read retry body");
                bodies.push(request_body);
                let body = if index == 0 {
                    r#"{"type":"error","error":{"type":"overloaded_error","message":"at capacity"}}"#
                } else {
                    r#"{"id":"msg_2","type":"message","role":"assistant","model":"claude-test","content":[{"type":"text","text":"backup worked"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":2}}"#
                };
                request
                    .respond(response_with_type(body, status, "application/json"))
                    .expect("respond upstream");
            }
            (authorizations, bodies)
        });
        let (status, response_body) = run_mocked_proxy(
            &storage,
            url,
            "/v1/messages",
            serde_json::json!({
                "model":"claude-test",
                "max_tokens":32,
                "stream":false,
                "messages":[
                    {"role":"assistant","content":[{"type":"tool_use","id":"toolu_retry_exact","name":"lookup","input":{"q":"hi"}}]},
                    {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_retry_exact","content":"done"}]}
                ],
                "safeguards":{"classifier":"server","opaque":{"retry":true}}
            }),
            false,
        );
        let (authorizations, bodies) = join.join().expect("join upstream server");
        assert_eq!(status, 200);
        assert!(response_body.contains("backup worked"));
        assert_eq!(authorizations.len(), 2);
        assert!(authorizations
            .iter()
            .all(|authorization| authorization.starts_with("Bearer token-claude:retry-")));
        assert_ne!(authorizations[0], authorizations[1]);
        let first_retry_body: Value = serde_json::from_slice(&bodies[0]).expect("first retry body");
        let second_retry_body: Value =
            serde_json::from_slice(&bodies[1]).expect("second retry body");
        assert_eq!(
            first_retry_body["messages"][0]["content"][0]["id"],
            "toolu_retry_exact"
        );
        assert_eq!(
            second_retry_body["messages"][0]["content"][0]["id"],
            "toolu_retry_exact"
        );
        assert_eq!(
            first_retry_body["messages"][1]["content"][0]["tool_use_id"],
            "toolu_retry_exact"
        );
        assert_eq!(
            second_retry_body["messages"][1]["content"][0]["tool_use_id"],
            "toolu_retry_exact"
        );
        assert_eq!(
            first_retry_body["safeguards"],
            serde_json::json!({"classifier":"server","opaque":{"retry":true}})
        );
        assert_eq!(
            first_retry_body["safeguards"],
            second_retry_body["safeguards"]
        );
    }

    #[test]
    fn incomplete_stream_is_never_replayed_to_backup_account() {
        let storage = storage_with_accounts(&["claude:incomplete-one", "claude:incomplete-two"]);
        let upstream = Server::http("127.0.0.1:0").expect("start mock upstream");
        let url = mock_upstream_url(&upstream);
        let join = thread::spawn(move || {
            let request = upstream
                .recv_timeout(Duration::from_secs(3))
                .expect("receive upstream request")
                .expect("request present");
            request.respond(response_with_type(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-test\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}\n\n",
                200,
                "text/event-stream",
            )).expect("respond partial stream");
            upstream
                .recv_timeout(Duration::from_millis(350))
                .expect("check for replay")
                .is_some()
        });
        let (_status, response_body) = run_mocked_proxy(
            &storage,
            url,
            "/v1/messages",
            serde_json::json!({"model":"claude-test","max_tokens":32,"stream":true,"messages":[{"role":"user","content":"hi"}]}),
            true,
        );
        assert!(response_body.contains("message_start"));
        assert!(!join.join().expect("join upstream server"));
    }

    #[test]
    fn account_disabled_after_refresh_is_rechecked_before_send() {
        let account_id = "claude:disable-before-send";
        let storage = storage_with_accounts(&[account_id]);
        let upstream = Server::http("127.0.0.1:0").expect("start mock upstream");
        let url = mock_upstream_url(&upstream);
        let (status, body) = run_mocked_proxy_with_client(
            &storage,
            url,
            "/v1/messages",
            serde_json::json!({"model":"claude-test","max_tokens":32,"stream":false,"messages":[{"role":"user","content":"hi"}]}),
            false,
            |_| {
                storage
                    .update_claude_subscription_account_status(
                        account_id,
                        "disabled",
                        codexmanager_core::storage::now_ts(),
                    )
                    .expect("disable selected account");
                Ok(Client::new())
            },
        );
        assert_eq!(status, 503);
        assert!(body.contains("no available Claude subscription account"));
        assert!(upstream
            .recv_timeout(Duration::from_millis(200))
            .expect("check upstream")
            .is_none());
    }

    #[test]
    fn native_server_scoped_file_reference_fails_before_account_selection() {
        let body = serde_json::json!({
            "model":"claude-test", "max_tokens":32,
            "messages":[{"role":"user","content":[{"type":"document","source":{"type":"file","file_id":"file-account-scoped"}}]}]
        });
        assert!(build_messages_body(
            "/v1/messages",
            &serde_json::to_vec(&body).unwrap(),
            None,
            false
        )
        .unwrap_err()
        .contains("cannot migrate"));
    }

    #[test]
    fn ordinary_tool_schema_fields_do_not_count_as_server_scoped_state() {
        let body = serde_json::json!({
            "model":"claude-test", "max_tokens":32,
            "tools":[{"name":"lookup","input_schema":{"type":"object","properties":{"container":{"type":"string"},"conversation":{"type":"string"},"file_id":{"type":"string"}}}}],
            "messages":[{"role":"user","content":[{"type":"text","text":"hello"}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":{"container":"ordinary data","conversation":"ordinary data"}}]}]
        });
        assert!(!has_server_scoped_reference(&body));
        assert!(safe_to_retry_before_delivery(
            &serde_json::to_vec(&body).unwrap()
        ));
        assert!(build_messages_body(
            "/v1/messages",
            &serde_json::to_vec(&body).unwrap(),
            None,
            false
        )
        .is_ok());
    }

    #[test]
    fn responses_unsupported_content_fails_before_upstream_send() {
        let storage = storage_with_accounts(&["claude:image-guard"]);
        let upstream = Server::http("127.0.0.1:0").expect("start mock upstream");
        let url = mock_upstream_url(&upstream);
        let (status, body) = run_mocked_proxy(
            &storage,
            url,
            "/v1/responses",
            serde_json::json!({
                "model":"claude-test",
                "input":[{"role":"user","content":[
                    {"type":"input_text","text":"Describe this image"},
                    {"type":"input_image","image_url":"data:image/png;base64,aGVsbG8="}
                ]}]
            }),
            false,
        );
        assert_eq!(status, 400);
        assert!(body.contains("unsupported responses content part type: input_image"));
        assert!(upstream
            .recv_timeout(Duration::from_millis(200))
            .expect("check upstream")
            .is_none());
    }

    #[test]
    fn connection_drop_before_http_status_does_not_retry_another_account() {
        let storage = storage_with_accounts(&["claude:network-one", "claude:network-two"]);
        let listener = TcpListener::bind("127.0.0.1:0").expect("start raw upstream");
        let address = listener.local_addr().expect("upstream address");
        let url =
            Url::parse(&format!("http://{address}/v1/messages?beta=true")).expect("upstream URL");
        let join = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("first upstream connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let mut bytes = [0_u8; 4096];
            let _ = std::io::Read::read(&mut stream, &mut bytes).expect("read first request");
            drop(stream);
            listener.set_nonblocking(true).expect("set nonblocking");
            thread::sleep(Duration::from_millis(250));
            listener.accept().is_ok()
        });
        let (status, _) = run_mocked_proxy(
            &storage,
            url,
            "/v1/messages",
            serde_json::json!({"model":"claude-test","max_tokens":32,"stream":false,"messages":[{"role":"user","content":"hi"}]}),
            false,
        );
        assert_eq!(status, 502);
        assert!(!join.join().expect("join raw upstream"));
    }
}
