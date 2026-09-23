use codexmanager_core::storage::{Storage, UpstreamProvider};
use std::collections::HashSet;

pub(crate) const CLIENT_CODEX: &str = "codex";
pub(crate) const PROTOCOL_OPENAI_COMPAT: &str = "openai_compat";
pub(crate) const PROTOCOL_ANTHROPIC_NATIVE: &str = "anthropic_native";
pub(crate) const PROTOCOL_GEMINI_NATIVE: &str = "gemini_native";
pub(crate) const AUTH_BEARER: &str = "authorization_bearer";
pub(crate) const AUTH_X_API_KEY: &str = "x_api_key";
pub(crate) const ROTATION_ACCOUNT: &str = "account_rotation";
pub(crate) const ROTATION_AGGREGATE_API: &str = "aggregate_api_rotation";
pub(crate) const ROTATION_HYBRID: &str = "hybrid_rotation";
pub(crate) const ROTATION_HYBRID_AGGREGATE_FIRST: &str = "hybrid_aggregate_first_rotation";

/// 函数 `normalize_key`
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
fn normalize_key(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('-', "_")
}

/// 函数 `is_anthropic_request_path`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-05
///
/// # 参数
/// - path: 参数 path
///
/// # 返回
/// 返回函数执行结果
pub(crate) fn is_anthropic_request_path(path: &str) -> bool {
    path == "/v1/messages" || path.starts_with("/v1/messages/") || path.starts_with("/v1/messages?")
}

fn normalized_request_path(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

fn is_gemini_internal_generate_content_request_path(path: &str) -> bool {
    matches!(
        normalized_request_path(path),
        "/v1internal:generateContent" | "/v1internal:streamGenerateContent"
    )
}

pub(crate) fn is_gemini_generate_content_request_path(path: &str) -> bool {
    let normalized = normalized_request_path(path);
    if is_gemini_internal_generate_content_request_path(normalized) {
        return true;
    }
    ["/v1/models/", "/v1beta/models/", "/v1alpha/models/"]
        .iter()
        .any(|prefix| {
            normalized.starts_with(prefix)
                && (normalized.contains(":generateContent")
                    || normalized.contains(":streamGenerateContent"))
        })
}

pub(crate) fn is_gemini_count_tokens_request_path(path: &str) -> bool {
    let normalized = normalized_request_path(path);
    if normalized == "/v1internal:countTokens" {
        return true;
    }
    ["/v1/models/", "/v1beta/models/", "/v1alpha/models/"]
        .iter()
        .any(|prefix| normalized.starts_with(prefix) && normalized.contains(":countTokens"))
}

pub(crate) fn is_gemini_request_path(path: &str) -> bool {
    is_gemini_generate_content_request_path(path) || is_gemini_count_tokens_request_path(path)
}

/// 函数 `resolve_gateway_protocol_type`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-05
///
/// # 参数
/// - protocol_type: 参数 protocol_type
/// - path: 参数 path
///
/// # 返回
/// 返回函数执行结果
pub(crate) fn resolve_gateway_protocol_type(protocol_type: &str, path: &str) -> &'static str {
    match normalize_key(protocol_type).as_str() {
        _ if is_gemini_request_path(path) => PROTOCOL_GEMINI_NATIVE,
        // 中文注释：平台 Key 对 Codex / Claude Code 默认按路径通配；
        // `/v1/messages*` 走 Claude 语义，Gemini 原生路径走 Gemini 语义，其余标准路径走 OpenAI/Codex 语义。
        _ if is_anthropic_request_path(path) => PROTOCOL_ANTHROPIC_NATIVE,
        _ => PROTOCOL_OPENAI_COMPAT,
    }
}

/// 函数 `normalize_protocol_type`
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
pub(crate) fn normalize_protocol_type(value: Option<String>) -> Result<String, String> {
    match value {
        Some(raw) => match normalize_key(&raw).as_str() {
            "openai" | "openai_compat" => Ok(PROTOCOL_OPENAI_COMPAT.to_string()),
            "anthropic" | "anthropic_native" => Ok(PROTOCOL_ANTHROPIC_NATIVE.to_string()),
            "gemini" | "gemini_native" => Ok(PROTOCOL_GEMINI_NATIVE.to_string()),
            other => Err(format!("unsupported protocol type: {other}")),
        },
        None => Ok(PROTOCOL_OPENAI_COMPAT.to_string()),
    }
}

