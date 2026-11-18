use super::{
    account_model_override_for_request, execute_candidate_sequence, is_challenge_failover_error,
    prepare_initial_candidates, should_forward_thread_anchor_as_prompt_cache_key,
    CandidateExecutionResult, CandidateExecutorParams,
};
use crate::gateway::{IncomingHeaderSnapshot, ResponseAdapter, ToolNameRestoreMap};
use axum::http::{HeaderMap, HeaderValue};
use bytes::Bytes;
use codexmanager_core::storage::{now_ts, Account, ManagedModelV2Upsert, Storage, Token};
use serde_json::Value;
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Response, Server, StatusCode};

#[derive(Debug)]
struct CapturedCandidateRequest {
    path: String,
    session_headers: Vec<bool>,
    body: Value,
}

#[derive(Debug)]
struct CandidateSequenceOutcome {
    requests: Vec<CapturedCandidateRequest>,
    logged_model: Option<String>,
    logged_upstream_model: Option<String>,
    binding_last_model: Option<String>,
}

struct CooldownReset;

impl Drop for CooldownReset {
    fn drop(&mut self) {
        crate::gateway::gateway_clear_account_cooldowns_for_tests();
    }
}

fn build_account(id: &str, now: i64) -> Account {
    Account {
        id: id.to_string(),
        label: id.to_string(),
        issuer: "https://auth.openai.com".to_string(),
        chatgpt_account_id: Some("chatgpt-account".to_string()),
        workspace_id: Some("workspace-account".to_string()),
        group_name: None,
        sort: 0,
        status: "active".to_string(),
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn same_account_retry_is_limited_to_one_retry() {
    assert!(super::should_retry_same_account_after_failover(0));
    assert!(!super::should_retry_same_account_after_failover(1));
}

fn build_token(account_id: &str, now: i64) -> Token {
    Token {
        account_id: account_id.to_string(),
        id_token: "id-token".to_string(),
        access_token: "access-token".to_string(),
        refresh_token: String::new(),
        api_key_access_token: Some("api-key-token".to_string()),
        last_refresh: now,
    }
}

fn candidate_accounts(ids: &[&str]) -> Vec<(Account, Token)> {
    let now = now_ts();
    ids.iter()
        .map(|id| (build_account(id, now), build_token(id, now)))
        .collect()
}

#[test]
fn nonportable_new_session_skips_cooling_account_before_single_send() {
    let _guard = crate::test_env_guard();
    let _cooldown_reset = CooldownReset;
    crate::gateway::gateway_mark_account_cooldown_for_status("acc-a", 429);

    let (remaining, temporary) = prepare_initial_candidates(
        candidate_accounts(&["acc-a", "acc-b", "acc-c"]),
        true,
        None,
        None,
        0,
    );

    assert!(temporary.is_none());
    assert_eq!(
        remaining
            .iter()
            .map(|(account, _)| account.id.as_str())
            .collect::<Vec<_>>(),
        vec!["acc-b", "acc-c", "acc-a"]
    );
}

#[test]
fn nonportable_bound_session_keeps_binding_and_reports_cooldown() {
    let _guard = crate::test_env_guard();
    let _cooldown_reset = CooldownReset;
    crate::gateway::gateway_mark_account_cooldown_for_status("acc-a", 429);

    let (remaining, temporary) = prepare_initial_candidates(
        candidate_accounts(&["acc-b", "acc-a", "acc-c"]),
        true,
        Some("acc-a"),
        None,
        0,
    );

    assert!(remaining.is_empty());
    let temporary = temporary.expect("bound account cooldown");
    assert_eq!(temporary.account_id, "acc-a");
    assert_eq!(temporary.reason, "bound_account_cooldown");
    assert!(temporary.retry_after_secs > 0);
}

#[test]
fn bound_session_inflight_backoff_does_not_invite_tight_retry_loops() {
    let _held = crate::gateway::acquire_account_inflight("acc-bound-inflight");
    let (remaining, temporary) = prepare_initial_candidates(
        candidate_accounts(&["acc-bound-inflight"]),
        true,
        Some("acc-bound-inflight"),
        None,
        1,
    );

    assert!(remaining.is_empty());
    let temporary = temporary.expect("bound account inflight");
    assert_eq!(temporary.account_id, "acc-bound-inflight");
    assert_eq!(temporary.reason, "bound_account_inflight");
    assert!(temporary.retry_after_secs >= 5);
}

#[test]
fn nonportable_bound_session_is_rotated_to_binding_before_one_send() {
    let _guard = crate::test_env_guard();
    let _cooldown_reset = CooldownReset;

    let (remaining, temporary) = prepare_initial_candidates(
        candidate_accounts(&["acc-b", "acc-a", "acc-c"]),
        true,
        Some("acc-a"),
        None,
        0,
    );

    assert!(temporary.is_none());
    assert_eq!(
        remaining
            .iter()
            .map(|(account, _)| account.id.as_str())
            .collect::<Vec<_>>(),
        vec!["acc-a", "acc-c", "acc-b"]
    );
}

#[test]
fn portable_bound_session_migrates_when_bound_account_is_missing() {
    let (remaining, temporary) = prepare_initial_candidates(
        candidate_accounts(&["acc-b", "acc-c"]),
        false,
        Some("acc-a"),
        Some(super::BoundAccountQuotaUnavailable {
            reset_at: Some(now_ts() + 3600),
        }),
        0,
    );

    assert!(temporary.is_none());
    assert_eq!(
        remaining
            .iter()
            .map(|(account, _)| account.id.as_str())
            .collect::<Vec<_>>(),
        vec!["acc-b", "acc-c"]
    );
}

#[test]
fn nonportable_bound_session_reports_quota_guard_recovery() {
    let now = now_ts();
    let reset_at = now + 7200;
    let (remaining, temporary) = prepare_initial_candidates(
        candidate_accounts(&["acc-b"]),
        true,
        Some("acc-a"),
        Some(super::BoundAccountQuotaUnavailable {
            reset_at: Some(reset_at),
        }),
        0,
    );

    assert!(remaining.is_empty());
    let temporary = temporary.expect("bound account quota guard");
    assert_eq!(temporary.account_id, "acc-a");
    assert_eq!(temporary.reason, "bound_account_low_quota");
    assert_eq!(temporary.low_quota_reset_at, Some(reset_at));
    assert!((7200..=7201).contains(&temporary.retry_after_secs));
}

fn codex_session_headers() -> IncomingHeaderSnapshot {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("session-id", "session-current"),
        ("thread-id", "thread-current"),
        ("x-client-request-id", "request-current"),
        ("x-codex-window-id", "session-current:7"),
        ("x-codex-turn-state", "turn-state-current"),
    ] {
        headers.insert(
            name,
            HeaderValue::from_str(value).expect("valid session header"),
        );
    }
    IncomingHeaderSnapshot::from_http_headers(&headers)
}

fn captured_session_headers(request: &tiny_http::Request) -> Vec<bool> {
    [
        "session-id",
        "thread-id",
        "x-client-request-id",
        "x-codex-window-id",
        "x-codex-turn-state",
    ]
    .iter()
    .map(|name| {
        request
            .headers()
            .iter()
            .any(|header| header.field.to_string().eq_ignore_ascii_case(name))
    })
    .collect()
}

fn json_contains_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key(key) || map.values().any(|child| json_contains_key(child, key))
        }
        Value::Array(items) => items.iter().any(|child| json_contains_key(child, key)),
        _ => false,
    }
}

