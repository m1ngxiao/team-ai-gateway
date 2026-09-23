use super::*;
use codexmanager_core::rpc::types::{ModelInfo, ModelServiceTier, ModelsResponse};
use codexmanager_core::storage::{
    now_ts, AggregateApi, ApiKey, ClaudeSubscriptionAccount, ManagedModelV2Upsert,
    ModelRouteV2, UpstreamProvider,
};
use serde_json::Value;

fn insert_catalog_aggregate_api(storage: &Storage, id: &str, provider: &str, status: &str) {
    let now = now_ts();
    storage
        .insert_aggregate_api(&AggregateApi {
            id: id.to_string(),
            provider_type: provider.to_string(),
            supplier_name: None,
            sort: 0,
            url: "https://api.example.test/v1".to_string(),
            auth_type: "apikey".to_string(),
            auth_params_json: None,
            action: None,
            model_override: None,
            user_agent: None,
            status: status.to_string(),
            created_at: now,
            updated_at: now,
            last_test_at: None,
            last_test_status: None,
            last_test_error: None,
            balance_query_enabled: false,
            balance_query_template: None,
            balance_query_base_url: None,
            balance_query_user_id: None,
            balance_query_config_json: None,
            last_balance_at: None,
            last_balance_status: None,
            last_balance_error: None,
            last_balance_json: None,
        })
        .expect("insert aggregate API");
}

fn set_catalog_aggregate_route(
    storage: &Storage,
    slug: &str,
    aggregate_id: &str,
    model_enabled: bool,
    route_enabled: bool,
) {
    let mut model = storage
        .get_managed_model_v2(slug)
        .expect("read model")
        .expect("seeded model");
    model.enabled = model_enabled;
    model.supported_in_api = true;
    model.visibility = "list".to_string();
    model.provider = Some("openai".to_string());
    model.routes = vec![ModelRouteV2 {
        id: String::new(),
        source_kind: "aggregate_api".to_string(),
        source_id: aggregate_id.to_string(),
        upstream_model: slug.to_string(),
        enabled: route_enabled,
        priority: 0,
        weight: 1,
    }];
    storage
        .upsert_managed_model_v2(&ManagedModelV2Upsert {
            previous_slug: Some(slug.to_string()),
            model,
        })
        .expect("save model route");
}

fn insert_catalog_key(storage: &Storage, id: &str, provider: UpstreamProvider) {
    let aggregate_id = match provider {
        UpstreamProvider::Claude => "agg-claude-pinned",
        UpstreamProvider::Openai => "agg-codex",
    };
    insert_catalog_key_with_route(
        storage,
        id,
        provider,
        "aggregate_api_rotation",
        "anthropic_native",
        Some(aggregate_id),
    );
}

fn insert_catalog_key_with_route(
    storage: &Storage,
    id: &str,
    provider: UpstreamProvider,
    rotation_strategy: &str,
    protocol_type: &str,
    aggregate_api_id: Option<&str>,
) {
    storage
        .insert_api_key(&ApiKey {
            id: id.to_string(),
            name: None,
            model_slug: None,
            reasoning_effort: None,
            service_tier: None,
            rotation_strategy: rotation_strategy.to_string(),
            upstream_provider: provider,
            aggregate_api_id: aggregate_api_id.map(str::to_string),
            account_plan_filter: None,
            aggregate_api_url: None,
            client_type: "claude_code".to_string(),
            protocol_type: protocol_type.to_string(),
            auth_scheme: "x_api_key".to_string(),
            upstream_base_url: None,
            static_headers_json: None,
            key_hash: format!("hash-{id}"),
            status: "active".to_string(),
            created_at: now_ts(),
            last_used_at: None,
        })
        .expect("insert platform key");
}

