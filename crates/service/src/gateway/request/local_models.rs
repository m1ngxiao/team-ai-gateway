use codexmanager_core::rpc::types::{ModelInfo, ModelsResponse};
use codexmanager_core::storage::{ApiKey, Storage, UpstreamProvider};
use std::collections::HashSet;

#[derive(serde::Serialize)]
struct CompatibleModelsResponse<'a> {
    object: &'static str,
    data: Vec<ApiModelInfo<'a>>,
    models: &'a [ModelInfo],
}

#[derive(serde::Serialize)]
struct ApiModelInfo<'a> {
    id: &'a str,
    object: &'static str,
    created: i64,
    owned_by: &'static str,
    display_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

fn serialize_models_response_body(models: &ModelsResponse) -> String {
    let data = models
        .models
        .iter()
        .filter(|model| model.supported_in_api)
        .map(|model| ApiModelInfo {
            id: model.slug.as_str(),
            object: "model",
            created: 0,
            owned_by: "codexmanager",
            display_name: model.display_name.as_str(),
            description: model.description.as_deref(),
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&CompatibleModelsResponse {
        object: "list",
        data,
        // Codex CLI consumes the private `models` catalog while generic OpenAI
        // clients consume `data`, so both representations must stay populated.
        models: &models.models,
    })
    .unwrap_or_else(|_| "{\"object\":\"list\",\"data\":[],\"models\":[]}".to_string())
}

fn serialize_models_response(models: &ModelsResponse) -> String {
    serialize_models_response_body(models)
}

fn filter_models_for_key(
    storage: &codexmanager_core::storage::Storage,
    key_id: &str,
    models: ModelsResponse,
) -> Result<(ModelsResponse, bool), String> {
    let Some(allowed_slugs) = crate::allowed_model_slugs_for_api_key(storage, key_id)? else {
        return Ok((models, true));
    };
    Ok((
        ModelsResponse {
            models: models
                .models
                .into_iter()
                .filter(|model| allowed_slugs.contains(model.slug.as_str()))
                .collect(),
            extra: models.extra,
        },
        false,
    ))
}

fn filter_models_for_catalog_policy(
    storage: &Storage,
    key_id: &str,
    models: ModelsResponse,
    policy: crate::codex_model_catalog::GatewayCatalogPolicy,
) -> Result<(ModelsResponse, bool), String> {
    match policy {
        crate::codex_model_catalog::GatewayCatalogPolicy::OfficialAccountPool => Ok((models, true)),
        crate::codex_model_catalog::GatewayCatalogPolicy::Managed => {
            filter_models_for_key(storage, key_id, models)
        }
    }
}

fn filter_models_for_claude_upstreams(
    storage: &Storage,
    mut models: ModelsResponse,
) -> Result<ModelsResponse, String> {
    // The Claude route resolver uses active Claude aggregate candidates across
    // the pool. A pinned aggregateApiId only changes their ordering.
    let active_claude_ids = storage
        .list_active_aggregate_apis_by_provider_type(
            crate::aggregate_api::AGGREGATE_API_PROVIDER_CLAUDE,
        )
        .map_err(|err| format!("list active Claude aggregate APIs failed: {err}"))?
        .into_iter()
        .filter(|api| {
            matches!(
                api.provider_type
                    .trim()
                    .to_ascii_lowercase()
                    .replace('-', "_")
                    .as_str(),
                "claude" | "anthropic" | "anthropic_native" | "claude_code"
            )
        })
        .map(|api| api.id)
        .collect::<HashSet<_>>();

    let available_slugs = storage
        .list_api_models_v2()
        .map_err(|err| format!("list Claude model routes failed: {err}"))?
        .into_iter()
        .filter(|model| {
            model.routes.iter().any(|route| {
                route.enabled
                    && route.source_kind == "aggregate_api"
                    && active_claude_ids.contains(&route.source_id)
            })
        })
        .map(|model| model.slug)
        .collect::<HashSet<_>>();
    models
        .models
        .retain(|model| available_slugs.contains(model.slug.as_str()));
    models.extra.remove("etag");
    Ok(models)
}

fn filter_models_for_claude_accounts(
    storage: &Storage,
    mut models: ModelsResponse,
) -> Result<ModelsResponse, String> {
    if storage
        .list_active_claude_subscription_accounts()
        .map_err(|err| format!("list active Claude subscription accounts failed: {err}"))?
        .is_empty()
    {
        models.models.clear();
        models.extra.remove("etag");
        return Ok(models);
    }
    let available_slugs = storage
        .list_api_models_v2()
        .map_err(|err| format!("list Claude subscription model routes failed: {err}"))?
        .into_iter()
        .filter(|model| {
            model.routes.iter().any(|route| {
                route.enabled
                    && route.source_kind == "account_pool"
                    && route.source_id == "claude"
            })
        })
        .map(|model| model.slug)
        .collect::<HashSet<_>>();
    models
        .models
        .retain(|model| available_slugs.contains(model.slug.as_str()));
    models.extra.remove("etag");
    Ok(models)
}

fn filter_models_for_openai_managed_upstreams(
    storage: &Storage,
    mut models: ModelsResponse,
    key: &ApiKey,
) -> Result<ModelsResponse, String> {
    // The runtime selects the aggregate provider from the request protocol.
    // For a Key's model catalog, use its configured protocol to avoid listing
    // Claude-only routes or models from an unrelated aggregate provider.
    let aggregate_provider = if key.protocol_type == crate::apikey_profile::PROTOCOL_GEMINI_NATIVE {
        crate::aggregate_api::AGGREGATE_API_PROVIDER_GEMINI
    } else {
        crate::aggregate_api::AGGREGATE_API_PROVIDER_CODEX
    };
    let active_aggregate_ids = storage
        .list_active_aggregate_apis_by_provider_type(aggregate_provider)
        .map_err(|err| format!("list active aggregate APIs failed: {err}"))?
        .into_iter()
        .map(|api| api.id)
        .collect::<HashSet<_>>();
    let include_account_pool = matches!(
        key.rotation_strategy.as_str(),
        crate::apikey_profile::ROTATION_HYBRID
            | crate::apikey_profile::ROTATION_HYBRID_AGGREGATE_FIRST
    );
    let available_slugs = storage
        .list_api_models_v2()
        .map_err(|err| format!("list OpenAI model routes failed: {err}"))?
        .into_iter()
        .filter(|model| {
            model.routes.iter().any(|route| {
                route.enabled
                    && ((route.source_kind == "aggregate_api"
                        && active_aggregate_ids.contains(&route.source_id))
                        || (include_account_pool
                            && route.source_kind == "account_pool"
                            && route.source_id == "default"))
            })
        })
        .map(|model| model.slug)
        .collect::<HashSet<_>>();
    models
        .models
        .retain(|model| available_slugs.contains(model.slug.as_str()));
    models.extra.remove("etag");
    Ok(models)
}

pub(crate) fn filter_managed_models_for_gateway_key(
    storage: &Storage,
    key_id: &str,
    models: ModelsResponse,
) -> Result<ModelsResponse, String> {
    let key = storage
        .find_api_key_by_id(key_id)
        .map_err(|err| format!("read api key upstream provider failed: {err}"))?
        .ok_or_else(|| "api key not found".to_string())?;
    let (models, _) = filter_models_for_key(storage, key_id, models)?;
    match key.upstream_provider {
        UpstreamProvider::Claude => {
            if key.rotation_strategy == crate::apikey_profile::ROTATION_ACCOUNT {
                filter_models_for_claude_accounts(storage, models)
            } else {
                filter_models_for_claude_upstreams(storage, models)
            }
        }
        UpstreamProvider::Openai => {
            filter_models_for_openai_managed_upstreams(storage, models, &key)
        }
    }
}

fn models_etag_header(models: &ModelsResponse) -> Result<Option<tiny_http::Header>, String> {
    let Some(etag) = models.extra.get("etag").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let header = tiny_http::Header::from_bytes(b"etag".as_slice(), etag.as_bytes())
        .map_err(|_| "build etag header failed".to_string())?;
    Ok(Some(header))
}

/// 函数 `read_cached_models_response`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-12
///
/// # 参数
/// - storage: 参数 storage
///
/// # 返回
/// 返回函数执行结果
fn read_models_response_for_key(
    storage: &Storage,
    key_id: &str,
) -> Result<
    (
        ModelsResponse,
        crate::codex_model_catalog::GatewayCatalogPolicy,
        ApiKey,
    ),
    String,
> {
    let key = storage
        .find_api_key_by_id(key_id)
        .map_err(|err| format!("read api key upstream provider failed: {err}"))?
        .ok_or_else(|| "api key not found".to_string())?;
    if key.upstream_provider == UpstreamProvider::Claude {
        return Ok((
            crate::models_v2::models_response_with_storage(storage)?,
            crate::codex_model_catalog::GatewayCatalogPolicy::Managed,
            key,
        ));
    }
    let (models, policy) =
        crate::codex_model_catalog::models_response_for_gateway_key(storage, key_id)?;
    Ok((models, policy, key))
}

/// 函数 `maybe_respond_local_models`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-02
///
/// # 参数
/// - super: 参数 super
///
/// # 返回
/// 返回函数执行结果
pub(super) fn maybe_respond_local_models(
    request: tiny_http::Request,
    trace_id: &str,
    key_id: &str,
    protocol_type: &str,
    original_path: &str,
    path: &str,
    response_adapter: super::ResponseAdapter,
    request_method: &str,
    model_for_log: Option<&str>,
    reasoning_for_log: Option<&str>,
    storage: &codexmanager_core::storage::Storage,
) -> Result<Option<tiny_http::Request>, String> {
    let is_models_list = request_method.eq_ignore_ascii_case("GET")
        && (path == "/v1/models" || path.starts_with("/v1/models?"));
    if !is_models_list {
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
    let (cached, catalog_policy, key) = match read_models_response_for_key(storage, key_id) {
        Ok(result) => result,
        Err(err) => {
            let message = crate::gateway::bilingual_error(
                "读取模型缓存失败",
                format!("model options cache read failed: {err}"),
            );
            super::local_response::respond_local_terminal_error(request, &context, 503, message)?;
            return Ok(None);
        }
    };

    let (output_models, include_implicit_models) =
        filter_models_for_catalog_policy(storage, key_id, cached, catalog_policy)?;
    let output_models = match (key.upstream_provider, catalog_policy) {
        (UpstreamProvider::Claude, _) => {
            if key.rotation_strategy == crate::apikey_profile::ROTATION_ACCOUNT {
                filter_models_for_claude_accounts(storage, output_models)?
            } else {
                filter_models_for_claude_upstreams(storage, output_models)?
            }
        }
        (UpstreamProvider::Openai, crate::codex_model_catalog::GatewayCatalogPolicy::Managed) => {
            filter_models_for_openai_managed_upstreams(storage, output_models, &key)?
        }
        _ => output_models,
    };
    let output = if include_implicit_models {
        serialize_models_response(&output_models)
    } else {
        serialize_models_response_body(&output_models)
    };
    let extra_headers = models_etag_header(&output_models)?.into_iter().collect();
    super::local_response::respond_local_json_with_headers(
        request,
        &context,
        output,
        super::request_log::RequestLogUsage::default(),
        extra_headers,
    )?;
    Ok(None)
}

#[cfg(test)]
#[path = "tests/local_models_tests.rs"]
mod tests;