fn run_candidate_sequence_with_model_and_statuses(
    test_name: &str,
    request_model: &str,
    model_for_log: Option<&str>,
    track_conversation_binding: bool,
    statuses: Vec<u16>,
) -> CandidateSequenceOutcome {
    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");
    let now = now_ts();
    let account = build_account(test_name, now);
    let token = build_token(account.id.as_str(), now);
    storage.insert_account(&account).expect("insert account");
    storage.insert_token(&token).expect("insert token");

    let server = Server::http("127.0.0.1:0").expect("start server");
    let addr = format!("http://{}", server.server_addr());
    let canonical_url = format!("{addr}/backend-api/codex/responses");
    let join = thread::spawn(move || {
        let mut captured = Vec::new();
        for (index, status) in statuses.into_iter().enumerate() {
            let timeout = if index == 0 {
                Duration::from_secs(2)
            } else {
                Duration::from_millis(750)
            };
            let Some(mut request) = server
                .recv_timeout(timeout)
                .expect("receive candidate request")
            else {
                break;
            };
            let mut body = Vec::new();
            std::io::Read::read_to_end(request.as_reader(), &mut body)
                .expect("read candidate request body");
            captured.push(CapturedCandidateRequest {
                path: request.url().to_string(),
                session_headers: captured_session_headers(&request),
                body: serde_json::from_slice(&body).expect("parse candidate request body"),
            });
            let response_body = if status == 200 {
                r#"{"id":"resp_candidate","object":"response","status":"completed","output":[]}"#
            } else {
                r#"{"detail":"candidate bad request"}"#
            };
            request
                .respond(
                    Response::from_string(response_body)
                        .with_status_code(StatusCode(status))
                        .with_header(
                            tiny_http::Header::from_bytes("content-type", "application/json")
                                .expect("content type header"),
                        ),
                )
                .expect("respond candidate request");
        }
        captured
    });

    let incoming_headers = codex_session_headers();
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": request_model,
            "input": [{"type": "reasoning", "encrypted_content": "encrypted-current"}]
        }))
        .expect("serialize candidate request body"),
    );
    let mut candidates = vec![(account, token)];
    let platform_key_hash = format!("platform-key-{test_name}");
    let conversation_id = format!("conversation-{test_name}");
    let conversation_routing = track_conversation_binding
        .then(|| {
            crate::gateway::conversation_binding::prepare_conversation_routing(
                platform_key_hash.as_str(),
                Some(conversation_id.as_str()),
                None,
                &mut candidates,
            )
        })
        .flatten();
    let setup = super::super::request_setup::UpstreamRequestSetup {
        upstream_base: "https://chatgpt.com/backend-api/codex".to_string(),
        upstream_fallback_base: None,
        url: canonical_url,
        url_alt: None,
        candidate_count: 1,
        candidate_account_ids: Vec::new(),
        account_max_inflight: 1,
        anthropic_has_thread_anchor: false,
        has_sticky_fallback_session: false,
        has_sticky_fallback_conversation: false,
        has_body_encrypted_content: true,
        conversation_routing,
        route_strategy_for_log: "ordered",
        route_source_for_log: "test",
    };
    let context = super::super::execution_context::GatewayUpstreamExecutionContext::new(
        test_name,
        &storage,
        "candidate-test-key",
        "/v1/responses",
        "/v1/responses",
        "POST",
        ResponseAdapter::Passthrough,
        crate::apikey_profile::PROTOCOL_OPENAI_COMPAT,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("account_rotation"),
        Some(setup.route_strategy_for_log),
        Some(setup.route_source_for_log),
        1,
        setup.candidate_count,
        setup.account_max_inflight,
    );
    let tool_name_restore_map = ToolNameRestoreMap::new();
    let request: tiny_http::Request = tiny_http::TestRequest::new()
        .with_method(tiny_http::Method::Post)
        .with_path("/v1/responses")
        .into();

    let result = execute_candidate_sequence(
        request,
        candidates,
        CandidateExecutorParams {
            storage: &storage,
            method: &reqwest::Method::POST,
            incoming_headers: &incoming_headers,
            body: &body,
            original_body: &body,
            path: "/v1/responses",
            request_shape: Some("responses"),
            trace_id: test_name,
            model_for_log,
            response_adapter: ResponseAdapter::Passthrough,
            gemini_stream_output_mode: None,
            tool_name_restore_map: &tool_name_restore_map,
            context: &context,
            setup: &setup,
            request_deadline: None,
            started_at: Instant::now(),
            client_is_stream: false,
            upstream_is_stream: false,
            debug: false,
            allow_openai_fallback: false,
            disable_challenge_stateless_retry: false,
            has_anthropic_safeguards: false,
            context_enrichment: None,
        },
    )
    .expect("execute candidate sequence");

    assert!(matches!(result, CandidateExecutionResult::Handled));
    let requests = join.join().expect("join upstream server");
    let request_log = storage
        .list_request_logs(None, 10)
        .expect("read candidate request log")
        .into_iter()
        .find(|log| log.trace_id.as_deref() == Some(test_name));
    let binding_last_model = track_conversation_binding
        .then(|| {
            storage
                .get_conversation_binding(platform_key_hash.as_str(), conversation_id.as_str())
                .expect("read candidate conversation binding")
                .expect("candidate conversation binding")
                .last_model
        })
        .flatten();
    CandidateSequenceOutcome {
        requests,
        logged_model: request_log.as_ref().and_then(|log| log.model.clone()),
        logged_upstream_model: request_log.and_then(|log| log.upstream_model),
        binding_last_model,
    }
}

