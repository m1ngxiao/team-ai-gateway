use super::{
    infer_upstream_provider, is_anthropic_request_path, is_gemini_count_tokens_request_path,
    is_gemini_generate_content_request_path, normalize_protocol_type, normalize_rotation_strategy,
    normalize_upstream_provider, resolve_gateway_protocol_type, validate_claude_model_route,
    validate_upstream_provider_route, PROTOCOL_ANTHROPIC_NATIVE,
    PROTOCOL_GEMINI_NATIVE, PROTOCOL_OPENAI_COMPAT, ROTATION_ACCOUNT, ROTATION_AGGREGATE_API,
    ROTATION_HYBRID, ROTATION_HYBRID_AGGREGATE_FIRST,
};
use codexmanager_core::storage::{
    AggregateApi, ManagedModelV2Upsert, ModelRouteV2, Storage, UpstreamProvider,
};

#[test]
fn claude_subscription_key_requires_its_own_account_pool_route() {
    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("initialize storage");
    assert!(validate_upstream_provider_route(
        &storage,
        UpstreamProvider::Claude,
        ROTATION_ACCOUNT,
        None,
    )
    .is_ok());
    assert!(validate_upstream_provider_route(
        &storage,
        UpstreamProvider::Claude,
        ROTATION_ACCOUNT,
        Some("aggregate-claude"),
    )
    .is_err());
    assert!(validate_claude_model_route(
        &storage,
        UpstreamProvider::Claude,
        ROTATION_ACCOUNT,
        Some("gpt-6-astra"),
    )
    .is_err());

    let mut model = storage
        .get_managed_model_v2("gpt-6-astra")
        .expect("read model")
        .expect("seeded model");
    model.routes.push(ModelRouteV2 {
        source_kind: "account_pool".to_string(),
        source_id: "claude".to_string(),
        upstream_model: "claude-sonnet-5".to_string(),
        enabled: true,
        priority: 0,
        weight: 1,
        ..Default::default()
    });
    storage
        .upsert_managed_model_v2(&ManagedModelV2Upsert {
            previous_slug: Some(model.slug.clone()),
            model,
        })
        .expect("save Claude route");
    assert!(validate_claude_model_route(
        &storage,
        UpstreamProvider::Claude,
        ROTATION_ACCOUNT,
        Some("gpt-6-astra"),
    )
    .is_ok());
    assert!(validate_claude_model_route(
        &storage,
        UpstreamProvider::Claude,
        ROTATION_AGGREGATE_API,
        Some("gpt-6-astra"),
    )
    .is_err());
}

#[test]
fn upstream_provider_accepts_only_openai_or_claude() {
    assert_eq!(
        normalize_upstream_provider(" OpenAI "),
        Ok(UpstreamProvider::Openai)
    );
    assert_eq!(
        normalize_upstream_provider("claude"),
        Ok(UpstreamProvider::Claude)
    );
    assert!(normalize_upstream_provider("gemini").is_err());
}

