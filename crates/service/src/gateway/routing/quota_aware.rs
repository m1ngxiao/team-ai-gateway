use codexmanager_core::storage::{Account, ConversationBinding, Storage, Token};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const RECENT_WINDOW_SECS_ENV: &str = "CODEXMANAGER_QUOTA_AWARE_RECENT_WINDOW_SECS";
const REBALANCE_IDLE_SECS_ENV: &str = "CODEXMANAGER_QUOTA_AWARE_REBALANCE_IDLE_SECS";
const REBALANCE_MIN_REMAINING_GAIN_ENV: &str =
    "CODEXMANAGER_QUOTA_AWARE_REBALANCE_MIN_REMAINING_GAIN_PERCENT";
const REBALANCE_MIN_SCORE_GAIN_ENV: &str = "CODEXMANAGER_QUOTA_AWARE_REBALANCE_MIN_SCORE_GAIN";
const SNAPSHOT_TTL_MS_ENV: &str = "CODEXMANAGER_QUOTA_AWARE_SNAPSHOT_TTL_MS";

const DEFAULT_RECENT_WINDOW_SECS: i64 = 60 * 60;
const DEFAULT_REBALANCE_IDLE_SECS: i64 = 30 * 60;
const DEFAULT_REBALANCE_MIN_REMAINING_GAIN: f64 = 10.0;
const DEFAULT_REBALANCE_MIN_SCORE_GAIN: f64 = 0.08;
const DEFAULT_SNAPSHOT_TTL_MS: u64 = 3_000;
const UNKNOWN_REMAINING_PERCENT: f64 = 50.0;
const ROUTE_HEALTH_MAX: f64 = 200.0;

const REMAINING_WEIGHT: f64 = 0.68;
const RECENT_TOKENS_WEIGHT: f64 = 0.17;
const ACTIVE_BINDINGS_WEIGHT: f64 = 0.07;
const INFLIGHT_WEIGHT: f64 = 0.04;
const ROUTE_HEALTH_WEIGHT: f64 = 0.04;

static SNAPSHOT_CACHE: OnceLock<Mutex<Option<SnapshotCache>>> = OnceLock::new();

