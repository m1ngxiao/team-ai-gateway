use super::should_failover_transport_error;

#[test]
fn chatgpt_transport_error_fails_over_when_more_candidates_exist() {
    assert!(should_failover_transport_error(
        "https://chatgpt.com/backend-api/codex/responses",
        true
    ));
}

#[test]
fn chatgpt_transport_error_stops_without_more_candidates() {
    assert!(!should_failover_transport_error(
        "https://chatgpt.com/backend-api/codex/responses",
        false
    ));
}

#[test]
fn openai_api_transport_error_keeps_terminal_behavior() {
    assert!(!should_failover_transport_error(
        "https://api.openai.com/v1/responses",
        true
    ));
}

#[test]
fn custom_upstream_transport_error_keeps_existing_failover_behavior() {
    assert!(should_failover_transport_error(
        "https://example.test/v1/responses",
        true
    ));
}

#[test]
fn exhausted_budget_primary_attempt_does_not_cooldown_account() {
    use super::super::transport::UpstreamRequestContext;
    use super::{run_primary_upstream_attempt, PrimaryAttemptResult};
    use crate::gateway::upstream::generation_budget;
    use bytes::Bytes;
    use codexmanager_core::storage::Account;
    use std::time::Instant;

    let pool = "primary_attempt_budget_no_cooldown_pool";
    let account = Account {
        id: "acc_primary_budget_test".to_string(),
        label: "test".to_string(),
        issuer: String::new(),
        chatgpt_account_id: None,
        workspace_id: None,
        group_name: None,
        sort: 0,
        status: "active".to_string(),
        created_at: 0,
        updated_at: 0,
    };
    let client = reqwest::blocking::Client::new();
    let incoming_headers = crate::gateway::IncomingHeaderSnapshot::default();
    let body = Bytes::from_static(b"{}");
    let request_ctx = UpstreamRequestContext {
        request_path: "/v1/responses",
        protocol_type: crate::apikey_profile::PROTOCOL_OPENAI_COMPAT,
        is_fedramp: false,
    };

    // Expired deadline budget must return 504 Terminal without marking Network cooldown.
    let _deadline_scope = generation_budget::enter(
        true,
        pool,
        "gpt-6.1-sol",
        "trace-deadline",
        Some(Instant::now()),
    );
    let result = run_primary_upstream_attempt(
        &client,
        &reqwest::Method::POST,
        "https://chatgpt.com/backend-api/codex/responses",
        None,
        request_ctx,
        &incoming_headers,
        &body,
        true,
        "tok",
        &account,
        false,
        true,
        |_, _, _| {},
    );
    assert!(matches!(
        result,
        PrimaryAttemptResult::Terminal { status_code: 504, .. }
    ));
    assert!(!crate::gateway::is_account_in_cooldown(&account.id));
    drop(_deadline_scope);

    // Exhausted shared retry pool after send #1 must not mark Network cooldown on next candidate.
    for _ in 0..3 {
        crate::gateway::dynamic_pool::reserve_retry(pool, "gpt-6.1-sol")
            .expect("retry token")
            .commit();
    }
    let _shared_scope = generation_budget::enter(true, pool, "gpt-6.1-sol", "trace-shared", None);
    let budget = generation_budget::current().expect("budget");
    budget.before_send().expect("first send allowed");
    let shared_res = run_primary_upstream_attempt(
        &client,
        &reqwest::Method::POST,
        "https://chatgpt.com/backend-api/codex/responses",
        None,
        request_ctx,
        &incoming_headers,
        &body,
        true,
        "tok",
        &account,
        false,
        true,
        |_, _, _| {},
    );
    assert!(matches!(shared_res, PrimaryAttemptResult::Failover));
    assert!(!crate::gateway::is_account_in_cooldown(&account.id));
}