#[test]
fn claude_subscription_models_require_active_claude_account_and_claude_pool_route() {
    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");
    let mut model = storage
        .get_managed_model_v2("gpt-6-astra")
        .expect("read model")
        .expect("seeded model");
    model.routes = vec![ModelRouteV2 {
        id: String::new(),
        source_kind: "account_pool".to_string(),
        source_id: "claude".to_string(),
        upstream_model: "claude-sonnet-5".to_string(),
        enabled: true,
        priority: 0,
        weight: 1,
    }];
    storage
        .upsert_managed_model_v2(&ManagedModelV2Upsert {
            previous_slug: Some(model.slug.clone()),
            model,
        })
        .expect("save Claude account route");
    let catalog = crate::models_v2::models_response_with_storage(&storage).expect("catalog");
    let empty = filter_models_for_claude_accounts(&storage, catalog.clone()).expect("filter");
    assert!(empty.models.is_empty());

    let now = now_ts();
    storage
        .upsert_claude_subscription_account(&ClaudeSubscriptionAccount {
            id: "claude-test".to_string(),
            label: "test".to_string(),
            email: None,
            account_uuid: None,
            organization_uuid: None,
            subscription_type: Some("pro".to_string()),
            status: "active".to_string(),
            sort: 0,
            access_token: "test-access".to_string(),
            refresh_token: "test-refresh".to_string(),
            scopes: "user:inference".to_string(),
            expires_at: now + 3600,
            last_error: None,
            created_at: now,
            updated_at: now,
        })
        .expect("add Claude account");
    let filtered = filter_models_for_claude_accounts(&storage, catalog).expect("filter");
    let slugs = filtered
        .models
        .into_iter()
        .map(|model| model.slug)
        .collect::<HashSet<_>>();
    assert_eq!(slugs, HashSet::from(["gpt-6-astra".to_string()]));
    assert!(!slugs.contains("gpt-6-sol"));
}

#[test]
fn claude_models_list_matches_active_claude_aggregate_routes_across_pool() {
    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");
    insert_catalog_aggregate_api(&storage, "agg-claude-pinned", "claude", "active");
    insert_catalog_aggregate_api(&storage, "agg-claude-other", "anthropic_native", "active");
    insert_catalog_aggregate_api(&storage, "agg-claude-disabled", "claude", "disabled");
    insert_catalog_aggregate_api(&storage, "agg-compatible", "compatible", "active");
    insert_catalog_aggregate_api(&storage, "agg-codex", "codex", "active");
    set_catalog_aggregate_route(&storage, "gpt-6-astra", "agg-claude-pinned", true, true);
    set_catalog_aggregate_route(&storage, "gpt-5.6-sol", "agg-claude-other", true, true);
    set_catalog_aggregate_route(&storage, "gpt-5.6-terra", "agg-claude-disabled", true, true);
    set_catalog_aggregate_route(&storage, "gpt-5.6-luna", "agg-compatible", true, true);
    set_catalog_aggregate_route(&storage, "gpt-5.4", "agg-claude-pinned", true, false);
    set_catalog_aggregate_route(&storage, "gpt-5.4-mini", "agg-claude-pinned", false, true);
    insert_catalog_key(&storage, "gk-claude-catalog", UpstreamProvider::Claude);
    insert_catalog_key(&storage, "gk-openai-catalog", UpstreamProvider::Openai);

    let (cached, policy, claude_key) =
        read_models_response_for_key(&storage, "gk-claude-catalog").expect("read Claude catalog");
    assert_eq!(claude_key.upstream_provider, UpstreamProvider::Claude);
    assert_eq!(
        policy,
        crate::codex_model_catalog::GatewayCatalogPolicy::Managed
    );
    let (allowed, _) =
        filter_models_for_catalog_policy(&storage, "gk-claude-catalog", cached, policy)
            .expect("apply key permissions");
    let filtered =
        filter_models_for_claude_upstreams(&storage, allowed).expect("filter Claude routes");
    let slugs = filtered
        .models
        .iter()
        .map(|model| model.slug.as_str())
        .collect::<Vec<_>>();
    assert_eq!(slugs.len(), 2);
    assert!(slugs.contains(&"gpt-6-astra"));
    assert!(slugs.contains(&"gpt-5.6-sol")); // Not limited to the pinned aggregate API.
    assert!(!slugs.contains(&"gpt-6-sol")); // Account-pool-only GPT model.

    let (openai_models, _, openai_key) =
        read_models_response_for_key(&storage, "gk-openai-catalog").expect("read OpenAI catalog");
    assert_eq!(openai_key.upstream_provider, UpstreamProvider::Openai);
    assert!(openai_models.models.iter().any(|model| model.slug == "gpt-6-sol"));
}

