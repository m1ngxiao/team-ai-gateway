use bytes::Bytes;
use codexmanager_core::storage::{ConversationBinding, UpstreamProvider};
use reqwest::Method;
use tiny_http::Request;

use super::conversation_binding::RouteConversationSource;

mod auth;
mod io;
mod request;

pub(super) struct LocalValidationResult {
    pub(super) trace_id: String,
    pub(super) incoming_headers: super::IncomingHeaderSnapshot,
    pub(super) storage: crate::storage_helpers::StorageHandle,
    pub(super) original_path: String,
    pub(super) passthrough_path: String,
    pub(super) path: String,
    pub(super) passthrough_body: Bytes,
    pub(super) body: Bytes,
    pub(super) is_stream: bool,
    pub(super) has_prompt_cache_key: bool,
    pub(super) request_shape: Option<String>,
    pub(super) protocol_type: String,
    pub(super) upstream_provider: UpstreamProvider,
    pub(super) rotation_strategy: String,
    pub(super) aggregate_api_id: Option<String>,
    pub(super) account_group_filter: Option<String>,
    pub(super) account_plan_filter: Option<String>,
    pub(super) response_adapter: super::ResponseAdapter,
    pub(super) gemini_stream_output_mode: Option<super::GeminiStreamOutputMode>,
    pub(super) tool_name_restore_map: super::ToolNameRestoreMap,
    pub(super) context_management: super::anthropic_context_management::ContextManagementState,
    pub(super) request_method: String,
    pub(super) key_id: String,
    pub(super) platform_key_hash: String,
    pub(super) local_conversation_id: Option<String>,
    pub(super) route_conversation_id: Option<String>,
    pub(super) route_conversation_source: Option<RouteConversationSource>,
    pub(super) conversation_binding: Option<ConversationBinding>,
    pub(super) client_model_for_log: Option<String>,
    pub(super) model_for_log: Option<String>,
    pub(super) model_source_for_log: Option<String>,
    pub(super) client_reasoning_for_log: Option<String>,
    pub(super) reasoning_for_log: Option<String>,
    pub(super) reasoning_source_for_log: Option<String>,
    pub(super) service_tier_for_log: Option<String>,
    pub(super) effective_service_tier_for_log: Option<String>,
    pub(super) service_tier_source_for_log: Option<String>,
    pub(super) gateway_mode_for_log: Option<String>,
    pub(super) method: Method,
    pub(super) has_anthropic_safeguards: bool,
    pub(super) anthropic_safeguards_capability:
        Option<super::anthropic_safeguards::SafeguardsCapability>,
}

pub(super) struct LocalValidationError {
    pub(super) status_code: u16,
    pub(super) message: String,
    pub(super) key_id: Option<String>,
    pub(super) model_for_log: Option<String>,
}

impl LocalValidationError {
    /// 函数 `new`
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
    pub(super) fn new(status_code: u16, message: impl Into<String>) -> Self {
        Self {
            status_code,
            message: message.into(),
            key_id: None,
            model_for_log: None,
        }
    }

    pub(super) fn with_key_id(mut self, key_id: impl Into<String>) -> Self {
        self.key_id = Some(key_id.into());
        self
    }

    pub(super) fn with_key_id_if_missing(mut self, key_id: impl Into<String>) -> Self {
        if self.key_id.is_none() {
            self.key_id = Some(key_id.into());
        }
        self
    }

    pub(super) fn with_model_if_missing(mut self, model: Option<String>) -> Self {
        if self.model_for_log.is_none() {
            self.model_for_log = model;
        }
        self
    }
}

fn extract_model_hint_for_error_log(request_url: &str, body: &[u8]) -> Option<String> {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(model) = value
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            return Some(model.to_string());
        }
    }
    let normalized = request_url.split('?').next().unwrap_or(request_url);
    let marker = "/models/";
    let start = normalized.find(marker)? + marker.len();
    let tail = &normalized[start..];
    let end = tail.find(':').unwrap_or(tail.len());
    let model = tail[..end].trim();
    (!model.is_empty()).then(|| model.to_string())
}

pub(super) fn load_active_api_key_for_platform_key(
    storage: &codexmanager_core::storage::Storage,
    platform_key: &str,
    request_url: &str,
) -> Result<codexmanager_core::storage::ApiKey, LocalValidationError> {
    load_active_api_key_for_platform_key_with_options(storage, platform_key, request_url, false)
}

