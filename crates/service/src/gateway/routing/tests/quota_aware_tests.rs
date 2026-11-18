use super::{order_candidates, rebalance_target, QuotaAwareAccountMetrics, QuotaAwareSnapshot};
use codexmanager_core::storage::{
    now_ts, Account, ConversationBinding, Storage, Token, UsageSnapshotRecord,
};

fn sample_account(id: &str, sort: i64) -> Account {
    Account {
        id: id.to_string(),
        label: id.to_string(),
        issuer: "https://auth.openai.com".to_string(),
        chatgpt_account_id: None,
        workspace_id: None,
        group_name: None,
        sort,
        status: "active".to_string(),
        created_at: 1,
        updated_at: 1,
    }
}

fn sample_token(id: &str) -> Token {
    Token {
        account_id: id.to_string(),
        id_token: String::new(),
        access_token: "access".to_string(),
        refresh_token: "refresh".to_string(),
        api_key_access_token: None,
        last_refresh: 1,
    }
}

fn sample_candidates(ids: &[&str]) -> Vec<(Account, Token)> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| (sample_account(id, index as i64), sample_token(id)))
        .collect()
}

fn metrics(remaining_percent: f64, recent_tokens: i64) -> QuotaAwareAccountMetrics {
    QuotaAwareAccountMetrics {
        remaining_percent: Some(remaining_percent),
        recent_tokens,
        active_bindings: 1,
    }
}

#[test]
fn quota_aware_order_prefers_remaining_quota_then_lower_recent_tokens() {
    let _guard = crate::test_env_guard();
    let mut candidates = sample_candidates(&["quota-low", "quota-busy", "quota-idle"]);
    let snapshot = QuotaAwareSnapshot::new([
        ("quota-low".to_string(), metrics(60.0, 0)),
        ("quota-busy".to_string(), metrics(90.0, 1_000_000)),
        ("quota-idle".to_string(), metrics(90.0, 10_000)),
    ]);

    assert!(order_candidates(candidates.as_mut_slice(), &snapshot));
    assert_eq!(candidates[0].0.id, "quota-idle");
    assert_eq!(candidates[1].0.id, "quota-busy");
    assert_eq!(candidates[2].0.id, "quota-low");
}

#[test]
fn quota_aware_rebalance_keeps_active_conversations_sticky() {
    let _guard = crate::test_env_guard();
    let now = now_ts();
    let candidates = sample_candidates(&["quota-target", "quota-bound"]);
    let snapshot = QuotaAwareSnapshot::new([
        ("quota-target".to_string(), metrics(99.0, 0)),
        ("quota-bound".to_string(), metrics(55.0, 50_000_000)),
    ]);
    let binding = ConversationBinding {
        platform_key_hash: "quota-key".to_string(),
        conversation_id: "quota-conversation".to_string(),
        account_id: "quota-bound".to_string(),
        thread_epoch: 1,
        thread_anchor: "quota-thread".to_string(),
        status: "active".to_string(),
        last_model: None,
        last_switch_reason: None,
        created_at: now.saturating_sub(60),
        updated_at: now.saturating_sub(60),
        last_used_at: now.saturating_sub(60),
    };

    assert_eq!(
        rebalance_target(candidates.as_slice(), &binding, &snapshot, now),
        None
    );
}

#[test]
fn quota_aware_rebalance_moves_idle_conversation_to_materially_better_account() {
    let _guard = crate::test_env_guard();
    let now = now_ts();
    let candidates = sample_candidates(&["quota-target", "quota-bound"]);
    let snapshot = QuotaAwareSnapshot::new([
        ("quota-target".to_string(), metrics(99.0, 0)),
        ("quota-bound".to_string(), metrics(55.0, 50_000_000)),
    ]);
    let binding = ConversationBinding {
        platform_key_hash: "quota-key".to_string(),
        conversation_id: "quota-conversation".to_string(),
        account_id: "quota-bound".to_string(),
        thread_epoch: 1,
        thread_anchor: "quota-thread".to_string(),
        status: "active".to_string(),
        last_model: None,
        last_switch_reason: None,
        created_at: now.saturating_sub(7_200),
        updated_at: now.saturating_sub(7_200),
        last_used_at: now.saturating_sub(7_200),
    };

    assert_eq!(
        rebalance_target(candidates.as_slice(), &binding, &snapshot, now),
        Some(0)
    );
}