/// 函数 `profile_from_protocol`
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
pub(crate) fn profile_from_protocol(
    protocol_type: &str,
) -> Result<(String, String, String), String> {
    let protocol = normalize_protocol_type(Some(protocol_type.to_string()))?;
    let auth_scheme = if protocol == PROTOCOL_ANTHROPIC_NATIVE {
        AUTH_X_API_KEY.to_string()
    } else {
        AUTH_BEARER.to_string()
    };
    Ok((CLIENT_CODEX.to_string(), protocol, auth_scheme))
}

/// 函数 `normalize_rotation_strategy`
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
pub(crate) fn normalize_rotation_strategy(value: Option<String>) -> Result<String, String> {
    match value {
        Some(raw) => match normalize_key(&raw).as_str() {
            "account" | "account_rotation" | "account_rotate" | "账号轮转" => {
                Ok(ROTATION_ACCOUNT.to_string())
            }
            "aggregateapi"
            | "aggregate_api"
            | "aggregate_api_rotation"
            | "aggregateapirotation"
            | "聚合api"
            | "聚合api轮转" => Ok(ROTATION_AGGREGATE_API.to_string()),
            "hybrid"
            | "mixed"
            | "hybrid_rotation"
            | "mixed_rotation"
            | "混合轮转"
            | "账号优先聚合兜底" => Ok(ROTATION_HYBRID.to_string()),
            "hybrid_aggregate_first"
            | "hybrid_aggregate_first_rotation"
            | "mixed_aggregate_first"
            | "aggregate_first"
            | "聚合优先"
            | "聚合优先账号兜底" => Ok(ROTATION_HYBRID_AGGREGATE_FIRST.to_string()),
            other => Err(format!("unsupported rotation strategy: {other}")),
        },
        None => Ok(ROTATION_ACCOUNT.to_string()),
    }
}

pub(crate) fn normalize_upstream_provider(value: &str) -> Result<UpstreamProvider, String> {
    UpstreamProvider::parse(value)
        .ok_or_else(|| format!("unsupported upstreamProvider: {}", value.trim()))
}

fn aggregate_api_is_claude(storage: &Storage, aggregate_api_id: &str) -> Result<bool, String> {
    let api = storage
        .find_aggregate_api_by_id(aggregate_api_id)
        .map_err(|err| format!("read aggregate api provider failed: {err}"))?;
    Ok(api.is_some_and(|api| is_claude_provider_type(&api.provider_type)))
}

fn is_claude_provider_type(provider_type: &str) -> bool {
    let normalized = provider_type.trim().to_ascii_lowercase().replace('-', "_");
    matches!(
        normalized.as_str(),
        "claude" | "anthropic" | "anthropic_native" | "claude_code"
    )
}

fn is_codex_provider_type(provider_type: &str) -> bool {
    let normalized = provider_type.trim().to_ascii_lowercase().replace('-', "_");
    !matches!(
        normalized.as_str(),
        "claude"
            | "anthropic"
            | "anthropic_native"
            | "claude_code"
            | "gemini"
            | "gemini_native"
            | "google"
            | "google_ai"
            | "google_gemini"
            | "compatible"
    )
}