pub(super) fn load_active_api_key_for_platform_key_with_options(
    storage: &codexmanager_core::storage::Storage,
    platform_key: &str,
    request_url: &str,
    skip_quota_check: bool,
) -> Result<codexmanager_core::storage::ApiKey, LocalValidationError> {
    auth::load_active_api_key_with_options(storage, platform_key, request_url, false, skip_quota_check)
}

pub(super) fn load_active_api_key_for_id(
    storage: &codexmanager_core::storage::Storage,
    key_id: &str,
    request_url: &str,
) -> Result<codexmanager_core::storage::ApiKey, LocalValidationError> {
    load_active_api_key_for_id_with_options(storage, key_id, request_url, false)
}

pub(super) fn load_active_api_key_for_id_with_options(
    storage: &codexmanager_core::storage::Storage,
    key_id: &str,
    request_url: &str,
    skip_quota_check: bool,
) -> Result<codexmanager_core::storage::ApiKey, LocalValidationError> {
    auth::load_active_api_key_by_id_with_options(storage, key_id, request_url, skip_quota_check)
}

pub(super) fn authenticate_platform_request(
    request: &Request,
    debug: bool,
) -> Result<
    (
        crate::storage_helpers::StorageHandle,
        super::IncomingHeaderSnapshot,
        codexmanager_core::storage::ApiKey,
    ),
    LocalValidationError,
> {
    let incoming_headers = super::IncomingHeaderSnapshot::from_request(request);
    let platform_key = io::extract_platform_key_or_error(request, &incoming_headers, debug)?;
    let storage = auth::open_storage_or_error()?;
    let api_key = auth::load_active_api_key(&storage, &platform_key, request.url(), debug)?;
    Ok((storage, incoming_headers, api_key))
}

/// 函数 `prepare_local_request`
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
pub(super) fn prepare_local_request(
    request: &mut Request,
    trace_id: String,
    debug: bool,
) -> Result<LocalValidationResult, LocalValidationError> {
    let body = io::read_request_body(request)?;
    let model_hint = extract_model_hint_for_error_log(request.url(), &body);
    let incoming_headers = super::IncomingHeaderSnapshot::from_request(request);
    let platform_key = io::extract_platform_key_or_error(request, &incoming_headers, debug)
        .map_err(|err| err.with_model_if_missing(model_hint.clone()))?;

    let storage = auth::open_storage_or_error()
        .map_err(|err| err.with_model_if_missing(model_hint.clone()))?;
    let skip_quota_check = super::local_usage::is_local_usage_shortcut_request(
        request.url(),
        request.method().as_str(),
        &body,
    );
    let api_key = auth::load_active_api_key_with_options(
        &storage,
        &platform_key,
        request.url(),
        debug,
        skip_quota_check,
    )
    .map_err(|err| err.with_model_if_missing(model_hint.clone()))?;
    let key_id_for_err = api_key.id.clone();

    request::build_local_validation_result(
        request,
        trace_id,
        incoming_headers,
        storage,
        body,
        api_key,
    )
    .map_err(|err| {
        err.with_key_id_if_missing(key_id_for_err)
            .with_model_if_missing(model_hint)
    })
}

#[cfg(test)]
mod tests {
    use super::prepare_local_request;
    use codexmanager_core::storage::{now_ts, ApiKey, RequestTokenStat, UpstreamProvider};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct LocalDbEnvGuard {
        original: Option<OsString>,
    }

    impl LocalDbEnvGuard {
        fn set(db_path: &str) -> Self {
            let original = std::env::var_os("CODEXMANAGER_DB_PATH");
            std::env::set_var("CODEXMANAGER_DB_PATH", db_path);
            Self { original }
        }
    }

    impl Drop for LocalDbEnvGuard {
        fn drop(&mut self) {
            match self.original.as_ref() {
                Some(value) => std::env::set_var("CODEXMANAGER_DB_PATH", value),
                None => std::env::remove_var("CODEXMANAGER_DB_PATH"),
            }
        }
    }

    struct TempDbCleanup(PathBuf);

