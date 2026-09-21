//! Short-lived, process-local capacity circuits and atomic fallback selection.
//!
//! Lock ordering is pool state -> global account inflight. No I/O or sleeping
//! occurs under either lock. Every reservation owns the existing metrics guard,
//! so old and new routing paths (and every API key) observe the same load.
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::metrics::{select_and_acquire_account_inflight, AccountInFlightGuard};

const ENV_ENABLED: &str = "CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED";
const RECOVERY_LIMIT: usize = 2;
const RECOVERY_MS: u64 = 30_000;
const RECOVERY_SUCCESSES: u32 = 3;
const RESET_BACKOFF_MS: u64 = 300_000;
const BUCKET_CAPACITY: u8 = 15; // Fifths of a generation POST; capacity = 3.
const RETRY_COST: u8 = 5;
const MAX_CIRCUITS: usize = 10_000;
const MAX_POOLS: usize = 10_000;
const IDLE_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
const MAINTENANCE_INTERVAL_MS: u64 = 60_000;

static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();
static ENABLED: OnceLock<bool> = OnceLock::new();

/// A startup-only switch. It intentionally cannot change during a request.
pub(crate) fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var(ENV_ENABLED)
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false)
    })
}

trait Clock: Send + Sync {
    fn millis(&self) -> u64;
}
struct MonotonicClock(Instant);
impl Clock for MonotonicClock {
    fn millis(&self) -> u64 {
        self.0.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
}
fn runtime() -> Arc<Runtime> {
    RUNTIME
        .get_or_init(|| Arc::new(Runtime::new(Arc::new(MonotonicClock(Instant::now())))))
        .clone()
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct CircuitKey {
    account: String,
    model: String,
}
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct PoolKey {
    pool: String,
    model: String,
}
impl PoolKey {
    fn new(pool: &str, model: &str) -> Self {
        Self { pool: pool.to_owned(), model: model.to_owned() }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Healthy { since: u64, successes: u32 },
    Open { until: u64 },
    HalfOpen { owner: u64 },
    Recovering { since: u64, successes: u32 },
}
impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Healthy { .. } => "healthy",
            Self::Open { .. } => "open",
            Self::HalfOpen { .. } => "half_open",
            Self::Recovering { .. } => "recovering",
        }
    }
}
struct Circuit {
    phase: Phase,
    generation: u64,
    backoff_level: u32,
    inflight: usize,
    last_used: u64,
}
impl Circuit {
    fn new(now: u64) -> Self {
        // A restart has no durable evidence of health. Admit one real request
        // as a probe, then at most two concurrent requests during recovery.
        Self { phase: Phase::Open { until: now }, generation: 0, backoff_level: 0, inflight: 0, last_used: now }
    }
    fn promote_stable_recovery(&mut self, now: u64) {
        if let Phase::Recovering { since, successes } = self.phase {
            if now.saturating_sub(since) >= RECOVERY_MS && successes >= RECOVERY_SUCCESSES {
                self.phase = Phase::Healthy { since, successes };
            }
        }
        if let Phase::Healthy { since, successes } = self.phase {
            if now.saturating_sub(since) >= RESET_BACKOFF_MS && successes >= RECOVERY_SUCCESSES {
                self.backoff_level = 0;
            }
        }
    }
}
struct PoolState {
    last_selected: Option<String>,
    retry_units: u8,
    last_used: u64,
    active_permits: usize,
    retry_reservations: usize,
}
impl PoolState {
    fn new(now: u64) -> Self {
        Self { last_selected: None, retry_units: BUCKET_CAPACITY,
            last_used: now, active_permits: 0, retry_reservations: 0 }
    }
}
#[derive(Default)]
struct State {
    circuits: HashMap<CircuitKey, Circuit>,
    pools: HashMap<PoolKey, PoolState>,
    next_owner: u64,
    last_maintenance: u64,
}
impl State {
    fn maintain(&mut self, now: u64) {
        if now.saturating_sub(self.last_maintenance) < MAINTENANCE_INTERVAL_MS { return; }
        self.last_maintenance = now;
        self.circuits.retain(|_, circuit| {
            circuit.inflight > 0 || now.saturating_sub(circuit.last_used) < IDLE_TTL_MS
                || matches!(circuit.phase, Phase::Open { until } if now < until)
                || matches!(circuit.phase, Phase::HalfOpen { .. })
        });
        self.pools.retain(|_, pool| pool.active_permits > 0 || pool.retry_reservations > 0
            || now.saturating_sub(pool.last_used) < IDLE_TTL_MS);
    }