#[test]
fn openai_managed_models_list_only_same_pool_or_hybrid_account_routes() {
    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");
    insert_catalog_aggregate_api(&storage, "agg-claude-pinned", "claude", "active");
    insert_catalog_aggregate_api(&storage, "agg-codex", "codex", "active");
    insert_catalog_aggregate_api(&storage, "agg-compatible", "compatible", "active");
    insert_catalog_aggregate_api(&storage, "agg-gemini", "gemini", "active");
    insert_catalog_aggregate_api(&storage, "agg-codex-disabled", "codex", "disabled");
    set_catalog_aggregate_route(&storage, "gpt-6-astra", "agg-claude-pinned", true, true);
    set_catalog_aggregate_route(&storage, "gpt-5.6-sol", "agg-codex", true, true);
    set_catalog_aggregate_route(&storage, "gpt-5.6-terra", "agg-compatible", true, true);
    set_catalog_aggregate_route(&storage, "gpt-5.6-luna", "agg-gemini", true, true);
    set_catalog_aggregate_route(&storage, "gpt-5.4", "agg-codex", true, false);
    set_catalog_aggregate_route(&storage, "gpt-5.4-mini", "agg-codex-disabled", true, true);
    insert_catalog_key_with_route(
        &storage,
        "gk-openai-aggregate",
        UpstreamProvider::Openai,
        "aggregate_api_rotation",
        "openai_compat",
        Some("agg-codex"),
    );
    insert_catalog_key_with_route(
        &storage,
        "gk-openai-hybrid",
        UpstreamProvider::Openai,
        "hybrid_rotation",
        "openai_compat",
        Some("agg-codex"),
    );
    let model = storage
        .get_managed_model_v2("gpt-6-astra")
        .expect("read mislabeled model")
        .expect("model exists");
    assert_eq!(model.provider.as_deref(), Some("openai"));

    let listed = |key_id| {
        let (cached, policy, key) =
            read_models_response_for_key(&storage, key_id).expect("read managed catalog");
        assert_eq!(policy, crate::codex_model_catalog::GatewayCatalogPolicy::Managed);
        let (allowed, _) = filter_models_for_catalog_policy(&storage, key_id, cached, policy)
            .expect("apply key permissions");
        filter_models_for_openai_managed_upstreams(&storage, allowed, &key)
            .expect("filter OpenAI routes")
            .models
            .into_iter()
            .map(|model| model.slug)
            .collect::<HashSet<_>>()
    };
    let aggregate = listed("gk-openai-aggregate");
    assert!(aggregate.contains("gpt-5.6-sol"));
    assert!(aggregate.contains("gpt-5.6-terra")); // Compatible is in the Codex pool.
    assert!(!aggregate.contains("gpt-6-astra")); // Metadata alone cannot select Claude.
    assert!(!aggregate.contains("gpt-6-sol")); // Account-pool route is not aggregate.
    assert!(!aggregate.contains("gpt-5.6-luna")); // Gemini is a separate pool.
    assert!(!aggregate.contains("gpt-5.4")); // Disabled route.
    assert!(!aggregate.contains("gpt-5.4-mini")); // Disabled upstream.

    let hybrid = listed("gk-openai-hybrid");
    assert!(hybrid.contains("gpt-6-sol"));
    assert!(hybrid.contains("gpt-5.6-sol"));
    assert!(!hybrid.contains("gpt-6-astra"));

    // The legacy API-key profile schema cannot persist gemini_native yet; test
    // the protocol-aware route selection with a request-time Key value.
    let (cached, policy, mut gemini_key) =
        read_models_response_for_key(&storage, "gk-openai-aggregate").expect("read Gemini base key");
    gemini_key.protocol_type = "gemini_native".to_string();
    let (allowed, _) =
        filter_models_for_catalog_policy(&storage, "gk-openai-aggregate", cached, policy)
            .expect("apply key permissions");
    let gemini = filter_models_for_openai_managed_upstreams(&storage, allowed, &gemini_key)
        .expect("filter Gemini routes")
        .models
        .into_iter()
        .map(|model| model.slug)
        .collect::<HashSet<_>>();
    assert!(gemini.contains("gpt-5.6-luna"));
    assert!(!gemini.contains("gpt-5.6-sol"));
    assert!(!gemini.contains("gpt-6-astra"));
}

