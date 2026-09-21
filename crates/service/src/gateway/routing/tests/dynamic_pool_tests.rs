use super::*;
use super::super::metrics::{account_inflight_count, acquire_account_inflight};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Barrier;

#[derive(Default)]
struct FakeClock(AtomicU64);
impl Clock for FakeClock {
    fn millis(&self) -> u64 { self.0.load(Ordering::SeqCst) }
}
impl FakeClock {
    fn set(&self, value: u64) { self.0.store(value, Ordering::SeqCst); }
}
fn fixture() -> (Arc<Runtime>, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::default());
    (Arc::new(Runtime::new(clock.clone())), clock)
}
fn key(account: &str, model: &str) -> CircuitKey {
    CircuitKey { account: account.to_owned(), model: model.to_owned() }
}
fn healthy(runtime: &Runtime, accounts: &[&str], model: &str) {
    let mut state = runtime.state.lock().unwrap();
    for account in accounts {
        state.circuits.insert(key(account, model), Circuit {
            phase: Phase::Healthy { since: 0, successes: 0 },
            generation: 0, backoff_level: 0, inflight: 0, last_used: runtime.clock.millis(),
        });
    }
}
fn reserve(runtime: &Arc<Runtime>, account: &str, model: &str) -> Option<DynamicAccountPermit> {
    runtime.select("shared-pool", model, &[account.to_owned()], &[], 0, true)
}
fn open_until(runtime: &Runtime, account: &str, model: &str) -> u64 {
    match runtime.state.lock().unwrap().circuits[&key(account, model)].phase {
        Phase::Open { until } => until,
        phase => panic!("expected open, found {phase:?}"),
    }
}

#[test]
fn startup_probe_is_single_and_recovery_requires_time_and_complete_successes() {
    let (runtime, clock) = fixture();
    let account = "dynamic-startup";
    let mut probe = reserve(&runtime, account, "m").unwrap();
    assert_eq!(probe.phase(), "half_open");
    assert!(reserve(&runtime, account, "m").is_none());
    probe.record_success();
    drop(probe);
    let mut first = reserve(&runtime, account, "m").unwrap();
    let mut second = reserve(&runtime, account, "m").unwrap();
    assert!(reserve(&runtime, account, "m").is_none());
    first.record_success(); second.record_success();
    drop((first, second));
    let mut third = reserve(&runtime, account, "m").unwrap();
    third.record_success(); drop(third);
    assert!(matches!(runtime.state.lock().unwrap().circuits[&key(account, "m")].phase, Phase::Recovering { .. }));
    clock.set(30_000);
    let permits = (0..4).map(|_| reserve(&runtime, account, "m").unwrap()).collect::<Vec<_>>();
    assert!(permits.iter().all(|permit| permit.phase() == "healthy"));
    drop(permits);
    assert_eq!(account_inflight_count(account), 0);
}

#[test]
fn dynamic_choice_reads_global_load_again_and_all_keys_share_reservations() {
    let (runtime, _) = fixture();
    let accounts = ["dynamic-live-a", "dynamic-live-b"];
    healthy(&runtime, &accounts, "m");
    let mut held = (0..5).map(|_| acquire_account_inflight(accounts[0])).collect::<Vec<_>>();
    let original_b = acquire_account_inflight(accounts[1]);
    let ids = accounts.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    let first = runtime.select("shared", "m", &ids, &[], 0, true).unwrap();
    assert_eq!(first.account_id(), accounts[1]);
    assert_eq!(first.inflight_before(), 1);
    held.clear();
    let second = runtime.select("shared", "m", &ids, &[], 0, true).unwrap();
    assert_eq!(second.account_id(), accounts[0]);
    assert_eq!(second.inflight_before(), 0);
    drop((first, second, original_b));
    assert!(accounts.iter().all(|id| account_inflight_count(id) == 0));
}