    fn pool_mut(&mut self, key: PoolKey, now: u64) -> Option<&mut PoolState> {
        if !self.pools.contains_key(&key) && self.pools.len() >= MAX_POOLS { return None; }
        let pool = self.pools.entry(key).or_insert_with(|| PoolState::new(now));
        pool.last_used = now;
        Some(pool)
    }
}
struct Runtime {
    state: Mutex<State>,
    clock: Arc<dyn Clock>,
}

/// Owns both the model circuit permit and the global account inflight slot.
/// Dropping an unfinished probe never certifies the account as healthy.
pub(crate) struct DynamicAccountPermit {
    runtime: Arc<Runtime>,
    key: CircuitKey,
    pool_key: PoolKey,
    generation: u64,
    owner: u64,
    phase: &'static str,
    inflight_before: usize,
    completed: bool,
    // Kept private to prevent accounting from being split or counted twice.
    account_guard: Option<AccountInFlightGuard>,
}
impl DynamicAccountPermit {
    pub(crate) fn account_id(&self) -> &str { &self.key.account }
    pub(crate) fn generation(&self) -> u64 { self.generation }
    pub(crate) fn phase(&self) -> &'static str { self.phase }
    pub(crate) fn inflight_before(&self) -> usize { self.inflight_before }

    /// Transfer only the global load guard into the existing response bridge.
    /// Keep this permit alive alongside that bridge to record semantic health.
    /// The guard must be transferred exactly once.
    pub(crate) fn take_inflight_guard(&mut self) -> AccountInFlightGuard {
        self.account_guard.take().expect("dynamic account guard transferred once")
    }

    /// Record only an explicitly classified semantic capacity failure. The
    /// first failure in a generation opens the circuit; concurrent old failures
    /// cannot repeatedly extend or double the same failure wave's cooldown.
    pub(crate) fn record_overload(&mut self, retry_after: Option<Duration>) {
        if self.completed { return; }
        self.completed = true;
        let now = self.runtime.clock.millis();
        let mut state = crate::lock_utils::lock_recover(&self.runtime.state, "dynamic_pool");
        let Some(circuit) = state.circuits.get_mut(&self.key) else { return; };
        circuit.last_used = now;
        if circuit.generation != self.generation { return; }
        circuit.backoff_level = circuit.backoff_level.saturating_add(1).min(3);
        circuit.generation = circuit.generation.saturating_add(1);
        let base = 30_000u64 << (circuit.backoff_level - 1);
        let cooldown = jittered_cooldown(base, &self.key, circuit.generation)
            .max(retry_after.map(|d| d.as_millis().min(u64::MAX as u128) as u64).unwrap_or(0));
        circuit.phase = Phase::Open { until: now.saturating_add(cooldown) };
    }

    /// Requires complete upstream semantic success. HTTP 200, a first token,
    /// and a client disconnect are deliberately not sufficient.
    pub(crate) fn record_success(&mut self) {
        if self.completed { return; }
        self.completed = true;
        let now = self.runtime.clock.millis();
        let mut state = crate::lock_utils::lock_recover(&self.runtime.state, "dynamic_pool");
        let Some(circuit) = state.circuits.get_mut(&self.key) else { return; };
        circuit.last_used = now;
        if circuit.generation != self.generation { return; }
        match circuit.phase {
            Phase::HalfOpen { owner } if owner == self.owner => {
                circuit.phase = Phase::Recovering { since: now, successes: 0 };
            }
            Phase::Recovering { since, successes } => {
                circuit.phase = Phase::Recovering { since, successes: successes.saturating_add(1) };
            }
            Phase::Healthy { since, successes } => {
                circuit.phase = Phase::Healthy { since, successes: successes.saturating_add(1) };
            }
            _ => {}
        }
        circuit.promote_stable_recovery(now);
    }
}
impl Drop for DynamicAccountPermit {
    fn drop(&mut self) {
        let now = self.runtime.clock.millis();
        let mut state = crate::lock_utils::lock_recover(&self.runtime.state, "dynamic_pool");
        if let Some(circuit) = state.circuits.get_mut(&self.key) {
            circuit.last_used = now;
            circuit.inflight = circuit.inflight.saturating_sub(1);
            if circuit.generation == self.generation
                && matches!(circuit.phase, Phase::HalfOpen { owner } if owner == self.owner)
            {
                circuit.phase = Phase::Open { until: now };
            }
        }
        if let Some(pool) = state.pools.get_mut(&self.pool_key) {
            pool.active_permits = pool.active_permits.saturating_sub(1);
            pool.last_used = now;
        }
        // The state lock is released before Rust drops the account guard.
    }
}