#[test]
fn managed_codex_catalog_only_contains_models_reachable_by_its_key() {
    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");
    insert_catalog_aggregate_api(&storage, "agg-claude-pinned", "claude", "active");
    insert_catalog_aggregate_api(&storage, "agg-codex", "codex", "active");
    set_catalog_aggregate_route(&storage, "gpt-6-astra", "agg-claude-pinned", true, true);
    set_catalog_aggregate_route(&storage, "gpt-5.6-sol", "agg-codex", true, true);
    insert_catalog_key_with_route(
        &storage,
        "gk-openai-local-catalog",
        UpstreamProvider::Openai,
        "aggregate_api_rotation",
        "openai_compat",
        Some("agg-codex"),
    );
    insert_catalog_key_with_route(
        &storage,
        "gk-claude-local-catalog",
        UpstreamProvider::Claude,
        "aggregate_api_rotation",
        "anthropic_native",
        Some("agg-claude-pinned"),
    );

    for (key_id, expected_slug) in [
        ("gk-openai-local-catalog", "gpt-5.6-sol"),
        ("gk-claude-local-catalog", "gpt-6-astra"),
    ] {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "gateway-model-catalog-{}-{unique}.json",
            std::process::id()
        ));
        let count = crate::codex_model_catalog::write_gateway_model_catalog(
            &storage,
            key_id,
            &path,
            crate::codex_model_catalog::GatewayCatalogPolicy::Managed,
        )
        .expect("write filtered Codex catalog");
        let catalog: Value = serde_json::from_str(
            &std::fs::read_to_string(&path).expect("read Codex catalog"),
        )
        .expect("parse Codex catalog");
        std::fs::remove_file(&path).expect("remove test catalog");

        assert_eq!(count, 1);
        assert_eq!(catalog["models"].as_array().map(Vec::len), Some(1));
        assert_eq!(catalog["models"][0]["slug"], expected_slug);
    }
}