fn run_candidate_sequence_with_statuses(
    test_name: &str,
    statuses: Vec<u16>,
) -> Vec<CapturedCandidateRequest> {
    run_candidate_sequence_with_model_and_statuses(test_name, "gpt-5.5", None, false, statuses)
        .requests
}

fn assert_three_stage_candidate_shape(captured: &[CapturedCandidateRequest]) {
    assert_eq!(captured.len(), 2, "unexpected upstream request count");
    assert!(captured
        .iter()
        .all(|request| request.path == "/backend-api/codex/responses"));
    assert_eq!(
        captured[0].session_headers,
        vec![true, true, true, true, true]
    );
    assert_eq!(
        captured[1].session_headers,
        vec![false, false, false, false, false]
    );
    assert!(json_contains_key(&captured[0].body, "encrypted_content"));
    assert!(json_contains_key(&captured[1].body, "encrypted_content"));
}

#[test]
fn gemini_native_does_not_forward_thread_anchor_as_prompt_cache_key() {
    assert!(!should_forward_thread_anchor_as_prompt_cache_key(
        crate::apikey_profile::PROTOCOL_GEMINI_NATIVE
    ));
}

#[test]
fn non_gemini_protocols_keep_thread_anchor_forwarding() {
    assert!(should_forward_thread_anchor_as_prompt_cache_key(
        crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE
    ));
    assert!(should_forward_thread_anchor_as_prompt_cache_key(
        crate::apikey_profile::PROTOCOL_OPENAI_COMPAT
    ));
}