#[derive(Debug, Clone, Default)]
pub(crate) struct QuotaAwareSnapshot {
    accounts: HashMap<String, QuotaAwareAccountMetrics>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct QuotaAwareAccountMetrics {
    pub(crate) remaining_percent: Option<f64>,
    pub(crate) recent_tokens: i64,
    pub(crate) active_bindings: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct QuotaAwareCandidateScore {
    pub(crate) score: f64,
    pub(crate) remaining_percent: Option<f64>,
}

#[derive(Clone)]
struct SnapshotCache {
    key: String,
    expires_at: Instant,
    snapshot: QuotaAwareSnapshot,
}

impl QuotaAwareSnapshot {
    #[cfg(test)]
    pub(crate) fn new(
        accounts: impl IntoIterator<Item = (String, QuotaAwareAccountMetrics)>,
    ) -> Self {
        Self {
            accounts: accounts.into_iter().collect(),
        }
    }

    pub(crate) fn load(
        storage: &Storage,
        platform_key_hash: &str,
        account_ids: &[String],
        now: i64,
    ) -> Result<Self, String> {
        let mut account_ids = account_ids
            .iter()
            .map(|account_id| account_id.trim().to_string())
            .filter(|account_id| !account_id.is_empty())
            .collect::<Vec<_>>();
        account_ids.sort();
        account_ids.dedup();
        if account_ids.is_empty() {
            return Ok(Self::default());
        }

        let cache_key = snapshot_cache_key(platform_key_hash, &account_ids);
        if let Some(snapshot) = read_cached_snapshot(cache_key.as_str()) {
            return Ok(snapshot);
        }

        let usage_snapshots = storage
            .latest_usage_snapshots_for_accounts(&account_ids)
            .map_err(|err| format!("load quota-aware usage snapshots failed: {err}"))?;
        let token_start = now.saturating_sub(recent_window_secs());
        let token_rollups = storage
            .summarize_request_token_stats_by_source_between("openai_account", token_start, now)
            .map_err(|err| format!("load quota-aware recent token usage failed: {err}"))?;
        let binding_counts = storage
            .active_conversation_binding_account_counts(platform_key_hash)
            .map_err(|err| format!("load quota-aware binding counts failed: {err}"))?;

        let mut accounts = account_ids
            .iter()
            .map(|account_id| (account_id.clone(), QuotaAwareAccountMetrics::default()))
            .collect::<HashMap<_, _>>();
        for snapshot in usage_snapshots {
            if let Some(metrics) = accounts.get_mut(snapshot.account_id.as_str()) {
                metrics.remaining_percent = remaining_percent_from_snapshot(&snapshot, now);
            }
        }
        for rollup in token_rollups {
            if let Some(metrics) = accounts.get_mut(rollup.source_id.as_str()) {
                metrics.recent_tokens = rollup.usage.total_tokens.max(0);
            }
        }
        for (account_id, count) in binding_counts {
            if let Some(metrics) = accounts.get_mut(account_id.as_str()) {
                metrics.active_bindings = count;
            }
        }

        let snapshot = Self { accounts };
        write_cached_snapshot(cache_key, snapshot.clone());
        Ok(snapshot)
    }

    pub(crate) fn metrics(&self, account_id: &str) -> Option<QuotaAwareAccountMetrics> {
        self.accounts.get(account_id).copied()
    }
}

pub(crate) fn rank_candidates(
    candidates: &[(Account, Token)],
    snapshot: &QuotaAwareSnapshot,
) -> Vec<QuotaAwareCandidateScore> {
    let raw_scores = candidates
        .iter()
        .map(|(account, _)| {
            let metrics = snapshot.metrics(account.id.as_str()).unwrap_or_default();
            let inflight = super::account_inflight_count(account.id.as_str());
            let health = super::route_quality::route_health_score(account.id.as_str()) as f64;
            (metrics, inflight, health)
        })
        .collect::<Vec<_>>();
    let max_tokens = raw_scores
        .iter()
        .map(|(metrics, _, _)| metrics.recent_tokens.max(0) as f64)
        .fold(0.0_f64, f64::max);
    let max_bindings = raw_scores
        .iter()
        .map(|(metrics, _, _)| metrics.active_bindings as f64)
        .fold(0.0_f64, f64::max);
    let max_inflight = raw_scores
        .iter()
        .map(|(_, inflight, _)| *inflight as f64)
        .fold(0.0_f64, f64::max);

    raw_scores
        .into_iter()
        .map(|(metrics, inflight, health)| {
            let remaining = metrics
                .remaining_percent
                .unwrap_or(UNKNOWN_REMAINING_PERCENT)
                .clamp(0.0, 100.0)
                / 100.0;
            let recent_fairness = inverse_ratio(metrics.recent_tokens.max(0) as f64, max_tokens);
            let binding_fairness = inverse_ratio(metrics.active_bindings as f64, max_bindings);
            let inflight_fairness = inverse_ratio(inflight as f64, max_inflight);
            let health_score = (health / ROUTE_HEALTH_MAX).clamp(0.0, 1.0);
            QuotaAwareCandidateScore {
                score: remaining * REMAINING_WEIGHT
                    + recent_fairness * RECENT_TOKENS_WEIGHT
                    + binding_fairness * ACTIVE_BINDINGS_WEIGHT
                    + inflight_fairness * INFLIGHT_WEIGHT
                    + health_score * ROUTE_HEALTH_WEIGHT,
                remaining_percent: metrics.remaining_percent,
            }
        })
        .collect()
}

pub(crate) fn order_candidates(
    candidates: &mut [(Account, Token)],
    snapshot: &QuotaAwareSnapshot,
) -> bool {
    if candidates.len() <= 1 {
        return false;
    }
    let scores = rank_candidates(candidates, snapshot);
    let mut indices = (0..candidates.len()).collect::<Vec<_>>();
    indices.sort_by(|left, right| {
        scores[*right]
            .score
            .total_cmp(&scores[*left].score)
            .then_with(|| left.cmp(right))
    });
    if indices
        .iter()
        .enumerate()
        .all(|(position, index)| position == *index)
    {
        return false;
    }

    let ordered = indices
        .into_iter()
        .map(|index| candidates[index].clone())
        .collect::<Vec<_>>();
    candidates.clone_from_slice(ordered.as_slice());
    true
}

pub(crate) fn rebalance_target(
    candidates: &[(Account, Token)],
    binding: &ConversationBinding,
    snapshot: &QuotaAwareSnapshot,
    now: i64,
) -> Option<usize> {
    if candidates.len() <= 1 || now.saturating_sub(binding.last_used_at) < rebalance_idle_secs() {
        return None;
    }
    let scores = rank_candidates(candidates, snapshot);
    let bound_index = candidates
        .iter()
        .position(|(account, _)| account.id == binding.account_id)?;
    let bound = scores.get(bound_index)?;
    let (best_index, best) = scores.iter().enumerate().fold(
        None,
        |best: Option<(usize, &QuotaAwareCandidateScore)>, (index, score)| match best {
            Some((_, current)) if current.score >= score.score => best,
            _ => Some((index, score)),
        },
    )?;
    if best_index == bound_index {
        return None;
    }
    let bound_remaining = bound.remaining_percent?;
    let best_remaining = best.remaining_percent?;
    if best_remaining + f64::EPSILON < bound_remaining {
        return None;
    }
    let remaining_gain = best_remaining - bound_remaining;
    let score_gain = best.score - bound.score;
    if remaining_gain < rebalance_min_remaining_gain() && score_gain < rebalance_min_score_gain() {
        return None;
    }
    Some(best_index)
}

fn remaining_percent_from_snapshot(
    snapshot: &codexmanager_core::storage::UsageSnapshotRecord,
    now: i64,
) -> Option<f64> {
    [
        window_remaining_percent(snapshot.used_percent, snapshot.resets_at, now),
        window_remaining_percent(
            snapshot.secondary_used_percent,
            snapshot.secondary_resets_at,
            now,
        ),
    ]
    .into_iter()
    .flatten()
    .reduce(f64::min)
}

fn window_remaining_percent(
    used_percent: Option<f64>,
    resets_at: Option<i64>,
    now: i64,
) -> Option<f64> {
    let used_percent = used_percent.filter(|value| value.is_finite())?;
    if resets_at.is_some_and(|resets_at| resets_at <= now) {
        Some(100.0)
    } else {
        Some(used_to_remaining(used_percent))
    }
}

fn used_to_remaining(used_percent: f64) -> f64 {
    (100.0 - used_percent).clamp(0.0, 100.0)
}

fn inverse_ratio(value: f64, maximum: f64) -> f64 {
    if maximum <= 0.0 {
        1.0
    } else {
        (1.0 - value / maximum).clamp(0.0, 1.0)
    }
}

fn recent_window_secs() -> i64 {
    env_i64_or(RECENT_WINDOW_SECS_ENV, DEFAULT_RECENT_WINDOW_SECS).max(1)
}

fn rebalance_idle_secs() -> i64 {
    env_i64_or(REBALANCE_IDLE_SECS_ENV, DEFAULT_REBALANCE_IDLE_SECS).max(0)
}

fn rebalance_min_remaining_gain() -> f64 {
    env_f64_or(
        REBALANCE_MIN_REMAINING_GAIN_ENV,
        DEFAULT_REBALANCE_MIN_REMAINING_GAIN,
    )
    .clamp(0.0, 100.0)
}

fn rebalance_min_score_gain() -> f64 {
    env_f64_or(
        REBALANCE_MIN_SCORE_GAIN_ENV,
        DEFAULT_REBALANCE_MIN_SCORE_GAIN,
    )
    .clamp(0.0, 1.0)
}

fn snapshot_cache_ttl() -> Duration {
    Duration::from_millis(env_u64_or(SNAPSHOT_TTL_MS_ENV, DEFAULT_SNAPSHOT_TTL_MS))
}

fn snapshot_cache_key(platform_key_hash: &str, account_ids: &[String]) -> String {
    format!(
        "{}\0{}\0{}",
        std::env::var("CODEXMANAGER_DB_PATH").unwrap_or_else(|_| "<unset>".to_string()),
        platform_key_hash,
        account_ids.join(",")
    )
}

fn read_cached_snapshot(key: &str) -> Option<QuotaAwareSnapshot> {
    let ttl = snapshot_cache_ttl();
    if ttl.is_zero() {
        return None;
    }
    let mutex = SNAPSHOT_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let cached = guard.as_ref()?;
    if cached.key != key || cached.expires_at <= Instant::now() {
        *guard = None;
        return None;
    }
    Some(cached.snapshot.clone())
}

fn write_cached_snapshot(key: String, snapshot: QuotaAwareSnapshot) {
    let ttl = snapshot_cache_ttl();
    if ttl.is_zero() {
        return;
    }
    let mutex = SNAPSHOT_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = Some(SnapshotCache {
        key,
        expires_at: Instant::now() + ttl,
        snapshot,
    });
}

fn env_i64_or(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(default)
}

fn env_u64_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_f64_or(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

#[cfg(test)]
pub(crate) fn clear_snapshot_cache_for_tests() {
    if let Some(mutex) = SNAPSHOT_CACHE.get() {
        let mut guard = match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = None;
    }
}

#[cfg(test)]
#[path = "tests/quota_aware_tests.rs"]
mod tests;