#[test]
fn official_account_catalog_bypasses_manager_key_filters() {
    let storage = codexmanager_core::storage::Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");
    let models = ModelsResponse {
        models: vec![ModelInfo {
            slug: "future-official-model".to_string(),
            display_name: "Future Official Model".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let (filtered, include_implicit) = filter_models_for_catalog_policy(
        &storage,
        "missing-key-would-fail-managed-filtering",
        models,
        crate::codex_model_catalog::GatewayCatalogPolicy::OfficialAccountPool,
    )
    .expect("official catalog bypasses key filter");

    assert!(include_implicit);
    assert_eq!(filtered.models.len(), 1);
    assert_eq!(filtered.models[0].slug, "future-official-model");
}

/// 函数 `serialize_models_response_outputs_codex_and_api_shapes`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-02
///
/// # 参数
/// 无
///
/// # 返回
/// 无
#[test]
fn serialize_models_response_outputs_codex_and_api_shapes() {
    let items = ModelsResponse {
        models: vec![
            ModelInfo {
                slug: "gpt-5.3-codex".to_string(),
                display_name: "GPT-5.3 Codex".to_string(),
                supported_in_api: true,
                visibility: Some("list".to_string()),
                ..Default::default()
            },
            ModelInfo {
                slug: "gpt-4o".to_string(),
                display_name: "GPT-4o".to_string(),
                supported_in_api: true,
                visibility: Some("list".to_string()),
                ..Default::default()
            },
        ],
        extra: std::collections::BTreeMap::from([(
            "etag".to_string(),
            serde_json::json!("\"abc\""),
        )]),
        ..Default::default()
    };
    let output = serialize_models_response(&items);
    let value: Value = serde_json::from_str(&output).expect("valid json");
    assert_eq!(value.get("object").and_then(Value::as_str), Some("list"));
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .expect("OpenAI-compatible data array");
    assert_eq!(data.len(), 2);
    assert_eq!(
        data[0].get("id").and_then(Value::as_str),
        Some("gpt-5.3-codex")
    );
    assert_eq!(data[0].get("object").and_then(Value::as_str), Some("model"));
    assert_eq!(
        data[0].get("owned_by").and_then(Value::as_str),
        Some("codexmanager")
    );
    assert_eq!(data[1].get("id").and_then(Value::as_str), Some("gpt-4o"));
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .expect("models array");
    assert_eq!(models.len(), 2);
    assert_eq!(
        models[0].get("slug").and_then(Value::as_str),
        Some("gpt-5.3-codex")
    );
    assert_eq!(
        models[1].get("slug").and_then(Value::as_str),
        Some("gpt-4o")
    );
    assert_eq!(
        models[0].get("display_name").and_then(Value::as_str),
        Some("GPT-5.3 Codex")
    );
    assert_eq!(
        models[1].get("visibility").and_then(Value::as_str),
        Some("list")
    );
    assert_eq!(value.as_object().map(|object| object.len()), Some(3));
    assert!(value.get("etag").is_none());
}

#[test]
fn serialize_models_response_preserves_description_for_codex_and_api_clients() {
    let items = ModelsResponse {
        models: vec![ModelInfo {
            slug: "gpt-5.3-codex".to_string(),
            display_name: "GPT-5.3 Codex".to_string(),
            description: Some("Latest frontier agentic coding model.".to_string()),
            supported_in_api: true,
            visibility: Some("list".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };

    let output = serialize_models_response(&items);
    let value: Value = serde_json::from_str(&output).expect("valid json");
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .expect("models array");
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .expect("OpenAI-compatible data array");
    assert_eq!(models.len(), 1);
    assert_eq!(
        models[0].get("description").and_then(Value::as_str),
        Some("Latest frontier agentic coding model.")
    );
    assert_eq!(
        data[0].get("description").and_then(Value::as_str),
        Some("Latest frontier agentic coding model.")
    );
}

#[test]
fn serialize_models_response_preserves_service_tier_capabilities_for_codex_clients() {
    let items = ModelsResponse {
        models: vec![ModelInfo {
            slug: "gpt-5.5-codex".to_string(),
            display_name: "GPT-5.5 Codex".to_string(),
            supported_in_api: true,
            additional_speed_tiers: vec!["fast".to_string()],
            service_tiers: vec![
                ModelServiceTier {
                    id: "priority".to_string(),
                    name: "Fast".to_string(),
                    description: "Faster responses with increased usage.".to_string(),
                    ..Default::default()
                },
                ModelServiceTier {
                    id: "flex".to_string(),
                    name: "Flex".to_string(),
                    description: "Lower priority capacity.".to_string(),
                    ..Default::default()
                },
            ],
            default_service_tier: Some("priority".to_string()),
            upgrade_info: Some(serde_json::json!({
                "model": "gpt-5.5-codex",
                "upgrade_copy": "Use the newer coding model"
            })),
            ..Default::default()
        }],
        ..Default::default()
    };

    let output = serialize_models_response(&items);
    let value: Value = serde_json::from_str(&output).expect("valid json");
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .expect("models array");

    assert_eq!(
        models[0]["service_tiers"][0]
            .get("id")
            .and_then(Value::as_str),
        Some("priority")
    );
    assert_eq!(
        models[0]["service_tiers"][0]
            .get("name")
            .and_then(Value::as_str),
        Some("Fast")
    );
    assert_eq!(
        models[0]["additional_speed_tiers"],
        serde_json::json!(["fast"])
    );
    assert_eq!(
        models[0]
            .get("default_service_tier")
            .and_then(Value::as_str),
        Some("priority")
    );
    assert_eq!(
        models[0]["upgrade_info"]
            .get("model")
            .and_then(Value::as_str),
        Some("gpt-5.5-codex")
    );
}

#[test]
fn serialize_models_response_does_not_invent_codex_image_tool_model() {
    let items = ModelsResponse {
        models: vec![ModelInfo {
            slug: "gpt-5.4-mini".to_string(),
            display_name: "GPT-5.4 Mini".to_string(),
            supported_in_api: true,
            visibility: Some("list".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };

    let output = serialize_models_response(&items);
    let value: Value = serde_json::from_str(&output).expect("valid json");
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .expect("models array");

    assert_eq!(models.len(), 1);
    assert_eq!(
        models[0].get("slug").and_then(Value::as_str),
        Some("gpt-5.4-mini")
    );
}

#[test]
fn serialize_models_response_filters_api_data_to_supported_models() {
    let items = ModelsResponse {
        models: vec![
            ModelInfo {
                slug: "gpt-supported".to_string(),
                display_name: "GPT Supported".to_string(),
                supported_in_api: true,
                visibility: Some("list".to_string()),
                ..Default::default()
            },
            ModelInfo {
                slug: "gpt-hidden".to_string(),
                display_name: "GPT Hidden".to_string(),
                supported_in_api: false,
                visibility: Some("hidden".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let output = serialize_models_response(&items);
    let value: Value = serde_json::from_str(&output).expect("valid json");
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .expect("OpenAI-compatible data array");
    let ids = data
        .iter()
        .filter_map(|model| model.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();

    assert!(ids.contains(&"gpt-supported"));
    assert!(!ids.contains(&"gpt-hidden"));
}

#[test]
fn models_etag_header_uses_extra_etag_value() {
    let items = ModelsResponse {
        models: vec![],
        extra: std::collections::BTreeMap::from([(
            "etag".to_string(),
            serde_json::json!("\"remote-etag\""),
        )]),
    };

    let header = models_etag_header(&items)
        .expect("etag header should build")
        .expect("etag header should exist");

    assert!(header.field.equiv("etag"));
    assert_eq!(header.value.as_str(), "\"remote-etag\"");
}

#[test]
fn local_models_lists_image_model_once_with_image_only_capabilities() {
    let storage = codexmanager_core::storage::Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");

    let response = crate::models_v2::models_response_with_storage(&storage)
        .expect("read managed local models");
    assert_eq!(response.models.len(), 10);
    assert!(response.models.iter().any(|model| model.slug == "gpt-6-sol"));
    let image_models = response
        .models
        .iter()
        .filter(|model| model.slug == "gpt-image-2")
        .collect::<Vec<_>>();
    assert_eq!(image_models.len(), 1);
    let image = image_models[0];
    assert_eq!(image.input_modalities, ["text", "image"]);
    assert_eq!(
        image.extra["output_modalities"],
        serde_json::json!(["image"])
    );
    assert_eq!(
        image.extra["supported_endpoints"],
        serde_json::json!(["/v1/images/generations", "/v1/images/edits"])
    );
    assert_eq!(image.extra["supports_text_generation"], false);

    let serialized: Value =
        serde_json::from_str(&serialize_models_response(&response)).expect("serialize models");
    let data = serialized["data"].as_array().expect("data array");
    assert_eq!(
        data.iter()
            .filter(|model| model["id"] == "gpt-image-2")
            .count(),
        1
    );
}