#[test]
fn concurrent_selection_and_reservation_spreads_load_without_exceeding_caps() {
    let (runtime, _) = fixture();
    let accounts = ["dynamic-concurrent-a", "dynamic-concurrent-b", "dynamic-concurrent-c"];
    healthy(&runtime, &accounts, "m");
    let ids = accounts.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    let start = Arc::new(Barrier::new(17));
    let acquired = Arc::new(Barrier::new(17));
    let release = Arc::new(Barrier::new(17));
    let threads = (0..16).map(|_| {
        let (runtime, ids, start, acquired, release) = (runtime.clone(), ids.clone(), start.clone(), acquired.clone(), release.clone());
        std::thread::spawn(move || {
            start.wait();
            let permit = runtime.select("one-authorized-pool", "m", &ids, &[], 3, true);
            acquired.wait(); release.wait();
            permit.is_some()
        })
    }).collect::<Vec<_>>();
    start.wait(); acquired.wait();
    let counts = accounts.map(account_inflight_count);
    release.wait();
    let successes = threads.into_iter().map(|thread| thread.join().unwrap()).filter(|success| *success).count();
    assert_eq!(counts, [3, 3, 3]);
    assert_eq!(successes, 9);
    assert!(accounts.iter().all(|id| account_inflight_count(id) == 0));
}

#[test]
fn fair_ties_rotate_across_pool_requests_independently_of_input_order() {
    let (runtime, _) = fixture();
    let accounts = ["dynamic-fair-a", "dynamic-fair-b", "dynamic-fair-c"];
    healthy(&runtime, &accounts, "m");
    let mut ids = accounts.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    let mut choices = Vec::new();
    for _ in 0..6 {
        let permit = runtime.select("shared", "m", &ids, &[], 0, true).unwrap();
        choices.push(permit.account_id().to_owned());
        drop(permit); ids.rotate_left(1);
    }
    assert_eq!(choices, accounts.repeat(2));
}

#[test]
fn exclusions_and_configured_cap_are_never_bypassed_for_last_candidate() {
    let (runtime, _) = fixture();
    let account = "dynamic-excluded";
    healthy(&runtime, &[account], "m");
    let ids = [account.to_owned()];
    assert!(runtime.select("p", "m", &ids, &ids, 0, true).is_none());
    assert!(runtime.select("p", "m", &[], &[], 0, true).is_none());
    let permit = runtime.select("p", "m", &ids, &[], 1, true).unwrap();
    assert!(runtime.select("p", "m", &ids, &[], 1, true).is_none());
    drop(permit);
    assert_eq!(account_inflight_count(account), 0);
}

#[test]
fn stale_generation_success_and_failure_cannot_change_new_cooldown() {
    let (runtime, clock) = fixture();
    let account = "dynamic-generation";
    healthy(&runtime, &[account], "m");
    let mut first = reserve(&runtime, account, "m").unwrap();
    let mut old_success = reserve(&runtime, account, "m").unwrap();
    let mut old_failure = reserve(&runtime, account, "m").unwrap();
    first.record_overload(None);
    let until = open_until(&runtime, account, "m");
    assert!((24_000..=36_000).contains(&until));
    clock.set(10_000);
    old_success.record_success(); old_failure.record_overload(Some(Duration::from_secs(300)));
    assert_eq!(open_until(&runtime, account, "m"), until);
    let state = runtime.state.lock().unwrap();
    assert_eq!(state.circuits[&key(account, "m")].generation, 1);
    assert_eq!(state.circuits[&key(account, "m")].backoff_level, 1);
}

#[test]
fn expired_cooldown_has_one_probe_and_cancel_never_marks_healthy() {
    let (runtime, clock) = fixture();
    let account = "dynamic-probe-cancel";
    let mut first = reserve(&runtime, account, "m").unwrap();
    first.record_overload(None); drop(first);
    let until = open_until(&runtime, account, "m");
    clock.set(until - 1);
    assert!(reserve(&runtime, account, "m").is_none());
    clock.set(until);
    let probe = reserve(&runtime, account, "m").unwrap();
    assert_eq!(probe.generation(), 1);
    assert!(reserve(&runtime, account, "m").is_none());
    drop(probe);
    assert_eq!(open_until(&runtime, account, "m"), until);
    let probe_again = reserve(&runtime, account, "m").unwrap();
    assert_eq!(probe_again.phase(), "half_open");
    drop(probe_again);
    assert_eq!(account_inflight_count(account), 0);
}

#[test]
fn expired_probe_gets_a_lane_even_when_healthy_accounts_remain_idle() {
    let (runtime, clock) = fixture();
    let healthy_id = "dynamic-probe-healthy";
    let probe_id = "dynamic-probe-expired";
    healthy(&runtime, &[healthy_id], "m");
    let mut failure = reserve(&runtime, probe_id, "m").unwrap();
    failure.record_overload(None); drop(failure);
    let until = open_until(&runtime, probe_id, "m");
    clock.set(until);
    let ids = [healthy_id.to_string(), probe_id.to_string()];
    let first = runtime.select("p", "m", &ids, &[], 0, true).unwrap();
    assert_eq!(first.account_id(), probe_id);
    let second = runtime.select("p", "m", &ids, &[], 0, true).unwrap();
    assert_eq!(second.account_id(), healthy_id);
}

