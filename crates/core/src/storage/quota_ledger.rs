use rusqlite::{params, OptionalExtension, Result};

use super::{now_ts, Storage};

pub const SECONDS_PER_HOUR: i64 = 60 * 60;
pub const SECONDS_PER_WEEK: i64 = 7 * 24 * SECONDS_PER_HOUR;

/// Default per-key rolling weekly budget: $240.00.
pub const DEFAULT_KEY_WEEKLY_LIMIT_CENTS: i64 = 24_000;
/// Default per-account weekly budget: $1,200.00.
pub const DEFAULT_ACCOUNT_WEEKLY_LIMIT_CENTS: i64 = 120_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRollingUsageSummary {
    pub used_cents: i64,
    pub next_release_at: Option<i64>,
    pub next_release_cents: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyQuotaState {
    pub key_id: String,
    pub epoch_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountWeeklyUsage {
    pub account_id: String,
    pub window_start: i64,
    pub window_end: i64,
    pub used_cents: i64,
    pub reserved_cents: i64,
    pub reset_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaResetEvent {
    pub id: i64,
    pub scope: String,
    pub source: String,
    pub occurred_at: i64,
    pub confirmed_by: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
}

fn key_quota_state_from_row(row: &rusqlite::Row<'_>) -> Result<KeyQuotaState> {
    Ok(KeyQuotaState {
        key_id: row.get(0)?,
        epoch_at: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

fn account_weekly_usage_from_row(row: &rusqlite::Row<'_>) -> Result<AccountWeeklyUsage> {
    Ok(AccountWeeklyUsage {
        account_id: row.get(0)?,
        window_start: row.get(1)?,
        window_end: row.get(2)?,
        used_cents: row.get(3)?,
        reserved_cents: row.get(4)?,
        reset_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn quota_reset_event_from_row(row: &rusqlite::Row<'_>) -> Result<QuotaResetEvent> {
    Ok(QuotaResetEvent {
        id: row.get(0)?,
        scope: row.get(1)?,
        source: row.get(2)?,
        occurred_at: row.get(3)?,
        confirmed_by: row.get(4)?,
        note: row.get(5)?,
        created_at: row.get(6)?,
    })
}

impl Storage {
    /// Ensure a durable per-key quota state row exists and return it.
    pub fn ensure_key_quota_state(&self, key_id: &str) -> Result<KeyQuotaState> {
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO key_quota_state (key_id, epoch_at, created_at, updated_at)
             VALUES (?1, 0, ?2, ?2)
             ON CONFLICT(key_id) DO NOTHING",
            params![key_id, now],
        )?;
        self.get_key_quota_state(key_id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn get_key_quota_state(&self, key_id: &str) -> Result<Option<KeyQuotaState>> {
        self.conn
            .query_row(
                "SELECT key_id, epoch_at, created_at, updated_at
                 FROM key_quota_state
                 WHERE key_id = ?1",
                params![key_id],
                key_quota_state_from_row,
            )
            .optional()
    }

    /// Advance a key's quota epoch. Usage before `epoch_at` no longer counts.
    pub fn set_key_quota_epoch(&self, key_id: &str, epoch_at: i64) -> Result<()> {
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO key_quota_state (key_id, epoch_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(key_id) DO UPDATE SET
                epoch_at = excluded.epoch_at,
                updated_at = excluded.updated_at",
            params![key_id, epoch_at, now],
        )?;
        Ok(())
    }

    /// Advance every active API key's rolling quota epoch. Returns how many
    /// keys were advanced. This is used for an official upstream global reset.
    pub fn reset_all_key_quota_epochs(&self, epoch_at: i64) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM api_keys ORDER BY id")?;
        let key_ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>>>()?;
        drop(stmt);
        for key_id in &key_ids {
            self.set_key_quota_epoch(key_id, epoch_at)?;
        }
        Ok(key_ids.len())
    }

    /// Add settled cost to the hourly bucket. Money is stored as integer cents.
    pub fn add_key_usage_hourly(
        &self,
        key_id: &str,
        hour_start: i64,
        cost_cents: i64,
        request_count: i64,
    ) -> Result<()> {
        if cost_cents <= 0 && request_count <= 0 {
            return Ok(());
        }
        let aligned_hour = hour_start - hour_start.rem_euclid(SECONDS_PER_HOUR);
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO key_usage_hourly (
                key_id, hour_start, cost_cents, request_count, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key_id, hour_start) DO UPDATE SET
                cost_cents = key_usage_hourly.cost_cents + excluded.cost_cents,
                request_count = key_usage_hourly.request_count + excluded.request_count,
                updated_at = excluded.updated_at",
            params![
                key_id,
                aligned_hour,
                cost_cents.max(0),
                request_count.max(0),
                now
            ],
        )?;
        Ok(())
    }

    pub fn key_usage_cents_since(&self, key_id: &str, since: i64) -> Result<i64> {
        self.conn.query_row(
            "SELECT COALESCE(SUM(cost_cents), 0)
             FROM key_usage_hourly
             WHERE key_id = ?1 AND hour_start >= ?2",
            params![key_id, since],
            |row| row.get(0),
        )
    }

    fn key_rolling_window_since(&self, key_id: &str, now: i64) -> Result<i64> {
        let epoch_at = self
            .get_key_quota_state(key_id)?
            .map(|state| state.epoch_at)
            .unwrap_or(0);
        Ok((now - SECONDS_PER_WEEK).max(epoch_at))
    }

    fn key_usage_cents_rolling_since(&self, key_id: &str, since: i64) -> Result<i64> {
        // Billing is finalized after the token-stat row is inserted, so the
        // incremental hourly ledger can be empty for successful requests. Derive
        // the rolling total from the durable cost stats first; fall back to the
        // hourly ledger for older rows/tests that predate cost snapshots.
        let derived: i64 = self.conn.query_row(
            "SELECT CAST(ROUND(COALESCE(SUM(estimated_cost_usd), 0) * 100.0) AS INTEGER)
             FROM request_token_stats
             WHERE key_id = ?1 AND created_at >= ?2 AND usage_included = 1",
            params![key_id, since],
            |row| row.get(0),
        )?;
        if derived > 0 {
            return Ok(derived);
        }
        self.key_usage_cents_since(key_id, since)
    }

    fn oldest_stat_release_at_least(
        &self,
        key_id: &str,
        since: i64,
        inclusive_min_usd: f64,
    ) -> Result<Option<(i64, i64)>> {
        let row: Option<(i64, f64)> = self
            .conn
            .query_row(
                "SELECT created_at, estimated_cost_usd
                 FROM request_token_stats
                 WHERE key_id = ?1
                   AND created_at >= ?2
                   AND usage_included = 1
                   AND estimated_cost_usd IS NOT NULL
                   AND estimated_cost_usd > 0
                   AND estimated_cost_usd >= ?3
                 ORDER BY created_at ASC, id ASC
                 LIMIT 1",
                params![key_id, since, inclusive_min_usd],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(created_at, cost_usd)| {
            (
                created_at + SECONDS_PER_WEEK,
                ((cost_usd * 100.0).round() as i64).max(1),
            )
        }))
    }

    /// Rolling seven-day usage, bounded below by the key's current epoch.
    pub fn key_usage_cents_rolling_7d(&self, key_id: &str, now: i64) -> Result<i64> {
        let since = self.key_rolling_window_since(key_id, now)?;
        self.key_usage_cents_rolling_since(key_id, since)
    }

    /// Timestamp when the oldest still-billable charge leaves the rolling window.
    pub fn key_next_release_at(&self, key_id: &str, now: i64) -> Result<Option<i64>> {
        let since = self.key_rolling_window_since(key_id, now)?;
        let oldest_stat: Option<i64> = self.conn.query_row(
            "SELECT MIN(created_at)
             FROM request_token_stats
             WHERE key_id = ?1 AND created_at >= ?2 AND usage_included = 1
               AND estimated_cost_usd IS NOT NULL AND estimated_cost_usd > 0",
            params![key_id, since],
            |row| row.get(0),
        )?;
        if let Some(created_at) = oldest_stat {
            return Ok(Some(created_at + SECONDS_PER_WEEK));
        }
        let oldest_hour: Option<i64> = self.conn.query_row(
            "SELECT MIN(hour_start)
             FROM key_usage_hourly
             WHERE key_id = ?1 AND hour_start >= ?2 AND cost_cents > 0",
            params![key_id, since],
            |row| row.get(0),
        )?;
        Ok(oldest_hour.map(|hour| hour + SECONDS_PER_WEEK))
    }

    /// Read-only rolling 7d usage summary including the next release timestamp and amount.
    pub fn key_rolling_7d_usage_summary(
        &self,
        key_id: &str,
        now: i64,
    ) -> Result<KeyRollingUsageSummary> {
        let since = self.key_rolling_window_since(key_id, now)?;
        let used_cents = self.key_usage_cents_rolling_since(key_id, since)?.max(0);
        let release = if let Some(hit) = self.oldest_stat_release_at_least(key_id, since, 0.005)? {
            Some(hit)
        } else if let Some(hit) = self.oldest_stat_release_at_least(key_id, since, 0.0)? {
            Some(hit)
        } else {
            let hourly: Option<(i64, i64)> = self
                .conn
                .query_row(
                    "SELECT hour_start, cost_cents
                     FROM key_usage_hourly
                     WHERE key_id = ?1 AND hour_start >= ?2 AND cost_cents > 0
                     ORDER BY hour_start ASC
                     LIMIT 1",
                    params![key_id, since],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            hourly.map(|(hour, cents)| (hour + SECONDS_PER_WEEK, cents.max(1)))
        };
        Ok(KeyRollingUsageSummary {
            used_cents,
            next_release_at: release.map(|(at, _)| at),
            next_release_cents: release.map(|(_, cents)| cents),
        })
    }

    /// Rolling seven-day account usage derived from durable request cost stats.
    /// The latest upstream weekly window end for an account. `usage_snapshots`
    /// stores the reset instant of the 7-day window, so the window start is
    /// `reset_at - 7d`; do not count usage from the previous window.
    pub fn latest_account_weekly_reset_at(&self, account_id: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT resets_at
                 FROM usage_snapshots
                 WHERE account_id = ?1
                   AND window_minutes >= 10080
                   AND resets_at IS NOT NULL
                 ORDER BY captured_at DESC, id DESC
                 LIMIT 1",
                params![account_id],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn account_usage_cents_rolling_7d(&self, account_id: &str, now: i64) -> Result<i64> {
        let reset_at = self.latest_account_weekly_reset_at(account_id)?;
        let since = reset_at
            .map(|reset| {
                reset
                    .saturating_sub(SECONDS_PER_WEEK)
                    .max(now.saturating_sub(SECONDS_PER_WEEK))
            })
            .unwrap_or_else(|| now.saturating_sub(SECONDS_PER_WEEK));
        self.conn.query_row(
            "SELECT COALESCE(SUM(CAST(ROUND(estimated_cost_usd * 100.0) AS INTEGER)), 0)
             FROM request_token_stats
             WHERE account_id = ?1 AND created_at >= ?2 AND usage_included = 1",
            params![account_id, since],
            |row| row.get(0),
        )
    }

    pub fn ensure_account_weekly_usage(
        &self,
        account_id: &str,
        window_start: i64,
        window_end: i64,
        reset_at: i64,
    ) -> Result<AccountWeeklyUsage> {
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO account_weekly_usage (
                account_id, window_start, window_end, used_cents, reserved_cents, reset_at, updated_at
             ) VALUES (?1, ?2, ?3, 0, 0, ?4, ?5)
             ON CONFLICT(account_id) DO NOTHING",
            params![account_id, window_start, window_end, reset_at, now],
        )?;
        self.get_account_weekly_usage(account_id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn get_account_weekly_usage(
        &self,
        account_id: &str,
    ) -> Result<Option<AccountWeeklyUsage>> {
        self.conn
            .query_row(
                "SELECT account_id, window_start, window_end, used_cents,
                        reserved_cents, reset_at, updated_at
                 FROM account_weekly_usage
                 WHERE account_id = ?1",
                params![account_id],
                account_weekly_usage_from_row,
            )
            .optional()
    }

    pub fn reset_account_weekly_usage(
        &self,
        account_id: &str,
        window_start: i64,
        window_end: i64,
        reset_at: i64,
    ) -> Result<()> {
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO account_weekly_usage (
                account_id, window_start, window_end, used_cents, reserved_cents, reset_at, updated_at
             ) VALUES (?1, ?2, ?3, 0, 0, ?4, ?5)
             ON CONFLICT(account_id) DO UPDATE SET
                window_start = excluded.window_start,
                window_end = excluded.window_end,
                used_cents = 0,
                reserved_cents = 0,
                reset_at = excluded.reset_at,
                updated_at = excluded.updated_at",
            params![account_id, window_start, window_end, reset_at, now],
        )?;
        Ok(())
    }

    pub fn add_account_usage_cents(
        &self,
        account_id: &str,
        window_start: i64,
        window_end: i64,
        reset_at: i64,
        cost_cents: i64,
    ) -> Result<()> {
        if cost_cents <= 0 {
            return Ok(());
        }
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO account_weekly_usage (
                account_id, window_start, window_end, used_cents, reserved_cents, reset_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)
             ON CONFLICT(account_id) DO UPDATE SET
                used_cents = account_weekly_usage.used_cents + excluded.used_cents,
                updated_at = excluded.updated_at",
            params![account_id, window_start, window_end, cost_cents, reset_at, now],
        )?;
        Ok(())
    }

    pub fn add_account_reserved_cents(
        &self,
        account_id: &str,
        window_start: i64,
        window_end: i64,
        reset_at: i64,
        reserved_cents: i64,
    ) -> Result<()> {
        if reserved_cents <= 0 {
            return Ok(());
        }
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO account_weekly_usage (
                account_id, window_start, window_end, used_cents, reserved_cents, reset_at, updated_at
             ) VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6)
             ON CONFLICT(account_id) DO UPDATE SET
                reserved_cents = account_weekly_usage.reserved_cents + excluded.reserved_cents,
                updated_at = excluded.updated_at",
            params![account_id, window_start, window_end, reserved_cents, reset_at, now],
        )?;
        Ok(())
    }

    pub fn record_quota_reset_event(
        &self,
        scope: &str,
        source: &str,
        occurred_at: i64,
        confirmed_by: Option<&str>,
        note: Option<&str>,
    ) -> Result<i64> {
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO quota_reset_events (
                scope, source, occurred_at, confirmed_by, note, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![scope, source, occurred_at, confirmed_by, note, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_quota_reset_events(&self, limit: i64) -> Result<Vec<QuotaResetEvent>> {
        let limit = limit.clamp(1, 500);
        let mut stmt = self.conn.prepare(
            "SELECT id, scope, source, occurred_at, confirmed_by, note, created_at
             FROM quota_reset_events
             ORDER BY occurred_at DESC, id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], quota_reset_event_from_row)?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ApiKey, RequestTokenStat, UpstreamProvider, UsageSnapshotRecord};

    fn open_storage() -> Storage {
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        storage
    }

    #[test]
    fn key_quota_epoch_excludes_old_usage() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        storage
            .add_key_usage_hourly("key-1", now - 12 * 3600, 100, 1)
            .expect("recent usage");
        storage
            .add_key_usage_hourly("key-1", now - 8 * 24 * 3600, 200, 1)
            .expect("old usage");
        storage
            .set_key_quota_epoch("key-1", now - 24 * 3600)
            .expect("epoch");

        let usage = storage
            .key_usage_cents_rolling_7d("key-1", now)
            .expect("rolling usage");
        assert_eq!(usage, 100);
    }

    #[test]
    fn account_weekly_usage_accumulates_and_resets() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        let week = SECONDS_PER_WEEK;

        storage
            .add_account_usage_cents("acct-1", now, now + week, now + week, 500)
            .expect("first usage");
        storage
            .add_account_usage_cents("acct-1", now, now + week, now + week, 250)
            .expect("second usage");

        let usage = storage
            .get_account_weekly_usage("acct-1")
            .expect("read usage")
            .expect("usage row");
        assert_eq!(usage.used_cents, 750);

        storage
            .reset_account_weekly_usage("acct-1", now + week, now + 2 * week, now + 2 * week)
            .expect("reset");
        let usage = storage
            .get_account_weekly_usage("acct-1")
            .expect("read usage")
            .expect("usage row");
        assert_eq!(usage.used_cents, 0);
        assert_eq!(usage.window_start, now + week);
    }

    #[test]
    fn latest_account_weekly_reset_uses_seven_day_window() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        storage
            .insert_usage_snapshot(&UsageSnapshotRecord {
                account_id: "acct-reset".to_string(),
                used_percent: Some(10.0),
                window_minutes: Some(10080),
                resets_at: Some(now + 3 * 24 * 3600),
                secondary_used_percent: None,
                secondary_window_minutes: None,
                secondary_resets_at: None,
                credits_json: None,
                captured_at: now,
            })
            .expect("snapshot");
        assert_eq!(
            storage
                .latest_account_weekly_reset_at("acct-reset")
                .expect("reset"),
            Some(now + 3 * 24 * 3600)
        );
    }

    #[test]
    fn quota_reset_events_round_trip() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        let id = storage
            .record_quota_reset_event(
                "global",
                "official",
                now,
                Some("admin"),
                Some("upstream reset"),
            )
            .expect("record event");

        let events = storage.list_quota_reset_events(10).expect("list events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, id);
        assert_eq!(events[0].scope, "global");
        assert_eq!(events[0].source, "official");
        assert_eq!(events[0].confirmed_by.as_deref(), Some("admin"));
    }
    #[test]
    fn key_usage_rolling_7d_prefers_cost_stats() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        storage
            .insert_request_token_stat(&RequestTokenStat {
                request_log_id: 0,
                key_id: Some("key-stats".to_string()),
                account_id: None,
                model: Some("gpt-6-astra".to_string()),
                actual_source_kind: None,
                actual_source_id: None,
                input_tokens: Some(1_000),
                cached_input_tokens: Some(0),
                output_tokens: Some(100),
                total_tokens: Some(1_100),
                reasoning_output_tokens: Some(0),
                estimated_cost_usd: Some(1.25),
                created_at: now - 12 * 3600,
            })
            .expect("token stat");
        assert_eq!(
            storage.key_usage_cents_rolling_7d("key-stats", now).expect("usage"),
            125
        );
    }

    #[test]
    fn key_usage_accumulates_sub_cent_costs_before_rounding() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        for request_log_id in 0..2 {
            storage
                .insert_request_token_stat(&RequestTokenStat {
                    request_log_id,
                    key_id: Some("key-subcent".to_string()),
                    account_id: None,
                    model: Some("gpt-6-luna".to_string()),
                    actual_source_kind: None,
                    actual_source_id: None,
                    input_tokens: Some(20),
                    cached_input_tokens: Some(0),
                    output_tokens: Some(5),
                    total_tokens: Some(25),
                    reasoning_output_tokens: Some(0),
                    estimated_cost_usd: Some(0.004),
                    created_at: now - 3600,
                })
                .expect("token stat");
        }
        assert_eq!(
            storage
                .key_usage_cents_rolling_7d("key-subcent", now)
                .expect("usage"),
            1
        );
    }

    #[test]
    fn reset_all_key_quota_epochs_zeroes_all_keys() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        for (id, status) in [("key-a", "active"), ("key-b", "active"), ("key-c", "disabled")] {
            storage
                .insert_api_key(&ApiKey {
                    id: id.to_string(),
                    name: Some(id.to_string()),
                    model_slug: None,
                    reasoning_effort: None,
                    service_tier: None,
                    rotation_strategy: "account_rotation".to_string(),
                    upstream_provider: UpstreamProvider::Openai,
                    aggregate_api_id: None,
                    account_plan_filter: None,
                    aggregate_api_url: None,
                    client_type: "codex".to_string(),
                    protocol_type: "openai_compat".to_string(),
                    auth_scheme: "authorization_bearer".to_string(),
                    upstream_base_url: None,
                    static_headers_json: None,
                    key_hash: format!("hash-{id}"),
                    status: status.to_string(),
                    created_at: now,
                    last_used_at: None,
                })
                .expect("api key");
            storage
                .add_key_usage_hourly(id, now - 3600, 1_000, 1)
                .expect("usage");
            assert_eq!(
                storage.key_usage_cents_rolling_7d(id, now).expect("usage"),
                1_000
            );
        }

        let reset = storage
            .reset_all_key_quota_epochs(now + 10)
            .expect("reset all");
        assert_eq!(reset, 3);
        for id in ["key-a", "key-b", "key-c"] {
            assert_eq!(
                storage
                    .key_usage_cents_rolling_7d(id, now + 10)
                    .expect("usage after reset"),
                0
            );
        }
    }

    #[test]
    fn rolling_usage_summary_tracks_release_amount_and_skips_negligible_dust() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        storage
            .insert_request_token_stat(&RequestTokenStat {
                request_log_id: 950,
                key_id: Some("key-summary".to_string()),
                account_id: None,
                model: Some("gpt-6-luna".to_string()),
                actual_source_kind: None,
                actual_source_id: None,
                input_tokens: Some(10),
                cached_input_tokens: Some(0),
                output_tokens: Some(1),
                total_tokens: Some(11),
                reasoning_output_tokens: Some(0),
                estimated_cost_usd: Some(0.001),
                created_at: now - 2_000,
            })
            .expect("dust stat");
        storage
            .insert_request_token_stat(&RequestTokenStat {
                request_log_id: 951,
                key_id: Some("key-summary".to_string()),
                account_id: None,
                model: Some("gpt-6-luna".to_string()),
                actual_source_kind: None,
                actual_source_id: None,
                input_tokens: Some(100),
                cached_input_tokens: Some(0),
                output_tokens: Some(50),
                total_tokens: Some(150),
                reasoning_output_tokens: Some(0),
                estimated_cost_usd: Some(1.25),
                created_at: now - 1_000,
            })
            .expect("main stat");
        let summary = storage
            .key_rolling_7d_usage_summary("key-summary", now)
            .expect("summary");
        assert_eq!(summary.used_cents, 125);
        assert_eq!(summary.next_release_at, Some(now - 1_000 + SECONDS_PER_WEEK));
        assert_eq!(summary.next_release_cents, Some(125));
    }

    #[test]
    fn rolling_usage_summary_falls_back_to_subcent_hourly_and_respects_epoch() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        for (idx, offset) in [1_200_i64, 600_i64].into_iter().enumerate() {
            storage
                .insert_request_token_stat(&RequestTokenStat {
                    request_log_id: 960 + idx as i64,
                    key_id: Some("key-sub".to_string()),
                    account_id: None,
                    model: Some("gpt-6-luna".to_string()),
                    actual_source_kind: None,
                    actual_source_id: None,
                    input_tokens: Some(20),
                    cached_input_tokens: Some(0),
                    output_tokens: Some(5),
                    total_tokens: Some(25),
                    reasoning_output_tokens: Some(0),
                    estimated_cost_usd: Some(0.004),
                    created_at: now - offset,
                })
                .expect("subcent stat");
        }
        let sub = storage
            .key_rolling_7d_usage_summary("key-sub", now)
            .expect("sub summary");
        assert_eq!(sub.used_cents, 1);
        assert_eq!(sub.next_release_at, Some(now - 1_200 + SECONDS_PER_WEEK));
        assert_eq!(sub.next_release_cents, Some(1));

        storage
            .add_key_usage_hourly("key-hourly", now - 3_600, 420, 2)
            .expect("hourly");
        let aligned = (now - 3_600) - (now - 3_600).rem_euclid(SECONDS_PER_HOUR);
        let hourly = storage
            .key_rolling_7d_usage_summary("key-hourly", now)
            .expect("hourly summary");
        assert_eq!(hourly.used_cents, 420);
        assert_eq!(hourly.next_release_at, Some(aligned + SECONDS_PER_WEEK));
        assert_eq!(hourly.next_release_cents, Some(420));

        storage
            .set_key_quota_epoch("key-hourly", now + 1)
            .expect("epoch");
        let after_epoch = storage
            .key_rolling_7d_usage_summary("key-hourly", now + 10)
            .expect("epoch summary");
        assert_eq!(after_epoch.used_cents, 0);
        assert_eq!(after_epoch.next_release_at, None);
        assert_eq!(after_epoch.next_release_cents, None);
    }

    #[test]
    fn next_release_tracks_oldest_billable_request() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        for (idx, age) in [0_i64, 1_i64].into_iter().enumerate() {
            storage
                .insert_request_token_stat(&RequestTokenStat {
                    request_log_id: 900 + idx as i64,
                    key_id: Some("key-release".to_string()),
                    account_id: None,
                    model: Some("gpt-6-luna".to_string()),
                    actual_source_kind: None,
                    actual_source_id: None,
                    input_tokens: Some(10),
                    cached_input_tokens: Some(0),
                    output_tokens: Some(5),
                    total_tokens: Some(15),
                    reasoning_output_tokens: Some(0),
                    estimated_cost_usd: Some(0.01),
                    created_at: now - 1_000 + age * 500,
                })
                .expect("token stat");
        }
        assert_eq!(
            storage
                .key_next_release_at("key-release", now)
                .expect("release"),
            Some(now - 1_000 + SECONDS_PER_WEEK)
        );
    }

    struct TempSqliteCleanup(std::path::PathBuf);

    impl Drop for TempSqliteCleanup {
        fn drop(&mut self) {
            let base = self.0.to_string_lossy();
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{base}{suffix}"));
            }
        }
    }

    fn unique_quota_db_path(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}.db", std::process::id()))
    }

    fn sample_quota_api_key(id: &str, now: i64) -> ApiKey {
        ApiKey {
            id: id.to_string(),
            name: Some(id.to_string()),
            model_slug: None,
            reasoning_effort: None,
            service_tier: None,
            rotation_strategy: "account_rotation".to_string(),
            upstream_provider: UpstreamProvider::Openai,
            aggregate_api_id: None,
            account_plan_filter: None,
            aggregate_api_url: None,
            client_type: "codex".to_string(),
            protocol_type: "openai_compat".to_string(),
            auth_scheme: "authorization_bearer".to_string(),
            upstream_base_url: None,
            static_headers_json: None,
            key_hash: format!("hash-{id}"),
            status: "active".to_string(),
            created_at: now,
            last_used_at: None,
        }
    }

    #[test]
    fn key_usage_rolling_7d_and_next_release_succeed_while_writer_holds_lock() {
        let db_path = unique_quota_db_path("codexmanager-quota-writer-lock");
        let _cleanup = TempSqliteCleanup(db_path.clone());
        let now = 1_700_000_000_i64;

        let writer = Storage::open(&db_path).expect("open writer");
        writer.init().expect("init writer");
        writer
            .insert_api_key(&sample_quota_api_key("key-lock", now))
            .expect("insert api key");
        writer
            .insert_api_key(&sample_quota_api_key("key-lock-holder", now))
            .expect("insert holder api key");
        writer
            .insert_request_token_stat(&RequestTokenStat {
                request_log_id: 101,
                key_id: Some("key-lock".to_string()),
                account_id: None,
                model: Some("gpt-6-astra".to_string()),
                actual_source_kind: None,
                actual_source_id: None,
                input_tokens: Some(100),
                cached_input_tokens: Some(0),
                output_tokens: Some(50),
                total_tokens: Some(150),
                reasoning_output_tokens: Some(0),
                estimated_cost_usd: Some(2.50),
                created_at: now - 3600,
            })
            .expect("insert token stat");

        let reader = Storage::open(&db_path).expect("open reader");
        reader
            .conn
            .busy_timeout(std::time::Duration::from_millis(50))
            .expect("set short reader busy timeout");

        let tx = writer
            .conn
            .unchecked_transaction()
            .expect("begin immediate writer transaction");
        let updated = tx
            .execute(
                "UPDATE api_keys SET name = 'holding' WHERE id = 'key-lock-holder'",
                [],
            )
            .expect("hold sqlite writer lock");
        assert_eq!(updated, 1);

        let usage = reader
            .key_usage_cents_rolling_7d("key-lock", now)
            .expect("read rolling usage while writer holds lock");
        let release_at = reader
            .key_next_release_at("key-lock", now)
            .expect("read next release while writer holds lock");
        assert_eq!(usage, 250);
        assert_eq!(release_at, Some(now - 3600 + SECONDS_PER_WEEK));

        let quota_state_rows: i64 = reader
            .conn
            .query_row(
                "SELECT COUNT(*) FROM key_quota_state WHERE key_id = ?1",
                params!["key-lock"],
                |row| row.get(0),
            )
            .expect("count key_quota_state rows");
        assert_eq!(quota_state_rows, 0);

        tx.commit().expect("commit writer transaction");
    }

    #[test]
    fn concurrent_readers_and_writers_stress_without_database_locked() {
        let db_path = unique_quota_db_path("codexmanager-quota-stress");
        let _cleanup = TempSqliteCleanup(db_path.clone());
        let now = 1_700_000_000_i64;

        let init = Storage::open(&db_path).expect("open init storage");
        init.init().expect("init storage");
        init.insert_api_key(&sample_quota_api_key("key-stress", now))
            .expect("insert api key");
        init.insert_request_token_stat(&RequestTokenStat {
            request_log_id: 1,
            key_id: Some("key-stress".to_string()),
            account_id: None,
            model: Some("gpt-6-astra".to_string()),
            actual_source_kind: None,
            actual_source_id: None,
            input_tokens: Some(100),
            cached_input_tokens: Some(0),
            output_tokens: Some(50),
            total_tokens: Some(150),
            reasoning_output_tokens: Some(0),
            estimated_cost_usd: Some(1.00),
            created_at: now - 3600,
        })
        .expect("insert seed stat");
        drop(init);

        let reader_threads = 8usize;
        let writer_threads = 4usize;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(reader_threads + writer_threads));
        let mut handles = Vec::new();

        for writer_idx in 0..writer_threads {
            let db_path = db_path.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let storage = Storage::open(&db_path).expect("open stress writer");
                barrier.wait();
                for step in 0..20usize {
                    let stat_id = 100 + (writer_idx * 100 + step) as i64;
                    storage
                        .insert_request_token_stat(&RequestTokenStat {
                            request_log_id: stat_id,
                            key_id: Some("key-stress".to_string()),
                            account_id: None,
                            model: Some("gpt-6-astra".to_string()),
                            actual_source_kind: None,
                            actual_source_id: None,
                            input_tokens: Some(20),
                            cached_input_tokens: Some(0),
                            output_tokens: Some(10),
                            total_tokens: Some(30),
                            reasoning_output_tokens: Some(0),
                            estimated_cost_usd: Some(0.10),
                            created_at: now - 1000 + step as i64,
                        })
                        .expect("writer insert_request_token_stat must not hit database locked");
                }
            }));
        }

        for _ in 0..reader_threads {
            let db_path = db_path.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let storage = Storage::open(&db_path).expect("open stress reader");
                barrier.wait();
                for _ in 0..30usize {
                    let cents = storage
                        .key_usage_cents_rolling_7d("key-stress", now)
                        .expect("reader key_usage_cents_rolling_7d must not hit database locked");
                    assert!(cents >= 100);
                    let release = storage
                        .key_next_release_at("key-stress", now)
                        .expect("reader key_next_release_at must not hit database locked");
                    assert_eq!(release, Some(now - 3600 + SECONDS_PER_WEEK));
                }
            }));
        }

        for handle in handles {
            handle.join().expect("stress worker thread panicked");
        }

        let check = Storage::open(&db_path).expect("open verify storage");
        let final_cents = check
            .key_usage_cents_rolling_7d("key-stress", now)
            .expect("final usage check");
        assert_eq!(final_cents, 900);
        let quota_state_rows: i64 = check
            .conn
            .query_row(
                "SELECT COUNT(*) FROM key_quota_state WHERE key_id = ?1",
                params!["key-stress"],
                |row| row.get(0),
            )
            .expect("count key_quota_state rows after stress");
        assert_eq!(quota_state_rows, 0);
    }

}