#[test]
fn legacy_unpinned_key_needs_provider_when_compatible_and_codex_coexist() {
    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("initialize storage");
    for (id, provider_type) in [("compatible-api", "compatible"), ("codex-api", "codex")] {
        storage
            .insert_aggregate_api(&AggregateApi {
                id: id.to_string(),
                provider_type: provider_type.to_string(),
                supplier_name: None,
                sort: 0,
                url: "https://example.test/v1".to_string(),
                auth_type: "apikey".to_string(),
                auth_params_json: None,
                action: None,
                model_override: None,
                user_agent: None,
                status: "active".to_string(),
                created_at: 1,
                updated_at: 1,
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
    assert!(infer_upstream_provider(&storage, ROTATION_AGGREGATE_API, None).is_err());
    assert!(
        infer_upstream_provider(&storage, ROTATION_AGGREGATE_API, Some("compatible-api")).is_err()
    );
    assert_eq!(
        infer_upstream_provider(&storage, ROTATION_AGGREGATE_API, Some("codex-api")),
        Ok(UpstreamProvider::Openai)
    );
}

#[test]
fn normalize_rotation_strategy_accepts_hybrid_aliases() {
    for value in [
        "hybrid_rotation",
        "hybrid",
        "mixed",
        "mixed-rotation",
        "混合轮转",
        "账号优先聚合兜底",
    ] {
        assert_eq!(
            normalize_rotation_strategy(Some(value.to_string())).as_deref(),
            Ok(ROTATION_HYBRID)
        );
    }
}

#[test]
fn normalize_rotation_strategy_accepts_hybrid_aggregate_first_aliases() {
    for value in [
        "hybrid_aggregate_first_rotation",
        "hybrid_aggregate_first",
        "mixed_aggregate_first",
        "aggregate_first",
        "聚合优先",
        "聚合优先账号兜底",
    ] {
        assert_eq!(
            normalize_rotation_strategy(Some(value.to_string())).as_deref(),
            Ok(ROTATION_HYBRID_AGGREGATE_FIRST)
        );
    }
}

#[test]
fn normalize_rotation_strategy_keeps_existing_values() {
    assert_eq!(
        normalize_rotation_strategy(None).as_deref(),
        Ok(ROTATION_ACCOUNT)
    );
    assert_eq!(
        normalize_rotation_strategy(Some("aggregate_api_rotation".to_string())).as_deref(),
        Ok(ROTATION_AGGREGATE_API)
    );
}

#[test]
fn wildcard_protocol_routes_messages_path_to_anthropic() {
    assert!(is_anthropic_request_path("/v1/messages"));
    assert_eq!(
        resolve_gateway_protocol_type(PROTOCOL_OPENAI_COMPAT, "/v1/messages"),
        PROTOCOL_ANTHROPIC_NATIVE
    );
}

#[test]
fn wildcard_protocol_routes_responses_path_to_openai() {
    assert_eq!(
        resolve_gateway_protocol_type(PROTOCOL_ANTHROPIC_NATIVE, "/v1/responses"),
        PROTOCOL_OPENAI_COMPAT
    );
}

#[test]
fn wildcard_protocol_routes_gemini_generate_content_path_to_gemini() {
    assert!(is_gemini_generate_content_request_path(
        "/v1beta/models/gemini-2.5-pro:generateContent"
    ));
    assert_eq!(
        resolve_gateway_protocol_type(
            PROTOCOL_OPENAI_COMPAT,
            "/v1beta/models/gemini-2.5-pro:generateContent"
        ),
        PROTOCOL_GEMINI_NATIVE
    );
}

#[test]
fn wildcard_protocol_routes_gemini_count_tokens_path_to_gemini() {
    assert!(is_gemini_count_tokens_request_path(
        "/v1beta/models/gemini-2.5-pro:countTokens?alt=json"
    ));
    assert_eq!(
        resolve_gateway_protocol_type(
            PROTOCOL_OPENAI_COMPAT,
            "/v1beta/models/gemini-2.5-pro:countTokens?alt=json"
        ),
        PROTOCOL_GEMINI_NATIVE
    );
}

#[test]
fn wildcard_protocol_routes_gemini_cli_internal_generate_content_path_to_gemini() {
    assert!(is_gemini_generate_content_request_path(
        "/v1internal:streamGenerateContent?alt=sse"
    ));
    assert_eq!(
        resolve_gateway_protocol_type(
            PROTOCOL_OPENAI_COMPAT,
            "/v1internal:streamGenerateContent?alt=sse"
        ),
        PROTOCOL_GEMINI_NATIVE
    );
}

#[test]
fn wildcard_protocol_routes_gemini_cli_internal_count_tokens_path_to_gemini() {
    assert!(is_gemini_count_tokens_request_path(
        "/v1internal:countTokens"
    ));
    assert_eq!(
        resolve_gateway_protocol_type(PROTOCOL_OPENAI_COMPAT, "/v1internal:countTokens"),
        PROTOCOL_GEMINI_NATIVE
    );
}

#[test]
fn removed_azure_protocol_falls_back_to_wildcard_routing() {
    assert_eq!(
        resolve_gateway_protocol_type("azure_openai", "/v1/messages"),
        PROTOCOL_ANTHROPIC_NATIVE
    );
    assert_eq!(
        resolve_gateway_protocol_type("azure_openai", "/v1/responses"),
        PROTOCOL_OPENAI_COMPAT
    );
}

#[test]
fn removed_azure_protocol_is_rejected_for_profile_configuration() {
    let err = normalize_protocol_type(Some("azure_openai".to_string()))
        .expect_err("azure profile protocol should be rejected");
    assert!(err.contains("unsupported protocol type: azure_openai"));
}