/// Legacy clients that explicitly pin a Claude aggregate upstream retain their
/// existing provider when the new field is omitted.
pub(crate) fn infer_upstream_provider(
    storage: &Storage,
    rotation_strategy: &str,
    aggregate_api_id: Option<&str>,
) -> Result<UpstreamProvider, String> {
    let pinned = aggregate_api_id.map(str::trim).filter(|id| !id.is_empty());
    if rotation_strategy == ROTATION_AGGREGATE_API {
        if let Some(id) = pinned {
            if aggregate_api_is_claude(storage, id)? {
                return Ok(UpstreamProvider::Claude);
            }
        }
    }
    if matches!(
        rotation_strategy,
        ROTATION_AGGREGATE_API | ROTATION_HYBRID | ROTATION_HYBRID_AGGREGATE_FIRST
    ) {
        let ambiguous_pin = match pinned {
            None => true,
            Some(id) => storage
                .find_aggregate_api_by_id(id)
                .map_err(|err| format!("read aggregate API provider failed: {err}"))?
                .is_none_or(|api| api.provider_type.trim().eq_ignore_ascii_case("compatible")),
        };
        if ambiguous_pin {
            let aggregate_apis = storage
                .list_aggregate_apis()
                .map_err(|err| format!("list aggregate API providers failed: {err}"))?;
            let has_claude = aggregate_apis
                .iter()
                .any(|api| is_claude_provider_type(&api.provider_type));
            let has_compatible = aggregate_apis
                .iter()
                .any(|api| api.provider_type.trim().eq_ignore_ascii_case("compatible"));
            let has_codex = aggregate_apis
                .iter()
                .any(|api| is_codex_provider_type(&api.provider_type));
            if has_claude || (has_compatible && has_codex) {
                return Err(
                    "upstreamProvider is required for an ambiguous unpinned or compatible aggregate route"
                        .to_string(),
                );
            }
        }
    }
    Ok(UpstreamProvider::Openai)
}

pub(crate) fn validate_upstream_provider_route(
    storage: &Storage,
    provider: UpstreamProvider,
    rotation_strategy: &str,
    aggregate_api_id: Option<&str>,
) -> Result<(), String> {
    if provider != UpstreamProvider::Claude {
        if let Some(aggregate_api_id) =
            aggregate_api_id.map(str::trim).filter(|id| !id.is_empty())
        {
            if aggregate_api_is_claude(storage, aggregate_api_id)? {
                return Err("OpenAI upstream cannot pin a Claude aggregateApiId".to_string());
            }
        }
        return Ok(());
    }
    if rotation_strategy == ROTATION_ACCOUNT {
        if aggregate_api_id.is_some_and(|id| !id.trim().is_empty()) {
            return Err("Claude subscription account rotation cannot pin an aggregateApiId".to_string());
        }
        return Ok(());
    }
    if rotation_strategy != ROTATION_AGGREGATE_API {
        return Err("Claude upstream requires account_rotation or aggregate_api_rotation".to_string());
    }
    let aggregate_api_id = aggregate_api_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Claude upstream requires a Claude aggregateApiId".to_string())?;
    let api = storage
        .find_aggregate_api_by_id(aggregate_api_id)
        .map_err(|err| format!("read aggregate api provider failed: {err}"))?
        .ok_or_else(|| "Claude aggregateApiId not found".to_string())?;
    if !api.status.trim().eq_ignore_ascii_case("active")
        || !is_claude_provider_type(&api.provider_type)
    {
        return Err("Claude upstream requires an active Claude aggregateApiId".to_string());
    }
    Ok(())
}

pub(crate) fn validate_claude_model_route(
    storage: &Storage,
    provider: UpstreamProvider,
    rotation_strategy: &str,
    model_slug: Option<&str>,
) -> Result<(), String> {
    if provider != UpstreamProvider::Claude {
        return Ok(());
    }
    let Some(model_slug) = model_slug.map(str::trim).filter(|slug| !slug.is_empty()) else {
        return Ok(());
    };
    let catalog_slug = crate::models_v2::policy_catalog_slug(model_slug);
    let model = storage
        .get_enabled_model_v2(catalog_slug)
        .map_err(|err| format!("read Claude model routes failed: {err}"))?;
    if rotation_strategy == ROTATION_ACCOUNT {
        if model.is_some_and(|model| {
            model.routes.iter().any(|route| {
                route.enabled
                    && route.source_kind == "account_pool"
                    && route.source_id == "claude"
            })
        }) {
            return Ok(());
        }
        return Err(format!(
            "Claude model has no enabled Claude subscription account route: {model_slug}"
        ));
    }
    if let Some(model) = model {
        for route in model
            .routes
            .iter()
            .filter(|route| route.enabled && route.source_kind == "aggregate_api")
        {
            let api = storage
                .find_aggregate_api_by_id(&route.source_id)
                .map_err(|err| format!("read Claude model source failed: {err}"))?;
            if api.is_some_and(|api| {
                api.status.trim().eq_ignore_ascii_case("active")
                    && is_claude_provider_type(&api.provider_type)
            }) {
                return Ok(());
            }
        }
    }
    Err(format!(
        "Claude model has no active Claude aggregate API route: {model_slug}"
    ))
}