#[test]
fn quota_aware_rebalance_never_moves_to_lower_remaining_quota() {
    let _guard = crate::test_env_guard();
    let now = now_ts();
    let candidates = sample_candidates(&["quota-low-target", "quota-high-bound"]);
    let snapshot = QuotaAwareSnapshot::new([
        ("quota-low-target".to_string(), metrics(45.0, 0)),
        ("quota-high-bound".to_string(), metrics(80.0, 100_000_000)),
    ]);
    let binding = ConversationBinding {
        platform_key_hash: "quota-key".to_string(),
        conversation_id: "quota-conversation".to_string(),
        account_id: "quota-high-bound".to_string(),
        thread_epoch: 1,
        thread_anchor: "quota-thread".to_string(),
        status: "active".to_string(),
        last_model: None,
        last_switch_reason: None,
        created_at: now.saturating_sub(7_200),
        updated_at: now.saturating_sub(7_200),
        last_used_at: now.saturating_sub(7_200),
    };

    assert_eq!(
        rebalance_target(candidates.as_slice(), &binding, &snapshot, now),
        None
    );
}

#[test]
fn quota_aware_snapshot_loads_remaining_quota_and_active_bindings() {
    let _guard = crate::test_env_guard();
    super::clear_snapshot_cache_for_tests();
    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");
    let now = now_ts();
    for (account_id, used_percent) in [("quota-load-a", 1.0), ("quota-load-b", 34.0)] {
        storage
            .insert_usage_snapshot(&UsageSnapshotRecord {
                account_id: account_id.to_string(),
                used_percent: Some(used_percent),
                window_minutes: Some(300),
                resets_at: None,
                secondary_used_percent: Some(used_percent + 20.0),
                secondary_window_minutes: Some(10_080),
                secondary_resets_at: None,
                credits_json: None,
                captured_at: now,
            })
            .expect("insert usage snapshot");
    }
    for conversation_id in ["quota-load-conv-1", "quota-load-conv-2"] {
        storage
            .upsert_conversation_binding(&ConversationBinding {
                platform_key_hash: "quota-load-key".to_string(),
                conversation_id: conversation_id.to_string(),
                account_id: "quota-load-b".to_string(),
                thread_epoch: 1,
                thread_anchor: conversation_id.to_string(),
                status: "active".to_string(),
                last_model: None,
                last_switch_reason: None,
                created_at: now,
                updated_at: now,
                last_used_at: now,
            })
            .expect("insert conversation binding");
    }

    let snapshot = QuotaAwareSnapshot::load(
        &storage,
        "quota-load-key",
        &["quota-load-a".to_string(), "quota-load-b".to_string()],
        now,
    )
    .expect("load snapshot");

    let account_a = snapshot.metrics("quota-load-a").expect("account a");
    let account_b = snapshot.metrics("quota-load-b").expect("account b");
    assert_eq!(account_a.remaining_percent, Some(79.0));
    assert_eq!(account_b.remaining_percent, Some(46.0));
    assert_eq!(account_a.active_bindings, 0);
    assert_eq!(account_b.active_bindings, 2);
}

#[test]
fn quota_aware_snapshot_treats_elapsed_usage_windows_as_reset() {
    let _guard = crate::test_env_guard();
    super::clear_snapshot_cache_for_tests();
    let storage = Storage::open_in_memory().expect("open storage");
    storage.init().expect("init storage");
    let now = now_ts();
    storage
        .insert_usage_snapshot(&UsageSnapshotRecord {
            account_id: "quota-reset-window".to_string(),
            used_percent: Some(92.0),
            window_minutes: Some(300),
            resets_at: Some(now.saturating_sub(1)),
            secondary_used_percent: Some(80.0),
            secondary_window_minutes: Some(10_080),
            secondary_resets_at: Some(now + 3_600),
            credits_json: None,
            captured_at: now,
        })
        .expect("insert usage snapshot");

    let snapshot = QuotaAwareSnapshot::load(
        &storage,
        "quota-reset-window-key",
        &["quota-reset-window".to_string()],
        now,
    )
    .expect("load snapshot");

    assert_eq!(
        snapshot
            .metrics("quota-reset-window")
            .expect("account metrics")
            .remaining_percent,
        Some(20.0)
    );
}