    impl Drop for TempDbCleanup {
        fn drop(&mut self) {
            let base = self.0.to_string_lossy();
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{base}{suffix}"));
            }
        }
    }

    fn setup_test_key(
        prefix: &str,
        key_id: &str,
        raw_secret: &str,
        protocol_type: &str,
    ) -> (
        crate::storage_helpers::StorageHandle,
        std::sync::MutexGuard<'static, ()>,
        LocalDbEnvGuard,
        TempDbCleanup,
    ) {
        let env_lock = crate::test_env_guard();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let db_path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}.db", std::process::id()));
        let cleanup = TempDbCleanup(db_path.clone());
        let db_env = LocalDbEnvGuard::set(db_path.to_str().expect("utf8 db path"));

        crate::storage_helpers::initialize_storage().expect("initialize local validation db");
        let storage = crate::storage_helpers::open_storage().expect("open local validation db");
        storage
            .insert_api_key(&ApiKey {
                id: key_id.to_string(),
                name: Some(key_id.to_string()),
                model_slug: None,
                reasoning_effort: None,
                service_tier: None,
                rotation_strategy: crate::apikey_profile::ROTATION_ACCOUNT.to_string(),
                upstream_provider: UpstreamProvider::Openai,
                aggregate_api_id: None,
                account_plan_filter: None,
                aggregate_api_url: None,
                client_type: "codex".to_string(),
                protocol_type: protocol_type.to_string(),
                auth_scheme: "authorization_bearer".to_string(),
                upstream_base_url: None,
                static_headers_json: None,
                key_hash: crate::storage_helpers::hash_platform_key(raw_secret),
                status: "active".to_string(),
                created_at: now_ts(),
                last_used_at: None,
            })
            .expect("insert test api key");

        (storage, env_lock, db_env, cleanup)
    }

    #[test]
    fn prepare_local_request_preserves_key_id_and_model_on_weekly_quota_exhausted() {
        let (storage, _env_lock, _db_env, _cleanup) = setup_test_key(
            "codexmanager-lv-quota",
            "gk_lv_quota",
            "sk-lv-quota-secret",
            crate::apikey_profile::PROTOCOL_OPENAI_COMPAT,
        );
        let now = now_ts();
        storage
            .insert_request_token_stat(&RequestTokenStat {
                request_log_id: 1,
                key_id: Some("gk_lv_quota".to_string()),
                account_id: None,
                model: Some("gpt-6-astra".to_string()),
                actual_source_kind: None,
                actual_source_id: None,
                input_tokens: Some(1_000),
                cached_input_tokens: Some(0),
                output_tokens: Some(500),
                total_tokens: Some(1_500),
                reasoning_output_tokens: Some(0),
                estimated_cost_usd: Some(250.0),
                created_at: now - 60,
            })
            .expect("insert quota exhausting token stat");
        drop(storage);

        let mut req: tiny_http::Request = tiny_http::TestRequest::new()
            .with_method(tiny_http::Method::Post)
            .with_path("/v1/responses")
            .with_header(
                tiny_http::Header::from_bytes("Authorization", "Bearer sk-lv-quota-secret")
                    .expect("auth header"),
            )
            .with_body(r#"{"model":"gpt-6-astra","input":"hello"}"#)
            .into();

        let err = match prepare_local_request(&mut req, "trace-lv-quota".to_string(), false) {
            Ok(_) => panic!("weekly quota exhaustion must return error"),
            Err(err) => err,
        };
        assert_eq!(err.status_code, 429);
        assert_eq!(err.key_id.as_deref(), Some("gk_lv_quota"));
        assert_eq!(err.model_for_log.as_deref(), Some("gpt-6-astra"));
    }

    #[test]
    fn prepare_local_request_preserves_key_id_and_model_on_unlisted_anthropic_model() {
        let (storage, _env_lock, _db_env, _cleanup) = setup_test_key(
            "codexmanager-lv-anthropic",
            "gk_lv_anthropic",
            "sk-lv-anthropic-secret",
            crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE,
        );
        drop(storage);

        let mut req: tiny_http::Request = tiny_http::TestRequest::new()
            .with_method(tiny_http::Method::Post)
            .with_path("/v1/messages")
            .with_header(
                tiny_http::Header::from_bytes("x-api-key", "sk-lv-anthropic-secret")
                    .expect("x-api-key header"),
            )
            .with_body(r#"{"model":"nonexistent-v25-model","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}"#)
            .into();

        let err = match prepare_local_request(&mut req, "trace-lv-anthropic".to_string(), false) {
            Ok(_) => panic!("unlisted anthropic model must return error"),
            Err(err) => err,
        };
        assert_eq!(err.status_code, 404);
        assert!(
            err.message.contains("model_not_found"),
            "unexpected error message: {}",
            err.message
        );
        assert_eq!(err.key_id.as_deref(), Some("gk_lv_anthropic"));
        assert_eq!(
            err.model_for_log.as_deref(),
            Some("nonexistent-v25-model")
        );
    }

    #[test]
    fn prepare_local_request_allows_usage_shortcut_when_quota_exhausted_or_model_unlisted() {
        let (storage, _env_lock, _db_env, _cleanup) = setup_test_key(
            "codexmanager-lv-usage-bypass",
            "gk_lv_usage",
            "sk-lv-usage-secret",
            crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE,
        );
        let now = now_ts();
        storage
            .insert_request_token_stat(&RequestTokenStat {
                request_log_id: 2,
                key_id: Some("gk_lv_usage".to_string()),
                account_id: None,
                model: Some("gpt-6-astra".to_string()),
                actual_source_kind: None,
                actual_source_id: None,
                input_tokens: Some(1_000),
                cached_input_tokens: Some(0),
                output_tokens: Some(500),
                total_tokens: Some(1_500),
                reasoning_output_tokens: Some(0),
                estimated_cost_usd: Some(250.0),
                created_at: now - 60,
            })
            .expect("insert quota exhausting token stat");
        drop(storage);

        let mut get_usage: tiny_http::Request = tiny_http::TestRequest::new()
            .with_method(tiny_http::Method::Get)
            .with_path("/usage?format=statusline")
            .with_header(
                tiny_http::Header::from_bytes("Authorization", "Bearer sk-lv-usage-secret")
                    .expect("auth header"),
            )
            .into();
        let validated = prepare_local_request(&mut get_usage, "tr-lv-u1".to_string(), false)
            .unwrap_or_else(|err| panic!("GET /usage must bypass quota exhaustion: {} {}", err.status_code, err.message));
        assert_eq!(validated.original_path, "/usage?format=statusline");

        let mut chat_usage: tiny_http::Request = tiny_http::TestRequest::new()
            .with_method(tiny_http::Method::Post)
            .with_path("/v1/messages")
            .with_header(
                tiny_http::Header::from_bytes("x-api-key", "sk-lv-usage-secret")
                    .expect("x-api-key header"),
            )
            .with_body(r#"{"model":"unlisted-model-name","max_tokens":1024,"messages":[{"role":"user","content":"<system-reminder>x</system-reminder>\n/usage"}]}"#)
            .into();
        let validated_chat = prepare_local_request(&mut chat_usage, "tr-lv-u2".to_string(), false)
            .unwrap_or_else(|err| panic!("POST /v1/messages /usage must bypass quota exhaustion: {} {}", err.status_code, err.message));
        assert_eq!(validated_chat.gateway_mode_for_log.as_deref(), Some("local_usage"));

        let mut normal_req: tiny_http::Request = tiny_http::TestRequest::new()
            .with_method(tiny_http::Method::Post)
            .with_path("/v1/messages")
            .with_header(
                tiny_http::Header::from_bytes("x-api-key", "sk-lv-usage-secret")
                    .expect("x-api-key header"),
            )
            .with_body(r#"{"model":"nonexistent-v25-model","max_tokens":16,"messages":[{"role":"user","content":"hello"}]}"#)
            .into();
        let err = match prepare_local_request(&mut normal_req, "tr-lv-u3".to_string(), false) {
            Ok(_) => panic!("normal request must still fail with 429"),
            Err(err) => err,
        };
        assert_eq!(err.status_code, 429);
    }

    #[test]
    fn prepare_local_request_supports_claude_forwarding_window_suffix_and_duplicate_v1() {
        let (storage, _env_lock, _db_env, _cleanup) = setup_test_key(
            "codexmanager-lv-claude-compat",
            "gk_lv_cc",
            "sk-lv-cc-secret",
            crate::apikey_profile::PROTOCOL_OPENAI_COMPAT,
        );
        let template = storage
            .get_managed_model_v2("gpt-5.4-mini")
            .expect("read template")
            .expect("template exists");
        for slug in ["gpt-6-luna", "gpt-6-astra"] {
            if storage.get_managed_model_v2(slug).expect("check model").is_none() {
                let mut m = template.clone();
                m.id.clear();
                m.slug = slug.to_string();
                m.display_name = slug.to_string();
                m.origin = "custom".to_string();
                m.builtin_revision = None;
                m.user_edited = false;
                m.routes.clear();
                storage
                    .upsert_managed_model_v2(&codexmanager_core::storage::ManagedModelV2Upsert {
                        previous_slug: None,
                        model: m,
                    })
                    .expect("insert test model");
            }
        }
        drop(storage);

        let previous_rules = crate::gateway::current_model_forward_rules();
        crate::gateway::set_model_forward_rules("claude-haiku*=gpt-6-luna")
            .expect("set test model forward rules");

        let mut haiku_req: tiny_http::Request = tiny_http::TestRequest::new()
            .with_method(tiny_http::Method::Post)
            .with_path("/v1/messages?beta=true")
            .with_header(
                tiny_http::Header::from_bytes("x-api-key", "sk-lv-cc-secret")
                    .expect("x-api-key header"),
            )
            .with_body(r#"{"model":"claude-haiku-5-5","max_tokens":1024,"messages":[{"role":"user","content":"hi"}]}"#)
            .into();
        let haiku_res = prepare_local_request(&mut haiku_req, "tr-cc-haiku".to_string(), false);

        let mut dup_v1_suffix_req: tiny_http::Request = tiny_http::TestRequest::new()
            .with_method(tiny_http::Method::Post)
            .with_path("/v1/v1/messages?beta=true")
            .with_header(
                tiny_http::Header::from_bytes("Authorization", "Bearer sk-lv-cc-secret")
                    .expect("auth header"),
            )
            .with_body(r#"{"model":"gpt-6-astra[272k]","max_tokens":1024,"messages":[{"role":"user","content":"hi"}]}"#)
            .into();
        let dup_res = prepare_local_request(&mut dup_v1_suffix_req, "tr-cc-dupv1".to_string(), false);

        let mut dup_v1_usage_req: tiny_http::Request = tiny_http::TestRequest::new()
            .with_method(tiny_http::Method::Post)
            .with_path("/v1/v1/messages?beta=true")
            .with_header(
                tiny_http::Header::from_bytes("x-api-key", "sk-lv-cc-secret")
                    .expect("x-api-key header"),
            )
            .with_body(r##"{"model":"claude-opus-5-5","max_tokens":1024,"messages":[{"role":"user","content":[{"type":"text","text":"<system-reminder>git</system-reminder>"},{"type":"text","text":"?usage"}]},{"role":"system","content":[{"type":"text","text":"# Environment"}]}]}"##)
            .into();
        let usage_res = prepare_local_request(&mut dup_v1_usage_req, "tr-cc-usage".to_string(), false);

        let _ = crate::gateway::set_model_forward_rules(previous_rules.as_str());

        let validated_haiku = haiku_res
            .unwrap_or_else(|err| panic!("haiku forwarding failed: {} {}", err.status_code, err.message));
        assert_eq!(validated_haiku.model_for_log.as_deref(), Some("gpt-6-luna"));
        let adapted_haiku: serde_json::Value =
            serde_json::from_slice(&validated_haiku.body).expect("adapted haiku json");
        assert_eq!(adapted_haiku["model"], "gpt-6-luna");

        let validated_dup = dup_res
            .unwrap_or_else(|err| panic!("duplicate /v1 + [272k] failed: {} {}", err.status_code, err.message));
        assert_eq!(validated_dup.original_path, "/v1/messages?beta=true");
        assert_eq!(validated_dup.model_for_log.as_deref(), Some("gpt-6-astra"));
        let adapted_dup: serde_json::Value =
            serde_json::from_slice(&validated_dup.body).expect("adapted dup json");
        assert_eq!(adapted_dup["model"], "gpt-6-astra");

        let validated_usage = usage_res
            .unwrap_or_else(|err| panic!("claude trailing system ?usage failed: {} {}", err.status_code, err.message));
        assert_eq!(validated_usage.gateway_mode_for_log.as_deref(), Some("local_usage"));
    }
}