/// The caller supplies a freshly authorized, model/plan/quota filtered list.
/// No account is invented here. A first attempt can probe with non-portable
/// context on its original account; a fallback must obey replay safety.
pub(crate) fn select_and_reserve(
    pool: &str,
    model: &str,
    eligible_accounts: &[String],
    excluded_accounts: &[String],
    account_limit: usize,
    allow_probe: bool,
) -> Option<DynamicAccountPermit> {
    runtime().select(pool, model, eligible_accounts, excluded_accounts, account_limit, allow_probe)
}

/// Preserve normal session affinity while enforcing the same circuit and cap.
pub(crate) fn reserve_account(
    pool: &str,
    model: &str,
    account_id: &str,
    account_limit: usize,
    allow_probe: bool,
) -> Option<DynamicAccountPermit> {
    select_and_reserve(pool, model, &[account_id.to_owned()], &[], account_limit, allow_probe)
}

impl Runtime {
    fn new(clock: Arc<dyn Clock>) -> Self { Self { state: Mutex::new(State::default()), clock } }

    fn select(
        self: &Arc<Self>,
        pool: &str,
        model: &str,
        eligible_accounts: &[String],
        excluded_accounts: &[String],
        account_limit: usize,
        allow_probe: bool,
    ) -> Option<DynamicAccountPermit> {
        let now = self.clock.millis();
        let mut state = crate::lock_utils::lock_recover(&self.state, "dynamic_pool");
        state.maintain(now);
        let pool_key = PoolKey::new(pool, model);
        state.pool_mut(pool_key.clone(), now)?;
        let exclusions = excluded_accounts.iter().map(String::as_str).collect::<HashSet<_>>();
        let mut unique = HashSet::new();
        let mut eligible = Vec::new();
        for account in eligible_accounts {
            if exclusions.contains(account.as_str()) || !unique.insert(account.as_str()) { continue; }
            let key = CircuitKey { account: account.clone(), model: model.to_owned() };
            if !state.circuits.contains_key(&key) && state.circuits.len() >= MAX_CIRCUITS { continue; }
            let circuit = state.circuits.entry(key).or_insert_with(|| Circuit::new(now));
            circuit.last_used = now;
            circuit.promote_stable_recovery(now);
            let probe_priority = match circuit.phase {
                // Expired probes get a separate priority lane. Selecting only
                // healthy accounts would otherwise starve recovery indefinitely.
                Phase::Open { until } if allow_probe && now >= until => 0,
                Phase::Healthy { .. } => 1,
                Phase::Recovering { .. } if circuit.inflight < RECOVERY_LIMIT => 1,
                _ => continue,
            };
            eligible.push((account.clone(), probe_priority));
        }
        let pool_state = state.pools.get_mut(&pool_key).expect("reserved pool state");
        let (guard, inflight_before) = select_and_acquire_account_inflight(|counts| {
            eligible.iter()
                .filter(|(account, _)| account_limit == 0 || counts.get(account).copied().unwrap_or(0) < account_limit)
                .min_by_key(|(account, probe_priority)| (
                    *probe_priority,
                    counts.get(account).copied().unwrap_or(0),
                    pool_state.last_selected.as_ref().is_some_and(|last| account <= last),
                    account.as_str(),
                ))
                .map(|(account, _)| account.clone())
        })?;
        // The selected ID is carried by the guard; avoid another load read.
        let account = guard.account_id().to_owned();
        pool_state.last_selected = Some(account.clone());
        pool_state.active_permits += 1;
        state.next_owner = state.next_owner.wrapping_add(1);
        let owner = state.next_owner;
        let key = CircuitKey { account, model: model.to_owned() };
        let circuit = state.circuits.get_mut(&key).expect("selected eligible circuit");
        if matches!(circuit.phase, Phase::Open { .. }) {
            circuit.phase = Phase::HalfOpen { owner };
        }
        circuit.inflight += 1;
        Some(DynamicAccountPermit {
            runtime: self.clone(), key, pool_key, generation: circuit.generation, owner,
            phase: circuit.phase.name(), inflight_before, completed: false, account_guard: Some(guard),
        })
    }