pub(crate) fn validate_openai_model_route(
    storage: &Storage,
    provider: UpstreamProvider,
    rotation_strategy: &str,
    protocol_type: &str,
    model_slug: Option<&str>,
) -> Result<(), String> {
    if provider != UpstreamProvider::Openai || rotation_strategy == ROTATION_ACCOUNT {
        return Ok(());
    }
    let Some(model_slug) = model_slug.map(str::trim).filter(|slug| !slug.is_empty()) else {
        return Ok(());
    };
    let catalog_slug = crate::models_v2::policy_catalog_slug(model_slug);
    let Some(model) = storage
        .get_enabled_model_v2(catalog_slug)
        .map_err(|err| format!("read OpenAI model routes failed: {err}"))?
    else {
        // Keep legacy external model bindings; the gateway performs its own
        // catalog validation when an actual request selects the model.
        return Ok(());
    };
    let aggregate_provider = if protocol_type == PROTOCOL_GEMINI_NATIVE {
        crate::aggregate_api::AGGREGATE_API_PROVIDER_GEMINI
    } else {
        crate::aggregate_api::AGGREGATE_API_PROVIDER_CODEX
    };
    let active_ids = storage
        .list_active_aggregate_apis_by_provider_type(aggregate_provider)
        .map_err(|err| format!("list OpenAI aggregate routes failed: {err}"))?
        .into_iter()
        .map(|api| api.id)
        .collect::<HashSet<_>>();
    let include_account_pool = matches!(
        rotation_strategy,
        ROTATION_HYBRID | ROTATION_HYBRID_AGGREGATE_FIRST
    );
    if model.routes.iter().any(|route| {
        route.enabled
            && ((route.source_kind == "aggregate_api" && active_ids.contains(&route.source_id))
                || (include_account_pool
                    && route.source_kind == "account_pool"
                    && route.source_id == "default"))
    }) {
        return Ok(());
    }
    Err(format!("OpenAI model has no active upstream route: {model_slug}"))
}

pub(crate) fn validate_claude_protocol(
    provider: UpstreamProvider,
    protocol_type: &str,
) -> Result<(), String> {
    if provider == UpstreamProvider::Claude && protocol_type == PROTOCOL_GEMINI_NATIVE {
        return Err("Claude upstream does not support gemini_native protocol".to_string());
    }
    Ok(())
}

/// 函数 `normalize_upstream_base_url`
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
pub(crate) fn normalize_upstream_base_url(value: Option<String>) -> Result<Option<String>, String> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed =
        reqwest::Url::parse(trimmed.as_str()).map_err(|_| "invalid upstreamBaseUrl".to_string())?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("invalid upstreamBaseUrl scheme".to_string());
    }
    Ok(Some(trimmed))
}

/// 函数 `normalize_static_headers_json`
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
pub(crate) fn normalize_static_headers_json(
    value: Option<String>,
) -> Result<Option<String>, String> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|_| "invalid staticHeadersJson: must be a JSON object".to_string())?;
    let obj = parsed
        .as_object()
        .ok_or_else(|| "invalid staticHeadersJson: must be a JSON object".to_string())?;
    for (name, value) in obj {
        if name.trim().is_empty() {
            return Err("invalid staticHeadersJson: header name is empty".to_string());
        }
        if !value.is_string() {
            return Err(format!(
                "invalid staticHeadersJson: header {name} value must be string"
            ));
        }
    }
    Ok(Some(trimmed.to_string()))
}

#[cfg(test)]
#[path = "apikey_profile_tests.rs"]
mod tests;