#[test]
fn repeated_probe_failures_back_off_with_jitter_and_respect_retry_after() {
    let (runtime, clock) = fixture();
    let account = "dynamic-backoff";
    for base in [30_000u64, 60_000, 120_000, 120_000] {
        let now = clock.millis();
        let mut probe = reserve(&runtime, account, "m").unwrap();
        probe.record_overload(None); drop(probe);
        let until = open_until(&runtime, account, "m");
        assert!((base * 8 / 10..=base * 12 / 10).contains(&(until - now)));
        clock.set(until);
    }
    let mut probe = reserve(&runtime, account, "m").unwrap();
    probe.record_overload(Some(Duration::from_secs(200))); drop(probe);
    assert_eq!(open_until(&runtime, account, "m") - clock.millis(), 200_000);
}

#[test]
fn circuits_are_model_specific_but_load_is_account_global() {
    let (runtime, _) = fixture();
    let account = "dynamic-model-isolation";
    let mut a = reserve(&runtime, account, "model-a").unwrap();
    a.record_overload(None);
    let b = reserve(&runtime, account, "model-b").unwrap();
    assert_eq!(b.inflight_before(), 1);
    assert!(reserve(&runtime, account, "model-a").is_none());
    assert_eq!(account_inflight_count(account), 2);
    drop((a, b));
    assert_eq!(account_inflight_count(account), 0);
}

#[test]
fn recovery_failure_reopens_and_backoff_resets_only_after_stability() {
    let (runtime, clock) = fixture();
    let account = "dynamic-stable-reset";
    let mut failed = reserve(&runtime, account, "m").unwrap();
    failed.record_overload(None); drop(failed);
    clock.set(open_until(&runtime, account, "m"));
    let mut probe = reserve(&runtime, account, "m").unwrap();
    probe.record_success(); drop(probe);
    let recovery_since = clock.millis();
    for _ in 0..3 {
        let mut success = reserve(&runtime, account, "m").unwrap();
        success.record_success();
    }
    clock.set(recovery_since + 30_000);
    let permit = reserve(&runtime, account, "m").unwrap(); drop(permit);
    assert_eq!(runtime.state.lock().unwrap().circuits[&key(account, "m")].backoff_level, 1);
    clock.set(recovery_since + 300_000);
    let permit = reserve(&runtime, account, "m").unwrap(); drop(permit);
    assert_eq!(runtime.state.lock().unwrap().circuits[&key(account, "m")].backoff_level, 0);
}

#[test]
fn retry_budget_has_three_initial_tokens_refunds_unsent_and_refills_by_first_posts() {
    let (runtime, _) = fixture();
    let unsent = runtime.reserve_retry("p", "m").unwrap();
    drop(unsent);
    for _ in 0..3 { runtime.reserve_retry("p", "m").unwrap().commit(); }
    assert!(runtime.reserve_retry("p", "m").is_none());
    for _ in 0..4 { runtime.credit_first("p", "m"); }
    assert!(runtime.reserve_retry("p", "m").is_none());
    runtime.credit_first("p", "m");
    runtime.reserve_retry("p", "m").unwrap().commit();
    assert!(runtime.reserve_retry("p", "m").is_none());
    assert!(runtime.reserve_retry("other-pool", "m").is_some());
    assert!(runtime.reserve_retry("p", "other-model").is_some());
    for _ in 0..100 { runtime.credit_first("p", "m"); }
    for _ in 0..3 { runtime.reserve_retry("p", "m").unwrap().commit(); }
    assert!(runtime.reserve_retry("p", "m").is_none());
}

#[test]
fn retry_budget_is_atomic_under_concurrent_keys() {
    let (runtime, _) = fixture();
    let start = Arc::new(Barrier::new(17));
    let threads = (0..16).map(|_| {
        let (runtime, start) = (runtime.clone(), start.clone());
        std::thread::spawn(move || {
            start.wait();
            let Some(mut token) = runtime.reserve_retry("shared", "m") else { return false; };
            token.commit(); true
        })
    }).collect::<Vec<_>>();
    start.wait();
    assert_eq!(threads.into_iter().map(|thread| thread.join().unwrap()).filter(|success| *success).count(), 3);
}