    fn credit_first(&self, pool: &str, model: &str) {
        let now = self.clock.millis();
        let mut state = crate::lock_utils::lock_recover(&self.state, "dynamic_pool");
        state.maintain(now);
        let Some(bucket) = state.pool_mut(PoolKey::new(pool, model), now) else { return; };
        bucket.retry_units = bucket.retry_units.saturating_add(1).min(BUCKET_CAPACITY);
    }

    fn reserve_retry(self: &Arc<Self>, pool: &str, model: &str) -> Option<RetryReservation> {
        let key = PoolKey::new(pool, model);
        let now = self.clock.millis();
        let mut state = crate::lock_utils::lock_recover(&self.state, "dynamic_pool");
        state.maintain(now);
        let bucket = state.pool_mut(key.clone(), now)?;
        if bucket.retry_units < RETRY_COST { return None; }
        bucket.retry_units -= RETRY_COST;
        bucket.retry_reservations += 1;
        Some(RetryReservation { runtime: self.clone(), key, committed: false })
    }
}

fn jittered_cooldown(base: u64, key: &CircuitKey, generation: u64) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    generation.hash(&mut hasher);
    base.saturating_mul(8_000 + hasher.finish() % 4_001) / 10_000
}

/// Call exactly once, immediately before the first generation POST is sent.
/// Retried POSTs and non-generation credential refreshes must not replenish it.
pub(crate) fn record_first_generation(pool: &str, model: &str) {
    runtime().credit_first(pool, model);
}

/// Shared by every Key in the same authorized pool and effective model.
pub(crate) fn reserve_retry(pool: &str, model: &str) -> Option<RetryReservation> {
    runtime().reserve_retry(pool, model)
}

/// Advisory only: the transport must still reserve atomically before sending.
pub(crate) fn retry_available(pool: &str, model: &str) -> bool {
    let runtime = runtime();
    let state = crate::lock_utils::lock_recover(&runtime.state, "dynamic_pool");
    state.pools.get(&PoolKey::new(pool, model))
        .map(|bucket| bucket.retry_units >= RETRY_COST)
        .unwrap_or(state.pools.len() < MAX_POOLS)
}

pub(crate) struct RetryReservation {
    runtime: Arc<Runtime>,
    key: PoolKey,
    committed: bool,
}
impl RetryReservation {
    /// Commit immediately before an extra generation POST. Committed tokens
    /// are never refunded, even when the send fails before receiving headers.
    pub(crate) fn commit(&mut self) {
        if self.committed { return; }
        let mut state = crate::lock_utils::lock_recover(&self.runtime.state, "dynamic_pool");
        if let Some(bucket) = state.pools.get_mut(&self.key) {
            bucket.retry_reservations = bucket.retry_reservations.saturating_sub(1);
            bucket.last_used = self.runtime.clock.millis();
        }
        self.committed = true;
    }
}
impl Drop for RetryReservation {
    fn drop(&mut self) {
        if self.committed { return; }
        let mut state = crate::lock_utils::lock_recover(&self.runtime.state, "dynamic_pool");
        if let Some(bucket) = state.pools.get_mut(&self.key) {
            bucket.retry_reservations = bucket.retry_reservations.saturating_sub(1);
            bucket.last_used = self.runtime.clock.millis();
            bucket.retry_units = bucket.retry_units.saturating_add(RETRY_COST).min(BUCKET_CAPACITY);
        }
    }
}

#[cfg(test)]
#[path = "tests/dynamic_pool_tests.rs"]
mod tests;