#[test]
fn challenge_failover_error_detection_matches_cloudflare_markers() {
    assert!(is_challenge_failover_error(Some(
        "upstream challenge blocked"
    )));
    assert!(is_challenge_failover_error(Some(
        "Cloudflare 安全验证页 [cf_ray=abc]"
    )));
    assert!(!is_challenge_failover_error(Some("upstream rate-limited")));
    assert!(!is_challenge_failover_error(None));
}

#[test]
fn candidate_sequence_continues_with_stripped_body_after_stateless_retry_fails() {
    let captured = run_candidate_sequence_with_statuses(
        "candidate-stateless-then-stripped-success",
        vec![400, 400, 200],
    );

    assert_three_stage_candidate_shape(&captured);
}

#[test]
fn candidate_sequence_does_not_add_fourth_stateless_retry() {
    let captured = run_candidate_sequence_with_statuses(
        "candidate-three-bad-requests-only",
        vec![400, 400, 400, 400],
    );

    assert_three_stage_candidate_shape(&captured);
}

#[test]
fn candidate_sequence_preserves_reserve_alias_in_final_upstream_body() {
    let captured = run_candidate_sequence_with_model_and_statuses(
        "candidate-reserve-alias",
        codexmanager_core::usage::LUNA_RESERVE_MODEL_SLUG,
        Some(codexmanager_core::usage::LUNA_RESERVE_MODEL_SLUG),
        true,
        vec![200],
    );

    assert_eq!(captured.requests.len(), 1);
    assert_eq!(
        captured.requests[0].body["model"],
        codexmanager_core::usage::LUNA_RESERVE_MODEL_SLUG
    );
    assert_eq!(
        captured.logged_model.as_deref(),
        Some(codexmanager_core::usage::LUNA_RESERVE_MODEL_SLUG)
    );
    assert_eq!(captured.logged_upstream_model, None);
    assert_eq!(
        captured.binding_last_model.as_deref(),
        Some(codexmanager_core::usage::LUNA_RESERVE_MODEL_SLUG)
    );
}

