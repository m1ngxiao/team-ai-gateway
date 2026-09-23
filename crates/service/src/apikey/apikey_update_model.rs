use crate::apikey::service_tier::normalize_service_tier_owned;
use crate::apikey_profile::{
    normalize_protocol_type, normalize_rotation_strategy,
    normalize_static_headers_json, normalize_upstream_base_url, normalize_upstream_provider,
    profile_from_protocol, validate_claude_model_route, validate_claude_protocol,
    validate_openai_model_route, validate_upstream_provider_route,
    ROTATION_AGGREGATE_API,
};
use crate::reasoning_effort::normalize_reasoning_effort;
use crate::storage_helpers::open_storage;

/// 函数 `update_api_key_model`
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
pub(crate) fn update_api_key_model(
    key_id: &str,
    name: Option<String>,
    has_name: bool,
    model_slug: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
    protocol_type: Option<String>,
    upstream_base_url: Option<String>,
    static_headers_json: Option<String>,
    rotation_strategy: Option<String>,
    upstream_provider: Option<String>,
    aggregate_api_id: Option<String>,
    account_plan_filter: Option<String>,
    account_group_filter: Option<String>,
    update_model_config: bool,
    update_routing_config: bool,
    update_upstream_provider: bool,
    confirm_route_review: bool,
    update_account_group_filter: bool,
    has_quota_limit_tokens: bool,
    quota_limit_tokens: Option<i64>,
) -> Result<(), String> {
    if key_id.is_empty() {
        return Err("key id required".to_string());
    }
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let current_key = storage
        .find_api_key_by_id(key_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "api key not found".to_string())?;
    if confirm_route_review && !update_upstream_provider {
        return Err("route review requires an explicit upstreamProvider".to_string());
    }
    if update_model_config {
        crate::models_v2::ensure_text_generation_model(&storage, model_slug.as_deref())?;
    }
    let normalized_name = name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let normalized = model_slug
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let normalized_reasoning = reasoning_effort
        .as_deref()
        .and_then(normalize_reasoning_effort);
    let normalized_service_tier = normalize_service_tier_owned(service_tier)?;
    // Validate every admin-only routing input before the first write. Member updates skip this
    // branch entirely and therefore preserve all administrator-managed routing fields.
    let has_openai_account_filters = account_plan_filter
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || account_group_filter
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    let normalized_routing_config = if update_routing_config {
        let normalized_rotation_strategy = normalize_rotation_strategy(rotation_strategy)?;
        let normalized_aggregate_api_id = if normalized_rotation_strategy == ROTATION_AGGREGATE_API
        {
            aggregate_api_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        } else {
            None
        };
        let normalized_account_plan_filter = if normalized_rotation_strategy
            == crate::apikey_profile::ROTATION_ACCOUNT
            || normalized_rotation_strategy == crate::apikey_profile::ROTATION_HYBRID
            || normalized_rotation_strategy
                == crate::apikey_profile::ROTATION_HYBRID_AGGREGATE_FIRST
        {
            crate::account_plan::normalize_account_plan_filter(account_plan_filter)?
        } else {
            None
        };
        Some((
            normalized_rotation_strategy,
            normalized_aggregate_api_id,
            normalized_account_plan_filter,
        ))
    } else {
        None
    };
    let effective_rotation_strategy = normalized_routing_config
        .as_ref()
        .map(|config| config.0.as_str())
        .unwrap_or(current_key.rotation_strategy.as_str());
    let effective_rotation_strategy_owned = effective_rotation_strategy.to_string();
    let effective_aggregate_api_id = normalized_routing_config
        .as_ref()
        .map(|config| config.1.as_deref())
        .unwrap_or(current_key.aggregate_api_id.as_deref());
    let normalized_upstream_provider = if update_upstream_provider {
        normalize_upstream_provider(
            upstream_provider
                .as_deref()
                .ok_or_else(|| "upstreamProvider is required".to_string())?,
        )?
    } else {
        current_key.upstream_provider
    };
    if normalized_upstream_provider == codexmanager_core::storage::UpstreamProvider::Claude
        && effective_rotation_strategy == crate::apikey_profile::ROTATION_ACCOUNT
        && has_openai_account_filters
    {
        return Err("Claude subscription account pool does not support OpenAI plan or group filters".to_string());
    }
    if update_routing_config || update_upstream_provider {
        validate_upstream_provider_route(
            &storage,
            normalized_upstream_provider,
            effective_rotation_strategy,
            effective_aggregate_api_id,
        )?;
    }
    let has_protocol_type = protocol_type.is_some();
    let effective_protocol_type = match protocol_type.as_deref() {
        Some(value) => normalize_protocol_type(Some(value.to_string()))?,
        None => current_key.protocol_type.clone(),
    };
    if update_model_config || update_routing_config || update_upstream_provider || has_protocol_type {
        let effective_model_slug = if update_model_config {
            normalized
        } else {
            current_key.model_slug.as_deref()
        };
        validate_claude_model_route(
            &storage,
            normalized_upstream_provider,
            effective_rotation_strategy,
            effective_model_slug,
        )?;
        validate_openai_model_route(
            &storage,
            normalized_upstream_provider,
            effective_rotation_strategy,
            &effective_protocol_type,
            effective_model_slug,
        )?;
    }
    if has_protocol_type || update_routing_config || update_upstream_provider {
        validate_claude_protocol(normalized_upstream_provider, &effective_protocol_type)?;
    }
    let effective_group_rotation_strategy =
        if let Some((strategy, _, _)) = normalized_routing_config.as_ref() {
            Some(strategy.clone())
        } else if update_account_group_filter {
            Some(
                storage
                    .find_api_key_by_id(key_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "api key not found".to_string())?
                    .rotation_strategy,
            )
        } else {
            None
        };
    let normalized_account_group_filter_update = if normalized_upstream_provider
        == codexmanager_core::storage::UpstreamProvider::Claude
        && effective_rotation_strategy == crate::apikey_profile::ROTATION_ACCOUNT
        && (update_routing_config || update_upstream_provider || update_account_group_filter)
    {
        Some(None)
    } else { match effective_group_rotation_strategy.as_deref() {
        Some(ROTATION_AGGREGATE_API) => Some(None),
        Some(crate::apikey_profile::ROTATION_ACCOUNT)
        | Some(crate::apikey_profile::ROTATION_HYBRID)
        | Some(crate::apikey_profile::ROTATION_HYBRID_AGGREGATE_FIRST)
            if update_account_group_filter =>
        {
            Some(crate::account_group::normalize_account_group_filter(
                account_group_filter,
            ))
        }
        _ => None,
    }};

    // Normalize every fallible profile input before changing the provider or
    // rotation fields. An invalid header must not leave a Key on a new pool.
    let has_upstream_base_url = upstream_base_url.is_some();
    let has_static_headers_json = static_headers_json.is_some();
    let normalized_upstream_base_url = normalize_upstream_base_url(upstream_base_url)?;
    let normalized_static_headers_json = normalize_static_headers_json(static_headers_json)?;
    let profile_update = if has_protocol_type || has_upstream_base_url || has_static_headers_json {
        let current = storage
            .find_api_key_profile_config_by_id(key_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "api key not found".to_string())?;
        let protocol = protocol_type.unwrap_or_else(|| current.protocol_type.clone());
        let normalized_protocol = normalize_protocol_type(Some(protocol))?;
        let (next_client, next_protocol, next_auth) = profile_from_protocol(&normalized_protocol)?;
        let next_upstream_base_url = if has_upstream_base_url {
            normalized_upstream_base_url
        } else {
            current.upstream_base_url
        };
        let next_static_headers_json = if has_static_headers_json {
            normalized_static_headers_json
        } else {
            current.static_headers_json
        };
        let next_service_tier = normalized_service_tier.clone().or(current.service_tier);
        Some((
            next_client,
            next_protocol,
            next_auth,
            next_upstream_base_url,
            next_static_headers_json,
            next_service_tier,
        ))
    } else {
        None
    };

    if has_name {
        storage
            .update_api_key_name(key_id, normalized_name)
            .map_err(|e| e.to_string())?;
    }
    if update_model_config {
        storage
            .update_api_key_model_config(
                key_id,
                normalized,
                normalized_reasoning,
                normalized_service_tier.as_deref(),
            )
            .map_err(|e| e.to_string())?;
    }
    if let Some((
        normalized_rotation_strategy,
        normalized_aggregate_api_id,
        normalized_account_plan_filter,
    )) = normalized_routing_config
    {
        storage
            .update_api_key_rotation_config(
                key_id,
                normalized_rotation_strategy.as_str(),
                normalized_aggregate_api_id.as_deref(),
                if normalized_upstream_provider == codexmanager_core::storage::UpstreamProvider::Claude {
                    None
                } else {
                    normalized_account_plan_filter.as_deref()
                },
            )
            .map_err(|e| e.to_string())?;
    }
    if update_upstream_provider
        && !update_routing_config
        && normalized_upstream_provider == codexmanager_core::storage::UpstreamProvider::Claude
        && effective_rotation_strategy_owned == crate::apikey_profile::ROTATION_ACCOUNT
    {
        storage
            .update_api_key_rotation_config(key_id, &effective_rotation_strategy_owned, None, None)
            .map_err(|e| e.to_string())?;
    }
    if update_routing_config || update_upstream_provider {
        storage
            .update_api_key_upstream_provider(key_id, normalized_upstream_provider)
            .map_err(|e| e.to_string())?;
    }
    if let Some(normalized_account_group_filter) = normalized_account_group_filter_update {
        storage
            .update_api_key_account_group_filter(key_id, normalized_account_group_filter.as_deref())
            .map_err(|e| e.to_string())?;
    }
    if has_quota_limit_tokens {
        storage
            .upsert_api_key_quota_limit(key_id, quota_limit_tokens)
            .map_err(|e| e.to_string())?;
    }

    if let Some((next_client, next_protocol, next_auth, next_upstream_base_url, next_static_headers_json, next_service_tier)) = profile_update {
        storage
            .update_api_key_profile_config(
                key_id,
                &next_client,
                &next_protocol,
                &next_auth,
                next_upstream_base_url.as_deref(),
                next_static_headers_json.as_deref(),
                next_service_tier.as_deref(),
            )
            .map_err(|e| e.to_string())?;
    }
    if update_routing_config || update_upstream_provider || has_protocol_type || has_upstream_base_url {
        crate::codex_profile::sync_active_gateway_profile_for_api_key(&storage, key_id)?;
    }
    if confirm_route_review {
        storage
            .clear_api_key_route_review(key_id)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