#[test]
fn bridge_guard_transfer_preserves_exactly_one_global_slot_and_probe_ownership() {
    let (runtime, _) = fixture();
    let account = "dynamic-bridge";
    let mut permit = reserve(&runtime, account, "m").unwrap();
    let bridge_guard = permit.take_inflight_guard();
    assert_eq!(account_inflight_count(account), 1);
    drop(bridge_guard);
    assert_eq!(account_inflight_count(account), 0);
    // A released socket slot alone does not resolve the semantic probe.
    assert!(reserve(&runtime, account, "m").is_none());
    permit.record_success(); drop(permit);
    let next = reserve(&runtime, account, "m").unwrap();
    assert_eq!(next.phase(), "recovering");
    drop(next);
    assert_eq!(account_inflight_count(account), 0);
}

#[test]
fn idle_state_is_reclaimed_without_forgetting_live_requests_or_pending_cooldowns() {
    let (runtime, clock) = fixture();
    let active = reserve(&runtime, "dynamic-ttl-active", "m").unwrap();
    let retry = runtime.reserve_retry("pending-retry", "m").unwrap();
    healthy(&runtime, &["dynamic-ttl-idle"], "m");
    runtime.credit_first("idle-pool", "m");
    {
        let mut state = runtime.state.lock().unwrap();
        let mut waiting = Circuit::new(0);
        waiting.phase = Phase::Open { until: IDLE_TTL_MS * 2 };
        state.circuits.insert(key("dynamic-ttl-waiting", "m"), waiting);
    }
    clock.set(IDLE_TTL_MS + 1);
    runtime.credit_first("trigger-cleanup", "m");
    {
        let state = runtime.state.lock().unwrap();
        assert!(!state.circuits.contains_key(&key("dynamic-ttl-idle", "m")));
        assert!(state.circuits.contains_key(&key("dynamic-ttl-active", "m")));
        assert!(state.circuits.contains_key(&key("dynamic-ttl-waiting", "m")));
        assert!(state.pools.contains_key(&PoolKey::new("shared-pool", "m")));
        assert!(state.pools.contains_key(&PoolKey::new("pending-retry", "m")));
        assert!(!state.pools.contains_key(&PoolKey::new("idle-pool", "m")));
    }
    drop((active, retry));
    clock.set(IDLE_TTL_MS * 3);
    runtime.credit_first("trigger-cleanup", "m");
    let state = runtime.state.lock().unwrap();
    assert!(!state.circuits.contains_key(&key("dynamic-ttl-active", "m")));
    assert!(!state.circuits.contains_key(&key("dynamic-ttl-waiting", "m")));
    assert!(!state.pools.contains_key(&PoolKey::new("pending-retry", "m")));
}

#[test]
fn state_limits_reject_new_keys_without_evicting_active_circuits_or_budgets() {
    let (runtime, clock) = fixture();
    {
        let mut state = runtime.state.lock().unwrap();
        for index in 0..MAX_CIRCUITS {
            state.circuits.insert(key(&format!("capacity-circuit-{index}"), "m"), Circuit::new(0));
        }
        for index in 0..MAX_POOLS {
            state.pools.insert(PoolKey::new(&format!("capacity-pool-{index}"), "m"), PoolState::new(0));
        }
    }
    assert!(runtime.select("capacity-pool-0", "m", &["new-account-at-limit".into()], &[], 0, true).is_none());
    assert!(runtime.reserve_retry("new-pool-at-limit", "m").is_none());
    runtime.credit_first("new-pool-at-limit", "m");
    assert_eq!(runtime.state.lock().unwrap().pools.len(), MAX_POOLS);
    let existing = runtime.select("capacity-pool-0", "m", &["capacity-circuit-0".into()], &[], 0, true).unwrap();
    assert!(runtime.reserve_retry("capacity-pool-0", "m").is_some());
    drop(existing);
    clock.set(IDLE_TTL_MS + 1);
    let fresh = reserve(&runtime, "new-account-after-ttl", "m").unwrap();
    assert_eq!(fresh.phase(), "half_open");
    assert!(runtime.state.lock().unwrap().circuits.len() < MAX_CIRCUITS);
}