#[test]
fn reserve_alias_keeps_non_luna_account_route_override_authoritative() {
    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");
    let mut luna = storage
        .get_managed_model_v2(codexmanager_core::usage::LUNA_MODEL_SLUG)
        .expect("read Luna model")
        .expect("Luna model");
    luna.routes
        .iter_mut()
        .find(|route| route.source_kind == "account_pool" && route.source_id == "default")
        .expect("default Luna account route")
        .upstream_model = "gpt-5.4".to_string();
    storage
        .upsert_managed_model_v2(&ManagedModelV2Upsert {
            previous_slug: Some(codexmanager_core::usage::LUNA_MODEL_SLUG.to_string()),
            model: luna,
        })
        .expect("update Luna account route");

    assert_eq!(
        account_model_override_for_request(
            &storage,
            Some(codexmanager_core::usage::LUNA_RESERVE_MODEL_SLUG),
        )
        .as_deref(),
        Some("gpt-5.4")
    );
}

struct CandidateEnvGuard {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl CandidateEnvGuard {
    fn set(name: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(name);
        std::env::set_var(name, value);
        Self { name, previous }
    }
}

impl Drop for CandidateEnvGuard {
    fn drop(&mut self) {
        match self.previous.as_ref() {
            Some(value) => std::env::set_var(self.name, value),
            None => std::env::remove_var(self.name),
        }
    }
}

fn run_multi_candidate_sequence(
    test_name: &str,
    account_ids: &[&str],
    cooldown_ids: &[&str],
    statuses: Vec<u16>,
) -> (
    Vec<CapturedCandidateRequest>,
    Option<codexmanager_core::storage::RequestLog>,
) {
    let _env_guard = crate::test_env_guard();
    let _cooldown_reset = CooldownReset;
    let _aggressive_guard =
        CandidateEnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_AGGRESSIVE", "false");

    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");
    let candidates = candidate_accounts(account_ids);
    for (account, token) in &candidates {
        storage.insert_account(account).expect("insert account");
        storage.insert_token(token).expect("insert token");
    }
    for id in cooldown_ids {
        crate::gateway::gateway_mark_account_cooldown_for_status(id, 429);
    }

    let server = Server::http("127.0.0.1:0").expect("start server");
    let addr = format!("http://{}", server.server_addr());
    let canonical_url = format!("{addr}/backend-api/codex/responses");
    let join = thread::spawn(move || {
        let mut captured = Vec::new();
        for (index, status) in statuses.into_iter().enumerate() {
            let timeout = if index == 0 {
                Duration::from_secs(2)
            } else {
                Duration::from_millis(500)
            };
            let Some(mut request) = server
                .recv_timeout(timeout)
                .expect("receive candidate request")
            else {
                break;
            };
            let mut body = Vec::new();
            std::io::Read::read_to_end(request.as_reader(), &mut body)
                .expect("read candidate request body");
            captured.push(CapturedCandidateRequest {
                path: request.url().to_string(),
                session_headers: captured_session_headers(&request),
                body: serde_json::from_slice(&body).expect("parse candidate request body"),
            });
            let response_body = if status == 200 {
                r#"{"id":"resp_ok","object":"response","status":"completed","output":[]}"#
            } else {
                r#"{"error":{"message":"upstream error","type":"upstream_error"}}"#
            };
            request
                .respond(
                    Response::from_string(response_body)
                        .with_status_code(StatusCode(status))
                        .with_header(
                            tiny_http::Header::from_bytes("content-type", "application/json")
                                .expect("content type header"),
                        ),
                )
                .expect("respond candidate request");
        }
        captured
    });

    let incoming_headers = IncomingHeaderSnapshot::from_http_headers(&HeaderMap::new());
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "gpt-6-astra",
            "input": "hello"
        }))
        .expect("serialize request body"),
    );
    let setup = super::super::request_setup::UpstreamRequestSetup {
        upstream_base: addr,
        upstream_fallback_base: None,
        url: canonical_url,
        url_alt: None,
        candidate_count: candidates.len(),
        candidate_account_ids: Vec::new(),
        account_max_inflight: 1,
        anthropic_has_thread_anchor: false,
        has_sticky_fallback_session: false,
        has_sticky_fallback_conversation: false,
        has_body_encrypted_content: false,
        conversation_routing: None,
        route_strategy_for_log: "ordered",
        route_source_for_log: "test",
    };
    let context = super::super::execution_context::GatewayUpstreamExecutionContext::new(
        test_name,
        &storage,
        "candidate-multi-test-key",
        "/v1/responses",
        "/v1/responses",
        "POST",
        ResponseAdapter::Passthrough,
        crate::apikey_profile::PROTOCOL_OPENAI_COMPAT,
        None,
        Some("gpt-6-astra"),
        Some("gpt-6-astra"),
        None,
        None,
        None,
        None,
        None,
        None,
        Some("account_rotation"),
        Some(setup.route_strategy_for_log),
        Some(setup.route_source_for_log),
        1,
        setup.candidate_count,
        setup.account_max_inflight,
    );
    let tool_name_restore_map = ToolNameRestoreMap::new();
    let request: tiny_http::Request = tiny_http::TestRequest::new()
        .with_method(tiny_http::Method::Post)
        .with_path("/v1/responses")
        .into();

    let result = execute_candidate_sequence(
        request,
        candidates,
        CandidateExecutorParams {
            storage: &storage,
            method: &reqwest::Method::POST,
            incoming_headers: &incoming_headers,
            body: &body,
            original_body: &body,
            path: "/v1/responses",
            request_shape: Some("responses"),
            trace_id: test_name,
            model_for_log: Some("gpt-6-astra"),
            response_adapter: ResponseAdapter::Passthrough,
            gemini_stream_output_mode: None,
            tool_name_restore_map: &tool_name_restore_map,
            context: &context,
            setup: &setup,
            request_deadline: None,
            started_at: Instant::now(),
            client_is_stream: false,
            upstream_is_stream: false,
            debug: false,
            allow_openai_fallback: false,
            disable_challenge_stateless_retry: false,
            has_anthropic_safeguards: false,
            context_enrichment: None,
        },
    )
    .expect("execute multi candidate sequence");

    assert!(matches!(result, CandidateExecutionResult::Handled));
    let requests = join.join().expect("join mock upstream server");
    let request_log = storage
        .list_request_logs(None, 10)
        .expect("read request logs")
        .into_iter()
        .find(|log| log.trace_id.as_deref() == Some(test_name));
    (requests, request_log)
}

#[test]
fn candidate_sequence_delivers_upstream_error_when_other_candidates_in_cooldown() {
    let (requests, request_log) = run_multi_candidate_sequence(
        "candidate-cooldown-delivers-upstream",
        &["acc-cd-a", "acc-cd-b", "acc-cd-c"],
        &["acc-cd-b", "acc-cd-c"],
        vec![404],
    );
    assert_eq!(requests.len(), 1);
    assert_eq!(
        request_log.and_then(|log| log.status_code),
        Some(404),
        "single available candidate error should be delivered instead of falling through to Exhausted"
    );
}

#[test]
fn candidate_sequence_delivers_backup_upstream_error_when_max_attempts_reached() {
    let (requests, request_log) = run_multi_candidate_sequence(
        "candidate-max-attempts-delivers-backup-error",
        &["acc-cap-a", "acc-cap-b", "acc-cap-c"],
        &[],
        vec![502, 502, 502, 502],
    );
    assert_eq!(
        requests.len(),
        3,
        "expected first candidate (1 attempt + 1 same-account retry) plus terminal backup attempt before delivery"
    );
    assert_eq!(
        request_log.and_then(|log| log.status_code),
        Some(502),
        "backup candidate error at max_attempts should be delivered instead of Exhausted 503"
    );
}
